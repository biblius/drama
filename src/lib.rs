use std::fmt::Debug;
use tokio::sync::oneshot;

pub mod actor;
pub mod message;
pub mod relay;
pub mod runtime;

#[derive(Debug)]
pub enum ActorCommand {
    Stop,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Actor channel closed")]
    ActorDisconnected,
    #[error("Relay stream closed")]
    RelayDisconnected,
    #[error("Channel closed: {0}")]
    ChannelClosed(#[from] oneshot::error::TryRecvError),
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{
        actor::{Actor, Recipient},
        message::Handler,
    };
    use async_trait::async_trait;
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
            async fn handle(&mut self, _: &Foo) -> usize {
                println!("Handling Foo");
                10
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            type Response = isize;
            async fn handle(&mut self, _: &Bar) -> isize {
                for _ in 0..10_000 {
                    println!("Handling Bar");
                }
                10
            }
        }

        let mut res = 0;
        let mut res2 = 0;

        let handle = Testor {}.start();
        println!("HELLO WORLDS");
        for _ in 0..100 {
            res += handle.send_wait(Foo {}).await.unwrap();
            res2 += handle.send_wait(Bar {}).await.unwrap();
        }

        handle.send(Foo {}).unwrap();
        handle.send_forget(Bar {});
        handle.send_cmd(ActorCommand::Stop).unwrap();

        let rec: Recipient<Foo> = handle.recipient();
        rec.send(Foo {}).unwrap();
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
            type Response = Result<usize, Error>;
            async fn handle(&mut self, _: &Foo) -> Result<usize, Error> {
                println!("INCREMENTING COUNT FOO");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            type Response = Result<isize, Error>;
            async fn handle(&mut self, _: &Bar) -> Result<isize, Error> {
                println!("INCREMENTING COUNT BAR");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        let handle = Testor {}.start();

        handle.send_wait(Bar {}).await.unwrap().unwrap();
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
