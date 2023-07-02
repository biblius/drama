use std::sync::Arc;

use crate::{Actor, Error, Handler};
use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio::sync::Mutex;

/// Represents a type erased message that ultimately gets stored in an [Envelope]. We need this indirection so we can abstract away the concrete message
/// type when creating an actor handle, otherwise we would only be able to send a single message type to the actor.
#[async_trait]
pub trait ActorMessage<A: Actor> {
    async fn handle(self: Box<Self>, actor: Arc<Mutex<A>>);
}

/// Used by [ActorHandle][super::ActorHandle]s to pack messages into [Envelope]s so we have a type erased message to send to the actor.
pub trait Enveloper<A, M>
where
    A: Handler<M>,
{
    /// Wrap a message in an envelope with an optional response channel.
    fn pack(message: M, tx: Option<oneshot::Sender<<A as Handler<M>>::Response>>) -> Envelope<A>;
}

/// A type erased wrapper for messages. This wrapper essentially enables us to send any message to the actor
/// so long as it implements the necessary handler.
pub struct Envelope<A>
where
    A: Actor,
{
    message: Box<dyn ActorMessage<A> + Send>,
}

impl<A> Envelope<A>
where
    A: Actor,
{
    pub fn new<M>(message: M, tx: Option<oneshot::Sender<A::Response>>) -> Self
    where
        A: Handler<M> + Send + 'static,
        A::Response: Send,
        M: Send + 'static,
    {
        Self {
            message: Box::new(EnvelopeInner {
                message: Box::new(message),
                tx,
            }),
        }
    }
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

/// The inner parts of the [Envelope] containing the actual message as well as an optional
/// response channel.
struct EnvelopeInner<M, R> {
    message: Box<M>,
    tx: Option<oneshot::Sender<R>>,
}

#[async_trait]
impl<A, M> ActorMessage<A> for EnvelopeInner<M, <A as Handler<M>>::Response>
where
    A: Handler<M> + Send + 'static,
    A::Response: Send,
    M: Send,
{
    async fn handle(self: Box<Self>, actor: Arc<Mutex<A>>) {
        let result = A::handle(actor, self.message).await;
        if let Some(res_tx) = self.tx {
            let _ = res_tx.send(result.unwrap());
        }
    }
}

impl<A, M> Enveloper<A, M> for A
where
    A: Handler<M> + Send + 'static,
    A::Response: Send,
    M: Send + Sync + 'static,
{
    fn pack(message: M, tx: Option<oneshot::Sender<<A as Handler<M>>::Response>>) -> Envelope<A> {
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
