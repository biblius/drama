use crate::runtime::{ActorRuntime, Runtime};
use async_trait::async_trait;
use flume::{SendError, Sender};
use message::{Envelope, Enveloper, Message, MessageRequest};
use std::{fmt::Debug, sync::Arc};
use tokio::sync::oneshot;
use tokio::sync::Mutex;

pub mod debug;
pub mod message;
pub mod runtime;
pub mod ws;

pub struct Hello {}

impl Actor for Hello {}

pub struct Msg {
    pub content: String,
}
impl Message for Msg {
    type Response = usize;
}

#[async_trait]
impl Handler<Msg> for Hello {
    async fn handle(_: Arc<Mutex<Self>>, _: Box<Msg>) -> Result<usize, Error> {
        println!("Handling message Hello");
        Ok(10)
    }
}

const DEFAULT_CHANNEL_CAPACITY: usize = 128;

#[async_trait]
pub trait Actor {
    async fn start(self) -> ActorHandle<Self>
    where
        Self: Sized + Send + 'static,
    {
        println!("Starting actor");
        ActorRuntime::run(Arc::new(Mutex::new(self))).await
    }
}

/// The main trait to implement on an [Actor] to enable it to handle messages.
#[async_trait]
pub trait Handler<M>: Actor
where
    M: Message,
{
    async fn handle(this: Arc<Mutex<Self>>, message: Box<M>) -> Result<M::Response, Error>;
}

/// A handle to a spawned actor. Obtained when calling `start` on an [Actor] and is used to send messages
/// to it.
#[derive(Debug)]
pub struct ActorHandle<A>
where
    A: Actor,
{
    message_tx: Sender<Envelope<A>>,
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

    /// Send a message to the actor without returning a response, but still returning an
    /// error if the channel is full or disconnected.
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
        M: Message + Send + 'static + Sync,
        M::Response: Send,
        A: Handler<M> + Send + 'static,
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
    message_tx: Box<dyn MessageSender<M>>,
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
trait MessageSender<M>
where
    M: Message + Send,
{
    fn send_sync(&self, message: M) -> Result<MessageRequest<M::Response>, SendError<M>>;

    fn send(&self, message: M) -> Result<(), SendError<M>>;
}

impl<A, M> MessageSender<M> for Sender<Envelope<A>>
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
    M: Message + Send + 'static + Sync,
    M::Response: Send,
    A: Actor + Handler<M> + Enveloper<A, M> + Send + 'static,
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

    use tokio::task::LocalSet;

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
            async fn handle(_: Arc<Mutex<Self>>, _: Box<Foo>) -> Result<usize, Error> {
                println!("Handling Foo");
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            async fn handle(_: Arc<Mutex<Self>>, _: Box<Bar>) -> Result<isize, Error> {
                for _ in 0..10_000 {
                    println!("Handling Bar");
                }
                Ok(10)
            }
        }

        let mut res = 0;
        let mut res2 = 0;

        let handle = Testor {}.start().await;
        println!("HELLO WORLDS");
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

    #[test]
    fn it_works_yolo() {
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
            async fn handle(_: Arc<Mutex<Testor>>, _: Box<Foo>) -> Result<usize, Error> {
                println!("INCREMENTING COUNT FOO");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            async fn handle(_: Arc<Mutex<Testor>>, _: Box<Bar>) -> Result<isize, Error> {
                println!("INCREMENTING COUNT BAR");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local_set = LocalSet::new();

        let task = async {
            let handle = Testor {}.start().await;

            handle.send_wait(Bar {}).unwrap().await.unwrap();
            handle.send(Foo {}).unwrap();
            handle.send_forget(Bar {});

            for _ in 0..100 {
                let _ = handle.send(Foo {});
                handle.send_forget(Bar {});
                tokio::time::sleep(Duration::from_micros(100)).await
            }

            assert_eq!(COUNT.load(std::sync::atomic::Ordering::Relaxed), 203);
            handle.send_cmd(ActorCommand::Stop)
        };

        local_set.spawn_local(task);
        rt.block_on(local_set)
    }
}
