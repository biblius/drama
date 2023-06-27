use crate::{message::PackedMessage, Actor, ActorCommand, ActorHandle, Envelope, Error};
use flume::Receiver;
use futures::Future;
use pin_project::pin_project;
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};

pub trait Runtime<A> {
    fn run(actor: A) -> ActorHandle<A>
    where
        A: Actor + Send + 'static,
    {
        let (tx, rx) = flume::unbounded();
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let rt = ActorRuntime::new(actor, cmd_rx, rx);
        tokio::spawn(rt);
        ActorHandle {
            message_tx: tx,
            command_tx: cmd_tx,
        }
    }
}

#[pin_project]
pub struct ActorRuntime<A>
where
    A: Actor,
{
    actor: A,
    command_rx: Receiver<ActorCommand>,
    message_rx: Receiver<Envelope<A>>,
    message_queue: VecDeque<Envelope<A>>,
}

impl<A> Runtime<A> for ActorRuntime<A> where A: Actor {}

impl<A> Future for ActorRuntime<A>
where
    A: Actor,
{
    type Output = Result<(), Error>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        loop {
            // Poll command receiver
            match Pin::new(&mut this.command_rx.recv_async()).poll(cx) {
                Poll::Ready(Ok(message)) => match message {
                    ActorCommand::Stop => {
                        println!("Actor stopping");
                        break Poll::Ready(Ok(())); // TODO drain the queue and all that graceful stuff
                    }
                },
                Poll::Ready(Err(_)) => {
                    println!("Command channel closed, ungracefully stopping actor");
                    break Poll::Ready(Err(Error::ActorChannelClosed));
                }
                Poll::Pending => {}
            };

            // Process all messages
            while let Some(mut message) = this.message_queue.pop_front() {
                message.handle(this.actor)
            }

            // Poll message receiver and continue to process if anything comes up
            while let Poll::Ready(Ok(message)) =
                Pin::new(&mut this.message_rx.recv_async()).poll(cx)
            {
                this.message_queue.push_back(message);
            }

            // Poll again and process new messages if any
            match Pin::new(&mut this.message_rx.recv_async()).poll(cx) {
                Poll::Ready(Ok(message)) => {
                    this.message_queue.push_back(message);
                    continue;
                }
                Poll::Ready(Err(_)) => {
                    println!("Message channel closed, ungracefully stopping actor");
                    break Poll::Ready(Err(Error::ActorChannelClosed));
                }
                Poll::Pending => {
                    if !this.message_queue.is_empty() {
                        continue;
                    }
                }
            };
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
    }
}

impl<A> ActorRuntime<A>
where
    A: Actor,
{
    pub fn new(
        actor: A,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<A>>,
    ) -> Self {
        println!("Building default runtime");
        Self {
            actor,
            command_rx,
            message_rx,
            message_queue: VecDeque::new(),
        }
    }
}
