//! `push_assist_suggestion` tool — proactive contextual hints for the wearer.

use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{AssistSensitivity, Event, Item};

use super::{
    current_assist_sensitivity, exceeds_hard_ceiling, meeting_is_live, sanitize_item_text,
    AgentToolError, ToolCtx, MAX_ASSIST_DETAIL_CHARS, MAX_ASSIST_HEADLINE_CHARS,
};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct PushAssistSuggestionArgs {
    /// One of: "definition", "question", "memory", "coach".
    /// See the system prompt for what each type means.
    pub(crate) r#type: String,
    /// Short, glanceable headline (≤80 chars). Shown as the primary
    /// text on the assist tab + (phase 2) on the glasses HUD. For a
    /// `definition` this is the bare term; for the other types a
    /// specific answer/connection/fact — never a meeting summary.
    pub(crate) headline: String,
    /// Optional longer explanation. Renders as expanded detail when
    /// the user taps the item. Keep ≤300 chars. For a `definition`,
    /// a standalone glossary-style explanation of the term.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    /// 0–100. The agent's own confidence that this suggestion will
    /// be useful RIGHT NOW. Gated server-side (below threshold → drop).
    pub(crate) confidence: u8,
}

pub(crate) struct PushAssistSuggestion(pub(crate) ToolCtx);

/// Minimum confidence to surface a suggestion of a given type and
/// the wearer's chosen sensitivity. The `coach` bar is universally
/// higher than the others because "guess what the wearer wants to
/// say" is the riskiest type — wrong coach suggestions are
/// distracting where wrong definitions are just unnecessary. The
/// sensitivity axis layers on top:
///
///   - `Aggressive` lowers both floors so the surface fires often.
///   - `Moderate` matches the historical pre-feature behavior
///     (coach ≥ 85, others ≥ 70).
///   - `Minimal` raises both floors so only unmistakable signals
///     land.
///
/// The agent's self-rating doesn't shift on its own; the system
/// prompt nudge in `agent::bootstrap` is what asks the model to
/// be more or less generous with its confidence numbers. The two
/// levers compound — Aggressive prompt + Aggressive floor yields
/// substantially more fires than either alone.
pub(crate) fn assist_confidence_threshold(t: &str, sensitivity: AssistSensitivity) -> u8 {
    let coach = matches!(t, "coach");
    match (sensitivity, coach) {
        (AssistSensitivity::Aggressive, true) => 65,
        (AssistSensitivity::Aggressive, false) => 45,
        (AssistSensitivity::Moderate, true) => 85,
        (AssistSensitivity::Moderate, false) => 70,
        (AssistSensitivity::Minimal, true) => 95,
        (AssistSensitivity::Minimal, false) => 85,
    }
}

/// Hard per-meeting cap on assist items. Assist is the only
/// Append-strategy agent surface, so `push_item_for_mode` applies no
/// 10-item FIFO cap to it — without this backstop, a model driven by
/// a transcript prompt-injection could grow the assist bucket (and
/// its WS broadcasts + Postgres rows) without bound. The confidence
/// gate and text dedup above are trivially bypassed by varying text.
pub(crate) const MAX_ASSIST_ITEMS_PER_MEETING: usize = 50;

/// Enforce the limits the tool schema advertises (headline ≤80,
/// detail ≤300): strip control chars, trim, clamp with `…`. Returns
/// `None` when the headline is unusable (over the hard ceiling, or
/// empty after sanitization) — the caller skips the suggestion with
/// corrective feedback. An oversized detail is dropped while the
/// headline is kept. Pure so the tests below hit it without a
/// ToolCtx (same pattern as threshold_tests).
pub(crate) fn sanitize_assist_fields(
    headline: &str,
    detail: Option<&str>,
) -> Option<(String, Option<String>)> {
    if exceeds_hard_ceiling(headline) {
        return None;
    }
    let headline = sanitize_item_text(headline, MAX_ASSIST_HEADLINE_CHARS, "assist.headline");
    if headline.is_empty() {
        return None;
    }
    let detail = detail
        .filter(|d| !exceeds_hard_ceiling(d))
        .map(|d| sanitize_item_text(d, MAX_ASSIST_DETAIL_CHARS, "assist.detail"))
        .filter(|d| !d.is_empty());
    Some((headline, detail))
}

impl Tool for PushAssistSuggestion {
    const NAME: &'static str = "push_assist_suggestion";
    type Args = PushAssistSuggestionArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: crate::agent::prompts::TOOL_DESC_PUSH_ASSIST_SUGGESTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["definition", "question", "memory", "coach"],
                        "description": "Suggestion category."
                    },
                    "headline": {
                        "type": "string",
                        "description": "The primary line (≤80 chars). For a definition this is the TERM ITSELF; for the other types a specific answer/connection/fact — never a meeting summary."
                    },
                    "detail": {
                        "type": "string",
                        "description": "Optional detail shown when tapped (≤300 chars). For a definition, a standalone glossary-style explanation of the term — not a recap of what the meeting said about it."
                    },
                    "confidence": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 100,
                        "description": "Your own confidence that this is useful right now. Gated server-side per type."
                    }
                },
                "required": ["type", "headline", "confidence"]
            }),
        }
    }

    async fn call(&self, args: PushAssistSuggestionArgs) -> Result<String, AgentToolError> {
        if !meeting_is_live(&self.0).await {
            return Ok("skipped: meeting no longer active".into());
        }
        // Validate type (the JSON schema's enum constraint isn't
        // always honored by the model; defense in depth).
        let kind = match args.r#type.as_str() {
            "definition" | "question" | "memory" | "coach" => args.r#type.clone(),
            other => return Ok(format!("skipped: unknown assist type {other:?}")),
        };
        // Enforce the schema-advertised text limits (headline ≤80,
        // detail ≤300) before anything downstream sees the text —
        // the dedup below MUST compare the clamped headline so the
        // dedup key and the stored item text agree.
        let Some((headline, detail)) =
            sanitize_assist_fields(&args.headline, args.detail.as_deref())
        else {
            return Ok("skipped: headline exceeds size limits".into());
        };
        // Server-side confidence gate. The agent self-rates per the
        // system prompt's calibration guidance, but we enforce the
        // per-type floor so a miscalibrated model can't flood the
        // assist feed. The floor is per-meeting via the
        // `assist_sensitivity` setting — the runtime field is the
        // source of truth, read fresh per call so a mid-meeting
        // toggle takes effect on the very next tool fire.
        let sensitivity = current_assist_sensitivity(&self.0).await;
        let threshold = assist_confidence_threshold(&kind, sensitivity);
        if args.confidence < threshold {
            return Ok(format!(
                "skipped: confidence {} below threshold {} for type {} at sensitivity {}",
                args.confidence,
                threshold,
                kind,
                sensitivity.as_str()
            ));
        }
        // Defense-in-depth text dedup. The LLM's tool-call history is
        // supposed to prevent duplicates, but a fresh meeting (no
        // history yet) or a cancelled fire can re-emit identical
        // suggestions. The guard makes a re-push a no-op without
        // filling the popup queue with dead duplicates.
        let meta = serde_json::json!({
            "type": kind,
            "confidence": args.confidence,
            "role": "assistant",
        });
        // Dedup + feed-full + elapsed read + push happen under ONE
        // registry lock acquisition, scoped to the meeting this fire
        // belongs to. `meeting_is_live` above is only a cheap early
        // exit — it is check-then-act across separate lock acquisitions,
        // so a stop (or stop+start) landing between it and here would
        // otherwise misfile this suggestion into idle state or the NEXT
        // meeting. `with_session_if_active` makes check+write atomic;
        // outer None = stale meeting.
        enum PushOutcome {
            FeedFull,
            Duplicate,
            Pushed { id: String, payload: Vec<Item> },
        }
        let outcome = {
            let mut s = self.0.sessions.lock().await;
            s.with_session_if_active(&self.0.user_id, &self.0.meeting_id, |u| {
                if u.mode_len("assist") >= MAX_ASSIST_ITEMS_PER_MEETING {
                    return PushOutcome::FeedFull;
                }
                if u.mode_contains_text("assist", &headline) {
                    return PushOutcome::Duplicate;
                }
                let elapsed_ms = u
                    .meeting_started_at()
                    .map(|start| start.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                let item = Item {
                    id: format!("as-{}", uuid::Uuid::new_v4()),
                    text: headline.clone(),
                    detail,
                    t: elapsed_ms,
                    meta: Some(meta),
                };
                let id = item.id.clone();
                let payload = u.push_item_for_mode("assist", item);
                PushOutcome::Pushed { id, payload }
            })
        };
        match outcome {
            None => Ok("skipped: meeting no longer active".into()),
            Some(PushOutcome::FeedFull) => Ok("skipped: assist feed full for this meeting".into()),
            Some(PushOutcome::Duplicate) => Ok("ok: skipped duplicate text".into()),
            Some(PushOutcome::Pushed { id, payload }) => {
                if !payload.is_empty() {
                    self.0
                        .bus
                        .emit(
                            self.0.user_id.clone(),
                            Event::ItemsUpdate {
                                mode: "assist".into(),
                                items: payload,
                            },
                        )
                        .await;
                }
                Ok(format!("ok: pushed assist {kind} {id}"))
            }
        }
    }
}

#[cfg(test)]
mod threshold_tests {
    use super::*;

    #[test]
    fn aggressive_thresholds_are_below_moderate() {
        // Sanity: the whole point of Aggressive is to fire more, so
        // every type's floor must be strictly lower than Moderate's.
        for kind in ["definition", "question", "memory", "coach"] {
            let a = assist_confidence_threshold(kind, AssistSensitivity::Aggressive);
            let m = assist_confidence_threshold(kind, AssistSensitivity::Moderate);
            assert!(
                a < m,
                "aggressive {kind} threshold {a} should be < moderate {m}"
            );
        }
    }

    #[test]
    fn minimal_thresholds_are_above_moderate() {
        for kind in ["definition", "question", "memory", "coach"] {
            let mi = assist_confidence_threshold(kind, AssistSensitivity::Minimal);
            let mo = assist_confidence_threshold(kind, AssistSensitivity::Moderate);
            assert!(
                mi > mo,
                "minimal {kind} threshold {mi} should be > moderate {mo}"
            );
        }
    }

    #[test]
    fn coach_floor_stays_above_other_types_at_every_sensitivity() {
        // Even when relaxed, coach is the riskiest type — wrong coach
        // suggestions are distracting where wrong definitions are
        // just unnecessary. Floor for coach must always be higher
        // than the floor for the other three at the same sensitivity.
        for s in [
            AssistSensitivity::Aggressive,
            AssistSensitivity::Moderate,
            AssistSensitivity::Minimal,
        ] {
            let coach = assist_confidence_threshold("coach", s);
            for kind in ["definition", "question", "memory"] {
                let other = assist_confidence_threshold(kind, s);
                assert!(
                    coach > other,
                    "coach {coach} should beat {kind} {other} at sensitivity {}",
                    s.as_str(),
                );
            }
        }
    }

    #[test]
    fn moderate_thresholds_match_historical_behavior() {
        // Pre-feature floors were coach ≥ 85, others ≥ 70. The
        // Moderate sensitivity row of the table must preserve them
        // exactly so existing tuning + dashboards don't shift the
        // moment we ship this.
        assert_eq!(
            assist_confidence_threshold("coach", AssistSensitivity::Moderate),
            85
        );
        for kind in ["definition", "question", "memory"] {
            assert_eq!(
                assist_confidence_threshold(kind, AssistSensitivity::Moderate),
                70
            );
        }
    }
}

#[cfg(test)]
mod sanitize_tests {
    use super::*;
    use crate::agent::tools::{
        ITEM_TEXT_HARD_CEILING, MAX_ASSIST_DETAIL_CHARS, MAX_ASSIST_HEADLINE_CHARS,
    };

    #[test]
    fn headline_clamped_to_80() {
        let long = "h".repeat(MAX_ASSIST_HEADLINE_CHARS + 40);
        let (headline, _) = sanitize_assist_fields(&long, None).expect("kept");
        assert_eq!(headline.chars().count(), MAX_ASSIST_HEADLINE_CHARS + 1);
        assert!(headline.ends_with('…'));
    }

    #[test]
    fn detail_clamped_to_300() {
        let long = "d".repeat(MAX_ASSIST_DETAIL_CHARS + 100);
        let (_, detail) = sanitize_assist_fields("RAG", Some(&long)).expect("kept");
        let detail = detail.expect("detail kept");
        assert_eq!(detail.chars().count(), MAX_ASSIST_DETAIL_CHARS + 1);
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn oversized_headline_rejected() {
        let huge = "h".repeat(ITEM_TEXT_HARD_CEILING + 1);
        assert!(sanitize_assist_fields(&huge, None).is_none());
    }

    #[test]
    fn empty_headline_after_sanitize_rejected() {
        assert!(sanitize_assist_fields("\u{0007}\u{001B}  ", None).is_none());
    }

    #[test]
    fn oversized_detail_dropped_but_headline_kept() {
        let huge = "d".repeat(ITEM_TEXT_HARD_CEILING + 1);
        let (headline, detail) = sanitize_assist_fields("RAG", Some(&huge)).expect("kept");
        assert_eq!(headline, "RAG");
        assert!(detail.is_none());
    }

    #[test]
    fn whitespace_only_detail_dropped() {
        let (_, detail) = sanitize_assist_fields("RAG", Some("   ")).expect("kept");
        assert!(detail.is_none());
    }

    #[test]
    fn short_fields_pass_through() {
        let (headline, detail) =
            sanitize_assist_fields("RAG", Some("Retrieval-augmented generation.")).expect("kept");
        assert_eq!(headline, "RAG");
        assert_eq!(detail.as_deref(), Some("Retrieval-augmented generation."));
    }

    #[test]
    fn assist_feed_cap_is_pinned() {
        assert_eq!(MAX_ASSIST_ITEMS_PER_MEETING, 50);
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

    fn args(headline: &str) -> PushAssistSuggestionArgs {
        PushAssistSuggestionArgs {
            r#type: "definition".into(),
            headline: headline.into(),
            detail: None,
            confidence: 100,
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
    async fn push_assist_skips_when_fires_meeting_is_no_longer_active() {
        let mut reg = SessionRegistry::new();
        let mid1 = start_meeting(&mut reg);
        reg.apply_intent(UID, Intent::StopMeeting);
        let _mid2 = start_meeting(&mut reg);
        let sessions = Arc::new(Mutex::new(reg));

        let (ctx, mut events_rx) = tool_ctx(Arc::clone(&sessions), UID, &mid1);
        let out = PushAssistSuggestion(ctx)
            .call(args("stale suggestion"))
            .await
            .unwrap();

        assert_eq!(out, "skipped: meeting no longer active");
        let s = sessions.lock().await;
        assert!(
            !s.user(UID)
                .unwrap()
                .mode_contains_text("assist", "stale suggestion"),
            "meeting-1 assist must not land in meeting 2"
        );
        drop(s);
        assert!(
            events_rx.try_recv().is_err(),
            "no ItemsUpdate broadcast for a stale push"
        );
    }

    #[tokio::test]
    async fn push_assist_writes_for_the_active_meeting() {
        let mut reg = SessionRegistry::new();
        let mid = start_meeting(&mut reg);
        let sessions = Arc::new(Mutex::new(reg));

        let (ctx, mut events_rx) = tool_ctx(Arc::clone(&sessions), UID, &mid);
        let out = PushAssistSuggestion(ctx)
            .call(args("live suggestion"))
            .await
            .unwrap();

        assert!(
            out.starts_with("ok: pushed assist definition"),
            "unexpected output: {out}"
        );
        let s = sessions.lock().await;
        assert!(s
            .user(UID)
            .unwrap()
            .mode_contains_text("assist", "live suggestion"));
        drop(s);
        match events_rx.try_recv() {
            Ok(ev) => {
                assert!(matches!(ev.event, Event::ItemsUpdate { ref mode, .. } if mode == "assist"))
            }
            Err(e) => panic!("expected ItemsUpdate, got {e:?}"),
        }
    }

    #[tokio::test]
    async fn push_assist_still_dedups_duplicate_text() {
        let mut reg = SessionRegistry::new();
        let mid = start_meeting(&mut reg);
        let sessions = Arc::new(Mutex::new(reg));

        let (ctx, _rx1) = tool_ctx(Arc::clone(&sessions), UID, &mid);
        let _ = PushAssistSuggestion(ctx)
            .call(args("Repeated"))
            .await
            .unwrap();
        let (ctx2, mut rx2) = tool_ctx(Arc::clone(&sessions), UID, &mid);
        let out = PushAssistSuggestion(ctx2)
            .call(args("Repeated"))
            .await
            .unwrap();

        assert_eq!(out, "ok: skipped duplicate text");
        assert!(rx2.try_recv().is_err(), "duplicate must not re-broadcast");
    }
}
