//! `replace_summary` tool — replaces the entire summary mode list.
//!
//! Called by the active extraction agent (`agent/active.rs`) when it
//! decides the running summary needs refreshing. Replace strategy:
//! the agent emits the new full list each call; the prior list is
//! clobbered. Capped server-side at 10 by `replace_items_for_mode`.

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    exceeds_hard_ceiling, meeting_is_live, sanitize_item_text, AgentToolError, ToolCtx,
    MAX_SUMMARY_BULLET_CHARS,
};
use crate::agent::prompts;
use crate::protocol::{Event, Item};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ReplaceSummaryArgs {
    /// Ordered list of summary bullets, oldest first. Each entry
    /// becomes one Item in the summary mode. ≤10 retained server-side.
    pub(crate) bullets: Vec<String>,
}

pub(crate) struct ReplaceSummary(pub(crate) ToolCtx);

impl Tool for ReplaceSummary {
    const NAME: &'static str = "replace_summary";
    type Args = ReplaceSummaryArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: prompts::TOOL_DESC_REPLACE_SUMMARY.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "bullets": {
                        "type": "array",
                        "description": "New full summary list, replacing whatever's there now.",
                        "items": { "type": "string" }
                    }
                },
                "required": ["bullets"]
            }),
        }
    }

    async fn call(&self, args: ReplaceSummaryArgs) -> Result<String, AgentToolError> {
        if !meeting_is_live(&self.0).await {
            return Ok("skipped: meeting no longer active".into());
        }
        let bullets = sanitized_bullets(args.bullets);
        if bullets.is_empty() {
            return Ok("skipped: no usable bullets".into());
        }
        let n = bullets.len();
        // Elapsed read + replace under ONE lock acquisition, scoped to
        // this fire's meeting — see assist.rs for the TOCTOU rationale.
        let payload = {
            let mut s = self.0.sessions.lock().await;
            s.with_session_if_active(&self.0.user_id, &self.0.meeting_id, |u| {
                let elapsed_ms = u
                    .meeting_started_at()
                    .map(|start| start.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                let items: Vec<Item> = bullets
                    .into_iter()
                    .map(|text| Item {
                        id: format!("summary-{}", uuid::Uuid::new_v4()),
                        text,
                        detail: None,
                        t: elapsed_ms,
                        meta: None,
                    })
                    .collect();
                u.replace_items_for_mode("summary", items)
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
                    mode: "summary".into(),
                    items: payload,
                },
            )
            .await;
        Ok(format!("ok: replaced summary with {n} bullets"))
    }
}

/// Sanitize the model's bullet list: drop bullets past the hard
/// ceiling (reject, don't truncate), strip control chars + clamp to
/// MAX_SUMMARY_BULLET_CHARS, drop empties, dedup-by-text. Dedup runs
/// on POST-sanitize text so the dedup key and the stored item text
/// always agree. Subsumes the old inline trim/empty/dedup pipeline
/// (defense the old workers/summary.rs applied). Pure so the tests
/// below hit it without a ToolCtx.
pub(crate) fn sanitized_bullets(raw: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.into_iter()
        .filter(|b| !exceeds_hard_ceiling(b))
        .map(|b| sanitize_item_text(&b, MAX_SUMMARY_BULLET_CHARS, "summary"))
        .filter(|b| !b.is_empty())
        .filter(|b| seen.insert(b.clone()))
        .collect()
}

#[cfg(test)]
mod sanitize_tests {
    use super::*;
    use crate::agent::tools::{ITEM_TEXT_HARD_CEILING, MAX_SUMMARY_BULLET_CHARS};

    #[test]
    fn bullets_clamped_to_max_chars() {
        let long = "w".repeat(MAX_SUMMARY_BULLET_CHARS + 100);
        let out = sanitized_bullets(vec![long]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chars().count(), MAX_SUMMARY_BULLET_CHARS + 1);
        assert!(out[0].ends_with('…'));
    }

    #[test]
    fn oversized_bullet_dropped_entirely() {
        let huge = "w".repeat(ITEM_TEXT_HARD_CEILING + 1);
        let out = sanitized_bullets(vec![huge, "keep me".into()]);
        assert_eq!(out, vec!["keep me".to_string()]);
    }

    #[test]
    fn dedup_runs_on_post_sanitize_text() {
        let a = format!("{}{}", "x".repeat(MAX_SUMMARY_BULLET_CHARS), "tail-a");
        let b = format!("{}{}", "x".repeat(MAX_SUMMARY_BULLET_CHARS), "tail-b");
        let out = sanitized_bullets(vec![a, b]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn control_chars_stripped_from_bullets() {
        let out = sanitized_bullets(vec!["dec\u{0007}ision: \u{001B}ship it".into()]);
        assert_eq!(out, vec!["decision: ship it".to_string()]);
    }

    #[test]
    fn keeps_existing_trim_empty_and_dedup_behavior() {
        let out = sanitized_bullets(vec![
            "  alpha  ".into(),
            "".into(),
            "   ".into(),
            "alpha".into(),
            "beta".into(),
        ]);
        assert_eq!(out, vec!["alpha".to_string(), "beta".to_string()]);
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

    fn args(bullet: &str) -> ReplaceSummaryArgs {
        ReplaceSummaryArgs {
            bullets: vec![bullet.into()],
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
    async fn replace_summary_skips_when_fires_meeting_is_no_longer_active() {
        let mut reg = SessionRegistry::new();
        let mid1 = start_meeting(&mut reg);
        reg.apply_intent(UID, Intent::StopMeeting);
        let _mid2 = start_meeting(&mut reg);
        let sessions = Arc::new(Mutex::new(reg));

        let (ctx, mut events_rx) = tool_ctx(Arc::clone(&sessions), UID, &mid1);
        let out = ReplaceSummary(ctx)
            .call(args("stale bullet"))
            .await
            .unwrap();

        assert_eq!(out, "skipped: meeting no longer active");
        let s = sessions.lock().await;
        assert!(
            !s.user(UID)
                .unwrap()
                .mode_contains_text("summary", "stale bullet"),
            "meeting-1 summary must not land in meeting 2"
        );
        drop(s);
        assert!(events_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn replace_summary_writes_for_the_active_meeting() {
        let mut reg = SessionRegistry::new();
        let mid = start_meeting(&mut reg);
        let sessions = Arc::new(Mutex::new(reg));

        let (ctx, mut events_rx) = tool_ctx(Arc::clone(&sessions), UID, &mid);
        let out = ReplaceSummary(ctx).call(args("live bullet")).await.unwrap();

        assert_eq!(out, "ok: replaced summary with 1 bullets");
        let s = sessions.lock().await;
        assert!(s
            .user(UID)
            .unwrap()
            .mode_contains_text("summary", "live bullet"));
        drop(s);
        match events_rx.try_recv() {
            Ok(ev) => assert!(
                matches!(ev.event, Event::ItemsUpdate { ref mode, .. } if mode == "summary")
            ),
            Err(e) => panic!("expected ItemsUpdate, got {e:?}"),
        }
    }
}
