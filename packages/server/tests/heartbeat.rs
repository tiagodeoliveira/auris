mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(2);

async fn drain_snapshot(ws: &mut Ws) {
    let _ = next_event(ws, T).await;
}

async fn next_status(ws: &mut Ws) -> serde_json::Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let evt = next_event(ws, Duration::from_millis(500)).await;
        if evt["type"] == "status" {
            return evt;
        }
        if std::time::Instant::now() > deadline {
            panic!("no status event within deadline");
        }
    }
}

#[tokio::test]
async fn heartbeat_idle() {
    let server = spawn_test_server_fast_heartbeat().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    let s = next_status(&mut ws).await;
    assert_eq!(s["status"]["listening"], false);
    assert_eq!(s["status"]["paused"], false);
}

#[tokio::test]
async fn heartbeat_active() {
    let server = spawn_test_server_fast_heartbeat().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 {
        let _ = next_event(&mut ws, T).await;
    }
    let s = next_status(&mut ws).await;
    assert_eq!(s["status"]["listening"], true);
    assert_eq!(s["status"]["paused"], false);
}
