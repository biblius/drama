use drama::ws::WebsocketActor;
use drama::{Actor, ActorHandle, Hello};
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

    let hello = Hello {}.start().await;
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
                    let handle = actor.start().await;
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
