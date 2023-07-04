use crate::{
    message::MailboxReceiver, Actor, ActorCommand, ActorHandle, Error, DEFAULT_CHANNEL_CAPACITY,
};
use async_trait::async_trait;
use flume::{r#async::RecvStream, Receiver, Sender};
use futures::{Stream, StreamExt};
use std::{fmt::Display, sync::atomic::AtomicUsize};

/// Represents an actor that has access to a stream and a sender channel
/// which it can respond to.
///
/// A websocket actor receives messages via the stream and processes them with
/// its [Handler] implementation. The handler implementation should always return an
/// `Option<M>` where M is the type used when implementing this trait. A handler that returns
/// `None` will not forward any response to the sink. If the handler returns `Some(M)` it will
/// be forwarded to the sink.
pub trait RelayActor<M, Str>: Actor
where
    Self: Relay<M>,
    Self::Error: Send,
    Str: Stream<Item = Result<M, Self::Error>> + Unpin + Send + 'static,
    M: Send + 'static,
{
    /// The error type of the underlying websocket implementation.
    type Error: Display;

    fn start_relay(self, stream: Str, sender: Sender<M>) -> ActorHandle<Self> {
        println!("Starting actor");
        let (message_tx, message_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        let (command_tx, command_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        tokio::spawn(RelayRuntime::new(self, command_rx, message_rx, stream, sender).runt());
        ActorHandle::new(message_tx, command_tx)
    }
}

#[async_trait]
pub trait Relay<M>: Actor {
    async fn handle(&mut self, message: M) -> Result<Option<M>, Error>;
}

pub struct RelayRuntime<A, M, Str>
where
    A: RelayActor<M, Str> + Relay<M> + 'static,
    Str: Stream<Item = Result<M, A::Error>> + Send + Unpin + 'static,
    M: Send + 'static,
    A::Error: Send,
{
    actor: A,

    /// The receiving end of the websocket
    ws_stream: Str,

    /// The sending end of the websocket. Hooked to a receiver that forwards any
    /// response sent from here.
    ws_sender: Sender<M>,

    /// Actor command receiver
    command_stream: RecvStream<'static, ActorCommand>,

    mailbox: MailboxReceiver<A>,
}

static PROCESS: AtomicUsize = AtomicUsize::new(0);

impl<A, M, Str> RelayRuntime<A, M, Str>
where
    Str: Stream<Item = Result<M, A::Error>> + Send + Unpin,
    A: RelayActor<M, Str> + Send + 'static + Relay<M>,
    M: Send,
    A::Error: Send,
{
    pub fn new(
        actor: A,
        command_rx: Receiver<ActorCommand>,
        mailbox: MailboxReceiver<A>,
        stream: Str,
        sender: Sender<M>,
    ) -> Self {
        Self {
            actor,
            ws_sender: sender,
            ws_stream: stream,
            mailbox,
            command_stream: command_rx.into_stream(),
        }
    }

    pub async fn runt(mut self) {
        loop {
            tokio::select! {
            Some(command) = self.command_stream.next() => {
               match command {
                        ActorCommand::Stop => {
                            println!("actor stopping");
                            return
                        },
                    }
            }
            message = self.mailbox.recv_async() => {
                if let Ok(mut message) = message {
                    message.handle(&mut self.actor).await;
                } else {
                    break;
                }
            }
            Some(ws_msg) = self.ws_stream.next() => {
                    match ws_msg {
                        Ok(msg) => {
                            let res = self.actor.handle(msg).await.unwrap();
                            PROCESS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                            println!("PROCESSED {}", PROCESS.load(std::sync::atomic::Ordering::Relaxed));
                            if let Some(res) = res {
                                self.ws_sender.send_async(res).await.unwrap();
                            }
                        },
                        Err(_) => todo!(),
                    }
            }
            }
        }
        println!("actor stopping");
    }
}
