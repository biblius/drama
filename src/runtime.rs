use crate::{
    message::ActorMessage, Actor, ActorCommand, ActorHandle, Envelope, Error,
    DEFAULT_CHANNEL_CAPACITY,
};
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
        let (message_tx, message_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        let (command_tx, command_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        tokio::spawn(ActorRuntime::new(actor, command_rx, message_rx));
        ActorHandle::new(message_tx, command_tx)
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

            // Process all pending messages
            let mut idx = 0;
            while idx < this.message_queue.len() {
                let pending = &mut this.message_queue[idx];
                match pending.handle(this.actor, cx) {
                    Poll::Ready(_) => {
                        this.message_queue.swap_remove_front(idx);
                        continue;
                    }
                    Poll::Pending => idx += 1,
                }
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
