//! Smoke test for the step 15 live pipeline using the mock STT + LLM-disabled.
//! Verifies that on start_meeting, summarizer tasks spawn and produce items
//! (transcript mode) without any real audio or external services.

mod common;

use common::{connect, next_event, send_intent, spawn_test_server_with_token};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn live_pipeline_emits_transcript_items_when_stt_mocked() {
    std::env::set_var("MEETING_COMPANION_STT_MOCK", "1");
    std::env::set_var("MEETING_COMPANION_STT_MOCK_INTERVAL_MS", "100");
    std::env::set_var("MEETING_COMPANION_LLM_DISABLED", "1");

    let server = spawn_test_server_with_token("test-token").await;
    let mut ws = connect(server.addr, "test-token").await;

    // Drain initial Snapshot event
    let _snapshot = next_event(&mut ws, Duration::from_secs(2)).await;

    // Switch to transcript mode so subsequent items are visible there
    send_intent(
        &mut ws,
        serde_json::json!({"type": "set_mode", "mode": "transcript"}),
    )
    .await;

    // Drain the mode_changed event
    let _mode_changed = next_event(&mut ws, Duration::from_secs(2)).await;

    // Start meeting (description is empty so no extraction fires)
    send_intent(
        &mut ws,
        serde_json::json!({"type": "start_meeting", "description": "test"}),
    )
    .await;

    // Wait for at least one items_update with mode="transcript"
    let mut transcript_items_seen = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && transcript_items_seen < 1 {
        let evt = next_event(&mut ws, Duration::from_millis(500)).await;
        if evt["type"] == "items_update" && evt["mode"] == "transcript" {
            let items = evt["items"].as_array().unwrap();
            transcript_items_seen += items.len();
        }
    }
    assert!(
        transcript_items_seen >= 1,
        "expected ≥1 transcript item via mock STT, got {}",
        transcript_items_seen
    );

    // Clean shutdown
    send_intent(&mut ws, serde_json::json!({"type": "stop_meeting"})).await;

    drop(server);
}
