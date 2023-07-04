use crate::{Actor, Error, Handler};
use async_trait::async_trait;
use std::marker::PhantomData;
use tokio::sync::oneshot;

#[async_trait]
pub trait MessageHandler<A: Actor>: Send + Sync {
    async fn handle(&mut self, actor: &mut A);
}

pub type BoxedMessageHandler<A> = Box<dyn MessageHandler<A>>;

pub type MailboxReceiver<A> = flume::Receiver<BoxedMessageHandler<A>>;
pub type MailboxSender<A> = flume::Sender<BoxedMessageHandler<A>>;

pub struct ActorMailbox<M, A: Handler<M>> {
    _phantom_actor: PhantomData<A>,
    _phantom_msg: PhantomData<M>,
}

/// Represents a type erased message that ultimately gets stored in an [Envelope]. We need this indirection so we can abstract away the concrete message
/// type when creating an actor handle, otherwise we would only be able to send a single message type to the actor.
/* #[async_trait]
pub trait ActorMessage<A: Actor> {
    async fn handle(self, actor: &mut A);
} */

/// Used by [ActorHandle][super::ActorHandle]s to pack messages into [Envelope]s so we have a type erased message to send to the actor.
pub trait Enveloper<A, M>
where
    A: Handler<M>,
    M: Clone + Send,
{
    /// Wrap a message in an envelope with an optional response channel.
    fn pack(
        message: M,
        tx: Option<oneshot::Sender<<A as Handler<M>>::Response>>,
    ) -> Box<dyn MessageHandler<A>>;
}

/// A type erased wrapper for messages. This wrapper essentially enables us to send any message to the actor
/// so long as it implements the necessary handler.
pub struct Envelope<M, A>
where
    A: Handler<M>,
    M: Clone + Send + 'static,
{
    message: M,
    response_tx: Option<oneshot::Sender<A::Response>>,
}

impl<M, A> Envelope<M, A>
where
    A: Handler<M> + Send + 'static,
    A::Response: Send,
    M: Clone + Send + Sync + 'static,
{
    pub fn new(message: M, tx: Option<oneshot::Sender<A::Response>>) -> Self {
        Self {
            message,
            response_tx: tx,
        }
    }
}

#[async_trait]
impl<M, A> MessageHandler<A> for Envelope<M, A>
where
    A: Actor + Handler<M> + Send,
    M: Clone + Send + Sync,
    A::Response: Send,
{
    async fn handle(&mut self, actor: &mut A) {
        let result = A::handle(actor, self.message.clone()).await;
        if let Some(res_tx) = self.response_tx.take() {
            let _ = res_tx.send(result.unwrap());
        }
    }
}

/// The inner parts of the [Envelope] containing the actual message as well as an optional
/// response channel.
/* struct EnvelopeInner<M, R> {
    message: M,
    tx: Option<oneshot::Sender<R>>,
}

#[async_trait]
impl<A, M> ActorMessage<A> for EnvelopeInner<M, <A as Handler<M>>::Response>
where
    A: Handler<M>,
    A::Response: Send,
    M: Clone + Send + Sync + 'static,
{
    async fn handle(self, actor: &mut A) {
        let result = A::handle(actor, self.message.clone()).await;
        if let Some(res_tx) = self.tx {
            let _ = res_tx.send(result.unwrap());
        }
    }
}
 */

impl<A, M> Enveloper<A, M> for A
where
    A: Handler<M> + Send + 'static,
    A::Response: Send,
    M: Clone + Send + Sync + 'static,
{
    fn pack(
        message: M,
        tx: Option<oneshot::Sender<<A as Handler<M>>::Response>>,
    ) -> Box<dyn MessageHandler<A>> {
        Box::new(Envelope::new(message, tx))
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
