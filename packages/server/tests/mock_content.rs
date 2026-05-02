mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(5);

async fn drain_snapshot(ws: &mut Ws) {
    let _ = next_event(ws, T).await;
}

async fn drain_n(ws: &mut Ws, n: usize) {
    for _ in 0..n {
        let _ = next_event(ws, T).await;
    }
}

#[tokio::test]
async fn mock_items_replace() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let evt = next_event(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(evt["type"], "items_update");
    let items = evt["items"].as_array().unwrap();
    assert!(!items.is_empty());
}

#[tokio::test]
async fn mock_items_append() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    send_intent(&mut ws, json!({"type":"set_mode", "mode": "transcript"})).await;
    let _ = next_event(&mut ws, T).await; // mode_changed
    let evt = next_event(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(evt["type"], "items_update");
    let items = evt["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
}

#[tokio::test]
async fn mock_stops_on_pause() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let _first = next_event(&mut ws, Duration::from_secs(5)).await; // wait for at least one items_update
    send_intent(&mut ws, json!({"type":"pause"})).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed
                                          // Now wait 5s and confirm no items_update arrives.
    let res = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "items_update" {
                return evt;
            }
        }
    })
    .await;
    assert!(res.is_err(), "items_update should not arrive while paused");
}

#[tokio::test]
async fn mock_stops_on_stop() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let _ = next_event(&mut ws, Duration::from_secs(5)).await;
    send_intent(&mut ws, json!({"type":"stop_meeting"})).await;
    let _ = next_event(&mut ws, T).await; // meeting_state_changed { idle }
    let res = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let evt = next_event(&mut ws, Duration::from_secs(10)).await;
            if evt["type"] == "items_update" {
                return evt;
            }
        }
    })
    .await;
    assert!(res.is_err(), "items_update should not arrive after stop");
}

#[tokio::test]
async fn mock_resumes_on_resume() {
    let server = spawn_test_server().await;
    let mut ws = connect(server.addr, "test-token").await;
    drain_snapshot(&mut ws).await;
    send_intent(&mut ws, json!({"type":"start_meeting"})).await;
    drain_n(&mut ws, 3).await;
    let _ = next_event(&mut ws, Duration::from_secs(5)).await;
    send_intent(&mut ws, json!({"type":"pause"})).await;
    let _ = next_event(&mut ws, T).await;
    send_intent(&mut ws, json!({"type":"resume"})).await;
    let _ = next_event(&mut ws, T).await;
    let evt = next_event(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(evt["type"], "items_update");
}
