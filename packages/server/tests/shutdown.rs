mod common;

use common::*;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn graceful_shutdown_sends_close() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    let _ = next_event(&mut ws, Duration::from_secs(1)).await;

    drop(server); // triggers shutdown_tx via TestServer::Drop

    use futures_util::StreamExt;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut got_close = false;
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            Ok(Some(Ok(msg))) if matches!(msg, Message::Close(_)) => {
                got_close = true;
                break;
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(got_close, "expected close frame on graceful shutdown");
}
