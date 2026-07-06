//! `replace_highlights` tool. Replace-strategy: agent emits the new
//! full list each call; the prior list is clobbered. Capped server-side
//! at 10 by `replace_items_for_mode`.

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{Event, Item};

use super::{
    exceeds_hard_ceiling, meeting_is_live, sanitize_item_text, AgentToolError, ToolCtx,
    MAX_HIGHLIGHT_CHARS,
};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ReplaceHighlightItem {
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) importance: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ReplaceHighlightsArgs {
    pub(crate) items: Vec<ReplaceHighlightItem>,
}

/// Importance is a short free-form tag ("high", "key decision") that
/// clients render verbatim as `IMPORTANCE · {value}` — clamp rather
/// than whitelist so the live tool stays consistent with the
/// post-meeting summarize worker, which also emits free-form tags.
pub(crate) const MAX_IMPORTANCE_CHARS: usize = 32;

/// Sanitize raw highlight items: drop items past the hard ceiling or
/// empty after control-strip (counted, so the tool result can give
/// the model corrective feedback without a retry round), clamp text
/// to MAX_HIGHLIGHT_CHARS and importance to MAX_IMPORTANCE_CHARS.
/// Returns `(kept (text, importance) pairs, dropped_count)`. Pure so
/// the tests below hit it without a ToolCtx.
pub(crate) fn sanitize_highlight_items(
    raw: Vec<ReplaceHighlightItem>,
) -> (Vec<(String, Option<String>)>, usize) {
    let mut dropped = 0usize;
    let mut out = Vec::with_capacity(raw.len());
    for h in raw {
        if exceeds_hard_ceiling(&h.text) {
            dropped += 1;
            continue;
        }
        let text = sanitize_item_text(&h.text, MAX_HIGHLIGHT_CHARS, "highlights");
        if text.is_empty() {
            dropped += 1;
            continue;
        }
        let importance = h
            .importance
            .as_deref()
            .map(|i| sanitize_item_text(i, MAX_IMPORTANCE_CHARS, "highlights.importance"))
            .filter(|i| !i.is_empty());
        out.push((text, importance));
    }
    (out, dropped)
}

pub(crate) struct ReplaceHighlights(pub(crate) ToolCtx);

impl Tool for ReplaceHighlights {
    const NAME: &'static str = "replace_highlights";
    type Args = ReplaceHighlightsArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::agent::prompts::TOOL_DESC_REPLACE_HIGHLIGHTS.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "description": "New full highlight list, replacing whatever's there now.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" },
                                "importance": { "type": "string" }
                            },
                            "required": ["text"]
                        }
                    }
                },
                "required": ["items"]
            }),
        }
    }

    async fn call(&self, args: ReplaceHighlightsArgs) -> Result<String, AgentToolError> {
        if !meeting_is_live(&self.0).await {
            return Ok("skipped: meeting no longer active".into());
        }
        let (sanitized, dropped) = sanitize_highlight_items(args.items);
        let n = sanitized.len();
        // Elapsed read + replace under ONE lock acquisition, scoped to
        // this fire's meeting — see assist.rs for the TOCTOU rationale.
        let payload = {
            let mut s = self.0.sessions.lock().await;
            s.with_session_if_active(&self.0.user_id, &self.0.meeting_id, |u| {
                let elapsed_ms = u
                    .meeting_started_at()
                    .map(|start| start.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                let items: Vec<Item> = sanitized
                    .into_iter()
                    .map(|(text, importance)| Item {
                        id: format!("h-{}", uuid::Uuid::new_v4()),
                        text,
                        detail: None,
                        t: elapsed_ms,
                        meta: importance.map(|i| serde_json::json!({"importance": i})),
                    })
                    .collect();
                u.replace_items_for_mode("highlights", items)
            })
        };
        let Some(payload) = payload else {
            return Ok("skipped: meeting no longer active".into());
        };
        self.0
            .bus
            .emit(
                self.0.user_id.clone(),
                Event::ItemsUpdate {
                    mode: "highlights".into(),
                    items: payload,
                },
            )
            .await;
        if dropped > 0 {
            Ok(format!(
                "ok: replaced highlights with {n} items ({dropped} dropped: too long or empty)"
            ))
        } else {
            Ok(format!("ok: replaced highlights with {n} items"))
        }
    }
}

#[cfg(test)]
mod sanitize_tests {
    use super::*;
    use crate::agent::tools::{ITEM_TEXT_HARD_CEILING, MAX_HIGHLIGHT_CHARS};

    fn raw(text: &str, importance: Option<&str>) -> ReplaceHighlightItem {
        ReplaceHighlightItem {
            text: text.into(),
            importance: importance.map(|s| s.into()),
        }
    }

    #[test]
    fn highlight_text_clamped() {
        let long = "h".repeat(MAX_HIGHLIGHT_CHARS + 50);
        let (items, dropped) = sanitize_highlight_items(vec![raw(&long, None)]);
        assert_eq!(dropped, 0);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0.chars().count(), MAX_HIGHLIGHT_CHARS + 1);
        assert!(items[0].0.ends_with('…'));
    }

    #[test]
    fn importance_meta_clamped() {
        let long_importance = "i".repeat(500);
        let (items, _) =
            sanitize_highlight_items(vec![raw("decision: ship it", Some(&long_importance))]);
        let importance = items[0].1.as_ref().expect("importance kept");
        assert_eq!(importance.chars().count(), MAX_IMPORTANCE_CHARS + 1);
        assert!(importance.ends_with('…'));
    }

    #[test]
    fn short_importance_passes_through() {
        let (items, _) = sanitize_highlight_items(vec![raw("kept", Some("high"))]);
        assert_eq!(items[0].1.as_deref(), Some("high"));
    }

    #[test]
    fn whitespace_only_importance_dropped() {
        let (items, _) = sanitize_highlight_items(vec![raw("kept", Some("   "))]);
        assert_eq!(items[0].1, None);
    }

    #[test]
    fn oversized_highlight_dropped_and_counted() {
        let huge = "h".repeat(ITEM_TEXT_HARD_CEILING + 1);
        let (items, dropped) = sanitize_highlight_items(vec![raw(&huge, None), raw("kept", None)]);
        assert_eq!(dropped, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, "kept");
    }

    #[test]
    fn empty_after_sanitize_dropped_and_counted() {
        let (items, dropped) = sanitize_highlight_items(vec![raw("\u{0007} \u{001B} ", None)]);
        assert!(items.is_empty());
        assert_eq!(dropped, 1);
    }

    #[test]
    fn control_chars_stripped_from_text() {
        let (items, _) = sanitize_highlight_items(vec![raw("a\u{0007}b\u{001B}c", None)]);
        assert_eq!(items[0].0, "abc");
    }
}

#[cfg(test)]
mod staleness_tests {
    use super::*;
    use crate::agent::tools::test_support::tool_ctx;
    use crate::protocol::Intent;
    use crate::session::SessionRegistry;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    const UID: &str = "u-test";

    fn args(text: &str) -> ReplaceHighlightsArgs {
        ReplaceHighlightsArgs {
            items: vec![ReplaceHighlightItem {
                text: text.into(),
                importance: None,
            }],
        }
    }

    fn start_meeting(reg: &mut SessionRegistry) -> String {
        reg.apply_intent(
            UID,
            Intent::StartMeeting {
                description: None,
                metadata: None,
                audio_source_device_id: None,
                assist_sensitivity: None,
            },
        );
        reg.active_meeting_id_for(UID).expect("meeting started")
    }

    #[tokio::test]
    async fn replace_highlights_skips_when_fires_meeting_is_no_longer_active() {
        let mut reg = SessionRegistry::new();
        let mid1 = start_meeting(&mut reg);
        reg.apply_intent(UID, Intent::StopMeeting);
        let _mid2 = start_meeting(&mut reg);
        let sessions = Arc::new(Mutex::new(reg));

        let (ctx, mut events_rx) = tool_ctx(Arc::clone(&sessions), UID, &mid1);
        let out = ReplaceHighlights(ctx)
            .call(args("stale highlight"))
            .await
            .unwrap();

        assert_eq!(out, "skipped: meeting no longer active");
        let s = sessions.lock().await;
        assert!(
            !s.user(UID)
                .unwrap()
                .mode_contains_text("highlights", "stale highlight"),
            "meeting-1 highlights must not land in meeting 2"
        );
        drop(s);
        assert!(events_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn replace_highlights_writes_for_the_active_meeting() {
        let mut reg = SessionRegistry::new();
        let mid = start_meeting(&mut reg);
        let sessions = Arc::new(Mutex::new(reg));

        let (ctx, mut events_rx) = tool_ctx(Arc::clone(&sessions), UID, &mid);
        let out = ReplaceHighlights(ctx)
            .call(args("live highlight"))
            .await
            .unwrap();

        assert_eq!(out, "ok: replaced highlights with 1 items");
        let s = sessions.lock().await;
        assert!(s
            .user(UID)
            .unwrap()
            .mode_contains_text("highlights", "live highlight"));
        drop(s);
        match events_rx.try_recv() {
            Ok(ev) => assert!(
                matches!(ev.event, Event::ItemsUpdate { ref mode, .. } if mode == "highlights")
            ),
            Err(e) => panic!("expected ItemsUpdate, got {e:?}"),
        }
    }
}
