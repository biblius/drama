use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use actors::ws::WebsocketActor;
use actors::{Actor, ActorHandle};
use warp::Filter;

type Arbiter = Arc<RwLock<HashMap<usize, ActorHandle<WebsocketActor>>>>;

#[tokio::main]
async fn main() {
    let arbiter = Arc::new(RwLock::new(HashMap::new()));
    let arbiter = warp::any().map(move || arbiter.clone());
    // GET /chat -> websocket upgrade
    let chat = warp::path("chat")
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(arbiter)
        .map(|ws: warp::ws::Ws, arbiter: Arbiter| {
            // This will call our function if the handshake succeeds.
            ws.on_upgrade(move |socket| {
                let actor = WebsocketActor::new(socket);
                let handle = actor.start();
                arbiter.write().unwrap().insert(0, handle);
                futures::future::ready(())
            })
        });

    // GET / -> index html

    let index = warp::path::end().map(|| warp::reply::html(INDEX_HTML));

    let routes = index.or(chat);

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
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

        function message(data) {

        }

        ws.onopen = function() {
            chat.innerHTML = '<p><em>Connected!</em></p>';
        };

        ws.onmessage = function(msg) {
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
            text.value = '';

            message('<You>: ' + msg);
        };
        </script>
    </body>
</html>
"#;
