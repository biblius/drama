use async_trait::async_trait;
use drama::runtime::Runtime;
use drama::ws::{WebsocketRuntime, WsActor};
use drama::{Actor, ActorHandle, Error, Handler};
use futures::stream::{SplitSink, SplitStream};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use warp::ws::{Message, WebSocket};
use warp::Filter;

type Arbiter = Arc<RwLock<HashMap<usize, ActorHandle<WebsocketActor>>>>;

static ID: AtomicUsize = AtomicUsize::new(0);

struct WebsocketActor {
    websocket: Option<WebSocket>,
    hello: ActorHandle<Hello>,
}

impl WebsocketActor {
    fn new(ws: WebSocket, handle: ActorHandle<Hello>) -> Self {
        Self {
            websocket: Some(ws),
            hello: handle,
        }
    }
}

impl Actor for WebsocketActor {
    fn start(self) -> ActorHandle<Self> {
        WebsocketRuntime::run(self)
    }
}

impl WsActor<Message, SplitStream<WebSocket>, SplitSink<WebSocket, Message>> for WebsocketActor {
    type Error = warp::Error;
    fn websocket(&mut self) -> (SplitSink<WebSocket, Message>, SplitStream<WebSocket>) {
        self.websocket
            .take()
            .expect("Websocket already split")
            .split()
    }
}

#[async_trait]
impl Handler<Message> for WebsocketActor {
    type Response = Option<Message>;
    async fn handle(
        this: Arc<Mutex<Self>>,
        message: Box<Message>,
    ) -> Result<Self::Response, Error> {
        this.lock()
            .await
            .hello
            .send(crate::Msg {
                _content: message.to_str().unwrap().to_owned(),
            })
            .unwrap_or_else(|e| println!("{e}"));

        Ok(Some(*message.clone()))
    }
}

struct Hello {}

impl Actor for Hello {}

struct Msg {
    pub _content: String,
}

#[async_trait]
impl Handler<Msg> for Hello {
    type Response = usize;
    async fn handle(_: Arc<Mutex<Self>>, _: Box<Msg>) -> Result<usize, Error> {
        println!("Handling message Hello");
        Ok(10)
    }
}

#[tokio::main]
async fn main() {
    let pool = Arc::new(RwLock::new(HashMap::new()));
    let pool = warp::any().map(move || pool.clone());

    let hello = Hello {}.start();
    let hello = warp::any().map(move || hello.clone());
    // GET /chat -> websocket upgrade
    let chat = warp::path("chat")
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(pool)
        .and(hello)
        .map(
            |ws: warp::ws::Ws, pool: Arbiter, hello: ActorHandle<Hello>| {
                // This will call our function if the handshake succeeds.
                ws.on_upgrade(|socket| async move {
                    let actor = WebsocketActor::new(socket, hello);
                    let handle = actor.start();
                    let id = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    println!("Adding actor {id}");
                    pool.write().unwrap().insert(id, handle);
                })
            },
        );

    // GET / -> index html

    let index = warp::path::end().map(|| warp::reply::html(INDEX_HTML));

    let routes = index.or(chat);

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await
}

static INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
    <head>
        <title>Warp Chat</title>
    </head>
    <body>
        <h1>Warp chat</h1>
        <div id="chat">
            <p><em>Connecting...</em></p>
        </div>
        <input type="text" id="text" />
        <button type="button" id="send">Send</button>
        <script type="text/javascript">
        const chat = document.getElementById('chat');
        const text = document.getElementById('text');
        const uri = 'ws://' + location.host + '/chat';
        const ws = new WebSocket(uri);

        let num = 0;

        function message(data) {
            if (num % 10000 === 0) chat.innerHTML = `${num}`
        }

        ws.onopen = function() {
            chat.innerHTML = '<p><em>Connected!</em></p>';
        };

        ws.onmessage = function(msg) {
            console.log(msg)
            num += 1;
            message(msg.data);
        };

        ws.onclose = function() {
            chat.getElementsByTagName('em')[0].innerText = 'Disconnected!';
        };

        send.onclick = function() {
            const msg = text.value;
            let i = 0;
            while (i < 100000) {
                ws.send(msg);
                i += 1;
            }
            text.value = '';

            message('<You>: ' + msg);
        };
        </script>
    </body>
</html>
"#;
