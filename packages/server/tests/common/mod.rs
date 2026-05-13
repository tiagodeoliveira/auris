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

pub async fn spawn_test_server_with_token(_token: &str) -> TestServer {
    // Load `.env` so DATABASE_URL etc. are visible to the test
    // process. The server binary loads this on its own from `main.rs`
    // via dotenvy; integration tests don't run through main, so they
    // have to opt in explicitly. Without it the WS handshake resets
    // on `connect` because db::open_pool() errors and tears down the
    // listener before the client gets to upgrade.
    let _ = dotenvy::dotenv();
    // Auth-disabled mode — every request maps to the synthetic
    // `dev|local` user. Removes the JWT validation path from the
    // critical path of these integration tests; the auth-on path is
    // exercised separately in the auth-focused unit tests.
    std::env::set_var("AURIS_AUTH_DISABLED", "1");
    // Disable LLM extraction in tests by default. The actual extract path
    // never fires from spawn_extraction; the LlmClient is constructed only
    // because the run_server signature requires one.
    std::env::set_var("AURIS_LLM_DISABLED", "1");
    // Prevent the AWS credential chain from blocking on IMDS / SSO if the
    // dev machine has no real credentials configured.
    if std::env::var("AWS_ACCESS_KEY_ID").is_err() {
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_REGION", "us-west-2");
    }
    // Skip Phase C boot recovery in integration tests. Without this,
    // any previous test that left a meeting active (ended_at IS NULL)
    // would be resurrected on the next test's boot, polluting state.
    // Production servers always set this off (default).
    std::env::set_var("AURIS_SKIP_BOOT_RECOVERY", "1");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = oneshot::channel();

    let llm = std::sync::Arc::new(
        auris_server::llm::LlmClient::from_env()
            .await
            .expect("LLM client init in tests"),
    );

    let auth = auris_server::ws::AuthMode::Disabled;
    tokio::spawn(async move {
        let _ = auris_server::ws::run_server_with_listener(listener, auth, llm, rx).await;
    });
    TestServer {
        addr,
        shutdown: Some(tx),
    }
}

pub async fn connect(addr: SocketAddr, token: &str) -> Ws {
    let req = ws_url(addr, token);
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("connect");
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
    ws.send(Message::Text(intent.to_string()))
        .await
        .expect("send");
}

pub fn ws_url(addr: SocketAddr, token: &str) -> Request {
    let url = format!("ws://{}/?token={}", addr, token);
    url.into_client_request().expect("client request")
}

pub async fn spawn_test_server_fast_heartbeat() -> TestServer {
    std::env::set_var("AURIS_HEARTBEAT_MS", "300");
    let s = spawn_test_server().await;
    s
}
