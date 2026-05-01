#![allow(dead_code)]

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;

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

pub fn ws_url(addr: SocketAddr, token: &str) -> Request {
    let url = format!("ws://{}/?token={}", addr, token);
    url.into_client_request().expect("client request")
}
