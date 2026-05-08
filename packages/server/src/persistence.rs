//! Per-meeting persistence: transcripts go to JSONL on disk; every
//! other mode (highlights / actions / open_questions / summary /
//! chat) goes to the `items` table in Postgres.
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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use sqlx::PgPool;
use tokio::fs::{create_dir_all, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

use crate::contract::{Event, Item, UpdateStrategy, UserEvent};
use crate::state::ServerState;

/// Spawn the transcript-persistence task. Subscribes to `events_tx`
/// and writes one JSONL line per committed transcript item to the
/// active meeting's `transcription.jsonl`. The task lives for the
/// server lifetime; lagged broadcasts log a warning and continue.
pub fn spawn_task(state: Arc<Mutex<ServerState>>, events_tx: &broadcast::Sender<UserEvent>) {
    let mut rx = events_tx.subscribe();
    tokio::spawn(async move {
        info!("transcript persistence task started");
        loop {
            match rx.recv().await {
                Ok(envelope) => {
                    if let Event::ItemsUpdate { mode, items } = &envelope.event {
                        if mode == "transcript" && !items.is_empty() {
                            if let Err(e) =
                                persist_transcript_items(&state, &envelope.user_id, items).await
                            {
                                warn!(error = ?e, "transcript persistence failed");
                            }
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(lagged = n, "persistence task lagged");
                }
            }
        }
    });
}

/// Spawn the items-persistence task. Subscribes to `events_tx` and
/// writes every non-transcript `ItemsUpdate` to Postgres. Replace-
/// strategy modes overwrite their slice atomically per fire; append
/// modes just insert. Lives for the server lifetime.
pub fn spawn_items_task(
    state: Arc<Mutex<ServerState>>,
    db: PgPool,
    events_tx: &broadcast::Sender<UserEvent>,
) {
    let mut rx = events_tx.subscribe();
    tokio::spawn(async move {
        info!("items persistence task started");
        loop {
            match rx.recv().await {
                Ok(envelope) => match &envelope.event {
                    Event::ItemsUpdate { mode, items } => {
                        if mode == "transcript" {
                            // Transcript persists to JSONL via spawn_task above.
                            // Skipping here keeps the items table free of the
                            // highest-volume mode and avoids double-writes.
                            continue;
                        }
                        if let Err(e) =
                            persist_items_update(&state, &db, &envelope.user_id, mode, items).await
                        {
                            warn!(error = ?e, %mode, "items persistence failed");
                        }
                    }
                    Event::ItemUpdated { mode, item } => {
                        // One-row in-place update — used by the
                        // expand_item flow to write the agent's
                        // expansion into the row's `detail` column.
                        // Skip transcript mode for the same reason
                        // ItemsUpdate does (transcripts live in JSONL).
                        if mode == "transcript" {
                            continue;
                        }
                        if let Err(e) =
                            persist_item_updated(&state, &db, &envelope.user_id, mode, item).await
                        {
                            warn!(error = ?e, %mode, "item update persistence failed");
                        }
                    }
                    _ => {}
                },
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(lagged = n, "items persistence task lagged");
                }
            }
        }
    });
}

/// Resolve `(user_id → meeting_id, mode → strategy)` and dispatch
/// to the right write path. Skips when the user has no active
/// meeting (race with stop_meeting) and when the mode isn't
/// declared in `available_modes` (defensive — shouldn't happen).
/// Persist a single-item in-place update. Looks up the user's
/// active meeting (skips on race with stop) and updates the row's
/// `detail` column. The full Item is passed for completeness even
/// though we only update one column today — opens the door to
/// future per-item edits without changing the persistence shape.
async fn persist_item_updated(
    state: &Arc<Mutex<ServerState>>,
    db: &PgPool,
    user_id: &str,
    mode: &str,
    item: &Item,
) -> Result<()> {
    let meeting_id = {
        let s = state.lock().await;
        match s.user(user_id).and_then(|u| u.current_meeting_id.clone()) {
            Some(m) => m,
            None => return Ok(()),
        }
    };
    crate::db::update_item_detail(db, &meeting_id, mode, &item.id, item.detail.as_deref()).await
}

async fn persist_items_update(
    state: &Arc<Mutex<ServerState>>,
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
        let meeting_id = match user.current_meeting_id.clone() {
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
            crate::db::replace_items_for_meeting_mode(db, &meeting_id, mode, items).await?;
        }
        UpdateStrategy::Append => {
            for item in items {
                crate::db::insert_item_row(db, &meeting_id, mode, item).await?;
            }
        }
    }
    Ok(())
}

/// Look up the active meeting for `user_id` and append `items` to
/// its transcription file. No-op if that user has no active meeting
/// — race with meeting-end teardown is the most common reason.
async fn persist_transcript_items(
    state: &Arc<Mutex<ServerState>>,
    user_id: &str,
    items: &[Item],
) -> Result<()> {
    let meeting_id = {
        let s = state.lock().await;
        s.user(user_id).and_then(|u| u.current_meeting_id.clone())
    };
    let Some(meeting_id) = meeting_id else {
        return Ok(());
    };
    let path = transcription_path(&meeting_id)?;
    append_jsonl(&path, items).await
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
    let dir = crate::db::data_dir()?;
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
    use crate::contract::Item;

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
}
