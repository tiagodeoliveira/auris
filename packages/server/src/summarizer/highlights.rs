//! Highlights summarizer — rig Extractor on a 20s heartbeat.
//! Produces 3-5 key points from the rolling transcript; replaces the
//! highlights-mode buffer each cycle.

use crate::contract::{Event, Item, UserEvent};
use crate::llm::{ExtractionError, LlmClient};
use crate::state::ServerState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub const SYSTEM_PROMPT: &str = "You are a meeting highlights extractor. \
Given the rolling transcript of a meeting in progress, return the 3-5 most important \
points so far. Order by importance, most decisive first. Use the speaker's wording where \
possible. Each point ≤ 120 characters.";

pub const HEARTBEAT_DEFAULT_MS: u64 = 20000;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HighlightsExtraction {
    /// 3-5 key points from the meeting transcript so far. Most important first.
    pub items: Vec<Highlight>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Highlight {
    /// Concise statement of the key point. ≤ 120 chars.
    pub text: String,
    /// 1 = nice-to-know, 2 = important, 3 = decisive.
    pub importance: u8,
}

pub async fn run_highlights_summarizer(
    state: Arc<Mutex<ServerState>>,
    llm: Arc<LlmClient>,
    events_tx: broadcast::Sender<UserEvent>,
    user_id: String,
    cancel: CancellationToken,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_seen_len: usize = 0;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() {
                    debug!("LLM disabled; skipping highlights cycle");
                    continue;
                }
                // Highlights intentionally do not consume mnemo prior
                // context. Per-user: read this user's rolling transcript
                // only — never anyone else's.
                let transcript = {
                    let s = state.lock().await;
                    s.rolling_transcript_text_for(&user_id).unwrap_or_default()
                };
                if transcript.is_empty() || transcript.len() == last_seen_len {
                    continue;
                }
                last_seen_len = transcript.len();
                match llm
                    .extract_with_prompt::<HighlightsExtraction>(&user_id, SYSTEM_PROMPT, &transcript)
                    .await
                {
                    Ok(extracted) => {
                        let items: Vec<Item> = extracted
                            .items
                            .into_iter()
                            .enumerate()
                            .map(|(i, h)| Item {
                                id: format!("h-{}", i),
                                text: h.text,
                                detail: None,
                                t: 0,
                                meta: Some(serde_json::json!({ "importance": h.importance })),
                            })
                            .collect();
                        let payload = {
                            let mut s = state.lock().await;
                            s.user_mut(&user_id).replace_items_for_mode("highlights", items)
                        };
                        let _ = events_tx.send(UserEvent::new(
                            user_id.clone(),
                            Event::ItemsUpdate {
                                mode: "highlights".into(),
                                items: payload,
                            },
                        ));
                    }
                    Err(e) => {
                        warn!(error = %e, "highlights extraction failed; skipping cycle");
                    }
                }
            }
        }
    }
}

// Re-export helpers used in tests; silences unused warnings for now.
#[allow(dead_code)]
fn _ensure_error_type(_: ExtractionError) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_extraction_round_trip() {
        let h = HighlightsExtraction {
            items: vec![
                Highlight {
                    text: "Q1 budget exceeded".into(),
                    importance: 3,
                },
                Highlight {
                    text: "Mobile team launches Friday".into(),
                    importance: 2,
                },
            ],
        };
        let json = serde_json::to_string(&h).unwrap();
        let round: HighlightsExtraction = serde_json::from_str(&json).unwrap();
        assert_eq!(round.items.len(), 2);
        assert_eq!(round.items[0].importance, 3);
    }

    #[test]
    fn heartbeat_default_is_20s() {
        assert_eq!(HEARTBEAT_DEFAULT_MS, 20000);
    }

    #[test]
    fn system_prompt_mentions_highlights() {
        assert!(SYSTEM_PROMPT.to_lowercase().contains("highlight"));
    }
}
