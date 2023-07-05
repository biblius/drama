use crate::{actor::Actor, message::MailboxReceiver, ActorCommand, Error};
use flume::Receiver;

/// The default actor runtime. Spins and listens for incoming commands
/// and messages.
pub struct ActorRuntime<A>
where
    A: Actor,
{
    actor: A,
    commands: Receiver<ActorCommand>,
    mailbox: MailboxReceiver<A>,
}

impl<A> ActorRuntime<A>
where
    A: Actor,
{
    pub fn new(
        actor: A,
        command_rx: Receiver<ActorCommand>,
        message_rx: MailboxReceiver<A>,
    ) -> Self {
        println!("Building default runtime");
        Self {
            actor,
            commands: command_rx,
            mailbox: message_rx,
        }
    }

    pub async fn run(mut self) -> Result<(), Error> {
        loop {
            // Only time we error here is when we disconnect
            tokio::select! {
                command = self.commands.recv_async() => {
                    let Ok(command) = command else { return Err(Error::ActorDisconnected); };
                    match command {
                        ActorCommand::Stop => {
                            tracing::trace!("Relay actor stopping");
                            return Ok(())
                        },
                    }
                }
                message = self.mailbox.recv_async() => {
                    let Ok(mut message) = message else { return Err(Error::ActorDisconnected); };
                    message.handle(&mut self.actor).await;
                }
            }
        }
    }
}
