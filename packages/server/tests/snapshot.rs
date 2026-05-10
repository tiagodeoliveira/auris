mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn snapshot_initial_state() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    let snap = next_event(&mut ws, Duration::from_secs(1)).await;
    assert_eq!(snap["type"], "snapshot");
    assert_eq!(snap["protocol_version"], 1);
    assert_eq!(snap["meeting_state"], "idle");
    // Six modes: transcript, highlights, actions, open_questions, summary, chat.
    assert_eq!(snap["available_modes"].as_array().unwrap().len(), 6);
    assert_eq!(snap["mode"], "transcript");
    assert!(snap["metadata"].as_object().unwrap().is_empty());
    assert!(snap["items"].as_array().unwrap().is_empty());
    assert_eq!(snap["status"]["listening"], false);
    assert_eq!(snap["status"]["paused"], false);
}

#[tokio::test]
async fn reconnect_snapshot_active() {
    let server = spawn_test_server().await;
    let mut ws1 = connect(server.addr, "test-token").await;
    let _ = next_event(&mut ws1, Duration::from_secs(1)).await; // snapshot
    send_intent(&mut ws1, json!({"type":"start_meeting"})).await;
    // Drain 3 events from the start-meeting sequence.
    for _ in 0..3 {
        let _ = next_event(&mut ws1, Duration::from_secs(1)).await;
    }
    drop(ws1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut ws2 = connect(server.addr, "test-token").await;
    let snap = next_event(&mut ws2, Duration::from_secs(1)).await;
    assert_eq!(snap["type"], "snapshot");
    assert_eq!(snap["meeting_state"], "active");
}
