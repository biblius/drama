use crate::{
    message::{BoxedActorMessage, Enveloper, Handler, MailboxSender, MessageRequest},
    runtime::ActorRuntime,
    ActorCommand,
};

use flume::{SendError, Sender};
use std::fmt::Debug;
use tokio::sync::oneshot;

pub trait Actor: Sized + Send + Sync + 'static {
    fn start(self) -> ActorHandle<Self> {
        println!("Starting actor");
        let (message_tx, message_rx) = flume::unbounded();
        let (command_tx, command_rx) = flume::unbounded();
        tokio::spawn(ActorRuntime::new(self, command_rx, message_rx).run());
        ActorHandle::new(message_tx, command_tx)
    }
}

/// A handle to a spawned actor. Obtained when calling `start` on an [Actor] and is used to send messages
/// to it.
#[derive(Debug)]
pub struct ActorHandle<A>
where
    A: Actor,
{
    message_tx: MailboxSender<A>,
    command_tx: Sender<ActorCommand>,
}

impl<A> Clone for ActorHandle<A>
where
    A: Actor,
{
    fn clone(&self) -> Self {
        Self {
            message_tx: self.message_tx.clone(),
            command_tx: self.command_tx.clone(),
        }
    }
}

impl<A> ActorHandle<A>
where
    A: Actor,
{
    pub fn new(message_tx: MailboxSender<A>, command_tx: Sender<ActorCommand>) -> Self {
        Self {
            message_tx,
            command_tx,
        }
    }

    /// Sends a message to the actor and returns a [MessageRequest] that can
    /// be awaited. This method should be used when one needs a response from the
    /// actor.
    pub async fn send_wait<M>(&self, message: M) -> Result<A::Response, SendError<M>>
    where
        M: Send,
        A: Handler<M> + Enveloper<A, M>,
    {
        if self.message_tx.is_disconnected() {
            return Err(SendError(message));
        }
        let (tx, rx) = oneshot::channel();
        let _ = self.message_tx.send(A::pack(message, Some(tx)));
        Ok(MessageRequest { response_rx: rx }.await.unwrap())
    }

    /// Send a message to the actor without returning a response, but still returning an
    /// error if the channel is full or disconnected.
    pub fn send<M>(&self, message: M) -> Result<(), SendError<M>>
    where
        M: Send,
        A: Handler<M> + Enveloper<A, M>,
    {
        if self.message_tx.is_disconnected() {
            return Err(SendError(message));
        }
        let _ = self.message_tx.send(A::pack(message, None));
        Ok(())
    }

    /// Send a message ignoring any errors in the process. The true YOLO way to send messages.
    pub fn send_forget<M>(&self, message: M)
    where
        M: Send,
        A: Handler<M> + Enveloper<A, M>,
    {
        let _ = self.message_tx.send(A::pack(message, None));
    }

    pub fn send_cmd(&self, cmd: ActorCommand) -> Result<(), SendError<ActorCommand>> {
        self.command_tx.send(cmd)
    }

    pub fn recipient<M>(&self) -> Recipient<M>
    where
        M: Send + Sync + 'static,
        A: Handler<M>,
    {
        Recipient {
            message_tx: Box::new(self.message_tx.clone()),
            command_tx: self.command_tx.clone(),
        }
    }
}

/// The same as an [ActorHandle], but instead of being tied to a specific actor, it is only
/// tied to the message type. Can be obtained from an [ActorHandle]. Be aware that you cannot
/// obtain responses from actors with this.
///
/// Useful for grouping different types of actors that can handle the same message.
pub struct Recipient<M> {
    message_tx: Box<dyn MessageSender<M>>,
    command_tx: Sender<ActorCommand>,
}

impl<M> Recipient<M>
where
    M: Send,
{
    pub fn send(&self, message: M) -> Result<(), SendError<M>> {
        self.message_tx.send(message)
    }

    pub fn send_forget(&self, message: M) {
        let _ = self.message_tx.send(message);
    }

    pub fn send_cmd(&self, cmd: ActorCommand) -> Result<(), SendError<ActorCommand>> {
        self.command_tx.send(cmd)
    }
}

/// A helper trait used solely by [Recipient]'s message channel to erase the actor type.
/// This is achieved by implementing it on [Sender<Envelope<A>].
trait MessageSender<M>
where
    M: Send,
{
    fn send(&self, message: M) -> Result<(), SendError<M>>;
}

impl<A, M> MessageSender<M> for Sender<BoxedActorMessage<A>>
where
    M: Send + 'static,
    A: Handler<M> + Enveloper<A, M>,
{
    fn send(&self, message: M) -> Result<(), SendError<M>> {
        if self.is_disconnected() {
            return Err(SendError(message));
        }
        let _ = self.send(A::pack(message, None));
        Ok(())
    }
}

impl<A, M> From<ActorHandle<A>> for Recipient<M>
where
    M: Send + Sync + 'static,
    A: Handler<M> + Enveloper<A, M> + 'static,
{
    /// Just calls `ActorHandler::recipient`, i.e. clones the underlying channels
    /// into the recipient and boxes the message one.
    fn from(handle: ActorHandle<A>) -> Self {
        handle.recipient()
    }
}
