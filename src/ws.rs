use crate::{
    message::{ActorMessage, Envelope},
    runtime::Runtime,
    Actor, ActorCommand, ActorHandle, ActorStatus, Error, Handler,
};
use async_trait::async_trait;
use flume::Receiver;
use futures::{
    sink::Feed,
    stream::{SplitSink, SplitStream},
    Future, SinkExt, Stream, StreamExt,
};
use parking_lot::Mutex;
use pin_project::pin_project;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::atomic::AtomicUsize,
    task::{Context, Poll},
};
use std::{sync::Arc, time::Duration};
use warp::ws::{Message, WebSocket};

pub struct WebsocketActor {
    websocket: Option<WebSocket>,
}

impl WebsocketActor {
    pub fn new(ws: WebSocket) -> Self {
        Self {
            websocket: Some(ws),
        }
    }
}

impl Actor for WebsocketActor {
    fn start(self) -> ActorHandle<Self> {
        WebsocketRuntime::run(Arc::new(Mutex::new(self)))
    }
}
type WsFuture = Pin<Box<dyn Future<Output = Result<Option<Message>, Error>> + Send>>;

struct ActorItem {
    message: Option<Message>,
    future: Option<WsFuture>,
}

impl ActorItem {
    fn poll(
        mut self: Pin<&mut Self>,
        actor: Arc<Mutex<WebsocketActor>>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<Option<Message>, warp::Error>> {
        //         let mut this = self.project();
        let message = self.as_mut().message.take();

        match message {
            Some(message) => {
                let fut = WebsocketActor::handle(actor, message);
                self.future = Some(fut);
                // let Some(ref mut fut) = self.future else {panic!("Rust no longer works")};
                match self.future.as_mut().unwrap().as_mut().poll(cx) {
                    Poll::Ready(result) => match result {
                        Ok(response) => Poll::Ready(Ok(response)),
                        Err(e) => {
                            println!("Shit's fucked son {e}");
                            Poll::Ready(Ok(None))
                        }
                    },
                    Poll::Pending => Poll::Pending,
                }
            }
            None => match self.future {
                Some(ref mut fut) => match fut.as_mut().poll(cx) {
                    Poll::Ready(result) => match result {
                        Ok(response) => Poll::Ready(Ok(response)),
                        Err(e) => {
                            println!("Shit's fucked son {e}");
                            Poll::Ready(Ok(None))
                        }
                    },
                    Poll::Pending => Poll::Pending,
                },
                None => Poll::Pending,
            },
        }
    }
}

impl ActorItem {
    pub fn new(message: Message) -> Self {
        Self {
            message: Some(message),
            future: None,
        }
    }
}
/*
trait WsActorFuture {
    fn poll(
        self: Pin<&mut Self>,
        actor: &mut WebsocketActor,
        sink: Pin<&mut SplitSink<WebSocket, Message>>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), warp::Error>>;
}

impl WsActorFuture for Message {
    fn poll(
        mut self: Pin<&mut Self>,
        actor: &mut WebsocketActor,
        mut sink: Pin<&mut SplitSink<WebSocket, Message>>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), warp::Error>> {
        match actor.handle(self.as_mut().clone()).as_mut().poll(cx) {
            Poll::Ready(result) => match result {
                Ok(response) => {
                    if let Some(response) = response {
                        Pin::new(&mut sink.feed(response)).poll(cx)
                    } else {
                        Poll::Pending
                    }
                }
                Err(e) => {
                    println!("Shit's fucked son {e}");
                    Poll::Ready(Ok(()))
                }
            },
            Poll::Pending => Poll::Pending,
        }
    }
}
 */
static PROCESSED: AtomicUsize = AtomicUsize::new(0);

#[pin_project]
pub struct WebsocketRuntime {
    actor: Arc<Mutex<WebsocketActor>>,

    status: ActorStatus,

    /// The receiving end of the websocket
    #[pin]
    ws_stream: SplitStream<WebSocket>,

    /// The sending end of the websocket
    #[pin]
    ws_sink: SplitSink<WebSocket, Message>,

    /// Actor message receiver
    message_rx: Receiver<Envelope<WebsocketActor>>,

    /// Actor command receiver
    command_rx: Receiver<ActorCommand>,

    /// Received, but not yet processed messages
    message_queue: VecDeque<Envelope<WebsocketActor>>,

    /// Received, but not yet processed websocket messages
    processing_queue: VecDeque<ActorItem>,

    /// Processed websocket messages being flushed to the sink
    response_queue: VecDeque<Message>,
}

impl WebsocketRuntime {
    pub fn new(
        actor: Arc<Mutex<WebsocketActor>>,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<WebsocketActor>>,
    ) -> Self {
        let (ws_sink, ws_stream) = actor
            .lock()
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
            processing_queue: VecDeque::new(),
            response_queue: VecDeque::new(),
            status: ActorStatus::Starting,
        }
    }
}

impl Future for WebsocketRuntime {
    type Output = Result<(), Error>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            // Poll command receiver and immediatelly process it
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
            while let Poll::Ready(Some(ws_message)) = this.ws_stream.as_mut().poll_next(cx) {
                match ws_message {
                    Ok(message) => {
                        this.processing_queue.push_back(ActorItem::new(message));
                        if this.processing_queue.len() > 500 {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("WS error occurred {e}")
                    }
                }
            }

            let mut idx = 0;
            //             let actor = &mut this.actor.lock();
            while idx < this.processing_queue.len() {
                let job = &mut this.processing_queue[idx];
                match ActorItem::poll(Pin::new(job), this.actor.clone(), cx) {
                    Poll::Ready(result) => match result {
                        Ok(response) => {
                            if let Some(response) = response {
                                let feed = &mut this.ws_sink.feed(response);
                                let mut feed = Pin::new(feed);
                                while feed.as_mut().poll(cx).is_pending() {
                                    let _ = feed.as_mut().poll(cx);
                                }
                            }
                            PROCESSED.fetch_add(1, std::sync::atomic::Ordering::Acquire);
                            this.processing_queue.swap_remove_front(idx);
                        }
                        Err(e) => {
                            println!("Shit's fucked my dude {e}")
                        }
                    },
                    Poll::Pending => idx += 1,
                }
            }

            println!(
                "PROCESSED {} CURRENT IN QUEUE {}",
                PROCESSED.load(std::sync::atomic::Ordering::Acquire),
                this.processing_queue.len(),
            );

            let _ = Pin::new(&mut this.ws_sink.flush()).poll(cx);

            // Process all messages
            /*             this.message_queue
            .retain_mut(|message| message.handle(actor).as_mut().poll(cx).is_pending()); */

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
    fn run(actor: Arc<Mutex<WebsocketActor>>) -> ActorHandle<WebsocketActor> {
        let (message_tx, message_rx) = flume::unbounded();
        let (command_tx, command_rx) = flume::unbounded();
        tokio::spawn(WebsocketRuntime::new(actor, command_rx, message_rx));
        ActorHandle::new(message_tx, command_tx)
    }
}

impl crate::Message for Message {
    type Response = Option<Message>;
}

#[async_trait]
impl Handler<Message> for WebsocketActor {
    async fn handle(
        this: Arc<Mutex<Self>>,
        message: Message,
    ) -> Result<<Message as crate::message::Message>::Response, crate::Error> {
        println!("Actor received message {message:?}");
        // tokio::time::sleep(Duration::from_micros(500)).await;
        if message.is_text() {
            Ok(Some(message))
        } else {
            Ok(None)
        }
    }
}
