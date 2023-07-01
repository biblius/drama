use crate::{
    message::Envelope, runtime::Runtime, Actor, ActorCommand, ActorHandle, ActorStatus, Error,
    Handler, Hello,
};
use async_trait::async_trait;
use flume::Receiver;
use futures::{
    stream::{SplitSink, SplitStream},
    Future, SinkExt, Stream, StreamExt,
};
use pin_project::pin_project;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::atomic::AtomicUsize,
    task::{Context, Poll},
    time::Duration,
};
use std::{sync::Arc, task::ready};
use tokio::sync::Mutex;
use warp::ws::{Message, WebSocket};

const WS_QUEUE_SIZE: usize = 128;

pub struct WebsocketActor {
    websocket: Option<WebSocket>,
    hello: ActorHandle<Hello>,
}

impl WebsocketActor {
    pub fn new(ws: WebSocket, handle: ActorHandle<Hello>) -> Self {
        Self {
            websocket: Some(ws),
            hello: handle,
        }
    }
}

#[async_trait]
impl Actor for WebsocketActor {
    async fn start(self) -> ActorHandle<Self> {
        WebsocketRuntime::run(Arc::new(Mutex::new(self))).await
    }
}
type WsFuture = Pin<Box<dyn Future<Output = Result<Option<Message>, Error>> + Send>>;

struct ActorItem {
    message: Option<Box<Message>>,
    future: Option<WsFuture>,
}

impl ActorItem {
    pub fn new(message: Message) -> Self {
        Self {
            message: Some(Box::new(message)),
            future: None,
        }
    }

    fn poll(
        mut self: Pin<&mut Self>,
        actor: Arc<Mutex<WebsocketActor>>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<Option<Message>, warp::Error>> {
        let message = self.as_mut().message.take();

        match message {
            Some(message) => {
                let fut = WebsocketActor::handle(actor, message);
                self.future = Some(fut);
                let result = ready!(self.future.as_mut().unwrap().as_mut().poll(cx));
                match result {
                    Ok(response) => Poll::Ready(Ok(response)),
                    Err(e) => {
                        println!("Shit's fucked son {e}");
                        Poll::Ready(Ok(None))
                    }
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
                    Poll::Pending => {
                        PENDING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        // println!("Websocket Future pending - COUNT {PENDING:?}");
                        Poll::Pending
                    }
                },
                None => panic!("Impossibru"),
            },
        }
    }
}

static PROCESSED: AtomicUsize = AtomicUsize::new(0);
static PENDING: AtomicUsize = AtomicUsize::new(0);

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
    pub async fn new(
        actor: Arc<Mutex<WebsocketActor>>,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<WebsocketActor>>,
    ) -> Self {
        let (ws_sink, ws_stream) = actor
            .lock()
            .await
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

        // Poll command receiver and immediatelly process it
        if let Poll::Ready(result) = Pin::new(&mut this.command_rx.recv_async()).poll(cx) {
            match result {
                Ok(command) => {
                    match command {
                        ActorCommand::Stop => {
                            println!("Actor stopping");
                            return Poll::Ready(Ok(())); // TODO drain the queue and all that graceful stuff
                        }
                    }
                }
                Err(e) => {
                    println!("Actor stopping - {e}"); // TODO drain the queue and all that graceful stuff
                    return Poll::Ready(Err(Error::ActorChannelClosed));
                }
            }
        };

        // Poll the websocket stream for any messages and store them to the queue
        if this.processing_queue.is_empty() {
            while let Poll::Ready(Some(ws_message)) = this.ws_stream.as_mut().poll_next(cx) {
                match ws_message {
                    Ok(message) => {
                        this.processing_queue.push_back(ActorItem::new(message));
                        if this.processing_queue.len() >= WS_QUEUE_SIZE {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("WS error occurred {e}")
                    }
                }
            }
        }

        let mut idx = 0;
        while idx < this.processing_queue.len() {
            let job = Pin::new(&mut this.processing_queue[idx]);
            match ActorItem::poll(job, this.actor.clone(), cx) {
                Poll::Ready(result) => match result {
                    Ok(response) => {
                        if let Some(response) = response {
                            let feed = &mut this.ws_sink.feed(response);
                            let mut feed = Pin::new(feed);
                            let _ = feed.as_mut().poll(cx);
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

        // println!(
        //     "PROCESSED {} CURRENT IN QUEUE {}",
        //     PROCESSED.load(std::sync::atomic::Ordering::Acquire),
        //     this.processing_queue.len(),
        // );

        let _ = Pin::new(&mut this.ws_sink.flush()).poll(cx);

        // Process all messages
        /*             this.message_queue
        .retain_mut(|message| message.handle(actor).as_mut().poll(cx).is_pending()); */

        // Poll message receiver and continue to process if anything comes up
        while let Poll::Ready(Ok(message)) = Pin::new(&mut this.message_rx.recv_async()).poll(cx) {
            this.message_queue.push_back(message);
        }

        // Poll again and process new messages if any
        match Pin::new(&mut this.message_rx.recv_async()).poll(cx) {
            Poll::Ready(Ok(message)) => {
                this.message_queue.push_back(message);
            }
            Poll::Ready(Err(_)) => {
                println!("Message channel closed, ungracefully stopping actor");
                return Poll::Ready(Err(Error::ActorChannelClosed));
            }
            Poll::Pending => {}
        };

        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[async_trait]
impl Runtime<WebsocketActor> for WebsocketRuntime {
    async fn run(actor: Arc<Mutex<WebsocketActor>>) -> ActorHandle<WebsocketActor> {
        let (message_tx, message_rx) = flume::unbounded();
        let (command_tx, command_rx) = flume::unbounded();
        tokio::spawn(WebsocketRuntime::new(actor, command_rx, message_rx).await);
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
        message: Box<Message>,
    ) -> Result<<Message as crate::message::Message>::Response, crate::Error> {
        //let mut act = this.lock().await;
        this.lock()
            .await
            .hello
            .send(crate::Msg {
                content: message.to_str().unwrap().to_owned(),
            })
            .unwrap_or_else(|e| println!("{e}"));
        //        println!("Actor retreived lock and sent message got response {res}");
        tokio::time::sleep(Duration::from_micros(1)).await;
        //act.wait().await;
        if message.is_text() {
            Ok(Some(*message.clone()))
        } else {
            Ok(None)
        }
    }
}
