

#![allow(dead_code)]

extern crate futures_cpupool;
extern crate tokio_timer;
extern crate tokio_core;
extern crate uuid;
extern crate futures;
extern crate futures_mpsc;

use tokio_core::reactor::{Core, Timeout, Handle};
use tokio_timer::Timer;

use futures_cpupool::CpuPool;
use futures::{Future, Stream, IntoFuture};
use futures::future::{Executor, Loop, Either, loop_fn, empty, select_all};

use futures_mpsc::{UnboundedSender, UnboundedReceiver, unbounded};

use std::thread;
use std::time::Duration;
use std::collections::HashMap;

enum ExecutorMessage {
    ExecuteAfter {id: String, timeout: Timeout, time: u64, f: Box<Fn() + Send + 'static>},
    Cancel {id: String}
}

pub struct ScheduledExecutor {
    sender: UnboundedSender<ExecutorMessage>,
}

impl ScheduledExecutor {
    pub fn new() -> ScheduledExecutor {
        let (sender, receiver): (UnboundedSender<_>, UnboundedReceiver<_>) = unbounded();

        thread::spawn(move || {
            let core = Core::new().unwrap();
            let handle = core.handle();
            let exec_service = ScheduledExecutorService::new(handle, receiver);

        });

        ScheduledExecutor {
            sender,
        }
    }

    pub fn execute_after(&mut self, timeout: Timeout, time: u64, f: Box<Fn() + Send + 'static>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.sender.send(
            ExecutorMessage::ExecuteAfter {
                id: id.clone(),
                timeout,
                time,
                f
            }
        ).unwrap();

        id
    }

    pub fn cancel(&mut self, id: String) {
        self.sender.send(
            ExecutorMessage::Cancel {
                id: id.clone(),
            }
        ).unwrap();
    }
}

struct ScheduledExecutorService {
    handle: Handle,
    timers: HashMap<String, (Box<Future<Item=(), Error=()>>, u64, Box<Fn() + Send + 'static>)>,
    receiver: UnboundedReceiver<ExecutorMessage>
}

impl ScheduledExecutorService {
    pub fn new(handle: Handle, receiver: UnboundedReceiver<ExecutorMessage>) -> ScheduledExecutorService {
        ScheduledExecutorService {
            handle,
            timers: HashMap::new(),
            receiver
        }
    }

    pub fn main_loop(mut handle: Handle,
                     mut receiver: UnboundedReceiver<ExecutorMessage>,
                     mut timers: HashMap<String, (Box<Future<Item=(), Error=()>>, u64, Box<Fn() + Send + 'static>)>) {
        let handle = handle.clone();
        let h = handle.clone();
        let timer_future = select_all(timers.drain().map(|(id, (fut, it, then))| {
            fut.map(move |f| (id, f, it, then))
        }));

        let main_loop = loop_fn((receiver.into_future(), timer_future, timers, handle), move |res| {

            let (receiver, timer_future, mut timers, handle) = res;
            // We want to either handle the case where we receive a new message or where one of our timers
            // has expired
            //
            // In the case of a new message we want to handle either cancelling an old future by ID or
            // we want to add the new timer to the timers map
            //
            // In the case of our timer expiring we want to reinsert the old timers back into the map
            // and then call the function associated with the timer
            receiver.into_future().select2(timer_future).then(|res| {
                match res {
                    Ok(o) => {
                        match o {
                            // Received a new message
                            Either::A((receive_future, timer_future)) => {

                                let msg = receive_future.0;

                                if let Some(msg) = msg {
                                    match msg {
                                        ExecutorMessage::ExecuteAfter {id, timeout, time, f} => {
                                            execute_after(&mut timers, id, timeout, time, f)
                                        },
                                        ExecutorMessage::Cancel {id} => {
                                            let timer = timers.remove(&id).unwrap();
                                            drop(timer)
                                        }
                                    }
                                }

                                Ok(Loop::Continue((receive_future.1.into_future(), timer_future, timers, handle)))
                            },
                            // Timer has fired
                            Either::B(((timer_result, _, timer_futures), receive_future)) => {
                                let (id, _, time, f) = timer_result;
                                f();

                                let time = time * 2;
                                let timeout = Timeout::new(Duration::from_secs(time), &handle).unwrap();
                                execute_after(&mut timers, id, timeout, time, f);

                                Ok(Loop::Continue((receive_future, select_all(timer_futures), timers, handle)))
                            }
                        }
                    },
                    Err(e) => {
                        Ok(Loop::Break(()))
                    }
                }
            })
        });

        h.execute(main_loop).unwrap();
    }
}

pub fn execute_after(timers: &mut HashMap<String, (Box<Future<Item=(), Error=()>>, u64, Box<Fn() + Send + 'static>)>,
                     id: String,
                     timeout: Timeout,
                     time: u64,
                     f: Box<Fn() + Send + 'static>)
{
    let future = Box::new(timeout.map_err(|_| ()));
    timers.insert(id, (future, time, f));
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn it_works() {

        let mut executor = ScheduledExecutor::new();
        let core = Core::new().unwrap();
        let handle = core.handle();

        let timeout = Timeout::new(Duration::from_secs(1), &handle).unwrap();

        executor.execute_after(timeout, 1,
        Box::new(|| {
            println!("foo");
        }));

        thread::sleep(Duration::from_secs(30));
    }
}