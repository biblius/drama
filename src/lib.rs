use crate::runtime::{DefaultActorRuntime, Runtime};
use message::{Envelope, Message, MessagePacker, MessageRequest};
use std::fmt::Debug;
use tokio::sync::{mpsc::Sender, oneshot};

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
        DefaultActorRuntime::start(self)
    }
}

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
    pub async fn send_sync<M>(&self, message: M) -> Result<M::Response, Error>
    where
        M: Message + Send + 'static,
        A: Handler<M> + MessagePacker<A, M>,
    {
        let (tx, rx) = oneshot::channel();
        let packed = A::pack(message, Some(tx));
        self.message_tx.send(packed).await.unwrap(); // TODO
        MessageRequest { response_rx: rx }.await
    }

    pub async fn send<M>(&self, message: M) -> Result<(), Error>
    where
        M: Message + Send + 'static,
        A: Handler<M> + MessagePacker<A, M> + 'static,
    {
        let packed = A::pack(message, None);
        self.message_tx.send(packed).await.unwrap(); // TODO
        Ok(())
    }

    pub async fn send_cmd(&self, cmd: ActorCommand) -> Result<(), Error> {
        self.command_tx.send(cmd).await.unwrap(); // TODO
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
                Ok(10)
            }
        }

        impl Handler<Bar> for Testor {
            fn handle(&mut self, _: Bar) -> Result<isize, Error> {
                Ok(10)
            }
        }

        let handle = Testor {}.start();

        let res = handle.send_sync(Foo {}).await.unwrap();
        let res2 = handle.send_sync(Bar {}).await.unwrap();

        handle.send_cmd(ActorCommand::Stop).await.unwrap();

        assert_eq!(res, 10);
        assert_eq!(res2, 10);
    }
}
