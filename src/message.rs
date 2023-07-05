use crate::{actor::Actor, Error};
use async_trait::async_trait;
use tokio::sync::oneshot;

/// The main trait to implement on an [Actor] to enable it to handle messages.
#[async_trait]
pub trait Handler<M>: Actor {
    type Response: Send;
    async fn handle(&mut self, message: &M) -> Self::Response;
}

/// Represents a message that can be handled by an [Actor]. This should never be manually
/// implemented and is implemented solely by [Envelope] to enable sending
/// different types of messages to an actor at runtime.
///
/// The main purpose of this trait is enabling us to send any message type to a running
/// actor. Notice how this trait contains no information about the message, only the
/// actor it is being sent to. This allows us to dynamically call the handle method
/// during runtime for any message.
///
/// The handle implementation in this trait just delegates to the actor's [Handler] implementation
/// for the message using self as the `message` parameter.
#[async_trait]
pub trait ActorMessage<A: Actor>: Send + Sync {
    async fn handle(&mut self, actor: &mut A);
}

/// A type erased message representation that is sent to any
/// actor that implements a handler for the underlying message.
pub type BoxedActorMessage<A> = Box<dyn ActorMessage<A>>;

/// Type erased message receiver. The part of the channel in the actor's runtime.
pub type MailboxReceiver<A> = flume::Receiver<BoxedActorMessage<A>>;

/// Type erased message sender. The part of the channel given in an [ActorHandle][super::ActorHandle].
pub type MailboxSender<A> = flume::Sender<BoxedActorMessage<A>>;

/// Contains the message in transit and an optional response channel for the message which will be invoked
/// if the message was sent with `send_wait`
pub(crate) struct Envelope<M, A>
where
    A: Handler<M>,
    M: Send + 'static,
{
    message: M,
    response_tx: Option<oneshot::Sender<A::Response>>,
}

impl<M, A> Envelope<M, A>
where
    A: Handler<M>,
    M: Send + Sync + 'static,
{
    pub fn new(message: M, tx: Option<oneshot::Sender<A::Response>>) -> Self {
        Self {
            message,
            response_tx: tx,
        }
    }
}

#[async_trait]
impl<M, A> ActorMessage<A> for Envelope<M, A>
where
    A: Handler<M>,
    M: Send + Sync,
{
    async fn handle(&mut self, actor: &mut A) {
        let result = A::handle(actor, &self.message).await;
        if let Some(res_tx) = self.response_tx.take() {
            let _ = res_tx.send(result);
        }
    }
}

/// Used by [ActorHandle][super::ActorHandle]s to pack messages into [Envelope]s.
pub trait Enveloper<A, M>
where
    A: Handler<M>,
    M: Send,
{
    /// Wrap a message in an envelope with an optional response channel.
    fn pack(
        message: M,
        tx: Option<oneshot::Sender<<A as Handler<M>>::Response>>,
    ) -> Box<dyn ActorMessage<A>>;
}

impl<A, M> Enveloper<A, M> for A
where
    A: Handler<M>,
    M: Send + Sync + 'static,
{
    fn pack(
        message: M,
        tx: Option<oneshot::Sender<<A as Handler<M>>::Response>>,
    ) -> Box<dyn ActorMessage<A>> {
        Box::new(Envelope::new(message, tx))
    }
}

/// A future that when awaited returns an actor's response to a message.
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

impl<M, A> std::fmt::Debug for Envelope<M, A>
where
    A: Actor + Handler<M>,
    M: Clone + Send,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("message", &"{{..}}")
            .finish()
    }
}
