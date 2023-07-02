use crate::{
    message::Envelope,
    runtime::{ActorJob, Runtime, QUEUE_CAPACITY},
    Actor, ActorCommand, ActorHandle, ActorStatus, Error, Handler, Hello,
};
use async_trait::async_trait;
use flume::{r#async::RecvStream, Receiver};
use futures::{
    stream::{SplitSink, SplitStream},
    Future, FutureExt, SinkExt, StreamExt,
};
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

struct WebsocketJob {
    message: Option<Box<Message>>,
    future: Option<WsFuture>,
}

impl WebsocketJob {
    pub fn new(message: Message) -> Self {
        Self {
            message: Some(Box::new(message)),
            future: None,
        }
    }

    fn poll(
        &mut self,
        actor: Arc<Mutex<WebsocketActor>>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<Option<Message>, warp::Error>> {
        let message = self.message.take();

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

pub struct WebsocketRuntime {
    actor: Arc<Mutex<WebsocketActor>>,

    status: ActorStatus,

    /// The receiving end of the websocket
    ws_stream: SplitStream<WebSocket>,

    /// The sending end of the websocket
    ws_sink: SplitSink<WebSocket, Message>,

    /// Actor message receiver
    message_stream: RecvStream<'static, Envelope<WebsocketActor>>,

    /// Actor command receiver
    command_stream: RecvStream<'static, ActorCommand>,

    /// Actor messages currently being processed
    process_queue: VecDeque<ActorJob<WebsocketActor>>,

    /// Received, but not yet processed websocket messages
    response_queue: VecDeque<WebsocketJob>,
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
            message_stream: message_rx.into_stream(),
            command_stream: command_rx.into_stream(),
            response_queue: VecDeque::new(),
            process_queue: VecDeque::new(),
            status: ActorStatus::Starting,
        }
    }
}

impl Future for WebsocketRuntime {
    type Output = Result<(), Error>;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let actor = self.actor();
        let mut this = self.as_mut();

        this.process_commands(cx)?;

        // Poll the websocket stream for any messages and store them to the queue
        if this.response_queue.len() < WS_QUEUE_SIZE {
            while let Poll::Ready(Some(ws_message)) = this.ws_stream.poll_next_unpin(cx) {
                match ws_message {
                    Ok(message) => {
                        this.response_queue.push_back(WebsocketJob::new(message));
                        if this.response_queue.len() >= WS_QUEUE_SIZE {
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
        while idx < this.response_queue.len() {
            let job = &mut this.response_queue[idx];
            match job.poll(actor.clone(), cx) {
                Poll::Ready(result) => match result {
                    Ok(response) => {
                        if let Some(response) = response {
                            let feed = &mut this.ws_sink.feed(response);
                            let mut feed = Pin::new(feed);
                            let _ = feed.as_mut().poll(cx);
                        }
                        PROCESSED.fetch_add(1, std::sync::atomic::Ordering::Acquire);
                        this.response_queue.swap_remove_front(idx);
                    }
                    Err(e) => {
                        println!("Shit's fucked my dude {e}")
                    }
                },
                Poll::Pending => idx += 1,
            }
        }

        this.process_messages(cx)?;

        println!(
            "PROCESSED {} CURRENT IN QUEUE {}",
            PROCESSED.load(std::sync::atomic::Ordering::Acquire),
            this.response_queue.len(),
        );

        let _ = this.ws_sink.flush().poll_unpin(cx)?;

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

    #[inline]
    fn processing_queue(&mut self) -> &mut VecDeque<ActorJob<WebsocketActor>> {
        &mut self.process_queue
    }

    #[inline]
    fn command_stream(&mut self) -> &mut RecvStream<'static, ActorCommand> {
        &mut self.command_stream
    }

    #[inline]
    fn message_stream(&mut self) -> &mut RecvStream<'static, Envelope<WebsocketActor>> {
        &mut self.message_stream
    }

    #[inline]
    fn actor(&self) -> Arc<Mutex<WebsocketActor>> {
        self.actor.clone()
    }

    #[inline]
    fn at_capacity(&self) -> bool {
        self.process_queue.len() >= QUEUE_CAPACITY
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
        if message.is_text() {
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
            Ok(Some(*message.clone()))
        } else {
            Ok(None)
        }
    }
}
