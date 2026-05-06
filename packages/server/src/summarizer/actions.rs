//! Actions summarizer — rig Extractor on a 15s heartbeat.
//! Detects action items from the rolling transcript; appends new ones to
//! the actions-mode buffer with server-side dedupe by exact text equality.

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

pub const SYSTEM_PROMPT: &str = "You are a meeting action-item detector. \
Given the rolling transcript and the action items already detected, return only NEW \
action items. Each must be an imperative-mood statement. Do not repeat existing items \
even if rephrased. Use empty string for owner/due if not stated explicitly. Each ≤ 120 chars. \
If a 'Prior context' section is provided, treat it as background from past meetings: \
do NOT re-extract actions that were already completed or recorded there, but use it to \
sharpen owner/due inference for genuinely new items.";

pub const HEARTBEAT_DEFAULT_MS: u64 = 15000;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ActionsExtraction {
    /// Action items detected since the start of the meeting. Empty if none.
    pub actions: Vec<ActionItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ActionItem {
    /// Imperative-mood action statement. ≤ 120 chars.
    pub action: String,
    /// Best guess at the owner if the transcript names one. Empty string if unclear.
    pub owner: String,
    /// Best guess at a due date if mentioned (e.g. "Friday", "next sprint"). Empty if unclear.
    pub due: String,
}

pub async fn run_actions_summarizer(
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
                    debug!("LLM disabled; skipping actions cycle");
                    continue;
                }
                let (transcript, existing_actions, prior_context) = {
                    let s = state.lock().await;
                    let user = s.user(&user_id);
                    let existing: Vec<String> = user
                        .and_then(|u| u.items_per_mode.get("actions"))
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
                    "{}Existing action items (do not repeat):\n{}\n\nTranscript:\n{}",
                    prior_context,
                    existing_actions.join("\n"),
                    transcript,
                );
                match llm
                    .extract_with_prompt::<ActionsExtraction>(SYSTEM_PROMPT, &user_input)
                    .await
                {
                    Ok(extracted) => {
                        let mut payload = Vec::new();
                        let mut s = state.lock().await;
                        let u = s.user_mut(&user_id);
                        for action in extracted.actions {
                            // Server-side dedupe: skip if action text already in buffer
                            if existing_actions.contains(&action.action) {
                                continue;
                            }
                            let item = Item {
                                id: format!("a-{}", uuid::Uuid::new_v4()),
                                text: action.action,
                                detail: None,
                                t: 0,
                                meta: Some(serde_json::json!({
                                    "owner": action.owner,
                                    "due": action.due,
                                })),
                            };
                            payload.extend(u.push_item_for_mode("actions", item));
                        }
                        drop(s);
                        if !payload.is_empty() {
                            let _ = events_tx.send(UserEvent::new(
                                user_id.clone(),
                                Event::ItemsUpdate {
                                    mode: "actions".into(),
                                    items: payload,
                                },
                            ));
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "actions extraction failed; skipping cycle");
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
    fn actions_extraction_round_trip() {
        let a = ActionsExtraction {
            actions: vec![ActionItem {
                action: "Write up the migration plan".into(),
                owner: "Tiago".into(),
                due: "Tuesday".into(),
            }],
        };
        let json = serde_json::to_string(&a).unwrap();
        let round: ActionsExtraction = serde_json::from_str(&json).unwrap();
        assert_eq!(round.actions.len(), 1);
        assert_eq!(round.actions[0].owner, "Tiago");
        assert_eq!(round.actions[0].due, "Tuesday");
    }

    #[test]
    fn heartbeat_default_is_15s() {
        assert_eq!(HEARTBEAT_DEFAULT_MS, 15000);
    }

    #[test]
    fn system_prompt_mentions_actions() {
        assert!(SYSTEM_PROMPT.to_lowercase().contains("action"));
    }
}
