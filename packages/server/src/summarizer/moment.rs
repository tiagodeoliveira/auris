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
a decision, a question, an idea, a turning point. Given the meeting's description, the \
transcript surrounding the bookmark, and (when available) a screenshot captured at the \
moment, produce a tight 2–3 sentence summary of what was being discussed at exactly that \
point. Anchor on the marked moment, not the whole meeting. \
\
When a screenshot is provided, weave concrete visual details into the summary — what was \
on screen (a slide, a doc, code, a chart, a Figma frame, etc.) and how it connects to the \
spoken content. Don't describe the screenshot in isolation; integrate it. If the screenshot \
contradicts or refines the transcript, the screenshot wins for *what was visible*. \
\
If a user note is provided, treat it as the bookmarker's intent — the summary should align \
with that intent. Do NOT speculate beyond what the transcript and screenshot support. If \
both are sparse or unrelated, say so honestly in one short sentence.";

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
    let agent_kick_tx = handle.agent_kick_tx.clone();
    let events_tx = handle.events_tx.clone();
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
                    let agent_kick_tx = agent_kick_tx.clone();
                    let events_tx = events_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            process_one(&db, &llm, &agent_kick_tx, &events_tx, &req).await
                        {
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
    db: &sqlx::PgPool,
    llm: &Arc<LlmClient>,
    agent_kick_tx: &tokio::sync::broadcast::Sender<crate::summarizer::agent::AgentKick>,
    events_tx: &tokio::sync::broadcast::Sender<crate::contract::UserEvent>,
    req: &MomentCreated,
) -> anyhow::Result<()> {
    if crate::env::flag("MEETING_COMPANION_LLM_DISABLED") {
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
        sqlx::query_as("SELECT description, metadata FROM meetings WHERE id = $1")
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
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT note, asset_path FROM moments WHERE id = $1")
            .bind(&req.moment_id)
            .fetch_optional(db)
            .await?;
    let (note, asset_path) = row.map(|(n, a)| (n, a)).unwrap_or((None, None));

    let window = read_transcript_window(&req.meeting_id, req.t_ms, window_ms).await;

    let prompt = build_user_prompt(
        description.as_deref(),
        &metadata_json,
        note.as_deref(),
        req.t_ms,
        window_ms,
        &window,
    );

    // Try to read the screenshot. Failures (file not yet uploaded,
    // disk error, missing path) fall through to a text-only call —
    // moments with no screenshot are a documented degraded path
    // (e.g., PWA-only meetings without a screen-capture device).
    let image = match asset_path.as_deref() {
        Some(rel) if !rel.is_empty() => read_screenshot_bytes(rel).await,
        _ => None,
    };

    let extraction: MomentSummaryExtraction = match image {
        Some(bytes) => {
            info!(
                meeting_id = %req.meeting_id,
                moment_id = %req.moment_id,
                bytes = bytes.len(),
                "moment summary: vision call (screenshot attached)"
            );
            llm.extract_with_prompt_and_image::<MomentSummaryExtraction>(
                &req.user_id,
                SYSTEM_PROMPT,
                &prompt,
                bytes,
                rig::completion::message::ImageMediaType::PNG,
            )
            .await
            .map_err(|e| anyhow::anyhow!("LLM extract (vision) failed: {e}"))?
        }
        None => {
            debug!(
                meeting_id = %req.meeting_id,
                moment_id = %req.moment_id,
                "moment summary: text-only call (no screenshot available)"
            );
            llm.extract_with_prompt::<MomentSummaryExtraction>(&req.user_id, SYSTEM_PROMPT, &prompt)
                .await
                .map_err(|e| anyhow::anyhow!("LLM extract failed: {e}"))?
        }
    };

    let summary_trimmed = extraction.summary.trim().to_string();
    crate::db::update_moment_summary(db, &req.moment_id, Some(&summary_trimmed), "done").await?;
    info!(meeting_id = %req.meeting_id, moment_id = %req.moment_id, "moment summary done");

    // Re-read note for the broadcast — the agent kick already had
    // `note` from MomentMarked, but the wire event downstream
    // consumers will see needs the user-attached note included so
    // mnemo can prefix the moment turn with the user's intent.
    let note_for_event: Option<String> =
        sqlx::query_scalar(r#"SELECT note FROM moments WHERE id = $1"#)
            .bind(&req.moment_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

    // Tell the agent the rich summary is now available.
    let _ = agent_kick_tx.send(crate::summarizer::agent::AgentKick {
        user_id: req.user_id.clone(),
        reason: crate::summarizer::agent::AgentKickReason::MomentSummarized {
            moment_id: req.moment_id.clone(),
            t_ms: req.t_ms,
            summary: summary_trimmed.clone(),
        },
    });

    // Wire-broadcast the same payload — mnemo pusher subscribes here
    // and pushes the summary as an assistant-role memory turn.
    // Clients today fall through (unknown event); a future "moment
    // ready" toast or list refresh would slot in here.
    let _ = events_tx.send(crate::contract::UserEvent::new(
        req.user_id.clone(),
        crate::contract::Event::MomentSummarized {
            moment_id: req.moment_id.clone(),
            meeting_id: req.meeting_id.clone(),
            t_ms: req.t_ms,
            summary: summary_trimmed,
            note: note_for_event,
        },
    ));
    Ok(())
}

/// Read a screenshot blob from `<DATA_DIR>/<rel>`. Returns `None` on
/// any error (file not yet uploaded, missing, unreadable) — the
/// caller falls back to a text-only LLM call.
async fn read_screenshot_bytes(rel: &str) -> Option<Vec<u8>> {
    let dir = match crate::db::data_dir() {
        Ok(d) => d,
        Err(e) => {
            warn!(error = ?e, "data_dir lookup failed for screenshot read");
            return None;
        }
    };
    let abs = dir.join(rel);
    match tokio::fs::read(&abs).await {
        Ok(bytes) if !bytes.is_empty() => Some(bytes),
        Ok(_) => {
            warn!(path = %abs.display(), "screenshot file is empty; falling back to text-only");
            None
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Common race: the moment-created broadcast fires
            // before the Mac finishes uploading the screenshot.
            // The 12 s grace upstream covers most of this; if the
            // upload is still in flight we just summarize the
            // transcript and call it a day.
            debug!(path = %abs.display(), "screenshot not yet on disk; text-only summary");
            None
        }
        Err(e) => {
            warn!(error = ?e, path = %abs.display(), "screenshot read failed");
            None
        }
    }
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
