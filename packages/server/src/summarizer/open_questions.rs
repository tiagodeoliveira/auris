//! Open-questions summarizer — rig Extractor on a 15s heartbeat.
//!
//! Surfaces two flavors of questions worth asking:
//!   1. Pending questions someone asked that don't have a clear answer in
//!      the transcript so far.
//!   2. Clarification opportunities where understanding may be incomplete
//!      — ambiguous phrasing, unsupported assertions, or topics the user
//!      could productively follow up on. Useful for catching things missed
//!      while multitasking during a meeting.
//!
//! Append strategy with server-side dedupe by exact question text. Same
//! shape as the actions summarizer; treat that as the template.

use crate::contract::{Event, Item, UserEvent};
use crate::llm::LlmClient;
use crate::state::ServerState;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

pub const SYSTEM_PROMPT: &str = "You are a meeting open-questions detector. \
Given the rolling transcript and the open questions already detected, return only NEW \
questions you haven't seen yet. Two flavors:\n\
  - kind=\"pending\": a question someone explicitly asked that doesn't have a clear \
    answer in the transcript so far.\n\
  - kind=\"clarification\": a moment where understanding may be incomplete — something \
    ambiguous, an unsupported assertion, or a topic the user could productively follow \
    up on. Useful for catching things missed while multitasking.\n\
Each question must be phrased as a question and end with '?'. Each ≤ 120 characters. \
Use empty string for context if not needed; otherwise ≤ 80 characters of brief context \
(e.g., 'speaker mentioned but didn't explain'). Do not repeat existing questions even \
if rephrased. \
If a 'Prior context' section is provided, treat it as background from past meetings: \
do NOT re-raise questions that were already answered there, and prefer questions that \
explicitly conflict with or refine prior facts.";

pub const HEARTBEAT_DEFAULT_MS: u64 = 15000;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct OpenQuestionsExtraction {
    /// Open questions detected in the meeting transcript so far. Empty if none.
    pub questions: Vec<OpenQuestion>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct OpenQuestion {
    /// The question itself, phrased as a question and ending with '?'. ≤ 120 chars.
    pub question: String,
    /// Either "pending" (asked but unanswered) or "clarification" (user
    /// could productively ask this to fill a gap).
    pub kind: String,
    /// Brief context for why this question matters. Empty if not needed; ≤ 80 chars.
    pub context: String,
}

pub async fn run_open_questions_summarizer(
    state: Arc<Mutex<ServerState>>,
    llm: Arc<LlmClient>,
    events_tx: broadcast::Sender<UserEvent>,
    user_id: String,
    cancel: CancellationToken,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = ticker.tick() => {
                if std::env::var("MEETING_COMPANION_LLM_DISABLED").is_ok() {
                    debug!("LLM disabled; skipping open_questions cycle");
                    continue;
                }
                let (transcript, existing_questions, prior_context) = {
                    let s = state.lock().await;
                    let user = s.user(&user_id);
                    let existing: Vec<String> = user
                        .and_then(|u| u.items_per_mode.get("open_questions"))
                        .map(|v| v.iter().map(|i| i.text.clone()).collect())
                        .unwrap_or_default();
                    let prior = user
                        .and_then(|u| u.recalled_context_clone())
                        .map(|c| c.format_for_prompt())
                        .unwrap_or_default();
                    let transcript = s.rolling_transcript_text_for(&user_id).unwrap_or_default();
                    (transcript, existing, prior)
                };
                if transcript.is_empty() {
                    continue;
                }
                let user_input = format!(
                    "{}Existing open questions (do not repeat):\n{}\n\nTranscript:\n{}",
                    prior_context,
                    existing_questions.join("\n"),
                    transcript,
                );
                match llm
                    .extract_with_prompt::<OpenQuestionsExtraction>(SYSTEM_PROMPT, &user_input)
                    .await
                {
                    Ok(extracted) => {
                        let mut payload = Vec::new();
                        let mut s = state.lock().await;
                        let u = s.user_mut(&user_id);
                        for q in extracted.questions {
                            // Server-side dedupe by exact question text
                            if existing_questions.contains(&q.question) {
                                continue;
                            }
                            let item = Item {
                                id: format!("oq-{}", uuid::Uuid::new_v4()),
                                text: q.question,
                                detail: None,
                                t: 0,
                                meta: Some(serde_json::json!({
                                    "kind": q.kind,
                                    "context": q.context,
                                })),
                            };
                            payload.extend(u.push_item_for_mode("open_questions", item));
                        }
                        drop(s);
                        if !payload.is_empty() {
                            let _ = events_tx.send(UserEvent::new(
                                user_id.clone(),
                                Event::ItemsUpdate {
                                    mode: "open_questions".into(),
                                    items: payload,
                                },
                            ));
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "open_questions extraction failed; skipping cycle");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_questions_extraction_round_trip() {
        let q = OpenQuestionsExtraction {
            questions: vec![OpenQuestion {
                question: "What's the budget for the migration?".into(),
                kind: "pending".into(),
                context: "Tiago asked but no answer given".into(),
            }],
        };
        let json = serde_json::to_string(&q).unwrap();
        let round: OpenQuestionsExtraction = serde_json::from_str(&json).unwrap();
        assert_eq!(round.questions.len(), 1);
        assert_eq!(round.questions[0].kind, "pending");
        assert!(round.questions[0].question.ends_with('?'));
    }

    #[test]
    fn heartbeat_default_is_15s() {
        assert_eq!(HEARTBEAT_DEFAULT_MS, 15000);
    }

    #[test]
    fn system_prompt_describes_both_flavors() {
        let p = SYSTEM_PROMPT.to_lowercase();
        assert!(p.contains("pending"));
        assert!(p.contains("clarification"));
    }
}
