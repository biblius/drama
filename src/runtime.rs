use crate::{
    message::ActorMessage, Actor, ActorCommand, ActorHandle, Envelope, Error,
    DEFAULT_CHANNEL_CAPACITY,
};
use flume::{r#async::RecvStream, Receiver};
use futures::{Future, StreamExt};
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::Mutex;

pub const QUEUE_CAPACITY: usize = 128;

pub trait Runtime<A>
where
    A: Actor + Send + 'static,
{
    fn command_stream(&mut self) -> &mut RecvStream<'static, ActorCommand>;

    fn message_stream(&mut self) -> &mut RecvStream<'static, Envelope<A>>;

    fn processing_queue(&mut self) -> &mut VecDeque<ActorJob<A>>;

    fn actor(&self) -> Arc<Mutex<A>>;

    fn at_capacity(&self) -> bool;

    fn run(actor: A) -> ActorHandle<A> {
        let (message_tx, message_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        let (command_tx, command_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        tokio::spawn(ActorRuntime::new(actor, command_rx, message_rx));
        ActorHandle::new(message_tx, command_tx)
    }

    fn process_commands(&mut self, cx: &mut Context<'_>) -> Result<Option<ActorCommand>, Error> {
        match self.command_stream().poll_next_unpin(cx) {
            Poll::Ready(Some(command)) => Ok(Some(command)),
            Poll::Ready(None) => {
                println!("Command channel closed, ungracefully stopping actor");
                Err(Error::ActorChannelClosed)
            }
            Poll::Pending => Ok(None),
        }
    }

    fn process_messages(&mut self, cx: &mut Context<'_>) -> Result<(), Error> {
        let actor = self.actor();

        self.processing_queue()
            .retain_mut(|job| job.poll(actor.clone(), cx).is_pending());

        // Poll message receiver
        if !self.at_capacity() {
            while let Poll::Ready(message) = self.message_stream().poll_next_unpin(cx) {
                let Some(message) = message else { return Err(Error::ActorChannelClosed) };
                self.processing_queue().push_back(ActorJob::new(message));
                if self.at_capacity() {
                    break;
                }
            }
        }

        // Process pending futures again after we've potentially received some
        self.processing_queue()
            .retain_mut(|job| job.poll(actor.clone(), cx).is_pending());

        if self.at_capacity() {
            return Ok(());
        }

        match self.message_stream().poll_next_unpin(cx) {
            Poll::Ready(Some(message)) => {
                self.processing_queue().push_back(ActorJob::new(message));
                Ok(())
            }
            Poll::Ready(None) => {
                println!("Message channel closed, ungracefully stopping actor");
                Err(Error::ActorChannelClosed)
            }
            Poll::Pending => Ok(()),
        }
    }
}

impl<A> Runtime<A> for ActorRuntime<A>
where
    A: Actor + Send + 'static,
{
    fn run(actor: A) -> ActorHandle<A> {
        let (message_tx, message_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        let (command_tx, command_rx) = flume::bounded(DEFAULT_CHANNEL_CAPACITY);
        tokio::spawn(ActorRuntime::new(actor, command_rx, message_rx));
        ActorHandle::new(message_tx, command_tx)
    }

    #[inline]
    fn processing_queue(&mut self) -> &mut VecDeque<ActorJob<A>> {
        &mut self.process_queue
    }

    #[inline]
    fn command_stream(&mut self) -> &mut RecvStream<'static, ActorCommand> {
        &mut self.command_stream
    }

    #[inline]
    fn message_stream(&mut self) -> &mut RecvStream<'static, Envelope<A>> {
        &mut self.message_stream
    }

    #[inline]
    fn actor(&self) -> Arc<Mutex<A>> {
        self.actor.clone()
    }

    #[inline]
    fn at_capacity(&self) -> bool {
        self.process_queue.len() >= QUEUE_CAPACITY
    }
}

/// A future representing a message currently being handled. Created when polling an [ActorJob].
pub type ActorFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

pub struct ActorRuntime<A>
where
    A: Actor + Send + 'static,
{
    actor: Arc<Mutex<A>>,
    command_stream: RecvStream<'static, ActorCommand>,
    message_stream: RecvStream<'static, Envelope<A>>,
    process_queue: VecDeque<ActorJob<A>>,
}

impl<A> ActorRuntime<A>
where
    A: Actor + 'static + Send,
{
    pub fn new(
        actor: A,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<A>>,
    ) -> Self {
        println!("Building default runtime");
        Self {
            actor: Arc::new(Mutex::new(actor)),
            command_stream: command_rx.into_stream(),
            message_stream: message_rx.into_stream(),
            process_queue: VecDeque::with_capacity(QUEUE_CAPACITY),
        }
    }
}

pub struct ActorJob<A>
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

    fn poll(&mut self, actor: Arc<Mutex<A>>, cx: &mut std::task::Context<'_>) -> Poll<()> {
        match self.message.take() {
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

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.as_mut();

        this.process_commands(cx)?;
        this.process_messages(cx)?;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
