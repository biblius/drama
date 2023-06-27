use crate::{Actor, Error, Handler};
use tokio::sync::oneshot;

/// Represents a message that can be sent to an actor. The response type is what the actor must return in its handler implementation.
pub trait Message {
    type Response;
}

/// Represents a type erased message that ultimately gets stored in an [Envelope]. We need this indirection so we can abstract away the concrete message
/// type when creating an actor handle, otherwise we would only be able to send a single message type to the actor.
pub trait ActorMessage<A: Actor> {
    fn handle(&mut self, actor: &mut A);
}

/// Used by [ActorHandle][super::ActorHandle]s to pack [Message]s into [Envelope]s so we have a type erased message to send to the actor.
pub trait Enveloper<A: Actor, M: Message> {
    /// Wrap a message in an envelope with an optional response channel.
    fn pack(message: M, tx: Option<oneshot::Sender<M::Response>>) -> Envelope<A>;
}

/// A type erased wrapper for messages. This wrapper essentially enables us to send any message to the actor
/// so long as it implements the necessary handler.
pub struct Envelope<A: Actor> {
    message: Box<dyn ActorMessage<A> + Send>,
}

impl<A> Envelope<A>
where
    A: Actor,
{
    pub fn new<M>(message: M, tx: Option<oneshot::Sender<M::Response>>) -> Self
    where
        A: Handler<M>,
        M: Message + Send + 'static,
        M::Response: Send,
    {
        Self {
            message: Box::new(EnvelopeInner {
                message: Some(message),
                tx,
            }),
        }
    }
}

/// The inner parts of the [Envelope] containing the actual message as well as an optional
/// response channel.
struct EnvelopeInner<M: Message> {
    message: Option<M>,
    tx: Option<oneshot::Sender<M::Response>>,
}

impl<A> ActorMessage<A> for Envelope<A>
where
    A: Actor,
{
    fn handle(&mut self, actor: &mut A) {
        self.message.handle(actor)
    }
}

impl<A, M> ActorMessage<A> for EnvelopeInner<M>
where
    M: Message,
    A: Actor + Handler<M>,
{
    fn handle(&mut self, actor: &mut A) {
        let Some(message) = self.message.take() else { panic!("Message already processed") };
        match actor.handle(message) {
            Ok(result) => {
                let Some(res_tx) = self.tx.take() else { panic!("Message already processed") };
                // TODO
                let _ = res_tx.send(result);
            }
            Err(_) => todo!(),
        };
    }
}

impl<A, M> Enveloper<A, M> for A
where
    A: Actor + Handler<M>,
    M: Message + Send + 'static,
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
        println!("Awaiting response");
        match self.as_mut().response_rx.try_recv() {
            Ok(msg) => {
                println!("Future ready");
                std::task::Poll::Ready(Ok(msg))
            }
            Err(e) => {
                println!("Future pending {e}");
                match e {
                    oneshot::error::TryRecvError::Empty => {
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                    oneshot::error::TryRecvError::Closed => std::task::Poll::Ready(Err(e.into())),
                }
            }
        }
    }
}
