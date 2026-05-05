//! Blob-side persistence: append committed transcript items to a
//! per-meeting JSONL file.
//!
//! Layout: `<DATA_DIR>/blobs/meetings/<meeting_id>/transcription.jsonl`,
//! one JSON-encoded `Item` per line. The transcript summarizer
//! pushes each finalized utterance as an `Item` into transcript
//! mode and emits an `ItemsUpdate { mode: "transcript", items }`
//! event; this task subscribes to that broadcast and appends.
//!
//! Why not in SQLite: transcripts are sequential append-only data
//! that we read whole, not query. They're also the only ground
//! truth in a meeting (highlights/actions/open_questions are
//! derived from them and can be re-run if lost). Keeping them as
//! flat files matches the future S3-key layout (one prefix per
//! meeting) and avoids bloating the relational DB with text blobs.
//!
//! Other modes are intentionally not persisted today. If we ever
//! want a "review meeting" feature that includes derived items,
//! we can replay the saved transcript through the summarizers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::fs::{create_dir_all, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

use crate::contract::{Event, Item};
use crate::state::ServerState;

/// Spawn the transcript-persistence task. Subscribes to `events_tx`
/// and writes one JSONL line per committed transcript item to the
/// active meeting's `transcription.jsonl`. The task lives for the
/// server lifetime; lagged broadcasts log a warning and continue.
pub fn spawn_task(state: Arc<Mutex<ServerState>>, events_tx: &broadcast::Sender<Event>) {
    let mut rx = events_tx.subscribe();
    tokio::spawn(async move {
        info!("transcript persistence task started");
        loop {
            match rx.recv().await {
                Ok(Event::ItemsUpdate { mode, items }) if mode == "transcript" => {
                    if items.is_empty() {
                        continue;
                    }
                    if let Err(e) = persist_transcript_items(&state, &items).await {
                        warn!(error = ?e, "transcript persistence failed");
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(lagged = n, "persistence task lagged");
                }
            }
        }
    });
}

/// Look up the active meeting and append `items` to its
/// transcription file. No-op if no meeting is active (defensive —
/// the transcript summarizer shouldn't be emitting in that case,
/// but the broadcast can race a meeting-end teardown).
async fn persist_transcript_items(state: &Arc<Mutex<ServerState>>, items: &[Item]) -> Result<()> {
    let meeting_id = {
        let s = state.lock().await;
        s.current_meeting_id.clone()
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
