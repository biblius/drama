use crate::{
    message::{ActorMessage, Envelope},
    runtime::Runtime,
    Actor, ActorCommand, ActorHandle, Error, Handler,
};
use async_trait::async_trait;
use flume::Receiver;
use futures::{
    stream::{SplitSink, SplitStream},
    Future, SinkExt, StreamExt,
};
use pin_project::pin_project;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::atomic::AtomicUsize,
    task::{Context, Poll},
};
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
        WebsocketRuntime::run(self)
    }
}

impl WebsocketActor {
    pub fn new(ws: WebSocket) -> Self {
        Self {
            websocket: Some(ws),
        }
    }
}

static PROCESSED: AtomicUsize = AtomicUsize::new(0);

#[pin_project]
pub struct WebsocketRuntime {
    actor: WebsocketActor,

    // Pin these 2 as we are polling them directly so we know they never move
    /// The receiving end of the websocket
    #[pin]
    ws_stream: SplitStream<WebSocket>,

    /// The sending end of the websocket
    #[pin]
    ws_sink: SplitSink<WebSocket, warp::ws::Message>,

    /// Actor message receiver
    message_rx: Receiver<Envelope<WebsocketActor>>,

    /// Actor command receiver
    command_rx: Receiver<ActorCommand>,

    /// Received, but not yet processed messages
    message_queue: VecDeque<Envelope<WebsocketActor>>,

    /// Received, but not yet processed websocket messages
    request_queue: VecDeque<warp::ws::Message>,

    /// Processed websocket messages ready to be flushed in the sink
    response_queue: VecDeque<warp::ws::Message>,
}

impl WebsocketRuntime {
    pub fn new(
        mut actor: WebsocketActor,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<WebsocketActor>>,
    ) -> Self {
        let (ws_sink, ws_stream) = actor
            .websocket
            .take()
            .expect("Websocket runtime already started")
            .split();

        Self {
            actor,
            ws_sink,
            ws_stream,
            message_rx,
            command_rx,
            message_queue: VecDeque::new(),
            request_queue: VecDeque::new(),
            response_queue: VecDeque::new(),
        }
    }
}

impl Future for WebsocketRuntime {
    type Output = Result<(), Error>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            // Poll command receiver
            match Pin::new(&mut this.command_rx.recv_async()).poll(cx) {
                Poll::Ready(Ok(message)) => match message {
                    ActorCommand::Stop => {
                        println!("Actor stopping");
                        break Poll::Ready(Ok(())); // TODO drain the queue and all that graceful stuff
                    }
                },
                Poll::Ready(Err(_)) => {
                    println!("Actor stopping"); // TODO drain the queue and all that graceful stuff
                    break Poll::Ready(Err(Error::ActorChannelClosed));
                }
                Poll::Pending => {}
            };

            // Poll the websocket stream for any messages and store them to the queue
            while let Poll::Ready(Some(ws_message)) = Pin::new(&mut this.ws_stream.next()).poll(cx)
            {
                match ws_message {
                    Ok(message) => this.request_queue.push_back(message),
                    Err(e) => {
                        eprintln!("WS error occurred {e}")
                    }
                }
            }

            // Respond to any queued and processed websocket messages
            let mut idx = 0;
            while idx < this.request_queue.len() {
                let ws_message = &this.request_queue[idx];
                match this.actor.handle(ws_message.to_owned()).as_mut().poll(cx) {
                    Poll::Ready(result) => match result {
                        Ok(response) => {
                            if let Some(response) = response {
                                match Pin::new(&mut this.ws_sink.feed(response)).poll(cx) {
                                    Poll::Ready(result) => {
                                        result?;
                                        this.request_queue.swap_remove_front(idx);
                                        PROCESSED
                                            .fetch_add(1, std::sync::atomic::Ordering::Acquire);
                                    }
                                    Poll::Pending => idx += 1,
                                }
                            }
                        }
                        Err(e) => return Poll::Ready(Err(e)),
                    },
                    Poll::Pending => idx += 1,
                }
            }

            println!(
                "PROCESSED {}",
                PROCESSED.load(std::sync::atomic::Ordering::Acquire)
            );

            let _ = Pin::new(&mut this.ws_sink.flush()).poll(cx);

            // Process all messages
            let mut idx = 0;
            while idx < this.message_queue.len() {
                let pending = &mut this.message_queue[idx];
                match pending.handle(this.actor, cx) {
                    Poll::Ready(_) => {
                        this.message_queue.swap_remove_front(idx);
                        continue;
                    }
                    Poll::Pending => idx += 1,
                }
            }

            // Poll message receiver and continue to process if anything comes up
            while let Poll::Ready(Ok(message)) =
                Pin::new(&mut this.message_rx.recv_async()).poll(cx)
            {
                this.message_queue.push_back(message);
            }

            // Poll again and process new messages if any
            match Pin::new(&mut this.message_rx.recv_async()).poll(cx) {
                Poll::Ready(Ok(message)) => {
                    this.message_queue.push_back(message);
                    continue;
                }
                Poll::Ready(Err(_)) => {
                    println!("Message channel closed, ungracefully stopping actor");
                    break Poll::Ready(Err(Error::ActorChannelClosed));
                }
                Poll::Pending => {
                    if !this.message_queue.is_empty() {
                        continue;
                    }
                }
            };

            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
    }
}

impl Runtime<WebsocketActor> for WebsocketRuntime {
    fn run(actor: WebsocketActor) -> ActorHandle<WebsocketActor> {
        let (message_tx, message_rx) = flume::unbounded();
        let (command_tx, command_rx) = flume::unbounded();
        tokio::spawn(WebsocketRuntime::new(actor, command_rx, message_rx));
        ActorHandle::new(message_tx, command_tx)
    }
}

impl crate::Message for warp::ws::Message {
    type Response = Option<warp::ws::Message>;
}

#[async_trait]
impl Handler<warp::ws::Message> for WebsocketActor {
    async fn handle(
        &mut self,
        message: warp::ws::Message,
    ) -> Result<<warp::ws::Message as crate::message::Message>::Response, crate::Error> {
        // println!("Actor received message {message:?}");
        if message.is_text() {
            Ok(Some(message))
        } else {
            Ok(None)
        }
    }
}
