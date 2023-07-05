use async_trait::async_trait;
use drama::{Actor, ActorHandle, Handler, Relay, RelayActor};
use flume::Sender;
use futures::stream::SplitStream;
use futures::StreamExt;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use warp::ws::{Message, WebSocket};
use warp::Filter;

struct WebsocketActor {
    hello: ActorHandle<Hello>,
    tx: Sender<Message>,
}

impl WebsocketActor {
    fn new(handle: ActorHandle<Hello>, tx: Sender<Message>) -> Self {
        Self { hello: handle, tx }
    }
}

impl Actor for WebsocketActor {}

impl RelayActor<Message, SplitStream<WebSocket>> for WebsocketActor {
    type Error = warp::Error;
}

#[async_trait]
impl Relay<Message> for WebsocketActor {
    async fn process(&mut self, message: Message) -> Option<Message> {
        self.hello
            .send(crate::Msg {
                _content: message.to_str().unwrap().to_owned(),
            })
            .unwrap_or_else(|e| tracing::trace!("FUKEN HELL M8 {e}"));
        self.tx.send(message.clone()).unwrap();
        Some(message)
    }
}

struct Hello {}

impl Actor for Hello {}

#[derive(Clone)]
struct Msg {
    pub _content: String,
}

#[async_trait]
impl Handler<Msg> for Hello {
    type Response = usize;
    async fn handle(&mut self, _: &Msg) -> usize {
        10
    }
}

type Arbiter = Arc<RwLock<HashMap<usize, ActorHandle<WebsocketActor>>>>;

static ID: AtomicUsize = AtomicUsize::new(0);

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
                    let (si, st) = socket.split();
                    let (tx, rx) = flume::unbounded();

                    let actor = WebsocketActor::new(hello, tx.clone());
                    let handle = actor.start_relay(st, tx);
                    tokio::spawn(rx.into_stream().map(Ok).forward(si));
                    let id = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::trace!("Adding actor {id}");
                    pool.write().insert(id, handle);
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
             while (i < 10000) {
                ws.send(msg);
                 i += 1;
             }
            // text.value = '';

            message('<You>: ' + msg);
        };
        </script>
    </body>
</html>
"#;
