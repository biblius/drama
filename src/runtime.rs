use crate::{
    message::ActorMessage, Actor, ActorCommand, ActorHandle, Envelope, Error,
    DEFAULT_CHANNEL_CAPACITY,
};
use flume::Receiver;
use futures::Future;
use parking_lot::Mutex;
use pin_project::pin_project;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

const DEFAULT_QUEUE_CAPACITY: usize = 16;

pub trait Runtime<A> {
    fn run(actor: Arc<Mutex<A>>) -> ActorHandle<A>
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
    A: Actor + Send + 'static,
{
    actor: Arc<Mutex<A>>,
    command_rx: Receiver<ActorCommand>,
    message_rx: Receiver<Envelope<A>>,
    process_queue: VecDeque<ActorJob<A>>,
}

impl<A> Runtime<A> for ActorRuntime<A> where A: Actor + Send + 'static {}

impl<A> Future for ActorRuntime<A>
where
    A: Actor + Send + 'static,
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

            // Process the pending futures
            this.process_queue
                .retain_mut(|job| job.poll(&mut this.actor.lock(), cx).is_pending());

            // Poll message receiver
            while let Poll::Ready(Ok(message)) =
                Pin::new(&mut this.message_rx.recv_async()).poll(cx)
            {
                // this.process_queue.push_back(ActorJob::new(message));
                if this.process_queue.len() >= DEFAULT_QUEUE_CAPACITY {
                    break;
                }
            }

            // Process pending futures again after we've potentially received some
            this.process_queue
                .retain_mut(|job| job.poll(&mut this.actor.lock(), cx).is_pending());

            // Poll again and process new messages if any
            match Pin::new(&mut this.message_rx.recv_async()).poll(cx) {
                Poll::Ready(Ok(message)) => {
                    // this.process_queue.push_back(ActorJob::new(message));
                    continue;
                }
                Poll::Ready(Err(_)) => {
                    println!("Message channel closed, ungracefully stopping actor");
                    break Poll::Ready(Err(Error::ActorChannelClosed));
                }
                Poll::Pending => {
                    if !this.process_queue.is_empty() {
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
    A: Actor + 'static + Send,
{
    pub fn new(
        actor: Arc<Mutex<A>>,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<A>>,
    ) -> Self {
        println!("Building default runtime");
        Self {
            actor,
            command_rx,
            message_rx,
            process_queue: VecDeque::with_capacity(DEFAULT_QUEUE_CAPACITY),
        }
    }
}

pub trait ActorFuture<A>
where
    A: Actor,
{
    type Output;
    fn poll(
        self: Pin<&mut Self>,
        actor: &mut A,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output>;
}

struct ActorJob<A>(pub Pin<Box<dyn ActorFuture<A, Output = ()> + Send + 'static>>);

impl<A> ActorJob<A>
where
    A: Actor,
{
    fn new<F>(future: F) -> Self
    where
        F: ActorFuture<A, Output = ()> + Send + 'static,
    {
        Self(Box::pin(future))
    }

    fn poll(&mut self, actor: &mut A, cx: &mut std::task::Context<'_>) -> Poll<()> {
        self.0.as_mut().poll(actor, cx)
    }
}

/* impl<A> ActorFuture<A> for Envelope<A>
where
    A: Actor,
{
    type Output = ();

    fn poll(
        self: Pin<&mut Self>,
        actor: &mut A,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        self.get_mut().handle(actor).as_mut().poll(cx)
    }
} */
