//! Integration tests for the audio liveness reaper — the safety net
//! that ends a meeting whose `/audio` source disconnected and never
//! came back (`spawn_liveness_reaper` / `reap_stale_meeting` /
//! `apply_reaped_stop_outcome` in `ws/control.rs`).
//!
//! These tests drive the REAL server: control WS + `/audio` WS, the
//! real sweep loop, and the real Postgres `ended_at` write. The grace
//! window and sweep period are shrunk via env (600 ms / 100 ms) so a
//! reap is observable within a test-sized deadline; production keeps
//! its 15 min / 60 s defaults.
//!
//! This file is a DEDICATED test binary because the env knobs below
//! are process-global. Never set `AURIS_AUDIO_LIVENESS_TIMEOUT_MS` or
//! `AURIS_LIVENESS_SWEEP_MS` in any other integration-test file.

mod common;

use common::*;
use futures_util::SinkExt;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// Grace window before reaping. Kept ≫ the sweep period so a
/// reconnect inside the window always beats the next sweep.
const LIVENESS_TIMEOUT_MS: u64 = 600;
/// Reaper sweep period — exercises the `AURIS_LIVENESS_SWEEP_MS` hook.
const SWEEP_MS: u64 = 100;

/// Process-wide liveness env, set exactly once before any server
/// spawns. All tests in this binary share these values, so in-file
/// parallelism is safe.
fn liveness_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var(
            "AURIS_AUDIO_LIVENESS_TIMEOUT_MS",
            LIVENESS_TIMEOUT_MS.to_string(),
        );
        std::env::set_var("AURIS_LIVENESS_SWEEP_MS", SWEEP_MS.to_string());
        // Mock STT: no real Soniox connection (needs creds, emits
        // error events). Interval is huge so no transcript chatter
        // interleaves with the meeting_state events asserted below.
        std::env::set_var("AURIS_STT_MOCK", "1");
        std::env::set_var("AURIS_STT_MOCK_INTERVAL_MS", "60000");
    });
}

/// Connect the `/audio` websocket (binary PCM ingress). Must be called
/// while a meeting is active — the server refuses `/audio` with close
/// code 1011 otherwise (spec §4.2).
async fn connect_audio(addr: SocketAddr, token: &str) -> Ws {
    let url = format!("ws://{}/audio?token={}", addr, token);
    let req = url.into_client_request().expect("audio client request");
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("connect /audio");
    ws
}

async fn drain_snapshot(ws: &mut Ws) {
    let _ = next_event(ws, Duration::from_secs(2)).await;
}

/// Scan control-WS events until a `meeting_state_changed` with the
/// given state arrives, skipping everything else (status heartbeats,
/// metadata_changed, mode_changed, items updates). `None` if the
/// deadline passes first.
async fn wait_for_meeting_state(ws: &mut Ws, state: &str, deadline: Duration) -> Option<Value> {
    let end = tokio::time::Instant::now() + deadline;
    loop {
        let now = tokio::time::Instant::now();
        if now >= end {
            return None;
        }
        match next_event_opt(ws, end - now).await {
            None => return None,
            Some(evt) => {
                if evt["type"] == "meeting_state_changed" && evt["meeting_state"] == state {
                    return Some(evt);
                }
            }
        }
    }
}

/// Start a meeting on the control socket; returns the new meeting id
/// (carried on the `meeting_state_changed: active` event).
async fn start_meeting(ws: &mut Ws) -> String {
    send_intent(ws, json!({"type": "start_meeting"})).await;
    let evt = wait_for_meeting_state(ws, "active", Duration::from_secs(2))
        .await
        .expect("meeting_state_changed: active after start_meeting");
    evt["meeting_id"]
        .as_str()
        .expect("meeting_id on active event")
        .to_string()
}

/// True while `meeting_id` is still open (`ended_at IS NULL`) in the
/// shared Postgres. Scoped to this test's own meeting id — the DB is
/// shared across test binaries, so never assert on row counts.
async fn meeting_is_open(pool: &sqlx::PgPool, meeting_id: &str) -> bool {
    auris_server::storage::meetings::find_active_meetings_per_user(pool)
        .await
        .expect("find_active_meetings_per_user")
        .iter()
        .any(|(_user_id, id, ..)| id == meeting_id)
}

/// Test A — the core safety net: /audio dies mid-meeting and never
/// returns; the reaper must end the meeting (broadcast idle + persist
/// `ended_at`) once the grace window elapses.
///
/// Timing budget: 600 ms window + ≤100 ms sweep lag + broadcast slack,
/// asserted against a generous 5 s deadline to stay calm in CI.
#[tokio::test(flavor = "multi_thread")]
async fn reaper_ends_meeting_when_audio_never_returns() {
    liveness_env();
    let server = spawn_test_server().await;
    let mut control = connect(server.addr, "test-token").await;
    drain_snapshot(&mut control).await;

    let meeting_id = start_meeting(&mut control).await;

    // /audio binds to the active meeting's pipe, streams one frame,
    // then dies — the crash/force-quit scenario the reaper exists for.
    let mut audio = connect_audio(server.addr, "test-token").await;
    audio
        .send(Message::Binary(vec![0u8; 640]))
        .await
        .expect("send pcm frame");
    audio.close(None).await.expect("close /audio");
    drop(audio);

    let idle = wait_for_meeting_state(&mut control, "idle", Duration::from_secs(5)).await;
    assert!(
        idle.is_some(),
        "liveness reaper never ended the abandoned meeting \
         (no meeting_state_changed: idle within 5s of /audio dropping)"
    );

    // `ended_at` is persisted AFTER the broadcast in
    // apply_reaped_stop_outcome — poll the DB briefly.
    let pool = auris_server::storage::open_pool().await.expect("open pool");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while meeting_is_open(&pool, &meeting_id).await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "meeting {meeting_id} still has ended_at NULL 3s after the reap broadcast"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Test B — the disarm path: the client comes back inside the grace
/// window (well under 600 ms), so the reaper must NOT end the meeting
/// even after several windows + sweeps have elapsed.
#[tokio::test(flavor = "multi_thread")]
async fn audio_reconnect_within_grace_window_saves_meeting() {
    liveness_env();
    let server = spawn_test_server().await;
    let mut control = connect(server.addr, "test-token").await;
    drain_snapshot(&mut control).await;

    let meeting_id = start_meeting(&mut control).await;

    // First /audio connection drops...
    let mut audio = connect_audio(server.addr, "test-token").await;
    audio.close(None).await.expect("close /audio");
    drop(audio);
    // ...give the server a beat to process the close (which arms the
    // loss timer) BEFORE reconnecting. Without this, the server can
    // observe connect-then-close and leave the timer armed while the
    // fresh socket is healthy. 120 ms is still well inside the 600 ms
    // grace window.
    tokio::time::sleep(Duration::from_millis(120)).await;

    // The client returns and stays connected for the rest of the test.
    let _audio2 = connect_audio(server.addr, "test-token").await;

    // Watch ≥2 full grace windows + sweeps (1.5 s vs 600 ms + 100 ms):
    // no idle transition may arrive. Bounded to idle events only —
    // status heartbeats etc. are skipped by the helper.
    let idle = wait_for_meeting_state(&mut control, "idle", Duration::from_millis(1500)).await;
    assert!(
        idle.is_none(),
        "meeting was reaped despite /audio reconnecting inside the grace window: {idle:?}"
    );

    // Still open in the DB (ended_at IS NULL).
    let pool = auris_server::storage::open_pool().await.expect("open pool");
    assert!(
        meeting_is_open(&pool, &meeting_id).await,
        "meeting {meeting_id} was closed in the DB despite the reconnect"
    );

    // Clean up: explicit stop so the shared DB doesn't accumulate
    // open meetings for the dev|local user.
    send_intent(&mut control, json!({"type": "stop_meeting"})).await;
    let stopped = wait_for_meeting_state(&mut control, "idle", Duration::from_secs(2)).await;
    assert!(
        stopped.is_some(),
        "explicit stop_meeting did not produce idle"
    );
}

/// Test C — leftover-timer hygiene: arm the loss timer (drop /audio),
/// then stop the meeting normally BEFORE the window elapses. Past the
/// window + several sweeps, the reaper must not produce a second stop:
/// no extra `meeting_state_changed: idle`, no error event. Pins the
/// `is_meeting_active` filter in `stale_audio_meetings` end-to-end —
/// a user stop does NOT clear the audio-loss timer, so that filter is
/// the only guard.
#[tokio::test(flavor = "multi_thread")]
async fn reap_is_noop_when_meeting_already_stopped() {
    liveness_env();
    let server = spawn_test_server().await;
    let mut control = connect(server.addr, "test-token").await;
    drain_snapshot(&mut control).await;

    let _meeting_id = start_meeting(&mut control).await;

    // Arm the loss timer.
    let mut audio = connect_audio(server.addr, "test-token").await;
    audio.close(None).await.expect("close /audio");
    drop(audio);
    // Let the server process the close so the timer is actually armed
    // before we stop — otherwise this test wouldn't exercise anything.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Normal stop, well inside the 600 ms window.
    send_intent(&mut control, json!({"type": "stop_meeting"})).await;
    let stopped = wait_for_meeting_state(&mut control, "idle", Duration::from_secs(2)).await;
    assert!(stopped.is_some(), "stop_meeting did not produce idle");

    // Past the grace window + several sweeps: the leftover timer must
    // not fire a second stop. Scan ALL events for 1.5 s and reject a
    // second idle or any error frame.
    let end = tokio::time::Instant::now() + Duration::from_millis(1500);
    loop {
        let now = tokio::time::Instant::now();
        if now >= end {
            break;
        }
        if let Some(evt) = next_event_opt(&mut control, end - now).await {
            assert!(
                !(evt["type"] == "meeting_state_changed" && evt["meeting_state"] == "idle"),
                "reaper produced a second stop for an already-stopped meeting: {evt}"
            );
            assert_ne!(
                evt["type"], "error",
                "unexpected error event after a clean stop: {evt}"
            );
        }
    }
}
