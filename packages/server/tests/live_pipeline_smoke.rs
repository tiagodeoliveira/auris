//! Smoke test for the step 15 live pipeline using the mock STT + LLM-disabled.
//! Verifies that on start_meeting, summarizer tasks spawn and produce items
//! (transcript mode) without any real audio or external services.

mod common;

use common::{connect, next_event, send_intent, spawn_test_server_with_token};
use std::collections::HashSet;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn live_pipeline_emits_transcript_items_when_stt_mocked() {
    // Scope the data dir so the A15 durable-loss assertion below can read
    // the per-meeting transcription.jsonl back. Safe to set process-wide:
    // this binary holds exactly one test, so there is no parallel boot to
    // race the env var.
    let data_dir = std::env::temp_dir().join(format!("auris-live-smoke-{}", uuid::Uuid::new_v4()));
    std::env::set_var("AURIS_DATA_DIR", &data_dir);
    std::env::set_var("AURIS_STT_MOCK", "1");
    std::env::set_var("AURIS_STT_MOCK_INTERVAL_MS", "100");
    std::env::set_var("AURIS_LLM_DISABLED", "1");
    std::env::set_var("AURIS_AUDIO_DISABLED", "1");

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

    // Wait for at least one items_update with mode="transcript", capturing
    // the meeting id (from meeting_state_changed → active) and the id of
    // every transcript item the client sees over the fan-out lane. The
    // A15 assertion is that the loss-proof durable lane persisted ALL of
    // these to disk — nothing the client saw may be missing from the
    // transcript of record.
    let mut transcript_items_seen = 0;
    let mut meeting_id: Option<String> = None;
    let mut wire_item_ids: HashSet<String> = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && transcript_items_seen < 3 {
        let evt = next_event(&mut ws, Duration::from_millis(500)).await;
        if evt["type"] == "meeting_state_changed" && evt["meeting_state"] == "active" {
            if let Some(id) = evt["meeting_id"].as_str() {
                meeting_id = Some(id.to_string());
            }
        }
        if evt["type"] == "items_update" && evt["mode"] == "transcript" {
            let items = evt["items"].as_array().unwrap();
            transcript_items_seen += items.len();
            for it in items {
                if let Some(id) = it["id"].as_str() {
                    wire_item_ids.insert(id.to_string());
                }
            }
        }
    }
    assert!(
        transcript_items_seen >= 1,
        "expected ≥1 transcript item via mock STT, got {}",
        transcript_items_seen
    );

    // Clean shutdown
    send_intent(&mut ws, serde_json::json!({"type": "stop_meeting"})).await;

    // A15 — durable-lane loss assertion (improvement #18). The durable
    // writer is still alive after stop (only `drop(server)` closes the
    // queue), so poll until the JSONL on disk catches up to what the
    // client saw, then assert it lost nothing.
    let meeting_id = meeting_id.expect("meeting_state_changed must carry an active meeting id");
    let mut disk_ids: HashSet<String> = HashSet::new();
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < drain_deadline {
        let items = auris_server::storage::persistence_loop::read_transcription(&meeting_id)
            .await
            .expect("read_transcription must not error");
        disk_ids = items.into_iter().map(|i| i.id).collect();
        if wire_item_ids.is_subset(&disk_ids) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let missing: Vec<&String> = wire_item_ids.difference(&disk_ids).collect();
    assert!(
        missing.is_empty(),
        "durable lane lost transcript items the client saw over fan-out: {:?} (on disk: {} items)",
        missing,
        disk_ids.len()
    );

    drop(server);
    tokio::fs::remove_dir_all(&data_dir).await.ok();
    std::env::remove_var("AURIS_DATA_DIR");
}
