use crate::runtime::ActorRuntime;
use async_trait::async_trait;
use flume::{SendError, Sender};
use message::MailboxSender;
use message::{Envelope, Enveloper, MessageRequest};
use std::fmt::Debug;
use tokio::sync::oneshot;

pub mod debug;
pub mod message;
pub mod relay;
pub mod runtime;

const DEFAULT_CHANNEL_CAPACITY: usize = 128;

pub trait Actor: Sized + Send + Sync + 'static {
    fn start(self) -> ActorHandle<Self> {
        println!("Starting actor");
        let (message_tx, message_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        let (command_tx, command_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        tokio::spawn(ActorRuntime::new(self, command_rx, message_rx).runt());
        ActorHandle::new(message_tx, command_tx)
    }
}

/// The main trait to implement on an [Actor] to enable it to handle messages.
#[async_trait]
pub trait Handler<M>: Actor {
    type Response;
    async fn handle(&mut self, message: M) -> Result<Self::Response, Error>;
}

/// A handle to a spawned actor. Obtained when calling `start` on an [Actor] and is used to send messages
/// to it.
#[derive(Debug)]
pub struct ActorHandle<A>
where
    A: Actor,
{
    message_tx: MailboxSender<A>,
    command_tx: Sender<ActorCommand>,
}

impl<A> Clone for ActorHandle<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        Self {
            message_tx: self.message_tx.clone(),
            command_tx: self.command_tx.clone(),
        }
    }
}

impl<A> ActorHandle<A>
where
    A: Actor,
{
    pub fn new(message_tx: MailboxSender<A>, command_tx: Sender<ActorCommand>) -> Self {
        Self {
            message_tx,
            command_tx,
        }
    }

    /// Sends a message to the actor and returns a [MessageRequest] that can
    /// be awaited. This method should be used when one needs a response from the
    /// actor.
    pub fn send_wait<M>(&self, message: M) -> Result<MessageRequest<A::Response>, SendError<M>>
    where
        M: Send + Clone,
        A: Handler<M> + Enveloper<A, M>,
    {
        if self.message_tx.is_full() || self.message_tx.is_disconnected() {
            return Err(SendError(message));
        }
        let (tx, rx) = oneshot::channel();
        let _ = self.message_tx.send(A::pack(message, Some(tx)));
        Ok(MessageRequest { response_rx: rx })
    }

    /// Send a message to the actor without returning a response, but still returning an
    /// error if the channel is full or disconnected.
    pub fn send<M>(&self, message: M) -> Result<(), SendError<M>>
    where
        M: Clone + Send,
        A: Handler<M> + Enveloper<A, M> + 'static,
    {
        if self.message_tx.is_full() || self.message_tx.is_disconnected() {
            return Err(SendError(message));
        }
        let _ = self.message_tx.send(A::pack(message, None));
        Ok(())
    }

    /// Send a message ignoring any errors in the process. The true YOLO way to send messages.
    pub fn send_forget<M>(&self, message: M)
    where
        M: Clone + Send + 'static,
        A: Handler<M> + Enveloper<A, M> + 'static,
    {
        let _ = self.message_tx.send(A::pack(message, None));
    }

    pub fn send_cmd(&self, cmd: ActorCommand) -> Result<(), SendError<ActorCommand>> {
        self.command_tx.send(cmd)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ActorStatus {
    Stopped = 0,
    Starting = 1,
    Running = 2,
    Stopping = 3,
    Idle = 4,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Actor channel closed")]
    ActorChannelClosed,
    #[error("Channel closed: {0}")]
    ChannelClosed(#[from] oneshot::error::TryRecvError),
    #[error("Warp error: {0}")]
    Warp(#[from] warp::Error),
}

pub enum SendErr<M> {
    Full(M),
    Closed(M),
}

#[derive(Debug)]
pub enum ActorCommand {
    Stop,
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::{sync::atomic::AtomicUsize, time::Duration};

    #[tokio::test]
    async fn it_works_sync() {
        #[derive(Debug)]
        struct Testor {}

        #[derive(Debug, Clone)]
        struct Foo {}

        #[derive(Debug, Clone)]
        struct Bar {}

        impl Actor for Testor {}

        #[async_trait]
        impl Handler<Foo> for Testor {
            type Response = usize;
            async fn handle(&mut self, _: Foo) -> Result<usize, Error> {
                println!("Handling Foo");
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            type Response = isize;
            async fn handle(&mut self, _: Bar) -> Result<isize, Error> {
                for _ in 0..10_000 {
                    println!("Handling Bar");
                }
                Ok(10)
            }
        }

        let mut res = 0;
        let mut res2 = 0;

        let handle = Testor {}.start();
        println!("HELLO WORLDS");
        for _ in 0..100 {
            res += handle.send_wait(Foo {}).unwrap().await.unwrap();
            res2 += handle.send_wait(Bar {}).unwrap().await.unwrap();
        }

        handle.send(Foo {}).unwrap();
        handle.send_forget(Bar {});
        handle.send_cmd(ActorCommand::Stop).unwrap();

        assert_eq!(res, 1000);
        assert_eq!(res2, 1000);
    }

    #[tokio::test]
    async fn it_works_yolo() {
        #[derive(Debug)]
        struct Testor {}

        #[derive(Debug, Clone)]
        struct Foo {}

        #[derive(Debug, Clone)]
        struct Bar {}

        impl Actor for Testor {}

        static COUNT: AtomicUsize = AtomicUsize::new(0);

        #[async_trait]
        impl Handler<Foo> for Testor {
            type Response = usize;
            async fn handle(&mut self, _: Foo) -> Result<usize, Error> {
                println!("INCREMENTING COUNT FOO");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            type Response = isize;
            async fn handle(&mut self, _: Bar) -> Result<isize, Error> {
                println!("INCREMENTING COUNT BAR");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        let handle = Testor {}.start();

        handle.send_wait(Bar {}).unwrap().await.unwrap();
        handle.send(Foo {}).unwrap();
        handle.send_forget(Bar {});

        for _ in 0..100 {
            let _ = handle.send(Foo {});
            handle.send_forget(Bar {});
            tokio::time::sleep(Duration::from_micros(100)).await
        }

        assert_eq!(COUNT.load(std::sync::atomic::Ordering::Relaxed), 203);
        handle.send_cmd(ActorCommand::Stop).unwrap();
    }
}
