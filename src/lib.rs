use std::fmt::Debug;
use tokio::sync::oneshot;

mod actor;
mod message;
mod relay;

pub use actor::{Actor, ActorHandle, Recipient};
pub use message::Handler;
pub use relay::{Relay, RelayActor};

#[derive(Debug)]
pub enum ActorCommand {
    Stop,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
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
        struct Testor {
            foos: usize,
            bars: isize,
        }

        #[derive(Debug, Clone)]
        struct Foo {}

        #[derive(Debug, Clone)]
        struct Bar {}

        impl Actor for Testor {}

        #[async_trait]
        impl Handler<Foo> for Testor {
            type Response = usize;
            async fn handle(&mut self, _: &Foo) -> usize {
                tracing::trace!("Handling Foo");
                self.foos += 1;
                10
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            type Response = isize;
            async fn handle(&mut self, _: &Bar) -> isize {
                tracing::trace!("Handling Bar");
                self.bars += 1;
                if self.foos == 100 {
                    assert_eq!(self.bars, 100);
                }
                10
            }
        }

        let mut res = 0;
        let mut res2 = 0;

        let handle = Testor { foos: 0, bars: 0 }.start();
        tracing::trace!("HELLO WORLDS");
        for _ in 0..100 {
            res += handle.send_wait(Foo {}).await.unwrap();
            res2 += handle.send_wait(Bar {}).await.unwrap();
        }

        handle.send(Foo {}).unwrap();
        handle.send_forget(Bar {});

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
                tracing::trace!("INCREMENTING COUNT FOO");
                COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(10)
            }
        }

        #[async_trait]
        impl Handler<Bar> for Testor {
            type Response = Result<isize, Error>;
            async fn handle(&mut self, _: &Bar) -> Result<isize, Error> {
                tracing::trace!("INCREMENTING COUNT BAR");
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
