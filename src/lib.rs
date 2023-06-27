use crate::runtime::{ActorRuntime, Runtime};
use flume::{SendError, Sender};
use message::{Envelope, Message, MessagePacker, MessageRequest};
use std::fmt::Debug;
use tokio::sync::oneshot;
pub mod debug;
pub mod message;
pub mod runtime;
pub mod ws;

pub trait Actor {
    fn start(self) -> ActorHandle<Self>
    where
        Self: Sized + Send + 'static,
    {
        println!("Starting actor");
        ActorRuntime::run(self)
    }
}

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
    A: Actor + 'static,
{
    pub async fn send_sync<M>(&self, message: M) -> Result<M::Response, Error>
    where
        M: Message + Send + 'static,
        A: Handler<M> + MessagePacker<A, M>,
    {
        let (tx, rx) = oneshot::channel();
        let packed = A::pack(message, Some(tx));
        self.message_tx
            .send(packed)
            .map_err(Error::send_err_boxed)?;
        MessageRequest { response_rx: rx }.await
    }

    pub async fn send<M>(&self, message: M) -> Result<(), Error>
    where
        M: Message + Send + 'static,
        A: Handler<M> + MessagePacker<A, M> + 'static,
    {
        let packed = A::pack(message, None);
        self.message_tx.send(packed).map_err(Error::send_err_boxed)
    }

    pub async fn send_cmd(&self, cmd: ActorCommand) -> Result<(), Error> {
        self.command_tx.send(cmd).unwrap();
        Ok(())
    }
}

pub trait Handler<M>: Actor
where
    M: Message,
{
    fn handle(&mut self, message: M) -> Result<M::Response, Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Actor channel closed")]
    ActorChannelClosed,
    #[error("Channel closed: {0}")]
    ChannelClosed(#[from] oneshot::error::TryRecvError),
    #[error("Send error: {0}")]
    Send(Box<dyn std::error::Error + Send + 'static>),
    #[error("Warp error: {0}")]
    Warp(#[from] warp::Error),
}

impl Error {
    fn send_err_boxed<T: Send + 'static>(error: SendError<T>) -> Self {
        Self::Send(Box::new(error))
    }
}

#[derive(Debug)]
pub enum ActorCommand {
    Stop,
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn it_works() {
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

        impl Handler<Foo> for Testor {
            fn handle(&mut self, _: Foo) -> Result<usize, Error> {
                println!("Handling Foo");
                Ok(10)
            }
        }

        impl Handler<Bar> for Testor {
            fn handle(&mut self, _: Bar) -> Result<isize, Error> {
                println!("Handling Bar");
                Ok(10)
            }
        }

        let handle = Testor {}.start();

        let mut res = 0;
        let mut res2 = 0;
        for _ in 0..100 {
            res += handle.send_sync(Foo {}).await.unwrap();
            res2 += handle.send_sync(Bar {}).await.unwrap();
        }

        handle.send_cmd(ActorCommand::Stop).await.unwrap();

        assert_eq!(res, 1000);
        assert_eq!(res2, 1000);
    }
}
