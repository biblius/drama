use crate::runtime::{ActorRuntime, Runtime};
use async_trait::async_trait;
use flume::{SendError, Sender};
use message::{Envelope, Enveloper, Message, MessageRequest};
use std::fmt::Debug;
use tokio::sync::oneshot;
pub mod debug;
pub mod message;
pub mod runtime;
pub mod ws;

const DEFAULT_CHANNEL_CAPACITY: usize = 16;

pub trait Actor {
    fn start(self) -> ActorHandle<Self>
    where
        Self: Sized + Send + 'static,
    {
        println!("Starting actor");
        ActorRuntime::run(self)
    }
}

/// The main trait to implement on an [Actor] to enable it to handle messages.
#[async_trait]
pub trait Handler<M>: Actor
where
    M: Message,
{
    async fn handle(&mut self, message: M) -> Result<M::Response, Error>;
}

/// A handle to a spawned actor. Obtained when calling `start` on an [Actor] and is used to send messages
/// to it.
#[derive(Debug, Clone)]
pub struct ActorHandle<A>
where
    A: Actor,
{
    message_tx: Sender<Envelope<A>>,
    command_tx: Sender<ActorCommand>,
}

impl<A> ActorHandle<A>
where
    A: Actor,
{
    pub fn new(message_tx: Sender<Envelope<A>>, command_tx: Sender<ActorCommand>) -> Self {
        Self {
            message_tx,
            command_tx,
        }
    }

    /// Sends a message to the actor and returns a [MessageRequest] that can
    /// be awaited. This method should be used when one needs a response from the
    /// actor.
    pub fn send_wait<M>(&self, message: M) -> Result<MessageRequest<M::Response>, SendError<M>>
    where
        M: Message + Send,
        A: Handler<M> + Enveloper<A, M>,
    {
        if self.message_tx.is_full() || self.message_tx.is_disconnected() {
            return Err(SendError(message));
        }
        let (tx, rx) = oneshot::channel();
        let _ = self.message_tx.send(A::pack(message, Some(tx)));
        Ok(MessageRequest { response_rx: rx })
    }

    /// Send a message to the actor without waiting for any response, but still returning an
    /// error if the channel is full.
    pub fn send<M>(&self, message: M) -> Result<(), SendError<M>>
    where
        M: Message + Send + 'static,
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
        M: Message + Send + 'static,
        A: Handler<M> + Enveloper<A, M> + 'static,
    {
        let _ = self.message_tx.send(A::pack(message, None));
    }

    pub fn send_cmd(&self, cmd: ActorCommand) -> Result<(), SendError<ActorCommand>> {
        self.command_tx.send(cmd)
    }

    pub fn recipient<M>(&self) -> Recipient<M>
    where
        M: Message + Send + 'static,
        M::Response: Send,
        A: Handler<M> + 'static,
    {
        Recipient {
            message_tx: Box::new(self.message_tx.clone()),
            command_tx: self.command_tx.clone(),
        }
    }
}

/// The same as an [ActorHandle], but instead of being tied to a specific actor, it is only
/// tied to the message type. Can be obtained from an [ActorHandle].
///
/// Useful for grouping different types of actors that can handle the same message.
pub struct Recipient<M>
where
    M: Message,
{
    message_tx: Box<dyn DynamicSender<M>>,
    command_tx: Sender<ActorCommand>,
}

impl<M> Recipient<M>
where
    M: Message + Send,
{
    pub fn send_wait(&self, message: M) -> Result<MessageRequest<M::Response>, SendError<M>> {
        self.message_tx.send_sync(message)
    }

    pub fn send(&self, message: M) -> Result<(), SendError<M>> {
        self.message_tx.send(message)
    }

    pub fn send_forget(&self, message: M) {
        let _ = self.message_tx.send(message);
    }

    pub fn send_cmd(&self, cmd: ActorCommand) -> Result<(), SendError<ActorCommand>> {
        self.command_tx.send(cmd)
    }
}

/// A helper trait used solely by [Recipient]'s message channel to erase the actor type.
/// This is achieved by implementing it on [Sender<Envelope<A>].
trait DynamicSender<M>
where
    M: Message + Send,
{
    fn send_sync(&self, message: M) -> Result<MessageRequest<M::Response>, SendError<M>>;

    fn send(&self, message: M) -> Result<(), SendError<M>>;
}

impl<A, M> DynamicSender<M> for Sender<Envelope<A>>
where
    M: Message + Send + 'static,
    M::Response: Send,
    A: Actor + Handler<M> + Enveloper<A, M>,
{
    fn send(&self, message: M) -> Result<(), SendError<M>> {
        if self.is_full() {
            return Err(SendError(message));
        }
        let _ = self.send(A::pack(message, None));
        Ok(())
    }

    fn send_sync(
        &self,
        message: M,
    ) -> Result<MessageRequest<<M as Message>::Response>, SendError<M>> {
        if self.is_full() {
            return Err(SendError(message));
        }
        let (tx, rx) = oneshot::channel();
        let _ = self.send(A::pack(message, Some(tx)));
        Ok(MessageRequest { response_rx: rx })
    }
}

impl<A, M> From<ActorHandle<A>> for Recipient<M>
where
    M: Message + Send + 'static,
    M::Response: Send,
    A: Actor + Handler<M> + Enveloper<A, M> + 'static,
{
    /// Just calls `ActorHandler::recipient`, i.e. clones the underlying channels
    /// into the recipient and boxes the message one.
    fn from(handle: ActorHandle<A>) -> Self {
        handle.recipient()
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

#[derive(Debug)]
pub enum ActorCommand {
    Stop,
}

#[cfg(test)]
mod tests {

    use std::{sync::atomic::AtomicUsize, time::Duration};

    use super::*;

    #[tokio::test]
    async fn it_works_sync() {
        #[derive(Debug)]
        struct Testor {}

        #[derive(Debug)]
        struct Foo {}

        #[derive(Debug)]
        struct Bar {}

        impl Message for Foo {
            type Response = usize;
        }

        impl Message for Bar {
            type Response = isize;
        }

        impl Actor for Testor {}

        #[async_trait]
        impl Handler<Foo> for Testor {
            async fn handle(&mut self, _: Foo) -> Result<usize, Error> {
                println!("Handling Foo");
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            async fn handle(&mut self, _: Bar) -> Result<isize, Error> {
                println!("Handling Bar");
                Ok(10)
            }
        }

        let handle = Testor {}.start();

        let mut res = 0;
        let mut res2 = 0;
        for _ in 0..100 {
            res += handle.send_wait(Foo {}).unwrap().await.unwrap();
            res2 += handle.send_wait(Bar {}).unwrap().await.unwrap();
        }

        handle.send(Foo {}).unwrap();
        handle.send_forget(Bar {});

        let rec: Recipient<Foo> = handle.recipient();
        res += rec.send_wait(Foo {}).unwrap().await.unwrap();
        handle.send_cmd(ActorCommand::Stop).unwrap();

        assert_eq!(res, 1010);
        assert_eq!(res2, 1000);
    }

    #[tokio::test]
    async fn it_works_yolo() {
        #[derive(Debug)]
        struct Testor {}

        #[derive(Debug)]
        struct Foo {}

        #[derive(Debug)]
        struct Bar {}

        impl Message for Foo {
            type Response = usize;
        }

        impl Message for Bar {
            type Response = isize;
        }

        impl Actor for Testor {}

        static COUNT: AtomicUsize = AtomicUsize::new(0);

        #[async_trait]
        impl Handler<Foo> for Testor {
            async fn handle(&mut self, _: Foo) -> Result<usize, Error> {
                println!("INCREMENTING COUNT FOO");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
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
    }
}
