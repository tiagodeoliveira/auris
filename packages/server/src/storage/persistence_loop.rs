//! Per-meeting persistence: transcripts go to JSONL on disk; every
//! other mode (highlights / actions / open_questions / summary /
//! chat) goes to the `items` table in Postgres.
//!
//! All writes flow through ONE durable-writer task fed by the bounded
//! mpsc lane of `context::EventBus` (see `spawn_durable_writer`): the queue
//! cannot lag, producers backpressure when the writer falls behind,
//! and I/O failures retry 3× before being dropped loudly.
//!
//! Transcript JSONL layout:
//!   `<DATA_DIR>/blobs/meetings/<meeting_id>/transcription.jsonl`,
//! one JSON-encoded `Item` per line. Sequential append-only; we
//! read the whole file as part of moment-summary windowing and
//! mnemo push, never query rows individually. Keeping it as a flat
//! file matches the future S3-key layout (one prefix per meeting)
//! and avoids bloating the relational DB with text blobs.
//!
//! Items table (everything except transcript): one row per
//! emitted item. Replace-strategy modes (highlights / summary /
//! chat) atomically delete + re-insert per fire so the persisted
//! state matches the live state exactly. Append modes (actions /
//! open_questions) just insert with `ON CONFLICT DO NOTHING`.
//! Powers the meeting-detail view's per-mode tabs once a meeting
//! has ended.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::PgPool;
use tokio::fs::{create_dir_all, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use crate::mnemo::pusher::PusherState;
use crate::mnemo::MnemoClient;
use crate::protocol::{Event, Item, UpdateStrategy, UserEvent};
use crate::session::SessionRegistry;

/// Spawn THE durable-writer task: the single consumer of the
/// durable mpsc lane. Owns, in per-event order:
///   1. transcript `ItemsUpdate` / `TranscriptTail` → per-meeting
///      JSONL append,
///   2. every other `ItemsUpdate` / non-streaming `ItemUpdated`
///      → the Postgres `items` table,
///   3. the mnemo pusher handler (session lifecycle + transcript
///      pushes; its HTTP calls are internally `tokio::spawn`ed so
///      they add no latency here).
///
/// An mpsc receiver cannot lag — unlike the broadcast ring this
/// replaces, overload backpressures producers in `EventBus::emit`
/// instead of silently losing the system of record. When every
/// sender drops (shutdown), `recv()` drains the backlog and then
/// returns `None`, so the queue is flushed before exit.
pub fn spawn_durable_writer(
    state: Arc<Mutex<SessionRegistry>>,
    db: PgPool,
    mnemo: MnemoClient,
) -> mpsc::Sender<UserEvent> {
    let (tx, rx) = mpsc::channel(crate::context::bus::DURABLE_QUEUE_CAPACITY);
    tokio::spawn(durable_writer_loop(state, db, mnemo, rx));
    tx
}

async fn durable_writer_loop(
    state: Arc<Mutex<SessionRegistry>>,
    db: PgPool,
    mnemo: MnemoClient,
    mut rx: mpsc::Receiver<UserEvent>,
) {
    info!("durable writer started");
    // Per-user mnemo pusher state (session_id / meeting_id /
    // metadata). Previously owned by the pusher's own broadcast
    // loop; now lives here so pusher handling shares the durable
    // FIFO with persistence.
    let mut pusher_state: HashMap<String, PusherState> = HashMap::new();
    while let Some(envelope) = rx.recv().await {
        handle_durable_event(&state, &db, &mnemo, &mut pusher_state, envelope).await;
    }
    info!("durable writer: senders dropped and queue drained; exiting");
}

/// Process one durable event: persistence first, mnemo push second.
async fn handle_durable_event(
    state: &Arc<Mutex<SessionRegistry>>,
    db: &PgPool,
    mnemo: &MnemoClient,
    pusher_state: &mut HashMap<String, PusherState>,
    envelope: UserEvent,
) {
    match &envelope.event {
        Event::ItemsUpdate { mode, items } if mode == "transcript" && !items.is_empty() => {
            let known_meeting_id = envelope.meeting_id.clone();
            retry_durable_write("transcript jsonl append", || {
                persist_transcript_items(
                    state,
                    &envelope.user_id,
                    known_meeting_id.as_deref(),
                    items,
                )
            })
            .await;
        }
        // Empty transcript update: nothing to append, and it must NOT
        // fall through to the items-DB arm below (transcript lives in
        // JSONL only — matches the old spawn_items_task skip).
        Event::ItemsUpdate { mode, .. } if mode == "transcript" => {}
        // Finalize's drained post-stop tail (improvement #19). Addressed
        // by the meeting_id carried in the event — no active-session
        // lookup, so it lands in the stopped meeting's JSONL.
        Event::TranscriptTail { meeting_id, items } if !items.is_empty() => {
            retry_durable_write("transcript tail jsonl append", || {
                persist_transcript_tail(meeting_id, items)
            })
            .await;
        }
        Event::ItemsUpdate { mode, items } => {
            retry_durable_write("items table write", || {
                persist_items_update(state, db, &envelope.user_id, mode, items)
            })
            .await;
        }
        // Streaming chat partials never reach this queue (routed
        // fanout-only at the emit site + excluded by is_durable);
        // what lands here is the expand-item detail write.
        Event::ItemUpdated { mode, item } if mode != "transcript" => {
            retry_durable_write("item detail update", || {
                persist_item_updated(state, db, &envelope.user_id, mode, item)
            })
            .await;
        }
        _ => {}
    }
    // Mnemo push handling (session lifecycle + transcript turns).
    // Skipped when disabled so we don't accumulate dead per-user
    // state; `handle_event` itself is also a no-op on Disabled.
    if mnemo.is_enabled() {
        let entry = pusher_state.entry(envelope.user_id.clone()).or_default();
        crate::mnemo::pusher::handle_event(mnemo, &envelope.user_id, entry, envelope.event).await;
    }
}

/// Run a fallible durable write with bounded retry: 3 attempts,
/// 100 ms / 200 ms backoff, then drop the write with a loud
/// `error!`. Never blocks the queue forever — a hard-down Postgres
/// must not stall transcript JSONL appends queued behind it. (The
/// pre-EventBus code swallowed every failure with a single
/// `warn!` and zero retries.)
async fn retry_durable_write<F, Fut>(what: &'static str, mut op: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    const MAX_ATTEMPTS: u32 = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        match op().await {
            Ok(()) => return,
            Err(e) if attempt < MAX_ATTEMPTS => {
                warn!(error = ?e, what, attempt, "durable write failed; retrying");
                tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
            }
            Err(e) => {
                error!(
                    error = ?e,
                    what,
                    attempts = MAX_ATTEMPTS,
                    "durable write failed after retries; event dropped"
                );
            }
        }
    }
}

/// Persist a single-item in-place update. Looks up the user's
/// active meeting (skips on race with stop) and updates the row's
/// `detail` column. The full Item is passed for completeness even
/// though we only update one column today — opens the door to
/// future per-item edits without changing the persistence shape.
async fn persist_item_updated(
    state: &Arc<Mutex<SessionRegistry>>,
    db: &PgPool,
    user_id: &str,
    mode: &str,
    item: &Item,
) -> Result<()> {
    let meeting_id = {
        let s = state.lock().await;
        match s
            .user(user_id)
            .and_then(|u| u.meeting.as_ref())
            .map(|m| m.meeting_id.clone())
        {
            Some(m) => m,
            None => return Ok(()),
        }
    };
    crate::storage::items::update_item_detail(
        db,
        &meeting_id,
        mode,
        &item.id,
        item.detail.as_deref(),
    )
    .await
}

async fn persist_items_update(
    state: &Arc<Mutex<SessionRegistry>>,
    db: &PgPool,
    user_id: &str,
    mode: &str,
    items: &[Item],
) -> Result<()> {
    let (meeting_id, strategy) = {
        let s = state.lock().await;
        let user = match s.user(user_id) {
            Some(u) => u,
            None => return Ok(()),
        };
        let meeting_id = match user.meeting.as_ref().map(|m| m.meeting_id.clone()) {
            Some(m) => m,
            None => return Ok(()),
        };
        let strategy = user
            .available_modes
            .iter()
            .find(|m| m.id == mode)
            .map(|m| m.update_strategy);
        (meeting_id, strategy)
    };
    let Some(strategy) = strategy else {
        return Ok(());
    };

    match strategy {
        UpdateStrategy::Replace => {
            crate::storage::items::replace_items_for_meeting_mode(db, &meeting_id, mode, items)
                .await?;
        }
        UpdateStrategy::Append => {
            // Each `insert_item_row` call below is its own implicit
            // transaction — do NOT wrap this loop in one shared
            // transaction as a perf optimization. Chat pairs arrive as
            // a batch (`vec![user_item, assistant_item]` in one
            // `ItemsUpdate`, see `agent::chat`), and chat rows carry
            // `t_ms = 0` (no other ordering signal), so
            // `storage::items::list_chat_messages_for_meeting` orders
            // them by `created_at`. Postgres `now()` is transaction-
            // start time; one shared transaction would give every row
            // in a batch an IDENTICAL `created_at` and make
            // wearer/assistant order nondeterministic. (A secondary
            // `, id` tie-breaker would not fix this — uuid ids don't
            // encode intent order, it would just make the wrong order
            // stable.)
            for item in items {
                // Skip transient optimistic placeholders — chat-mode
                // emits a `meta.role == "assistant-pending"` row to
                // every connected client the moment a question lands,
                // then the agent replaces it under the same id with
                // the real reply (handled by merge_items_in_mode).
                // Persisting the pending would lock the row's text at
                // empty (insert_item_row uses ON CONFLICT DO NOTHING)
                // and the real reply would never land in the DB.
                if is_pending_chat_item(item) {
                    continue;
                }
                crate::storage::items::insert_item_row(db, &meeting_id, mode, item).await?;
            }
        }
    }
    Ok(())
}

/// True if `item` is a transient chat placeholder (the bubble shown
/// while the agent is still composing a reply). These are emitted on
/// the wire so every surface sees the pending state, but they don't
/// belong in the DB — the final reply will land under the same id and
/// is the row worth persisting.
fn is_pending_chat_item(item: &Item) -> bool {
    item.meta
        .as_ref()
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        == Some("assistant-pending")
}

/// Append `items` to a meeting's transcription file. Prefers the
/// producer-stamped `known_meeting_id` (no registry race); falls back
/// to the user's active meeting for producers that don't stamp. No-op
/// if neither resolves — race with teardown.
async fn persist_transcript_items(
    state: &Arc<Mutex<SessionRegistry>>,
    user_id: &str,
    known_meeting_id: Option<&str>,
    items: &[Item],
) -> Result<()> {
    let meeting_id = match known_meeting_id {
        Some(m) => Some(m.to_string()),
        None => {
            let s = state.lock().await;
            s.user(user_id)
                .and_then(|u| u.meeting.as_ref())
                .map(|m| m.meeting_id.clone())
        }
    };
    let Some(meeting_id) = meeting_id else {
        return Ok(());
    };
    let path = transcription_path(&meeting_id)?;
    append_jsonl(&path, items).await
}

/// Append finalize's drained tail to the NAMED meeting's transcription
/// file. Unlike `persist_transcript_items` this takes the meeting id
/// from the event instead of resolving the user's currently-active
/// meeting — at tail time the meeting has already stopped (and a new
/// one may even be active), so an active-session lookup would either
/// silently drop the items or write them into the wrong meeting's file.
/// The `info!` line is the production signal that the drain tail made
/// it to disk (grep `transcript tail persisted` in kleos logs).
async fn persist_transcript_tail(meeting_id: &str, items: &[Item]) -> Result<()> {
    let path = transcription_path(meeting_id)?;
    append_jsonl(&path, items).await?;
    info!(%meeting_id, items_len = items.len(), "transcript tail persisted");
    Ok(())
}

/// Read the per-meeting transcription jsonl back into `Item`s.
/// Returns `Ok(vec![])` when the file doesn't exist (no transcript
/// was ever committed) or any line fails to parse — boot recovery
/// is best-effort, partial transcripts are better than aborting
/// the whole resume because of one corrupted line.
pub async fn read_transcription(meeting_id: &str) -> Result<Vec<Item>> {
    let path = transcription_path(meeting_id)?;
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(anyhow::Error::from(e).context(format!("read {}", path.display()))),
    };
    Ok(content
        .lines()
        .filter_map(|line| serde_json::from_str::<Item>(line).ok())
        .collect())
}

/// `<DATA_DIR>/blobs/meetings/<meeting_id>/transcription.jsonl`.
pub fn transcription_path(meeting_id: &str) -> Result<PathBuf> {
    let dir = crate::storage::data_dir()?;
    Ok(dir
        .join("blobs")
        .join("meetings")
        .join(meeting_id)
        .join("transcription.jsonl"))
}

/// Append items as JSON-encoded lines to `path`, creating the
/// parent directory if missing. One `serde_json::to_string` +
/// newline per item; flushed at the end.
pub async fn append_jsonl(path: &Path, items: &[Item]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)
            .await
            .with_context(|| format!("create_dir_all {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    for item in items {
        let mut line = serde_json::to_string(item).context("serialize item")?;
        line.push('\n');
        file.write_all(line.as_bytes())
            .await
            .with_context(|| format!("write {}", path.display()))?;
    }
    file.flush().await.context("flush jsonl")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Item;

    fn temp_path(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mc-persist-{}-{}", uuid::Uuid::new_v4(), suffix))
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

    #[tokio::test]
    async fn append_creates_file_and_writes_jsonl() {
        let path = temp_path("created.jsonl");
        append_jsonl(&path, &[item("a", "hello", 100)])
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(
            content.starts_with('{'),
            "expected JSON object on first line"
        );
        assert!(content.contains("\"hello\""));
        assert!(content.ends_with('\n'));

        tokio::fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn append_is_additive() {
        let path = temp_path("append.jsonl");
        append_jsonl(&path, &[item("a", "first", 100)])
            .await
            .unwrap();
        append_jsonl(&path, &[item("b", "second", 200)])
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "expected one line per call");
        assert!(lines[0].contains("first"));
        assert!(lines[1].contains("second"));

        tokio::fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn append_creates_missing_parent_dirs() {
        let base = temp_path("nested");
        let path = base.join("a/b/c/transcription.jsonl");
        append_jsonl(&path, &[item("x", "deep", 10)]).await.unwrap();
        assert!(path.exists(), "file should be created with all parent dirs");
        tokio::fs::remove_dir_all(&base).await.ok();
    }

    /// Regression (improvement #19): the drained post-stop tail must be
    /// appended to the NAMED meeting's transcription file even though no
    /// session/user is active anymore — `persist_transcript_items` would
    /// have silently dropped it (it resolves the file via the user's
    /// currently-active meeting). Uses a scoped AURIS_DATA_DIR, same
    /// pattern as `api::artifacts` tests.
    #[tokio::test]
    async fn transcript_tail_appends_for_named_meeting_without_active_session() {
        let dir = std::env::temp_dir().join(format!("auris-tail-test-{}", uuid::Uuid::new_v4()));
        std::env::set_var("AURIS_DATA_DIR", &dir);

        let meeting_id = format!("m-{}", uuid::Uuid::new_v4());
        persist_transcript_tail(
            &meeting_id,
            &[
                item("tail-1", "so let's wrap up", 90_000),
                item("tail-2", "I'll send that tomorrow", 93_000),
            ],
        )
        .await
        .expect("tail persistence must succeed without any active session");

        let read_back = read_transcription(&meeting_id).await.unwrap();
        assert_eq!(read_back.len(), 2, "both tail items must be on disk");
        assert_eq!(read_back[0].id, "tail-1");
        assert_eq!(read_back[0].text, "so let's wrap up");
        assert_eq!(read_back[1].id, "tail-2");
        assert_eq!(read_back[1].t, 93_000);

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    // ─── Durable-writer task (improvement #18) ──────────────────────

    use crate::protocol::Intent;

    /// Serialise `AURIS_DATA_DIR` mutation across writer tests —
    /// `data_dir()` reads the env var at every call, and `cargo test`
    /// runs tests in parallel threads sharing one process env.
    static DATA_DIR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn registry_with_active_meeting(uid: &str) -> (Arc<Mutex<SessionRegistry>>, String) {
        let state = Arc::new(Mutex::new(SessionRegistry::new()));
        let meeting_id = {
            let mut s = state.lock().await;
            s.apply_intent(
                uid,
                Intent::StartMeeting {
                    description: None,
                    metadata: None,
                    audio_source_device_id: None,
                    assist_sensitivity: None,
                },
            );
            s.active_meeting_id_for(uid).expect("meeting active")
        };
        (state, meeting_id)
    }

    /// A PgPool that parses but never connects — transcript-mode
    /// writes don't touch Postgres, so the JSONL tests need no DB.
    fn lazy_pool() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .expect("lazy pool parses")
    }

    #[tokio::test]
    async fn durable_writer_appends_transcript_jsonl_in_order_and_drains_on_close() {
        let _env_guard = DATA_DIR_LOCK.lock().await;
        let data_dir = temp_path("durable-writer");
        std::env::set_var("AURIS_DATA_DIR", &data_dir);

        let uid = "u-durable-writer";
        let (state, meeting_id) = registry_with_active_meeting(uid).await;
        let tx = spawn_durable_writer(
            state.clone(),
            lazy_pool(),
            crate::mnemo::MnemoClient::Disabled,
        );

        // 5 sequential transcript events. The old broadcast design
        // could drop any of these on Lagged; the mpsc queue cannot.
        for i in 0u64..5 {
            tx.send(UserEvent::new(
                uid,
                Event::ItemsUpdate {
                    mode: "transcript".into(),
                    items: vec![item(&format!("t{i}"), &format!("line {i}"), i)],
                },
            ))
            .await
            .expect("queue accepts while writer alive");
        }
        // Dropping the only sender closes the queue: the writer must
        // DRAIN the backlog before exiting (shutdown-drain guarantee).
        drop(tx);

        let path = transcription_path(&meeting_id).unwrap();
        let mut lines: Vec<String> = Vec::new();
        for _ in 0..200 {
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                lines = content.lines().map(str::to_owned).collect();
                if lines.len() == 5 {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(lines.len(), 5, "all 5 lines drained to {}", path.display());
        for (i, line) in lines.iter().enumerate() {
            let it: Item = serde_json::from_str(line).expect("valid JSONL line");
            assert_eq!(
                it.text,
                format!("line {i}"),
                "JSONL order must match emit order"
            );
        }

        std::env::remove_var("AURIS_DATA_DIR");
        tokio::fs::remove_dir_all(&data_dir).await.ok();
    }

    #[sqlx::test]
    async fn durable_writer_routes_items_update_to_db(pool: sqlx::PgPool) {
        // Non-transcript ItemsUpdate → items table, same semantics as
        // the old spawn_items_task (strategy from available_modes,
        // meeting from the registry).
        let user_row = crate::storage::users::upsert_user_by_auth0_sub(
            &pool,
            &format!("test|{}", uuid::Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        let uid = user_row.id;
        let (state, meeting_id) = registry_with_active_meeting(&uid).await;
        crate::storage::meetings::insert_meeting(
            &pool,
            &meeting_id,
            &uid,
            chrono::Utc::now(),
            None,
            "{}",
            None,
        )
        .await
        .unwrap();

        let tx = spawn_durable_writer(
            state.clone(),
            pool.clone(),
            crate::mnemo::MnemoClient::Disabled,
        );
        tx.send(UserEvent::new(
            uid.clone(),
            Event::ItemsUpdate {
                mode: "highlights".into(),
                items: vec![item("h-1", "key point", 5)],
            },
        ))
        .await
        .unwrap();
        drop(tx);

        let mut grouped = std::collections::HashMap::new();
        for _ in 0..200 {
            grouped = crate::storage::items::list_items_for_meeting_grouped(&pool, &meeting_id)
                .await
                .unwrap();
            if grouped.contains_key("highlights") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let hi = grouped
            .get("highlights")
            .expect("highlights row persisted via durable queue");
        assert_eq!(hi.len(), 1);
        assert_eq!(hi[0].id, "h-1");
        assert_eq!(hi[0].text, "key point");
    }

    /// Stop/start straddle race (improvement #18, Task 5): a transcript
    /// event STAMPED for meeting A but CONSUMED by the writer while the
    /// registry's active meeting is already B must land in A's JSONL —
    /// not B's. Before the envelope carried `meeting_id`, the writer
    /// resolved the file via the user's currently-active meeting, so a
    /// line queued under A and dequeued after the A→B cutover would
    /// silently contaminate B's transcript. Stamping at emit time makes
    /// the destination immutable in transit.
    #[tokio::test]
    async fn durable_writer_prefers_envelope_meeting_id_over_registry() {
        let _env_guard = DATA_DIR_LOCK.lock().await;
        let data_dir = temp_path("durable-writer-straddle");
        std::env::set_var("AURIS_DATA_DIR", &data_dir);

        let uid = "u-straddle";
        // Meeting A goes active, then stops; meeting B becomes the
        // registry's active meeting — exactly the state a queued A-line
        // would be dequeued into.
        let (state, meeting_a) = registry_with_active_meeting(uid).await;
        let meeting_b = {
            let mut s = state.lock().await;
            s.apply_intent(uid, Intent::StopMeeting);
            s.apply_intent(
                uid,
                Intent::StartMeeting {
                    description: None,
                    metadata: None,
                    audio_source_device_id: None,
                    assist_sensitivity: None,
                },
            );
            s.active_meeting_id_for(uid).expect("meeting B active")
        };
        assert_ne!(meeting_a, meeting_b, "A and B must be distinct meetings");

        let tx = spawn_durable_writer(
            state.clone(),
            lazy_pool(),
            crate::mnemo::MnemoClient::Disabled,
        );
        // Stamped for A even though B is what the registry reports active.
        tx.send(UserEvent::with_meeting(
            uid,
            meeting_a.clone(),
            Event::ItemsUpdate {
                mode: "transcript".into(),
                items: vec![item("a-tail", "tail of meeting A.", 42)],
            },
        ))
        .await
        .expect("queue accepts while writer alive");
        drop(tx);

        let path_a = transcription_path(&meeting_a).unwrap();
        let mut content = String::new();
        for _ in 0..200 {
            if let Ok(c) = tokio::fs::read_to_string(&path_a).await {
                content = c;
                if content.contains("tail of meeting A.") {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            content.contains("tail of meeting A."),
            "stamped line must land in meeting A's transcript ({})",
            path_a.display()
        );
        let path_b = transcription_path(&meeting_b).unwrap();
        assert!(
            !path_b.exists(),
            "meeting B's transcript must not exist — the A-line must never leak into the registry's active meeting"
        );

        std::env::remove_var("AURIS_DATA_DIR");
        tokio::fs::remove_dir_all(&data_dir).await.ok();
    }
}
