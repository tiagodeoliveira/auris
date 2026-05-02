mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(3);

async fn drain_snapshot(ws: &mut Ws) { let _ = next_event(ws, T).await; }

#[tokio::test]
async fn extraction_merge_manual_wins() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({
        "type":"start_meeting",
        "description":"Q1 budget review",
        "metadata": {"project": "helix"}
    })).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed
    let m1 = next_event(&mut ws, T).await; // first metadata_changed (manual only)
    assert_eq!(m1["type"], "metadata_changed");
    assert_eq!(m1["metadata"]["project"], "helix");
    assert!(m1["metadata"].get("title").is_none());
    let _ = next_event(&mut ws, T).await; // mode_changed

    // Wait for extraction (1.5s + slop).
    let m2 = next_event(&mut ws, Duration::from_secs(3)).await;
    assert_eq!(m2["type"], "metadata_changed");
    assert_eq!(m2["metadata"]["project"], "helix"); // manual wins
    assert_eq!(m2["metadata"]["title"], "Q1 budget review");
}

#[tokio::test]
async fn extraction_no_description() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    // After 2.5s, no extraction event should arrive.
    let res = tokio::time::timeout(Duration::from_millis(2500), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "metadata_changed" { return evt; }
        }
    }).await;
    assert!(res.is_err(), "extraction should not run without description");
}

#[tokio::test]
async fn extraction_cancelled_on_stop() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({
        "type":"start_meeting",
        "description":"some description here"
    })).await;
    for _ in 0..3 { let _ = next_event(&mut ws, T).await; }
    send_intent(&mut ws, json!({"type":"stop_meeting"})).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed { idle }
    // After 2.5s, no late metadata_changed should arrive.
    let res = tokio::time::timeout(Duration::from_millis(2500), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "metadata_changed" { return evt; }
        }
    }).await;
    assert!(res.is_err());
}
