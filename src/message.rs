use std::sync::Arc;

use crate::{Actor, Error, Handler};
use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio::sync::Mutex;

/// Represents a message that can be sent to an actor. The response type is what the actor must return in its handler implementation.
pub trait Message {
    type Response;
}

/// Represents a type erased message that ultimately gets stored in an [Envelope]. We need this indirection so we can abstract away the concrete message
/// type when creating an actor handle, otherwise we would only be able to send a single message type to the actor.
#[async_trait]
pub trait ActorMessage<A: Actor> {
    async fn handle(self: Box<Self>, actor: Arc<Mutex<A>>);
}

/// Used by [ActorHandle][super::ActorHandle]s to pack [Message]s into [Envelope]s so we have a type erased message to send to the actor.
pub trait Enveloper<A: Actor, M: Message> {
    /// Wrap a message in an envelope with an optional response channel.
    fn pack(message: M, tx: Option<oneshot::Sender<M::Response>>) -> Envelope<A>;
}

/// A type erased wrapper for messages. This wrapper essentially enables us to send any message to the actor
/// so long as it implements the necessary handler.
pub struct Envelope<A: Actor> {
    message: Box<dyn ActorMessage<A> + Send + Sync>,
}

impl<A> Envelope<A>
where
    A: Actor,
{
    pub fn new<M>(message: M, tx: Option<oneshot::Sender<M::Response>>) -> Self
    where
        A: Handler<M> + Send + 'static,
        M: Message + Send + 'static + Sync,
        M::Response: Send,
    {
        Self {
            message: Box::new(EnvelopeInner {
                message: Some(Box::new(message)),
                tx,
            }),
        }
    }
}

/// The inner parts of the [Envelope] containing the actual message as well as an optional
/// response channel.
struct EnvelopeInner<M: Message> {
    message: Option<Box<M>>,
    tx: Option<oneshot::Sender<M::Response>>,
}

#[async_trait]
impl<A> ActorMessage<A> for Envelope<A>
where
    A: Actor + Send,
{
    async fn handle(self: Box<Self>, actor: Arc<Mutex<A>>) {
        ActorMessage::handle(self.message, actor).await
    }
}

#[async_trait]
impl<A, M> ActorMessage<A> for EnvelopeInner<M>
where
    M: Message + Send + Sync,
    M::Response: Send,
    A: Actor + Handler<M> + Send + 'static,
{
    async fn handle(self: Box<Self>, actor: Arc<Mutex<A>>) {
        let Some(message) = self.message else { panic!("Handle already called") };
        let result = A::handle(actor, message).await;
        if let Some(res_tx) = self.tx {
            let _ = res_tx.send(result.unwrap());
        }
    }
}

impl<A, M> Enveloper<A, M> for A
where
    A: Actor + Handler<M> + Send + 'static,
    M: Message + Send + 'static + Sync,
    M::Response: Send,
{
    fn pack(message: M, tx: Option<oneshot::Sender<M::Response>>) -> Envelope<A> {
        Envelope::new(message, tx)
    }
}

pub struct MessageRequest<R> {
    pub response_rx: oneshot::Receiver<R>,
}

impl<R> std::future::Future for MessageRequest<R> {
    type Output = Result<R, Error>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.as_mut().response_rx.try_recv() {
            Ok(response) => std::task::Poll::Ready(Ok(response)),
            Err(e) => match e {
                oneshot::error::TryRecvError::Empty => {
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
                oneshot::error::TryRecvError::Closed => std::task::Poll::Ready(Err(e.into())),
            },
        }
    }
}
