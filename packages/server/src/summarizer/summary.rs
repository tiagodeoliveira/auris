//! Conversation summary mode summarizer.
//!
//! A new mode (`summary`) holding a single Replace-strategy item:
//! a running 3-5 sentence summary of the meeting transcript so far.
//! Refreshes on a hybrid trigger:
//!
//! - **Token threshold** (`SUMMARY_TRIGGER_TOKENS`, default 500):
//!   fire when ~this many new transcript tokens have accumulated
//!   since the last fire. ~3 minutes of speech.
//! - **Hard ceiling** (`SUMMARY_TRIGGER_MAX_MS`, default 300_000):
//!   refresh at least this often as long as the transcript has
//!   grown at all. Keeps the summary fresh during slow stretches.
//!
//! Each fire reads the **full** rolling transcript (not a delta) and
//! re-summarizes from scratch. Cost grows with meeting length, but
//! the simple-and-honest shape beats incremental summarization for
//! coherence — the LLM sees the whole conversation every time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::contract::{Event, Item, UserEvent};
use crate::llm::LlmClient;
use crate::state::ServerState;

const SUMMARY_TRIGGER_TOKENS_DEFAULT: usize = 500;
const SUMMARY_TRIGGER_MAX_MS_DEFAULT: u64 = 300_000;
const CHARS_PER_TOKEN: usize = 4;

const SYSTEM_PROMPT: &str = "You produce a running summary of a live meeting transcript. \
The transcript may contain disfluencies, fillers, and partial sentences from streaming STT.\n\
\n\
Write 3-5 concise sentences covering:\n\
- What was discussed (the main topics).\n\
- Key decisions made.\n\
- Outstanding questions or work yet to be done.\n\
\n\
Speak in the same language as the transcript. Don't translate. Use neutral past tense \
(\"the team discussed…\", \"X agreed to…\"). If the transcript is too short or empty, \
return a single sentence acknowledging that.";

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SummaryExtraction {
    /// 3-5 sentence summary of the meeting so far. Plain prose,
    /// no bullet points or markdown.
    pub summary: String,
}

pub async fn run_summary_summarizer(
    state: Arc<Mutex<ServerState>>,
    llm: Arc<LlmClient>,
    events_tx: broadcast::Sender<UserEvent>,
    user_id: String,
    cancel: CancellationToken,
) {
    let token_threshold = env_usize("SUMMARY_TRIGGER_TOKENS", SUMMARY_TRIGGER_TOKENS_DEFAULT);
    let max_ms = env_u64("SUMMARY_TRIGGER_MAX_MS", SUMMARY_TRIGGER_MAX_MS_DEFAULT);

    info!(
        user_id = %user_id,
        token_threshold,
        max_ms,
        "summary loop started",
    );

    let mut last_fired_chars: usize = 0;
    let mut last_fired_at = Instant::now();
    // 5 s tick is plenty — the threshold check is cheap and we're not
    // racing a real-time signal here, just polling for "enough new
    // content" or "hard ceiling expired."
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.tick().await;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!(user_id = %user_id, "summary loop cancelled");
                return;
            }
            _ = tick.tick() => {
                let transcript = {
                    let s = state.lock().await;
                    s.user(&user_id).map(|u| u.rolling_transcript_text())
                };
                let Some(transcript) = transcript else { continue };
                if transcript.is_empty() {
                    continue;
                }

                let new_chars = transcript.len().saturating_sub(last_fired_chars);
                let new_tokens = new_chars / CHARS_PER_TOKEN;
                let aged_with_growth = last_fired_at.elapsed() >= Duration::from_millis(max_ms)
                    && new_chars > 0;
                let bootstrap = last_fired_chars == 0
                    && transcript.len() >= token_threshold * CHARS_PER_TOKEN;

                if !(bootstrap || new_tokens >= token_threshold || aged_with_growth) {
                    continue;
                }

                let started = Instant::now();
                let user_input = format!("Transcript so far:\n\n{transcript}");
                let result = llm
                    .extract_with_prompt::<SummaryExtraction>(&user_id, SYSTEM_PROMPT, &user_input)
                    .await;
                let latency_ms = started.elapsed().as_millis() as u64;

                match result {
                    Ok(ext) => {
                        last_fired_chars = transcript.len();
                        last_fired_at = Instant::now();
                        let summary = ext.summary.trim().to_string();
                        if summary.is_empty() {
                            warn!(user_id = %user_id, "summary extraction returned empty; skipping update");
                            continue;
                        }
                        let item = Item {
                            id: format!("summary-{}", uuid::Uuid::new_v4()),
                            text: summary.clone(),
                            detail: None,
                            t: 0,
                            meta: None,
                        };
                        let items = {
                            let mut s = state.lock().await;
                            s.user_mut(&user_id)
                                .replace_items_for_mode("summary", vec![item])
                        };
                        if !items.is_empty() {
                            let _ = events_tx.send(UserEvent::new(
                                user_id.clone(),
                                Event::ItemsUpdate {
                                    mode: "summary".into(),
                                    items,
                                },
                            ));
                        }
                        info!(
                            user_id = %user_id,
                            transcript_chars = transcript.len(),
                            summary_chars = summary.len(),
                            latency_ms,
                            "summary fire done",
                        );
                    }
                    Err(e) => {
                        warn!(
                            user_id = %user_id,
                            error = %e,
                            latency_ms,
                            "summary fire failed",
                        );
                    }
                }
            }
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
