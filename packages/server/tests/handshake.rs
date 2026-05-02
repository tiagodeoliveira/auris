mod common;

use common::*;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error;

#[tokio::test]
async fn handshake_token_match() {
    let server = spawn_test_server().await;
    let (mut ws, _) = connect_async(ws_url(server.addr, "test-token"))
        .await
        .expect("connect");
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        futures_util::StreamExt::next(&mut ws),
    )
    .await
    .expect("timeout")
    .expect("frame")
    .expect("msg");
    assert!(msg.is_text(), "expected text frame, got {:?}", msg);
    let json: serde_json::Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
    assert_eq!(json["type"], "snapshot");
}

#[tokio::test]
async fn handshake_token_mismatch() {
    let server = spawn_test_server().await;
    let res = connect_async(ws_url(server.addr, "wrong-token")).await;
    let mut ws = match res {
        Ok((ws, _)) => ws,
        Err(_) => return, // some clients see the close as a connect error; that's also OK.
    };
    use futures_util::StreamExt;
    loop {
        match ws.next().await {
            Some(Ok(msg)) if msg.is_close() => return,
            Some(Ok(_)) => continue,
            Some(Err(Error::ConnectionClosed)) | None => return,
            Some(Err(e)) => panic!("unexpected error: {}", e),
        }
    }
}

#[tokio::test]
async fn handshake_token_missing() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let server = spawn_test_server().await;
    let url = format!("ws://{}/", server.addr);
    let req = url.into_client_request().unwrap();
    let res = connect_async(req).await;
    let mut ws = match res {
        Ok((ws, _)) => ws,
        Err(_) => return,
    };
    use futures_util::StreamExt;
    loop {
        match ws.next().await {
            Some(Ok(msg)) if msg.is_close() => return,
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return,
        }
    }
}
