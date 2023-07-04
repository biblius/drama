use crate::{message::MailboxReceiver, Actor, ActorCommand};
use flume::{r#async::RecvStream, Receiver};
use futures::StreamExt;

pub const QUEUE_CAPACITY: usize = 128;

pub struct ActorRuntime<A>
where
    A: Actor + Send + 'static,
{
    actor: A,
    command_stream: RecvStream<'static, ActorCommand>,
    mailbox: MailboxReceiver<A>,
}

impl<A> ActorRuntime<A>
where
    A: Actor + 'static + Send,
{
    pub fn new(
        actor: A,
        command_rx: Receiver<ActorCommand>,
        message_rx: MailboxReceiver<A>,
    ) -> Self {
        println!("Building default runtime");
        Self {
            actor,
            command_stream: command_rx.into_stream(),
            mailbox: message_rx,
        }
    }

    pub async fn runt(mut self) {
        loop {
            tokio::select! {
                Some(command) = self.command_stream.next() => {
                   match command {
                            ActorCommand::Stop => {
                                println!("actor stopping");
                                return
                            },
                        }
                }
                message = self.mailbox.recv_async() => {
                    if let Ok(mut message) = message {
                        message.handle(&mut self.actor).await
                    } else {
                         break;
                     }
                }
            }
        }
        println!("actor stopping");
    }
}
