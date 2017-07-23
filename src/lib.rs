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
use futures::future::{Executor, Loop, Either, lazy, loop_fn, empty, select_all};

use futures_mpsc::{UnboundedSender, UnboundedReceiver, unbounded};

use std::thread;
use std::time::Duration;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

enum ExecutorMessage {
    ExecuteAfter { id: String, time: u64, f: Box<Fn() + Send + 'static> },
    Cancel { id: String }
}

pub struct ScheduledExecutorHandle {
    exec_ref: ScheduledExecutor,
    id: String
}

impl ScheduledExecutorHandle {
    pub fn cancel(mut self) {
        self.exec_ref.cancel(self.id);
    }
}

#[derive(Clone)]
pub struct ScheduledExecutor {
    sender: UnboundedSender<ExecutorMessage>,
}

impl ScheduledExecutor {
    pub fn new() -> ScheduledExecutor {
        let (sender, receiver): (UnboundedSender<_>, UnboundedReceiver<_>) = unbounded();

        thread::spawn(|| {
            let exec_service = ScheduledExecutorService::main_loop(receiver, HashMap::new());


            println!("Thread is returning");
        });

        ScheduledExecutor {
            sender,
        }
    }

    pub fn execute_after(&mut self, time: u64, f: Box<Fn() + Send + 'static>)
        -> ScheduledExecutorHandle {
        let id = uuid::Uuid::new_v4().to_string();
        self.sender.send(
            ExecutorMessage::ExecuteAfter {
                id: id.clone(),
                time,
                f
            }
        ).unwrap();

        ScheduledExecutorHandle {
            exec_ref: self.clone(),
            id
        }
    }

    fn cancel(&mut self, id: String) {
        self.sender.send(
            ExecutorMessage::Cancel {
                id: id.clone(),
            }
        ).expect("ScheduledExecutor failed to send cancel message");
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

    pub fn main_loop(receiver: UnboundedReceiver<ExecutorMessage>,
                     mut timers: HashMap<String, (Box<Future<Item=(), Error=()>>, u64, Box<Fn() + Send + 'static>)>) {

        let mut core = Core::new().unwrap();

        let handle = core.handle();
        let h = handle.clone();

        let main_loop: Box<Future<Item=(), Error=()>> =
            Box::new(loop_fn((receiver.into_future(), timers, handle), move |res| {
                let (receiver, mut timers, handle) = res;

                let timer_future = if timers.len() == 0 {
                    Either::A(empty())
                } else {
                    Either::B(select_all(timers.drain().map(|(id, (fut, it, then))| {
                        fut.map(move |f| (id, f, it, then))
                    })))
                };

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
                                Either::A((receive_future, timer_futures)) => {
                                    println!("received message");
                                    let msg = receive_future.0;

                                    if let Some(msg) = msg {
                                        match msg {
                                            ExecutorMessage::ExecuteAfter { id, time, f } => {
                                                let timeout = Timeout::new(
                                                    Duration::from_secs(time),
                                                    &handle
                                                ).expect("Failed to create timeout");

                                                execute_after(&mut timers, id, timeout, time, f)
                                            }
                                            ExecutorMessage::Cancel { id } => {
                                                let timer = timers.remove(&id).expect("Timer not found by id");
                                                drop(timer)
                                            }
                                        }
                                    }

                                    Ok(Loop::Continue((receive_future.1.into_future(), timers, handle)))
                                }
                                // Timer has fired
                                Either::B(((timer_result, _, timer_futures), receive_future)) => {
                                    println!("timer has fired");
                                    let (id, _, time, f) = timer_result;
                                    f();

                                    let time = time * 2;
                                    let timeout = Timeout::new(Duration::from_secs(time), &handle).unwrap();
                                    execute_after(&mut timers, id, timeout, time, f);

                                    Ok(Loop::Continue((receive_future, timers, handle)))
                                }
                            }
                        }
                        Err(e) => {
                            println!("err");
                            Ok(Loop::Break(()))
                        }
                    }
                })
            }));

        core.run(main_loop).unwrap();
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

        let foo = executor.execute_after(1,
                               Box::new(|| {
                                   println!("foo");
                               }));

        let bar = executor.execute_after(1,
                                         Box::new(|| {
                                             println!("bar");
                                         }));

        thread::sleep(Duration::from_secs(5));

        foo.cancel();
        bar.cancel();
    }
}