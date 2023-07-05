use crate::{
    actor::{Actor, ActorHandle},
    message::MailboxReceiver,
    ActorCommand, Error,
};
use async_trait::async_trait;
use flume::{Receiver, Sender};
use futures::{Stream, StreamExt};
use std::fmt::Display;

/// Represents an actor that has access to a stream and a sender channel
/// which it can use to respond.
///
/// The intended usecase for this is to have a receiver channel for responses
/// that forwards anything it receives to the sink. This can be achieved
/// by creating a stream from the receiver (many channel implementations provide this)
/// which forwards to the sink. The receiver stream is then spawned to a runtime.
///
/// ### Example
///
/// ```ignore
/// // Imagine if you will, a websocket
/// ws.on_upgrade(|socket| async move {
///   let (sink, stream) = socket.split();
///   let (tx, rx) = flume::unbounded();
///   
///   let actor = WebsocketActor::new();
///   let handle = actor.start_relay(tx);
///   tokio::spawn(rx.into_stream().map(Ok).forward(si));
/// })
/// ```
///
/// A relay actor receives messages via its stream and processes them with
/// its [Relay] implementation. A relay that returns `None` will not forward any
/// response to the sink. If the relay returns `Some(M)` it will be forwarded to the sink.
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
        tracing::trace!("Starting relay actor");
        let (message_tx, message_rx) = flume::unbounded();
        let (command_tx, command_rx) = flume::unbounded();
        tokio::spawn(RelayRuntime::new(self, command_rx, message_rx, stream, sender).run());
        ActorHandle::new(message_tx, command_tx)
    }
}

#[async_trait]
pub trait Relay<M>: Actor {
    async fn process(&mut self, message: M) -> Option<M>;
}

pub struct RelayRuntime<A, M, Str>
where
    A: RelayActor<M, Str> + Relay<M>,
    Str: Stream<Item = Result<M, A::Error>> + Unpin + Send + 'static,
    M: Send + 'static,
    A::Error: Send,
{
    actor: A,

    /// The receiving end of the websocket
    stream: Str,

    /// The sending end of the stream. Hooked to a receiver that forwards any
    /// response sent from here.
    sender: Sender<M>,

    /// Actor command receiver
    commands: Receiver<ActorCommand>,

    /// Actor message receiver
    mailbox: MailboxReceiver<A>,
}

impl<A, M, Str> RelayRuntime<A, M, Str>
where
    Str: Stream<Item = Result<M, A::Error>> + Send + Unpin,
    A: RelayActor<M, Str> + Relay<M>,
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
            sender,
            stream,
            mailbox,
            commands: command_rx,
        }
    }

    pub async fn run(mut self) -> Result<(), Error> {
        loop {
            // Only time we error here is when we disconnect, stream errors are just logged
            tokio::select! {
                command = self.commands.recv_async() => {
                    let Ok(command) = command else { return Err(Error::ActorDisconnected); };
                    match command {
                        ActorCommand::Stop => {
                            tracing::trace!("Relay actor stopping");
                            return Ok(())
                        },
                    }
                }
                message = self.mailbox.recv_async() => {
                    let Ok(mut message) = message else { return Err(Error::ActorDisconnected); };
                    message.handle(&mut self.actor).await;
                }
                ws_msg = self.stream.next() => {
                    let Some(ws_msg) = ws_msg else { return Err(Error::RelayDisconnected) };
                    match ws_msg {
                        Ok(msg) => {
                            let res = self.actor.process(msg).await;
                            if let Some(res) = res {
                                let Ok(_) = self.sender.send_async(res).await else { return Err(Error::RelayDisconnected) };
                            }
                        },
                        Err(e) => {
                            tracing::error!("Stream error occurred: {e}")
                        },
                    }
                }
            }
        }
    }
}
