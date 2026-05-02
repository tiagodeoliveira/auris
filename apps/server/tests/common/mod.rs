#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct TestServer {
    pub addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

pub async fn spawn_test_server() -> TestServer {
    spawn_test_server_with_token("test-token").await
}

pub async fn spawn_test_server_with_token(token: &str) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = oneshot::channel();
    let token = token.to_string();
    tokio::spawn(async move {
        let _ = meeting_companion_server::ws::run_server_with_listener(listener, token, rx).await;
    });
    TestServer { addr, shutdown: Some(tx) }
}

pub async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let req = ws_url(addr, token);
    let (ws, _) = tokio_tungstenite::connect_async(req).await.expect("connect");
    ws
}

pub async fn next_event(ws: &mut Ws, timeout: Duration) -> Value {
    let msg = tokio::time::timeout(timeout, ws.next())
        .await
        .expect("timeout waiting for event")
        .expect("stream ended")
        .expect("ws error");
    let text = msg.to_text().expect("text frame").to_string();
    serde_json::from_str(&text).expect("json")
}

pub async fn send_intent(ws: &mut Ws, intent: Value) {
    ws.send(Message::Text(intent.to_string())).await.expect("send");
}

pub fn ws_url(addr: SocketAddr, token: &str) -> Request {
    let url = format!("ws://{}/?token={}", addr, token);
    url.into_client_request().expect("client request")
}

pub async fn spawn_test_server_fast_heartbeat() -> TestServer {
    std::env::set_var("MEETING_COMPANION_HEARTBEAT_MS", "300");
    let s = spawn_test_server().await;
    s
}
