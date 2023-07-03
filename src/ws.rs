use crate::{
    message::Envelope,
    runtime::{ActorJob, Runtime, QUEUE_CAPACITY},
    Actor, ActorCommand, ActorHandle, Error, Handler,
};
use flume::{r#async::RecvStream, Receiver};
use futures::{Future, FutureExt, Sink, SinkExt, Stream, StreamExt};
use std::{
    collections::VecDeque,
    fmt::Display,
    marker::PhantomData,
    pin::Pin,
    sync::atomic::AtomicUsize,
    task::{Context, Poll},
};
use std::{sync::Arc, task::ready};
use tokio::sync::Mutex;

const WS_QUEUE_SIZE: usize = 128;

/// Represents an actor that can get access to a websocket stream and sink.
///
/// A websocket actor receives messages via the stream and processes them with
/// its [Handler] implementation. The handler implementation should always return an
/// `Option<M>` where M is the type used when implementing this trait. A handler that returns
/// `None` will not forward any response to the sink. If the handler returns `Some(M)` it will
/// be forwarded to the sink.
pub trait WsActor<M, Str, Sin>
where
    Str: Stream<Item = Result<M, Self::Error>>,
    Sin: Sink<M>,
{
    /// The error type of the underlying websocket implementation.
    type Error: Display;
    fn websocket(&mut self) -> (Sin, Str);
}

pub struct WebsocketRuntime<A, M, Str, Sin>
where
    A: Actor + WsActor<M, Str, Sin> + Handler<M> + 'static,
    Str: Stream<Item = Result<M, A::Error>>,
    Sin: Sink<M>,
{
    actor: Arc<Mutex<A>>,

    /// The receiving end of the websocket
    ws_stream: Str,

    /// The sending end of the websocket
    ws_sink: Sin,

    /// Actor message receiver
    message_stream: RecvStream<'static, Envelope<A>>,

    /// Actor command receiver
    command_stream: RecvStream<'static, ActorCommand>,

    /// Actor messages currently being processed
    process_queue: VecDeque<ActorJob<A>>,

    /// Received, but not yet processed websocket messages
    response_queue: VecDeque<WebsocketJob<A, M>>,
}

impl<A, M, Str, Sin> WebsocketRuntime<A, M, Str, Sin>
where
    Str: Stream<Item = Result<M, A::Error>>,
    Sin: Sink<M>,
    A: Actor + WsActor<M, Str, Sin> + Send + 'static + Handler<M>,
{
    pub fn new(
        mut actor: A,
        command_rx: Receiver<ActorCommand>,
        message_rx: Receiver<Envelope<A>>,
    ) -> Self {
        let (ws_sink, ws_stream) = actor.websocket();

        Self {
            actor: Arc::new(Mutex::new(actor)),
            ws_sink,
            ws_stream,
            message_stream: message_rx.into_stream(),
            command_stream: command_rx.into_stream(),
            response_queue: VecDeque::new(),
            process_queue: VecDeque::new(),
        }
    }
}

impl<A, M, Str, Sin> Future for WebsocketRuntime<A, M, Str, Sin>
where
    Self: Runtime<A>,
    Str: Stream<Item = Result<M, A::Error>> + Unpin,
    Sin: Sink<M> + Unpin,
    A: Actor + WsActor<M, Str, Sin> + Handler<M, Response = Option<M>> + Send + Unpin + 'static,
{
    type Output = Result<(), Error>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let actor = self.actor();
        let this = self.get_mut();

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
                            while feed.as_mut().poll(cx).is_pending() {
                                // Yikes, but too dumb to figure out a better solution
                                cx.waker().wake_by_ref();
                            }
                        }
                        PROCESSED.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
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

        let _ = this.ws_sink.flush().poll_unpin(cx);

        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<A, M, Str, Sin> Runtime<A> for WebsocketRuntime<A, M, Str, Sin>
where
    Str: Stream<Item = Result<M, A::Error>> + Unpin + Send + 'static,
    Sin: Sink<M> + Unpin + Send + 'static,
    A: Actor + WsActor<M, Str, Sin> + Send + 'static + Handler<M, Response = Option<M>> + Unpin,
    M: Send + 'static,
{
    fn run(actor: A) -> ActorHandle<A> {
        let (message_tx, message_rx) = flume::unbounded();
        let (command_tx, command_rx) = flume::unbounded();
        tokio::spawn(WebsocketRuntime::new(actor, command_rx, message_rx));
        ActorHandle::new(message_tx, command_tx)
    }

    #[inline]
    fn processing_queue(&mut self) -> &mut VecDeque<ActorJob<A>> {
        &mut self.process_queue
    }

    #[inline]
    fn command_stream(&mut self) -> &mut RecvStream<'static, ActorCommand> {
        &mut self.command_stream
    }

    #[inline]
    fn message_stream(&mut self) -> &mut RecvStream<'static, Envelope<A>> {
        &mut self.message_stream
    }

    #[inline]
    fn actor(&self) -> Arc<Mutex<A>> {
        self.actor.clone()
    }

    #[inline]
    fn at_capacity(&self) -> bool {
        self.process_queue.len() >= QUEUE_CAPACITY
    }
}

struct WebsocketJob<A, M>
where
    A: Handler<M>,
{
    message: Option<Box<M>>,
    future: Option<WsFuture<A, M>>,
    __a: PhantomData<A>,
}

impl<A, M> WebsocketJob<A, M>
where
    A: Handler<M> + 'static,
{
    pub fn new(message: M) -> Self {
        Self {
            message: Some(Box::new(message)),
            future: None,
            __a: PhantomData,
        }
    }

    fn poll(
        &mut self,
        actor: Arc<Mutex<A>>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<A::Response, Error>> {
        let message = self.message.take();

        match message {
            Some(message) => {
                let fut = A::handle(actor, message);
                self.future = Some(fut);
                let result = ready!(self.future.as_mut().unwrap().as_mut().poll(cx));
                match result {
                    Ok(response) => Poll::Ready(Ok(response)),
                    Err(e) => {
                        println!("Shit's fucked son {e}");
                        Poll::Ready(Err(e))
                    }
                }
            }
            None => {
                let Some(ref mut fut) = self.future else { panic!("Impossibru") };
                let result = ready!(fut.as_mut().poll(cx));
                match result {
                    Ok(response) => Poll::Ready(Ok(response)),
                    Err(e) => {
                        println!("Shit's fucked son {e}");
                        Poll::Ready(Err(e))
                    }
                }
            }
        }
    }
}

type WsFuture<A, M> =
    Pin<Box<dyn Future<Output = Result<<A as Handler<M>>::Response, Error>> + Send>>;

static PROCESSED: AtomicUsize = AtomicUsize::new(0);
