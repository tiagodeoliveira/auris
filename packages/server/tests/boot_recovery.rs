//! Boot-recovery integration tests (improvement #28).
//!
//! `recover_active_meetings` (`src/ws/control.rs`) is the boot path
//! that resurrects a meeting whose `ended_at IS NULL` after a crash
//! or redeploy — the documented kleos deploy flow (`compose up -d
//! --force-recreate auris`) exercises it on every redeploy during a
//! live meeting. Every OTHER integration test forces it off via
//! `AURIS_SKIP_BOOT_RECOVERY=1`; this binary is the one place it
//! runs for real.
//!
//! A crash leaves exactly two durable artifacts: a `meetings` row
//! with NULL `ended_at`, and the per-meeting transcription JSONL
//! (`<AURIS_DATA_DIR>/blobs/meetings/<id>/transcription.jsonl`).
//! Nothing in-memory survives, so these tests fabricate the
//! artifacts directly and boot ONE server against them — no
//! second in-process server, no race with a still-running mock STT.
//!
//! Separate test binary ON PURPOSE: removing
//! `AURIS_SKIP_BOOT_RECOVERY` is process-global; a separate process
//! guarantees it can't leak into the other test binaries (which
//! depend on the flag staying set).

mod common;

use auris_server::protocol::Item;
use auris_server::storage;
use auris_server::storage::persistence_loop;
use common::{connect, next_event, send_intent, spawn_test_server_with_opts, SpawnOpts, Ws};
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;

/// Tests here mutate process-global env (AURIS_SKIP_BOOT_RECOVERY,
/// AURIS_DATA_DIR, the mock-STT knobs) and share one Postgres.
/// Serialize them — belt-and-braces on top of the repo-standard
/// `--test-threads=1` invocation, so a bare `cargo test --test
/// boot_recovery` (default parallel threads) is still safe.
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Per-test baseline. Loads `.env` (DATABASE_URL), points
/// AURIS_DATA_DIR at a fresh per-test temp dir (so fabricated JSONL
/// can't collide across tests), forces mock STT + disabled audio/LLM,
/// opens the pool, and pre-cleans unfinished meetings.
///
/// Pre-clean rationale: recovery scans ALL users' unfinished
/// meetings (`find_active_meetings_per_user`), so a straggler from
/// an earlier test — or a zombie row on the dev DB — would be
/// resurrected alongside this test's fixture and pollute the
/// snapshot. NOTE: on a shared dev DB this ends a genuinely live
/// meeting too; acceptable — the integration tests already assume
/// they own the dev DB (see chat_attachments.rs seeding).
async fn setup() -> sqlx::PgPool {
    let _ = dotenvy::dotenv();
    let data_dir =
        std::env::temp_dir().join(format!("auris-boot-recovery-{}", uuid::Uuid::new_v4()));
    // storage::data_dir() creates the directory on first use.
    std::env::set_var("AURIS_DATA_DIR", &data_dir);
    std::env::set_var("AURIS_STT_MOCK", "1");
    std::env::set_var("AURIS_AUDIO_DISABLED", "1");
    std::env::set_var("AURIS_LLM_DISABLED", "1");
    // Default to an effectively-silent mock STT (60 s between canned
    // chunks) so snapshots contain ONLY fabricated items. The
    // pipeline-respawn test overrides this to 100 ms at its entry.
    std::env::set_var("AURIS_STT_MOCK_INTERVAL_MS", "60000");

    let pool = storage::open_pool()
        .await
        .expect("open pool — DATABASE_URL required (run `just db-up`)");
    sqlx::query("UPDATE meetings SET ended_at = NOW() WHERE ended_at IS NULL")
        .execute(&pool)
        .await
        .expect("pre-clean unfinished meetings");
    pool
}

/// The auth-disabled WS handler resolves every connection to the
/// dev user's local `users.id` UUID (resolve_user_id →
/// upsert_user_by_auth0_sub("dev|local", …)). Fabricated meeting
/// rows must use the SAME UUID or recovery rehydrates a session the
/// connecting client never sees.
async fn dev_user_id(pool: &sqlx::PgPool) -> String {
    storage::users::upsert_user_by_auth0_sub(
        pool,
        "dev|local",
        Some("dev@local"),
        Some("Local Dev"),
    )
    .await
    .expect("upsert dev user")
    .id
}

fn item(id: &str, text: &str, t: u64) -> Item {
    Item {
        id: id.into(),
        text: text.into(),
        detail: None,
        t,
        meta: None,
    }
}

/// Fabricate exactly the durable artifacts a crashed server leaves
/// behind: a `meetings` row with `ended_at IS NULL`, and (when
/// `items` is non-empty) the per-meeting transcription JSONL written
/// through the same public API the live persistence loop uses.
/// Returns the new meeting id.
async fn fabricate_crashed_meeting(
    pool: &sqlx::PgPool,
    user_id: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    metadata_json: &str,
    assist_sensitivity: Option<&str>,
    items: &[Item],
) -> String {
    let meeting_id = uuid::Uuid::new_v4().to_string();
    storage::meetings::insert_meeting(
        pool,
        &meeting_id,
        user_id,
        started_at,
        Some("fabricated crash fixture"),
        metadata_json,
        assist_sensitivity,
    )
    .await
    .expect("insert crashed-meeting row");
    if !items.is_empty() {
        let path = persistence_loop::transcription_path(&meeting_id).expect("transcription path");
        persistence_loop::append_jsonl(&path, items)
            .await
            .expect("write transcription.jsonl");
    }
    meeting_id
}

/// Connect the control WS and return (socket, snapshot). The
/// snapshot is always the FIRST frame — `run_control_socket` sends
/// it directly before entering the broadcast loop — so no drain
/// loop is needed. Generous 5 s timeout: boot recovery does DB +
/// disk I/O before axum starts serving.
async fn connect_and_snapshot(addr: SocketAddr) -> (Ws, Value) {
    let mut ws = connect(addr, "test-token").await;
    let snap = next_event(&mut ws, Duration::from_secs(5)).await;
    assert_eq!(
        snap["type"], "snapshot",
        "first frame must be the snapshot: {snap:#?}"
    );
    (ws, snap)
}

/// Send `stop_meeting` and drain events until `meeting_state_changed
/// → idle`. Tolerates interleaved events (the post-snapshot
/// quick_asks items_update replay, mock-STT items_update frames,
/// status heartbeats) — same pattern as chat_attachments.rs'
/// start_meeting_via_ws.
async fn stop_meeting_and_wait_idle(ws: &mut Ws) {
    send_intent(ws, serde_json::json!({"type": "stop_meeting"})).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let evt = next_event(ws, Duration::from_secs(2)).await;
        if evt["type"] == "meeting_state_changed" {
            assert_eq!(
                evt["meeting_state"], "idle",
                "stop_meeting must land in idle: {evt:#?}"
            );
            return;
        }
    }
    panic!("no meeting_state_changed within 5s of stop_meeting");
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_recovery_resurrects_unfinished_meeting_with_replayed_transcript() {
    let _guard = LOCK.lock().await;
    let pool = setup().await;
    let uid = dev_user_id(&pool).await;

    let items = vec![
        item("r1", "Let's pick up the budget discussion.", 1_000),
        item("r2", "Engineering asked for three more weeks.", 2_000),
        item("r3", "Action: circulate the revised timeline.", 3_000),
    ];
    let meeting_id = fabricate_crashed_meeting(
        &pool,
        &uid,
        chrono::Utc::now(),
        r#"{"topic":"budget"}"#,
        Some("aggressive"),
        &items,
    )
    .await;

    let server = spawn_test_server_with_opts(SpawnOpts {
        boot_recovery: true,
    })
    .await;
    let (mut ws, snap) = connect_and_snapshot(server.addr).await;

    assert_eq!(
        snap["meeting_state"], "active",
        "recovered meeting must be active: {snap:#?}"
    );
    assert_eq!(snap["meeting_id"].as_str(), Some(meeting_id.as_str()));
    assert_eq!(snap["status"]["listening"], true);
    // assist_sensitivity round-trip: NULL/unknown falls back to
    // moderate, so asserting "aggressive" proves parse_wire ran
    // against the recovered column value (control.rs).
    assert_eq!(snap["assist_sensitivity"], "aggressive");
    // metadata JSON round-trip (control.rs serde_json::from_str path).
    assert_eq!(snap["metadata"]["topic"], "budget");
    // Recovery resets the mode to the default ("transcript") and
    // replays the JSONL there; the snapshot ships the current
    // mode's items.
    assert_eq!(snap["mode"], "transcript");
    let ids: Vec<&str> = snap["items"]
        .as_array()
        .expect("items array")
        .iter()
        .map(|i| i["id"].as_str().expect("item id"))
        .collect();
    assert_eq!(
        ids,
        vec!["r1", "r2", "r3"],
        "replayed transcript items, in JSONL order"
    );

    // Hygiene: end the recovered meeting so its pipeline tasks stop
    // and the row doesn't lean on the next test's pre-clean.
    stop_meeting_and_wait_idle(&mut ws).await;
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
async fn skip_flag_set_means_no_recovery() {
    let _guard = LOCK.lock().await;
    let pool = setup().await;
    let uid = dev_user_id(&pool).await;
    let meeting_id = fabricate_crashed_meeting(
        &pool,
        &uid,
        chrono::Utc::now(),
        "{}",
        None,
        &[item("g1", "should not be replayed", 1_000)],
    )
    .await;

    // boot_recovery: false sets AURIS_SKIP_BOOT_RECOVERY=1 — the gate
    // every other integration test relies on (control.rs early
    // return). Permanent negative control: proves the resurrect test
    // above depends on the recovery path, not residual registry
    // state, and pins the gate's behavior.
    let server = spawn_test_server_with_opts(SpawnOpts {
        boot_recovery: false,
    })
    .await;
    let (_ws, snap) = connect_and_snapshot(server.addr).await;

    assert_eq!(
        snap["meeting_state"], "idle",
        "skip flag must gate recovery"
    );
    assert!(snap["items"].as_array().expect("items array").is_empty());
    assert!(
        snap.get("meeting_id").is_none() || snap["meeting_id"].is_null(),
        "no meeting_id when idle: {snap:#?}"
    );

    // End the fixture row ourselves instead of leaning on the next
    // test's pre-clean.
    sqlx::query("UPDATE meetings SET ended_at = NOW() WHERE id = $1")
        .bind(&meeting_id)
        .execute(&pool)
        .await
        .expect("cleanup fixture row");
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_recovery_respawns_live_pipeline() {
    let _guard = LOCK.lock().await;
    let pool = setup().await;
    // Fast mock STT: the respawned pipeline should produce NEW
    // transcript items well inside the 5 s deadline (same 100 ms
    // cadence live_pipeline_smoke.rs uses).
    std::env::set_var("AURIS_STT_MOCK_INTERVAL_MS", "100");
    let uid = dev_user_id(&pool).await;
    let _meeting_id = fabricate_crashed_meeting(
        &pool,
        &uid,
        chrono::Utc::now(),
        "{}",
        None,
        &[item("r1", "pre-crash line", 1_000)],
    )
    .await;

    let server = spawn_test_server_with_opts(SpawnOpts {
        boot_recovery: true,
    })
    .await;
    let (mut ws, snap) = connect_and_snapshot(server.addr).await;
    assert_eq!(snap["meeting_state"], "active");

    // An items_update carrying a transcript item whose id is NOT the
    // fabricated one proves spawn_live_pipeline actually ran (mock
    // STT → transcript summarizer → broadcast) — registry mutation
    // alone can't produce this.
    let mut new_items_seen = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && new_items_seen < 1 {
        let evt = next_event(&mut ws, Duration::from_millis(500)).await;
        if evt["type"] == "items_update" && evt["mode"] == "transcript" {
            new_items_seen += evt["items"]
                .as_array()
                .expect("items array")
                .iter()
                .filter(|i| i["id"] != "r1")
                .count();
        }
    }
    assert!(
        new_items_seen >= 1,
        "expected ≥1 NEW transcript item from the respawned pipeline, got {new_items_seen}"
    );

    stop_meeting_and_wait_idle(&mut ws).await;
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_recovery_picks_newest_meeting_per_user() {
    let _guard = LOCK.lock().await;
    let pool = setup().await;
    let uid = dev_user_id(&pool).await;

    // Two unfinished rows for the same user — e.g. a crash mid-stop
    // left a straggler. find_active_meetings_per_user orders
    // started_at DESC; the seen_users dedupe must keep the newest.
    let older_id = fabricate_crashed_meeting(
        &pool,
        &uid,
        chrono::Utc::now() - chrono::Duration::hours(1),
        "{}",
        None,
        &[],
    )
    .await;
    let newer_id =
        fabricate_crashed_meeting(&pool, &uid, chrono::Utc::now(), "{}", None, &[]).await;

    let server = spawn_test_server_with_opts(SpawnOpts {
        boot_recovery: true,
    })
    .await;
    let (mut ws, snap) = connect_and_snapshot(server.addr).await;

    assert_eq!(snap["meeting_state"], "active");
    assert_eq!(
        snap["meeting_id"].as_str(),
        Some(newer_id.as_str()),
        "dedupe must recover the NEWEST meeting; older straggler {older_id} ignored"
    );

    stop_meeting_and_wait_idle(&mut ws).await;
    // The ignored older row stays ended_at IS NULL by design — end it
    // here instead of leaning on the next test's pre-clean.
    sqlx::query("UPDATE meetings SET ended_at = NOW() WHERE id = $1")
        .bind(&older_id)
        .execute(&pool)
        .await
        .expect("cleanup older straggler row");
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_recovery_missing_jsonl_recovers_with_empty_transcript() {
    let _guard = LOCK.lock().await;
    let pool = setup().await;
    let uid = dev_user_id(&pool).await;
    // Row only — no transcription.jsonl was ever written (crash
    // before the first committed transcript item). read_transcription
    // returns Ok(vec![]) for NotFound; recovery must proceed.
    let meeting_id =
        fabricate_crashed_meeting(&pool, &uid, chrono::Utc::now(), "{}", None, &[]).await;

    let server = spawn_test_server_with_opts(SpawnOpts {
        boot_recovery: true,
    })
    .await;
    let (mut ws, snap) = connect_and_snapshot(server.addr).await;

    assert_eq!(
        snap["meeting_state"], "active",
        "missing JSONL must not abort recovery: {snap:#?}"
    );
    assert_eq!(snap["meeting_id"].as_str(), Some(meeting_id.as_str()));
    assert!(
        snap["items"].as_array().expect("items array").is_empty(),
        "no JSONL → empty replayed transcript (mock STT is on the 60 s \
         silent cadence, so nothing else can have landed): {snap:#?}"
    );

    stop_meeting_and_wait_idle(&mut ws).await;
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
async fn recovered_meeting_stops_cleanly() {
    let _guard = LOCK.lock().await;
    let pool = setup().await;
    let uid = dev_user_id(&pool).await;
    let meeting_id = fabricate_crashed_meeting(
        &pool,
        &uid,
        chrono::Utc::now(),
        "{}",
        None,
        &[item("r1", "pre-crash line", 1_000)],
    )
    .await;

    let server = spawn_test_server_with_opts(SpawnOpts {
        boot_recovery: true,
    })
    .await;
    let (mut ws, snap) = connect_and_snapshot(server.addr).await;
    assert_eq!(snap["meeting_state"], "active");

    // Stop the RECOVERED runtime: apply_intent + the closed_meeting
    // persistence side-effect must work against a MeetingRuntime that
    // was rehydrated, not created via start_meeting.
    stop_meeting_and_wait_idle(&mut ws).await;

    // end_meeting is written AFTER the broadcast (persistence
    // side-effects run at the tail of dispatch_intent) — poll the DB
    // rather than asserting immediately.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut ended_at: Option<chrono::DateTime<chrono::Utc>> = None;
    while tokio::time::Instant::now() < deadline {
        let (row,): (Option<chrono::DateTime<chrono::Utc>>,) =
            sqlx::query_as("SELECT ended_at FROM meetings WHERE id = $1")
                .bind(&meeting_id)
                .fetch_one(&pool)
                .await
                .expect("recovered meeting row still exists");
        if row.is_some() {
            ended_at = row;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        ended_at.is_some(),
        "stop_meeting on a recovered meeting must persist ended_at"
    );
    drop(server);
}
