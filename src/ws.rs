use crate::{
    message::{Envelope, PackedMessage},
    runtime::Runtime,
    Actor, ActorCommand, ActorHandle, Handler,
};
use futures::{SinkExt, StreamExt, TryFutureExt};
use tokio::{select, sync::mpsc::Receiver};
use warp::ws::WebSocket;

pub struct WebsocketActor {
    websocket: Option<WebSocket>,
}

impl Actor for WebsocketActor {
    fn start(self) -> ActorHandle<Self>
    where
        Self: Sized + Send + 'static,
    {
        println!("Starting websocket actor");
        WebsocketRuntime::start(self)
    }
}

impl WebsocketActor {
    pub fn new(ws: WebSocket) -> Self {
        Self {
            websocket: Some(ws),
        }
    }
}

pub struct WebsocketRuntime {
    actor: WebsocketActor,
    message_rx: Receiver<Envelope<WebsocketActor>>,
    command_rx: Receiver<ActorCommand>,
}

impl WebsocketRuntime {
    pub fn new(
        actor: WebsocketActor,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<WebsocketActor>>,
    ) -> Self {
        Self {
            actor,
            message_rx,
            command_rx,
        }
    }
}

impl WebsocketRuntime {
    pub async fn run(mut self) {
        let (mut ws_sender, mut ws_receiver) = self
            .actor
            .websocket
            .take()
            .expect("Websocket runtime already started")
            .split();

        loop {
            select! {
                // Handle any pending commands
                Some(msg) = self.command_rx.recv() => {
                    match msg {
                        ActorCommand::Stop => {
                            println!("Actor stopping");
                            break;
                        }
                    }
                }
                // Handle any in-process messages
                Some(mut message) = self.message_rx.recv() => {
                    println!("Processing Message");
                    message.handle(&mut self.actor)
                }
                // Handle any messages from the websocket
                Some(message) = ws_receiver.next() => {
                    match message {
                        Ok(message) => {
                            if let Some(res) = self.actor.handle(message).unwrap() {// TODO
                                ws_sender.send(res)
                                    .unwrap_or_else(|e| {
                                        eprintln!("websocket send error: {}", e);
                                    })
                                    .await;
                            }
                        },
                        Err(e) => {
                            eprintln!("WS error occurred {e}")
                        },
                    }
                }
                else => {
                    println!("No messages")
                }
            }
        }
    }
}

impl Runtime<WebsocketActor> for WebsocketRuntime {
    fn start(actor: WebsocketActor) -> ActorHandle<WebsocketActor> {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(100);
        let rt = WebsocketRuntime::new(actor, cmd_rx, rx);
        tokio::spawn(rt.run());
        ActorHandle {
            message_tx: tx,
            command_tx: cmd_tx,
        }
    }
}

impl crate::Message for warp::ws::Message {
    type Response = Option<warp::ws::Message>;
}

impl Handler<warp::ws::Message> for WebsocketActor {
    fn handle(
        &mut self,
        message: warp::ws::Message,
    ) -> Result<<warp::ws::Message as crate::message::Message>::Response, crate::Error> {
        println!("Actor received message {message:?}");
        if message.is_text() {
            Ok(Some(message))
        } else {
            Ok(None)
        }
    }
}
