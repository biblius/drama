use actors::ws::WebsocketActor;
use actors::{Actor, ActorHandle};
use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock};
use warp::Filter;

type Arbiter = Arc<RwLock<HashMap<usize, ActorHandle<WebsocketActor>>>>;

static ID: AtomicUsize = AtomicUsize::new(0);

#[tokio::main]
async fn main() {
    let pool = Arc::new(RwLock::new(HashMap::new()));
    let pool = warp::any().map(move || pool.clone());

    // GET /chat -> websocket upgrade
    let chat = warp::path("chat")
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(pool)
        .map(|ws: warp::ws::Ws, pool: Arbiter| {
            // This will call our function if the handshake succeeds.
            ws.on_upgrade(move |socket| {
                let actor = WebsocketActor::new(socket);
                let handle = actor.start();
                // let id = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // println!("Adding actor {id}");
                pool.write().unwrap().insert(0, handle);
                futures::future::ready(())
            })
        });

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
            message(msg.data);
            num += 1;
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
