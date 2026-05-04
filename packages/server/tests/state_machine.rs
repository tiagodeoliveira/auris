mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(2);

async fn drain_snapshot(ws: &mut Ws) {
    let _ = next_event(ws, T).await;
}

#[tokio::test]
async fn start_stop_meeting_events() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;

    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    let e1 = next_event(&mut ws, T).await;
    let e2 = next_event(&mut ws, T).await;
    let e3 = next_event(&mut ws, T).await;
    assert_eq!(e1["type"], "meeting_state_changed");
    assert_eq!(e1["meeting_state"], "active");
    assert_eq!(e2["type"], "metadata_changed");
    assert_eq!(e3["type"], "mode_changed");
    assert_eq!(e3["mode"], "transcript");

    send_intent(&mut ws, json!({"type":"stop_meeting"})).await;
    let e4 = next_event(&mut ws, T).await;
    assert_eq!(e4["type"], "meeting_state_changed");
    assert_eq!(e4["meeting_state"], "idle");
}

#[tokio::test]
async fn start_meeting_with_metadata() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;

    send_intent(
        &mut ws,
        json!({"type":"start_meeting", "metadata": {"project": "helix"}}),
    )
    .await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed
    let meta = next_event(&mut ws, T).await;
    assert_eq!(meta["type"], "metadata_changed");
    assert_eq!(meta["metadata"]["project"], "helix");
}

#[tokio::test]
async fn pause_resume_events() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 {
        let _ = next_event(&mut ws, T).await;
    }
    send_intent(&mut ws, json!({"type":"pause"})).await;
    let p = next_event(&mut ws, T).await;
    assert_eq!(p["meeting_state"], "paused");
    send_intent(&mut ws, json!({"type":"resume"})).await;
    let r = next_event(&mut ws, T).await;
    assert_eq!(r["meeting_state"], "active");
}

#[tokio::test]
async fn set_mode_valid() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 {
        let _ = next_event(&mut ws, T).await;
    }
    send_intent(&mut ws, json!({"type":"set_mode", "mode": "transcript"})).await;
    let m = next_event(&mut ws, T).await;
    assert_eq!(m["type"], "mode_changed");
    assert_eq!(m["mode"], "transcript");
}

#[tokio::test]
async fn set_mode_unknown_returns_error_to_originator_only() {
    let server = spawn_test_server().await;
    let mut a = connect(server.addr, "test-token").await;
    let mut b = connect(server.addr, "test-token").await;
    drain_snapshot(&mut a).await;
    drain_snapshot(&mut b).await;

    send_intent(&mut a, json!({"type":"set_mode", "mode": "bogus"})).await;
    let err = next_event(&mut a, T).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "unknown_mode");
    assert_eq!(err["intent_ref"], "bogus");

    // B should see nothing within 500ms.
    let res = tokio::time::timeout(
        Duration::from_millis(500),
        futures_util::StreamExt::next(&mut b),
    )
    .await;
    assert!(res.is_err(), "B should not have received an event");
}

#[tokio::test]
async fn set_mode_in_idle_allowed() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"set_mode", "mode": "transcript"})).await;
    let m = next_event(&mut ws, T).await;
    assert_eq!(m["type"], "mode_changed");
    assert_eq!(m["mode"], "transcript");
}

#[tokio::test]
async fn set_metadata_basic() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(
        &mut ws,
        json!({"type":"set_metadata", "key": "foo", "value": "bar"}),
    )
    .await;
    let m = next_event(&mut ws, T).await;
    assert_eq!(m["metadata"]["foo"], "bar");
}

#[tokio::test]
async fn set_metadata_delete() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(
        &mut ws,
        json!({"type":"set_metadata", "key": "foo", "value": "bar"}),
    )
    .await;
    let _ = next_event(&mut ws, T).await;
    send_intent(
        &mut ws,
        json!({"type":"set_metadata", "key": "foo", "value": null}),
    )
    .await;
    let m = next_event(&mut ws, T).await;
    assert!(m["metadata"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn two_clients_broadcast() {
    let server = spawn_test_server().await;
    let mut a = connect(server.addr, "test-token").await;
    let mut b = connect(server.addr, "test-token").await;
    drain_snapshot(&mut a).await;
    drain_snapshot(&mut b).await;
    send_intent(&mut a, json!({"type":"start_meeting"})).await;
    let bn = next_event(&mut b, T).await;
    assert_eq!(bn["type"], "meeting_state_changed");
    assert_eq!(bn["meeting_state"], "active");
}

#[tokio::test]
async fn expand_item_unknown() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 {
        let _ = next_event(&mut ws, T).await;
    }
    send_intent(&mut ws, json!({"type":"expand_item", "item_id": "nope"})).await;
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "unknown_item");
}

#[tokio::test]
async fn mark_moment_active() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    for _ in 0..3 {
        let _ = next_event(&mut ws, T).await;
    }
    send_intent(&mut ws, json!({"type":"mark_moment", "t": 1234})).await;
    let s = next_event(&mut ws, T).await;
    assert_eq!(s["type"], "status");
    assert_eq!(s["status"]["listening"], true);
}

#[tokio::test]
async fn mark_moment_idle_no_event() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"mark_moment", "t": 0})).await;
    let res = tokio::time::timeout(
        Duration::from_millis(500),
        futures_util::StreamExt::next(&mut ws),
    )
    .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn bad_json() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    ws.send(Message::Text("not json at all".into()))
        .await
        .unwrap();
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "bad_json");
}

#[tokio::test]
async fn unknown_intent() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"fly_to_moon"})).await;
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["code"], "unknown_intent");
}

#[tokio::test]
async fn bad_payload() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"set_mode"})).await; // missing 'mode'
    let err = next_event(&mut ws, T).await;
    assert_eq!(err["code"], "bad_payload");
}
