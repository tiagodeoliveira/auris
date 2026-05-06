//! Async moment-summary worker.
//!
//! Spawned at server boot. Subscribes to
//! `ServerHandle.moment_created_tx`; for each freshly-captured
//! moment it reads the persisted transcript JSONL, extracts the
//! ±N-second window around the moment's `t`, prompts the LLM, and
//! writes the resulting summary into the moments row (flipping
//! `summary_status` to `done` or `failed`).
//!
//! Future-proof: dispatches on `moment.kind`. Today only "manual"
//! is wired; an "interview" mode could land alongside without
//! disturbing the worker's lifecycle.

use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::api::MomentCreated;
use crate::contract::Item;
use crate::llm::LlmClient;
use crate::ws::ServerHandle;

/// Default ±N around the moment's `t` for transcript context.
/// Override with `MEETING_COMPANION_MOMENT_WINDOW_MS`.
const DEFAULT_WINDOW_MS: i64 = 15_000;

/// Default grace period before reading the transcript, to give
/// in-flight Soniox chunks time to finalize. STT providers commit
/// chunks only on punctuation or silence — the chunk *spanning*
/// the moment can land several seconds after the user pressed
/// "Mark moment". Without this delay, the worker reads a JSONL
/// that's missing the most relevant utterance and the LLM is
/// forced to say "no transcript".
/// Override with `MEETING_COMPANION_MOMENT_GRACE_MS`.
const DEFAULT_GRACE_MS: u64 = 12_000;

const SYSTEM_PROMPT: &str = "You summarize a single moment a user explicitly bookmarked \
during a live meeting. The user marks moments because something noteworthy was happening: \
a decision, a question, an idea, a turning point. Given the meeting's description and \
the transcript surrounding the bookmark, produce a tight 2–3 sentence summary of what \
was being discussed at exactly that point. Anchor on the marked moment, not the whole \
meeting. If a user note is provided, treat it as the bookmarker's intent — the summary \
should align with that intent. Do NOT speculate beyond what the transcript supports. \
If the transcript window is sparse or unrelated, say so honestly in one short sentence.";

/// Wire shape the LLM produces. Minimal — we only need the summary
/// today. Adding more fields later is a non-breaking schema change.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MomentSummaryExtraction {
    /// 2–3 sentence summary anchored on the marked moment.
    pub summary: String,
}

/// Spawn the worker. One subscriber to `moment_created_tx`; lives
/// for the server lifetime. Returns immediately.
pub fn spawn_worker(handle: ServerHandle) {
    let mut rx = handle.moment_created_tx.subscribe();
    let llm = handle.llm.clone();
    let db = handle.db.clone();
    tokio::spawn(async move {
        info!("moment summary worker started");
        loop {
            match rx.recv().await {
                Ok(req) => {
                    // Each moment runs in its own task so the
                    // finalization-grace sleep on one doesn't delay
                    // the next. Moments are infrequent enough that
                    // unbounded fan-out isn't a concern.
                    let db = db.clone();
                    let llm = llm.clone();
                    tokio::spawn(async move {
                        if let Err(e) = process_one(&db, &llm, &req).await {
                            warn!(error = ?e, moment_id = %req.moment_id, "moment summary failed");
                            let _ = crate::db::update_moment_summary(
                                &db,
                                &req.moment_id,
                                None,
                                "failed",
                            )
                            .await;
                        }
                    });
                }
                Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(lagged = n, "moment summary worker lagged");
                }
            }
        }
    });
}

async fn process_one(
    db: &sqlx::SqlitePool,
    llm: &Arc<LlmClient>,
    req: &MomentCreated,
) -> anyhow::Result<()> {
    if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() {
        debug!(moment_id = %req.moment_id, "LLM disabled; skipping moment summary");
        return Ok(());
    }

    // Gate dispatch on `kind`. Today only "manual" runs; future
    // modes (interview) plug in here without touching the worker
    // lifecycle.
    if req.kind != "manual" {
        debug!(kind = %req.kind, "unhandled moment kind; leaving summary pending");
        return Ok(());
    }

    let window_ms = std::env::var("MEETING_COMPANION_MOMENT_WINDOW_MS")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(DEFAULT_WINDOW_MS);

    // Wait for in-flight STT chunks to finalize. Soniox commits a
    // chunk only on punctuation or silence; a moment marked mid-
    // utterance otherwise reads an empty (or stale) transcript window.
    let grace_ms = std::env::var("MEETING_COMPANION_MOMENT_GRACE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_GRACE_MS);
    if grace_ms > 0 {
        tokio::time::sleep(Duration::from_millis(grace_ms)).await;
    }

    // Fetch meeting metadata + the user note for prompt context.
    let meeting: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT description, metadata FROM meetings WHERE id = ?1")
            .bind(&req.meeting_id)
            .fetch_optional(db)
            .await?;
    let (description, metadata_json) = match meeting {
        Some(m) => m,
        None => {
            warn!(meeting_id = %req.meeting_id, "moment refers to missing meeting");
            return Ok(());
        }
    };
    let note: Option<String> = sqlx::query_scalar("SELECT note FROM moments WHERE id = ?1")
        .bind(&req.moment_id)
        .fetch_optional(db)
        .await?
        .flatten();

    let window = read_transcript_window(&req.meeting_id, req.t_ms, window_ms).await;

    let prompt = build_user_prompt(
        description.as_deref(),
        &metadata_json,
        note.as_deref(),
        req.t_ms,
        window_ms,
        &window,
    );

    let extraction: MomentSummaryExtraction = tokio::time::timeout(
        Duration::from_secs(60),
        llm.extract_with_prompt::<MomentSummaryExtraction>(SYSTEM_PROMPT, &prompt),
    )
    .await
    .map_err(|_| anyhow::anyhow!("LLM timed out"))?
    .map_err(|e| anyhow::anyhow!("LLM extract failed: {e}"))?;

    crate::db::update_moment_summary(db, &req.moment_id, Some(extraction.summary.trim()), "done")
        .await?;
    info!(meeting_id = %req.meeting_id, moment_id = %req.moment_id, "moment summary done");
    Ok(())
}

/// Read the transcript JSONL and return only items whose `t` falls
/// within [moment_t - window, moment_t + window]. Best-effort — a
/// missing or unreadable file just yields an empty slice and the
/// LLM gets thin context, which is honest.
async fn read_transcript_window(meeting_id: &str, moment_t_ms: i64, window_ms: i64) -> Vec<Item> {
    let all = match crate::persistence::read_transcription(meeting_id).await {
        Ok(items) => items,
        Err(e) => {
            warn!(error = ?e, meeting_id = %meeting_id, "read_transcription failed for moment");
            return Vec::new();
        }
    };
    let lo = (moment_t_ms - window_ms).max(0) as u64;
    let hi = (moment_t_ms + window_ms).max(0) as u64;
    all.into_iter()
        .filter(|item| item.t >= lo && item.t <= hi)
        .collect()
}

fn build_user_prompt(
    description: Option<&str>,
    metadata_json: &str,
    note: Option<&str>,
    moment_t_ms: i64,
    window_ms: i64,
    window: &[Item],
) -> String {
    let mut buf = String::new();
    if let Some(d) = description {
        if !d.trim().is_empty() {
            buf.push_str("Meeting description:\n");
            buf.push_str(d.trim());
            buf.push_str("\n\n");
        }
    }
    if metadata_json.trim() != "{}" && !metadata_json.trim().is_empty() {
        buf.push_str("Meeting metadata (JSON):\n");
        buf.push_str(metadata_json);
        buf.push_str("\n\n");
    }
    if let Some(n) = note {
        if !n.trim().is_empty() {
            buf.push_str("User's note on the moment:\n");
            buf.push_str(n.trim());
            buf.push_str("\n\n");
        }
    }
    buf.push_str(&format!(
        "Moment t: {} ms (±{} ms window).\n",
        moment_t_ms, window_ms
    ));
    buf.push_str("Transcript in window (each line: 'tMs: text'):\n");
    if window.is_empty() {
        buf.push_str("(no transcript items captured in this window)\n");
    } else {
        for item in window {
            buf.push_str(&format!("{}: {}\n", item.t, item.text));
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(t: u64, text: &str) -> Item {
        Item {
            id: format!("i-{t}"),
            text: text.into(),
            detail: None,
            t,
            meta: None,
        }
    }

    #[test]
    fn build_prompt_includes_metadata_and_note() {
        let p = build_user_prompt(
            Some("Daily standup"),
            r#"{"team":"product"}"#,
            Some("decision point"),
            10_000,
            5_000,
            &[item(8_500, "we should ship"), item(11_000, "I agree")],
        );
        assert!(p.contains("Daily standup"));
        assert!(p.contains("team"));
        assert!(p.contains("decision point"));
        assert!(p.contains("we should ship"));
        assert!(p.contains("I agree"));
        assert!(p.contains("10000"));
    }

    #[test]
    fn build_prompt_handles_empty_window() {
        let p = build_user_prompt(None, "{}", None, 0, 1000, &[]);
        assert!(p.contains("no transcript items"));
    }
}
