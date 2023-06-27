use crate::{message::PackedMessage, Actor, ActorCommand, ActorHandle, Envelope, Error};
use futures::Future;
use pin_project::pin_project;
use std::{collections::VecDeque, task::Poll};
use tokio::sync::mpsc::Receiver;

pub trait Runtime<A> {
    fn start(actor: A) -> ActorHandle<A>
    where
        A: Actor + Send + 'static,
    {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(100);
        let rt = DefaultActorRuntime::new(actor, cmd_rx, rx);
        tokio::spawn(rt);
        ActorHandle {
            message_tx: tx,
            command_tx: cmd_tx,
        }
    }
}

#[pin_project]
pub struct DefaultActorRuntime<A>
where
    A: Actor,
{
    actor: A,
    command_rx: Receiver<ActorCommand>,
    message_rx: Receiver<Envelope<A>>,
    message_queue: VecDeque<Envelope<A>>,
}

impl<A> Runtime<A> for DefaultActorRuntime<A> where A: Actor {}

impl<A> Future for DefaultActorRuntime<A>
where
    A: Actor,
{
    type Output = Result<(), Error>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        loop {
            // Poll command receiver
            match self.command_rx.poll_recv(cx) {
                std::task::Poll::Ready(Some(message)) => match message {
                    ActorCommand::Stop => {
                        println!("Actor stopping");
                        break Poll::Ready(Ok(())); // TODO drain the queue and all that graceful stuff
                    }
                },
                std::task::Poll::Ready(None) => {
                    println!("Command channel closed, ungracefully stopping actor");
                    break Poll::Ready(Err(Error::ActorChannelClosed));
                }
                std::task::Poll::Pending => {}
            };

            // Process all messages
            while let Some(mut message) = self.message_queue.pop_front() {
                message.handle(&mut self.actor)
            }

            // Poll message receiver and continue to process if anything comes up
            let mut new_messages = false;
            while let Poll::Ready(Some(message)) = self.message_rx.poll_recv(cx) {
                self.message_queue.push_back(message);
                new_messages = true;
            }

            match self.message_rx.poll_recv(cx) {
                std::task::Poll::Ready(Some(message)) => {
                    self.message_queue.push_back(message);
                    continue;
                }
                std::task::Poll::Ready(None) => {
                    println!("Message channel closed, ungracefully stopping actor");
                    break Poll::Ready(Err(Error::ActorChannelClosed));
                }
                std::task::Poll::Pending => {
                    if new_messages {
                        continue;
                    }
                }
            };

            return std::task::Poll::Pending;
        }
    }
}

impl<A> DefaultActorRuntime<A>
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
