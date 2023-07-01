use crate::{
    message::ActorMessage, Actor, ActorCommand, ActorHandle, Envelope, Error,
    DEFAULT_CHANNEL_CAPACITY,
};
use async_trait::async_trait;
use flume::Receiver;
use futures::Future;
use pin_project::pin_project;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::Mutex;

const QUEUE_CAPACITY: usize = 128;

#[async_trait]
pub trait Runtime<A> {
    async fn run(actor: Arc<Mutex<A>>) -> ActorHandle<A>
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

type ActorFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

struct ActorJob<A>
where
    A: Actor,
{
    message: Option<Envelope<A>>,
    future: Option<ActorFuture>,
}

impl<A> ActorJob<A>
where
    A: Actor + Send + 'static,
{
    fn new(message: Envelope<A>) -> Self {
        Self {
            message: Some(message),
            future: None,
        }
    }

    fn poll(
        mut self: Pin<&mut Self>,
        actor: Arc<Mutex<A>>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<()> {
        match self.as_mut().message.take() {
            Some(message) => {
                let fut = Box::new(message).handle(actor);
                self.future = Some(fut);
                ready!(self.future.as_mut().unwrap().as_mut().poll(cx));
                Poll::Ready(())
            }
            None => {
                ready!(self.future.as_mut().unwrap().as_mut().poll(cx));
                Poll::Ready(())
            }
        }
    }
}

impl<A> Future for ActorRuntime<A>
where
    A: Actor + Send + 'static,
{
    type Output = Result<(), Error>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // Poll command receiver
        match Pin::new(&mut this.command_rx.recv_async()).poll(cx) {
            Poll::Ready(Ok(message)) => match message {
                ActorCommand::Stop => {
                    println!("Actor stopping");
                    return Poll::Ready(Ok(())); // TODO drain the queue and all that graceful stuff
                }
            },
            Poll::Ready(Err(_)) => {
                println!("Command channel closed, ungracefully stopping actor");
                return Poll::Ready(Err(Error::ActorChannelClosed));
            }
            Poll::Pending => {}
        };

        // Process the pending futures
        this.process_queue
            .retain_mut(|job| Pin::new(job).poll(this.actor.clone(), cx).is_pending());

        // Poll message receiver
        while let Poll::Ready(Ok(message)) = Pin::new(&mut this.message_rx.recv_async()).poll(cx) {
            this.process_queue.push_back(ActorJob::new(message));
            if this.process_queue.len() >= QUEUE_CAPACITY {
                break;
            }
        }

        // Process pending futures again after we've potentially received some
        this.process_queue
            .retain_mut(|job| Pin::new(job).poll(this.actor.clone(), cx).is_pending());

        // Poll again and process new messages if any
        match Pin::new(&mut this.message_rx.recv_async()).poll(cx) {
            Poll::Ready(Ok(message)) => {
                this.process_queue.push_back(ActorJob::new(message));
            }
            Poll::Ready(Err(_)) => {
                println!("Message channel closed, ungracefully stopping actor");
                return Poll::Ready(Err(Error::ActorChannelClosed));
            }
            Poll::Pending => {}
        };

        cx.waker().wake_by_ref();
        Poll::Pending
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
            process_queue: VecDeque::with_capacity(QUEUE_CAPACITY),
        }
    }
}
