//! Tool wiring shared across all agent tools.
//!
//! `ToolCtx` is the single shared context struct every tool receives.
//! `AgentToolError` is the unified error type every tool returns.
//! `meeting_is_live` is a guard all mutating tools call before writing
//! items (prevents panics when the meeting ends mid-flight).
//!
//! Constants placed here because the `fire_chat!` / `fire_chat_stream!`
//! macros in `chat.rs` expand at the call site and need them in scope
//! via `use super::tools::{MAX_TURNS_PER_FIRE, AGENT_MAX_TOKENS}`.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Mutex;

use crate::mnemo::MnemoClient;
use crate::session::SessionRegistry;

pub mod artifacts;
pub mod assist;
pub mod highlights;
pub mod recall;
pub mod summary;

/// Max tool-call rounds per fire. The agent sometimes wants to
/// fetch_artifact_summary then act on it — that's 2 rounds. Allow
/// a few more headroom turns for fetch → reason → emit chains.
pub(crate) const MAX_TURNS_PER_FIRE: usize = 8;

/// Output-token ceiling per agent fire. Anthropic-direct *requires*
/// this be set on every request (rig raises `max_tokens must be set`
/// otherwise); Bedrock + OpenAI default it but we set it everywhere
/// for predictable behavior across providers. The agent emits a
/// handful of short tool calls plus optional chat replies — 4096 is
/// generous headroom without hitting Claude's 8192 default ceiling.
pub(crate) const AGENT_MAX_TOKENS: u64 = 4096;

/// Per-surface char limits for model-emitted item text. These are
/// the server-side enforcement of contracts the tool schemas and
/// prompts already advertise (assist.rs schema: headline ≤80,
/// detail ≤300; TOOL_DESC_REPLACE_SUMMARY: "one sentence, ≤20
/// words"). The model is partially driven by untrusted transcript
/// audio, so advertised-but-unenforced limits are an injection
/// surface — see improvements.md §29.
pub(crate) const MAX_HIGHLIGHT_CHARS: usize = 200; // prompt says one short line
pub(crate) const MAX_SUMMARY_BULLET_CHARS: usize = 200; // ≤20 words + headroom
pub(crate) const MAX_ASSIST_HEADLINE_CHARS: usize = 80; // matches assist.rs schema
pub(crate) const MAX_ASSIST_DETAIL_CHARS: usize = 300; // matches assist.rs schema

/// Beyond this, text is dropped/rejected rather than truncated:
/// clamping a 50 KB payload to 200 chars would silently legitimize
/// garbage and still cost the full deserialization. Callers return a
/// corrective `skipped`/`dropped` tool-result string so the model
/// gets feedback without an extra retry round.
pub(crate) const ITEM_TEXT_HARD_CEILING: usize = 2_000;

/// Sanitize model-emitted item text before it reaches session state,
/// the WS broadcast, or Postgres: strip control characters (C0, C1,
/// and DEL — Unicode `Cc` — keeping `\n` and `\t`), trim surrounding
/// whitespace, and truncate on a char boundary with a visible `…`
/// when over `max_chars`. Same truncation pattern as
/// `chat.rs::cap_chat_text`. `surface` labels the warn log so an
/// over-firing tool is attributable in kleos logs.
///
/// CONVENTION for tool authors: every agent tool that builds an
/// `Item` from model output MUST route each text field through this
/// helper (and gate on `exceeds_hard_ceiling` first for the reject
/// path) before constructing the `Item`.
pub(crate) fn sanitize_item_text(s: &str, max_chars: usize, surface: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    tracing::warn!(
        surface,
        original_chars = s.chars().count(),
        max_chars,
        "agent item text exceeded advertised limit; truncating"
    );
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// True when raw model text is so far past any advertised limit that
/// truncation would hide the problem — callers drop the item (and say
/// so in the tool result) instead of clamping.
pub(crate) fn exceeds_hard_ceiling(s: &str) -> bool {
    s.chars().count() > ITEM_TEXT_HARD_CEILING
}

#[derive(Debug, Error)]
pub enum AgentToolError {
    #[error("internal: {0}")]
    Internal(String),
}

/// Shared dependencies every tool needs. `events_tx` is only used
/// by the push/replace tools; `db` is only used by the fetch-artifact
/// tools; `mnemo` is only used by the fetch-meeting tools. Keeping
/// them on one struct keeps the per-tool wiring uniform at the
/// (small) cost of unused fields per tool. Cloning is cheap
/// (Arc + Sender + String + PgPool + reqwest::Client clones).
#[derive(Clone)]
pub(crate) struct ToolCtx {
    pub(crate) sessions: Arc<Mutex<SessionRegistry>>,
    pub(crate) bus: crate::context::EventBus,
    pub(crate) db: sqlx::PgPool,
    pub(crate) user_id: String,
    /// The meeting this agent fire belongs to. Mutating tools scope
    /// their writes to it via `with_session_if_active` so a result
    /// landing after stop_meeting (or after the next meeting started)
    /// is skipped instead of misfiled.
    pub(crate) meeting_id: String,
    pub(crate) mnemo: MnemoClient,
}

/// Guard against tool calls landing after the meeting has already
/// transitioned to Idle (e.g., user clicked Stop while an LLM
/// call was in flight). `push_item_for_mode` asserts the
/// items-empty-when-idle invariant; we'd panic on commit. Returns
/// `true` if the meeting is still active and the tool should proceed.
pub(crate) async fn meeting_is_live(ctx: &ToolCtx) -> bool {
    let s = ctx.sessions.lock().await;
    matches!(
        s.user(&ctx.user_id).map(|u| u.meeting_state),
        Some(crate::protocol::MeetingState::Active)
    )
}

/// Read the active meeting's assist sensitivity, locking the
/// registry only briefly. Returns the default (`Moderate`) if no
/// meeting is active — defensive for tool callers that race with
/// `stop_meeting`. The tool would normally also fall through the
/// `meeting_is_live` guard in that case, but having a non-panicking
/// fallback keeps this helper safe to call from anywhere.
pub(crate) async fn current_assist_sensitivity(
    ctx: &ToolCtx,
) -> crate::protocol::AssistSensitivity {
    let s = ctx.sessions.lock().await;
    s.user(&ctx.user_id)
        .and_then(|u| u.meeting.as_ref())
        .map(|m| m.assist_sensitivity)
        .unwrap_or_default()
}

#[cfg(test)]
mod sanitize_tests {
    use super::*;

    #[test]
    fn short_text_passes_through_unchanged() {
        assert_eq!(
            sanitize_item_text("a short highlight", 200, "test"),
            "a short highlight"
        );
    }

    #[test]
    fn strips_c0_and_c1_control_chars_keeps_newline_and_tab() {
        let input = "a\u{0007}b\u{001B}[31mc\u{0085}d\ne\tf\u{007F}g";
        assert_eq!(sanitize_item_text(input, 200, "test"), "ab[31mcd\ne\tfg");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(sanitize_item_text("  padded \n ", 200, "test"), "padded");
    }

    #[test]
    fn truncates_on_char_boundary_with_ellipsis() {
        let long: String = "é".repeat(250);
        let out = sanitize_item_text(&long, 200, "test");
        assert_eq!(out.chars().count(), 201); // 200 kept + '…'
        assert!(out.ends_with('…'));
        assert!(out.starts_with("ééé"));
    }

    #[test]
    fn truncates_emoji_around_the_limit_without_invalid_utf8() {
        let long: String = "🎙".repeat(205);
        let out = sanitize_item_text(&long, 200, "test");
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn exceeds_hard_ceiling_is_detected() {
        assert!(exceeds_hard_ceiling(
            &"x".repeat(ITEM_TEXT_HARD_CEILING + 1)
        ));
        assert!(!exceeds_hard_ceiling(&"x".repeat(ITEM_TEXT_HARD_CEILING)));
        assert!(!exceeds_hard_ceiling(""));
    }

    // Compile-time invariant: the per-surface clamps must sit under
    // the hard ceiling, else a value could be clamped to a length the
    // reject path would also have tripped. Const assert so an edit
    // that inverts them fails to build, not at runtime.
    const _: () = assert!(MAX_HIGHLIGHT_CHARS <= ITEM_TEXT_HARD_CEILING);
    const _: () = assert!(MAX_SUMMARY_BULLET_CHARS <= ITEM_TEXT_HARD_CEILING);

    #[test]
    fn advertised_limits_match_tool_schemas() {
        assert_eq!(MAX_ASSIST_HEADLINE_CHARS, 80);
        assert_eq!(MAX_ASSIST_DETAIL_CHARS, 300);
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Build a ToolCtx around an in-memory registry for tool unit
    /// tests. The PgPool is lazy — no connection is ever attempted
    /// (these tools never touch the DB) — and mnemo is Disabled.
    /// Returns the ctx plus a receiver on its events channel so tests
    /// can assert what was (not) broadcast.
    pub(crate) fn tool_ctx(
        sessions: Arc<Mutex<SessionRegistry>>,
        user_id: &str,
        meeting_id: &str,
    ) -> (
        ToolCtx,
        tokio::sync::broadcast::Receiver<crate::protocol::UserEvent>,
    ) {
        let (fanout, fanout_rx) = tokio::sync::broadcast::channel(16);
        // Drain the durable lane so `bus.emit` (durable side) completes
        // instead of erroring on a closed channel; tests assert on the
        // fan-out receiver above.
        let (durable_tx, mut durable_rx) =
            tokio::sync::mpsc::channel::<crate::protocol::UserEvent>(16);
        tokio::spawn(async move { while durable_rx.recv().await.is_some() {} });
        let bus = crate::context::EventBus::new(fanout, durable_tx);
        let db = sqlx::PgPool::connect_lazy("postgres://unused:unused@localhost:1/unused")
            .expect("connect_lazy only parses the URL");
        (
            ToolCtx {
                sessions,
                bus,
                db,
                user_id: user_id.to_string(),
                meeting_id: meeting_id.to_string(),
                mnemo: MnemoClient::Disabled,
            },
            fanout_rx,
        )
    }
}
