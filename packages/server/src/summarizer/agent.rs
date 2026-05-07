//! Agentic summarizer loop.
//!
//! One LLM agent per active meeting, holding the ONLY path from
//! transcript → items. Replaces the previous three per-mode
//! summarizers (highlights / actions / open_questions); they no
//! longer exist as separate summarizer tasks. The agent reasons
//! over each batch of new transcript chunks and emits items via
//! tool calls.
//!
//! Trigger model — fires when ANY of:
//!   - new-token threshold (`AGENT_TRIGGER_TOKENS`, default 200)
//!   - new-sentence threshold (`AGENT_TRIGGER_SENTENCES`, default 4)
//!   - silence boundary (`AGENT_TRIGGER_SILENCE_MS`, default 4000)
//!   - hard ceiling (`AGENT_TRIGGER_MAX_MS`, default 30000)
//!   - kick (e.g., user attached an artifact mid-meeting)
//!
//! Working-context shape (current — pre-stateful rewrite):
//!   - items-as-memory + tail transcript window + meeting meta +
//!     attached-artifact pre-load.
//!
//! Tools:
//!   - `push_highlight`, `replace_highlights`,
//!   - `push_action`, `push_open_question`,
//!   - `fetch_artifact_summary`, `fetch_artifact` (3-tier artifact
//!     access: pre-loaded short summary, fetchable long summary,
//!     fetchable full text).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rig::completion::{Prompt, ToolDefinition};
use rig::prelude::*;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::contract::{Event, Item, UserEvent};
use crate::llm::{LlmBackend, LlmClient};
use crate::state::ServerState;
use crate::stt::TranscriptChunk;

// ─── Tunables ───────────────────────────────────────────────────────────

const AGENT_TRIGGER_TOKENS_DEFAULT: usize = 200;
const AGENT_TRIGGER_SILENCE_MS_DEFAULT: u64 = 4000;
const AGENT_TRIGGER_MAX_MS_DEFAULT: u64 = 30_000;

/// Sentence-count trigger. The token threshold favors multi-speaker
/// chatter (lots of small chunks); a single-speaker monologue can
/// arrive as one ~30-second chunk and not hit the silence boundary
/// for a while. Counting sentences across the buffer (split by
/// `.!?`) gives a substance-based trigger that fires the same way
/// for "1 chunk, 5 sentences" and "5 chunks, 5 sentences".
const AGENT_TRIGGER_SENTENCES_DEFAULT: usize = 4;

/// Tail-window cap on transcript chunks the agent sees verbatim.
/// PLAN.md §3.5 starts at N=80 (~5-10 minutes of speech). Older
/// chunks are dropped when the window exceeds this — v1 has no
/// rolling-summary compression yet (PLAN.md §3.6). Truncation is
/// fine for personal-use meetings under ~30 minutes.
const TAIL_WINDOW_MAX_CHUNKS: usize = 80;

/// Rough chars-per-token estimate for the trigger threshold.
/// 4:1 is the well-known English ballpark. Provider-specific
/// tokenization is more accurate but rig doesn't surface it.
const CHARS_PER_TOKEN: usize = 4;

const SYSTEM_PROMPT: &str = "You are an agent inside a real-time meeting note-taker. \
Your job: emit structured items via tool calls when transcript chunks contain something noteworthy.\n\
\n\
CRITICAL RULES (in priority order):\n\
\n\
1. CHECK ITEMS-AS-MEMORY BEFORE EMITTING.\n\
The user message contains \"# Current items\" sections showing what's already been recorded. \
Never push anything that paraphrases, restates, or expands on an existing item. If the new chunk \
says \"Bob will send slides\" and an existing action says \"Bob to share design\", these are the \
SAME THING — do not push. Treat dedup by intent, not by exact wording.\n\
\n\
2. EMIT NOTHING WHEN THERE'S NOTHING NEW.\n\
Most chunks should produce zero tool calls. When in doubt, skip. The user prefers 5 high-signal \
items over 30 mediocre ones.\n\
\n\
3. PICK THE RIGHT MODE. The three modes are distinct:\n\
\n\
ACTIONS — COMMITMENTS. Someone said they (or someone) will do something. \
Trigger phrases: \"I'll\", \"I will\", \"we'll\", \"X to Y\", \"X is responsible for\", \"next week we'll\", \
\"I want to present\", \"will share\", \"will have results\". \
The `owner` is whoever was named or self-referenced. The `due` is the timing if stated. \
OMIT optional fields entirely if not stated — never pass empty strings.\n\
\n\
Examples:\n\
- \"I'll keep you in the loop on what I find out\" → push_action(text=\"Share findings with the team\")\n\
- \"We will have testing results next week\" → push_action(text=\"Deliver testing results\", due=\"next week\")\n\
- \"Next week I want to present infrastructure costs\" → push_action(text=\"Present AWS infrastructure costs\", due=\"next week\")\n\
\n\
OPEN_QUESTIONS — UNRESOLVED queries. Real questions raised but not answered, or topics that need follow-up. \
Trigger: chunks ending in \"?\", \"we need to figure out\", \"still TBD\", \"who's responsible for\".\n\
\n\
Examples:\n\
- \"Is it a migration or a new workload?\" → push_open_question\n\
- \"What's responsible for access?\" → push_open_question\n\
- \"Did everyone get access yet?\" → push_open_question\n\
\n\
HIGHLIGHTS — DECISIONS, surprising facts, named entities, conclusions, specific numbers. \
Reserve for substance the user would highlight in a re-read. SKIP pleasantries, introductions, \
small talk, process commentary, and meta-commentary about the meeting itself.\n\
\n\
Examples:\n\
- \"The cutover target is January or February of next year\" → push_highlight (specific decision)\n\
- \"There's a Slack channel #oracle-database-at-aws for relevant posts\" → push_highlight (named resource)\n\
- SKIP: \"The meeting is titled X and is related to project Y\" (meta about the meeting itself)\n\
- SKIP: \"OK\", \"yeah\", \"I see\", \"Thank you for being here\"\n\
\n\
4. ACTIONS AND QUESTIONS ARE USUALLY MORE NUMEROUS THAN HIGHLIGHTS in working meetings. \
Expect 5-10 actions, 3-7 open_questions, 2-5 highlights for a 30-minute call. \
DO NOT default to push_highlight when the chunk is really an action or question.\n\
\n\
5. ATTACHED ARTIFACTS — use them. The user message may contain a \"# Attached artifacts\" \
section listing documents the user uploaded for this meeting. Each row has id + name + mime \
+ short_summary. When the transcript references an attached artifact (e.g., \"per the agenda…\", \
\"as the design doc says…\"), use the retrieval tools to ground your reasoning:\n\
\n\
- fetch_artifact_summary {id}: get the LONG summary (~500 tokens). Cheap, use freely.\n\
- fetch_artifact {id}: get the FULL text content. Use sparingly — large docs are expensive. \
Falls back to long summary for binary formats (PDFs, images).\n\
\n\
The pre-load short summary is enough for ~70-80% of references. Only fetch when you need \
specific facts, decisions, or named entities the short summary doesn't capture.\n\
\n\
6. Speak in the same language as the transcript. Don't translate.";

// ─── Tool surface ───────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum AgentToolError {
    #[error("internal: {0}")]
    Internal(String),
}

/// Shared dependencies every tool needs. `events_tx` is only used
/// by the push/replace tools; `db` is only used by the fetch tools;
/// keeping them on one struct keeps the per-tool wiring uniform
/// at the (small) cost of unused fields per tool. Cloning is cheap
/// (Arc + Sender + String + PgPool clone).
#[derive(Clone)]
struct ToolCtx {
    state: Arc<Mutex<ServerState>>,
    events_tx: broadcast::Sender<UserEvent>,
    db: sqlx::PgPool,
    user_id: String,
}

/// Guard against tool calls landing after the meeting has already
/// transitioned to Idle (e.g., user clicked Stop while an LLM
/// call was in flight). `push_item_for_mode` asserts the
/// items-empty-when-idle invariant; we'd panic on commit. Returns
/// `true` if the meeting is still active/paused and the tool
/// should proceed.
async fn meeting_is_live(ctx: &ToolCtx) -> bool {
    let s = ctx.state.lock().await;
    matches!(
        s.user(&ctx.user_id).map(|u| u.meeting_state),
        Some(crate::contract::MeetingState::Active) | Some(crate::contract::MeetingState::Paused)
    )
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct PushHighlightArgs {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    importance: Option<String>,
}

struct PushHighlight(ToolCtx);

impl Tool for PushHighlight {
    const NAME: &'static str = "push_highlight";
    type Args = PushHighlightArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Add a highlight item — a short standalone insight or noteworthy point \
worth remembering. Use for concrete observations, decisions, or surprising details. \
Don't push duplicates of items already in the highlights buffer."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The highlight, one short sentence." },
                    "importance": { "type": "string", "description": "Optional importance label (\"high\", \"medium\", \"low\")." }
                },
                "required": ["text"]
            }),
        }
    }

    async fn call(&self, args: PushHighlightArgs) -> Result<String, AgentToolError> {
        if !meeting_is_live(&self.0).await {
            return Ok("skipped: meeting no longer active".into());
        }
        let item = Item {
            id: format!("h-{}", uuid::Uuid::new_v4()),
            text: args.text.clone(),
            detail: None,
            t: 0,
            meta: args
                .importance
                .as_ref()
                .map(|i| serde_json::json!({"importance": i})),
        };
        let id = item.id.clone();
        let payload = {
            let mut s = self.0.state.lock().await;
            s.user_mut(&self.0.user_id)
                .push_item_for_mode("highlights", item)
        };
        if !payload.is_empty() {
            let _ = self.0.events_tx.send(UserEvent::new(
                self.0.user_id.clone(),
                Event::ItemsUpdate {
                    mode: "highlights".into(),
                    items: payload,
                },
            ));
        }
        Ok(format!("ok: pushed highlight {id}"))
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ReplaceHighlightItem {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    importance: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ReplaceHighlightsArgs {
    items: Vec<ReplaceHighlightItem>,
}

struct ReplaceHighlights(ToolCtx);

impl Tool for ReplaceHighlights {
    const NAME: &'static str = "replace_highlights";
    type Args = ReplaceHighlightsArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Replace ALL highlights with a fresh list. Use sparingly — only when \
the existing highlights need genuine reorganization (e.g., consolidate redundant entries, \
re-order by importance). Pass the new full list."
                .to_string(),
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
        let n = args.items.len();
        let items: Vec<Item> = args
            .items
            .into_iter()
            .map(|h| Item {
                id: format!("h-{}", uuid::Uuid::new_v4()),
                text: h.text,
                detail: None,
                t: 0,
                meta: h.importance.map(|i| serde_json::json!({"importance": i})),
            })
            .collect();
        let payload = {
            let mut s = self.0.state.lock().await;
            s.user_mut(&self.0.user_id)
                .replace_items_for_mode("highlights", items)
        };
        let _ = self.0.events_tx.send(UserEvent::new(
            self.0.user_id.clone(),
            Event::ItemsUpdate {
                mode: "highlights".into(),
                items: payload,
            },
        ));
        Ok(format!("ok: replaced highlights with {n} items"))
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct PushActionArgs {
    text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<String>,
}

struct PushAction(ToolCtx);

impl Tool for PushAction {
    const NAME: &'static str = "push_action";
    type Args = PushActionArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Add an action item — something a participant committed to do. \
Include `owner` if a person was named; include `due` if a deadline was stated. Don't \
infer owners from context. Don't push duplicates of items already in the actions buffer."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The action, e.g., \"Send the design doc by EOW\"." },
                    "owner": { "type": "string", "description": "Person responsible, when stated." },
                    "due": { "type": "string", "description": "Deadline, when stated." }
                },
                "required": ["text"]
            }),
        }
    }

    async fn call(&self, args: PushActionArgs) -> Result<String, AgentToolError> {
        if !meeting_is_live(&self.0).await {
            return Ok("skipped: meeting no longer active".into());
        }
        // Models like gpt-4.1 sometimes pass empty strings instead
        // of omitting optional fields. Normalize so meta carries
        // only fields that actually have content; otherwise the
        // PWA's metadata renderer shows ugly "OWNER · " labels.
        let owner = args.owner.as_deref().filter(|s| !s.trim().is_empty());
        let due = args.due.as_deref().filter(|s| !s.trim().is_empty());
        let mut meta_map = serde_json::Map::new();
        if let Some(o) = owner {
            meta_map.insert("owner".into(), serde_json::Value::String(o.to_string()));
        }
        if let Some(d) = due {
            meta_map.insert("due".into(), serde_json::Value::String(d.to_string()));
        }
        let meta = if meta_map.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(meta_map))
        };
        let item = Item {
            id: format!("a-{}", uuid::Uuid::new_v4()),
            text: args.text.clone(),
            detail: None,
            t: 0,
            meta,
        };
        let id = item.id.clone();
        let payload = {
            let mut s = self.0.state.lock().await;
            s.user_mut(&self.0.user_id)
                .push_item_for_mode("actions", item)
        };
        if !payload.is_empty() {
            let _ = self.0.events_tx.send(UserEvent::new(
                self.0.user_id.clone(),
                Event::ItemsUpdate {
                    mode: "actions".into(),
                    items: payload,
                },
            ));
        }
        Ok(format!("ok: pushed action {id}"))
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct PushOpenQuestionArgs {
    question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

struct PushOpenQuestion(ToolCtx);

impl Tool for PushOpenQuestion {
    const NAME: &'static str = "push_open_question";
    type Args = PushOpenQuestionArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Add an open question — something raised but not resolved, or a \
topic that needs follow-up. `kind` can be \"factual\" / \"decision\" / \"design\" etc. \
Don't push duplicates."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The unresolved question." },
                    "kind": { "type": "string", "description": "Optional category." },
                    "context": { "type": "string", "description": "Optional one-line context for why it's open." }
                },
                "required": ["question"]
            }),
        }
    }

    async fn call(&self, args: PushOpenQuestionArgs) -> Result<String, AgentToolError> {
        if !meeting_is_live(&self.0).await {
            return Ok("skipped: meeting no longer active".into());
        }
        let kind = args.kind.as_deref().filter(|s| !s.trim().is_empty());
        let context = args.context.as_deref().filter(|s| !s.trim().is_empty());
        let mut meta_map = serde_json::Map::new();
        if let Some(k) = kind {
            meta_map.insert("kind".into(), serde_json::Value::String(k.to_string()));
        }
        if let Some(c) = context {
            meta_map.insert("context".into(), serde_json::Value::String(c.to_string()));
        }
        let meta = if meta_map.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(meta_map))
        };
        let item = Item {
            id: format!("q-{}", uuid::Uuid::new_v4()),
            text: args.question.clone(),
            detail: None,
            t: 0,
            meta,
        };
        let id = item.id.clone();
        let payload = {
            let mut s = self.0.state.lock().await;
            s.user_mut(&self.0.user_id)
                .push_item_for_mode("open_questions", item)
        };
        if !payload.is_empty() {
            let _ = self.0.events_tx.send(UserEvent::new(
                self.0.user_id.clone(),
                Event::ItemsUpdate {
                    mode: "open_questions".into(),
                    items: payload,
                },
            ));
        }
        Ok(format!("ok: pushed open_question {id}"))
    }
}

// ─── Retrieval tools (PLAN.md §3.8) ─────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct FetchArtifactArgs {
    /// The id of an attached artifact (from the "# Attached
    /// artifacts" section in the working context).
    id: String,
}

/// Returns the artifact's `long_summary` as the tool result. Cheap
/// — DB read only. Use this when the pre-load short summary isn't
/// detailed enough to ground reasoning but the full document
/// would burn too many tokens.
struct FetchArtifactSummary(ToolCtx);

impl Tool for FetchArtifactSummary {
    const NAME: &'static str = "fetch_artifact_summary";
    type Args = FetchArtifactArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Fetch the LONG summary (~500 tokens) of an attached artifact. \
The pre-load only includes the SHORT summary (~50 tokens) for each artifact; this tool \
gives you a more detailed view when the short summary isn't enough to ground your \
reasoning. Cheap — use it freely when an artifact is relevant to the current chunk."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Artifact id from the # Attached artifacts list." }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: FetchArtifactArgs) -> Result<String, AgentToolError> {
        let row = crate::db::get_artifact_for_user(&self.0.db, &args.id, &self.0.user_id)
            .await
            .map_err(|e| AgentToolError::Internal(e.to_string()))?;
        match row {
            Some(a) => match a.long_summary {
                Some(s) if !s.is_empty() => Ok(format!("Long summary of '{}':\n\n{}", a.name, s)),
                _ => Ok(format!(
                    "Artifact '{}' has no long summary yet (status: {})",
                    a.name, a.summary_status
                )),
            },
            None => Ok(format!(
                "error: no such artifact {} (or not yours)",
                args.id
            )),
        }
    }
}

/// Returns the full text content of an attached artifact when
/// possible. Text formats (markdown, plain, html, csv, json) are
/// inlined as-is. PDFs and images fall back to the long summary
/// — full binary attachment into the agent's chat history would
/// need a custom prompt loop (PLAN.md v1.6 work).
struct FetchArtifact(ToolCtx);

impl Tool for FetchArtifact {
    const NAME: &'static str = "fetch_artifact";
    type Args = FetchArtifactArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Fetch the FULL content of an attached artifact for inline inspection. \
For text formats (markdown, plain text, html, csv, json), returns the document body. \
For PDFs and images, returns the long summary as a fallback (full binary content can't \
be inlined yet). Use sparingly — the full body can be large. Prefer fetch_artifact_summary \
when the long summary suffices."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Artifact id from the # Attached artifacts list." }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: FetchArtifactArgs) -> Result<String, AgentToolError> {
        let row = crate::db::get_artifact_for_user(&self.0.db, &args.id, &self.0.user_id)
            .await
            .map_err(|e| AgentToolError::Internal(e.to_string()))?;
        let Some(a) = row else {
            return Ok(format!(
                "error: no such artifact {} (or not yours)",
                args.id
            ));
        };
        // Text formats: inline the bytes as UTF-8.
        let is_text = matches!(
            a.mime_type.as_str(),
            "text/plain" | "text/markdown" | "text/html" | "text/csv" | "application/json"
        );
        if is_text {
            let dir = crate::db::data_dir().map_err(|e| AgentToolError::Internal(e.to_string()))?;
            let abs = dir.join(&a.asset_path);
            let bytes = tokio::fs::read(&abs)
                .await
                .map_err(|e| AgentToolError::Internal(format!("read {}: {e}", abs.display())))?;
            return match String::from_utf8(bytes) {
                Ok(content) => Ok(format!(
                    "Full content of '{}' ({}):\n\n{}",
                    a.name, a.mime_type, content
                )),
                Err(e) => Ok(format!("error: artifact {} not valid UTF-8: {e}", args.id)),
            };
        }
        // Binary: fall back to long summary so the model gets the
        // most-informative grounding signal we can offer today.
        match a.long_summary {
            Some(s) if !s.is_empty() => Ok(format!(
                "Artifact '{}' is {} (binary; full content can't be inlined). Long summary instead:\n\n{}",
                a.name, a.mime_type, s
            )),
            _ => Ok(format!(
                "Artifact '{}' is {} (binary; full content can't be inlined) and has no long summary yet.",
                a.name, a.mime_type
            )),
        }
    }
}

// ─── Trigger loop ───────────────────────────────────────────────────────

/// Spawn the agent task. Runs for the meeting's lifetime; cancels
/// when the per-meeting cancel token fires (matches the per-mode
/// summarizer task lifecycle).
pub fn spawn_meeting_agent(
    state: Arc<Mutex<ServerState>>,
    db: sqlx::PgPool,
    transcript_rx: broadcast::Receiver<TranscriptChunk>,
    kick_rx: broadcast::Receiver<AgentKick>,
    events_tx: broadcast::Sender<UserEvent>,
    user_id: String,
    llm: Arc<LlmClient>,
    cancel: CancellationToken,
) {
    let token_threshold = env_usize("AGENT_TRIGGER_TOKENS", AGENT_TRIGGER_TOKENS_DEFAULT);
    let silence_ms = env_u64("AGENT_TRIGGER_SILENCE_MS", AGENT_TRIGGER_SILENCE_MS_DEFAULT);
    let max_ms = env_u64("AGENT_TRIGGER_MAX_MS", AGENT_TRIGGER_MAX_MS_DEFAULT);
    let sentence_threshold = env_usize("AGENT_TRIGGER_SENTENCES", AGENT_TRIGGER_SENTENCES_DEFAULT);

    tokio::spawn(async move {
        info!(
            user_id = %user_id,
            token_threshold,
            sentence_threshold,
            silence_ms,
            max_ms,
            "agent loop started",
        );

        let mut buffer: Vec<TranscriptChunk> = Vec::new();
        let mut tail_window: Vec<TranscriptChunk> = Vec::new();
        let mut last_fire_at = Instant::now();
        let mut last_chunk_at: Option<Instant> = None;
        let mut transcript_rx = transcript_rx;
        let mut kick_rx = kick_rx;
        // 500 ms tick covers silence + hard-cap checks; chunks that
        // arrive between ticks are still picked up immediately on
        // the recv() arm via the token-threshold path.
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tick.tick().await; // discard immediate tick

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!(user_id = %user_id, "agent loop cancelled");
                    return;
                }
                recv_result = transcript_rx.recv() => {
                    match recv_result {
                        Ok(chunk) => {
                            if chunk.user_id != user_id { continue; }
                            buffer.push(chunk);
                            last_chunk_at = Some(Instant::now());
                            // Token + sentence threshold check after each
                            // chunk push. Token covers "lots of text";
                            // sentence covers "many complete thoughts" —
                            // the latter fires similarly for monologues
                            // and multi-speaker exchanges.
                            let approx_tokens: usize = buffer
                                .iter()
                                .map(|c| c.text.len() / CHARS_PER_TOKEN)
                                .sum();
                            let sentences: usize = buffer
                                .iter()
                                .map(|c| count_sentences(&c.text))
                                .sum();
                            if approx_tokens >= token_threshold || sentences >= sentence_threshold {
                                fire(&state, &db, &events_tx, &user_id, &llm, &mut buffer, &mut tail_window).await;
                                last_fire_at = Instant::now();
                                last_chunk_at = None;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, user_id = %user_id, "agent loop transcript lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!(user_id = %user_id, "agent loop transcript channel closed");
                            return;
                        }
                    }
                }
                kick_result = kick_rx.recv() => {
                    match kick_result {
                        Ok(kick) if kick.user_id == user_id => {
                            // Instant fire on artifact attach (or future
                            // SystemNotice causes). Skips threshold checks
                            // entirely so the agent reacts within a turn
                            // of the user attaching context. The fire
                            // queries attached_artifacts fresh in
                            // build_working_context, so the new artifact
                            // shows up in the same fire that was kicked.
                            info!(
                                user_id = %user_id,
                                reason = ?kick.reason,
                                "agent loop kicked",
                            );
                            fire(&state, &db, &events_tx, &user_id, &llm, &mut buffer, &mut tail_window).await;
                            last_fire_at = Instant::now();
                            last_chunk_at = None;
                        }
                        Ok(_) => { /* kick for another user — ignore */ }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, user_id = %user_id, "agent loop kick channel lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            // Kick channel closing isn't fatal; the loop
                            // still reacts to transcripts and ticks.
                            warn!(user_id = %user_id, "agent loop kick channel closed");
                        }
                    }
                }
                _ = tick.tick() => {
                    if buffer.is_empty() { continue; }
                    let now = Instant::now();
                    let silent = last_chunk_at
                        .map(|t| now.duration_since(t) >= Duration::from_millis(silence_ms))
                        .unwrap_or(false);
                    let aged = now.duration_since(last_fire_at) >= Duration::from_millis(max_ms);
                    if silent || aged {
                        fire(&state, &db, &events_tx, &user_id, &llm, &mut buffer, &mut tail_window).await;
                        last_fire_at = now;
                        last_chunk_at = None;
                    }
                }
            }
        }
    });
}

/// One agent invocation. Drains `buffer` into `tail_window`, builds
/// the working-context string, and prompts the LLM with the four
/// tools registered. Tool calls mutate state + emit events as side
/// effects; this fn returns when rig's prompt loop completes.
async fn fire(
    state: &Arc<Mutex<ServerState>>,
    db: &sqlx::PgPool,
    events_tx: &broadcast::Sender<UserEvent>,
    user_id: &str,
    llm: &Arc<LlmClient>,
    buffer: &mut Vec<TranscriptChunk>,
    tail_window: &mut Vec<TranscriptChunk>,
) {
    let new_chunks: Vec<TranscriptChunk> = std::mem::take(buffer);
    if new_chunks.is_empty() {
        return;
    }
    // Roll the new chunks into the tail window, capped at
    // TAIL_WINDOW_MAX_CHUNKS. Anything that falls off the front is
    // simply dropped — v1 has no rolling-summary compression.
    let new_chunks_count = new_chunks.len();
    tail_window.extend(new_chunks);
    if tail_window.len() > TAIL_WINDOW_MAX_CHUNKS {
        let drop_n = tail_window.len() - TAIL_WINDOW_MAX_CHUNKS;
        tail_window.drain(..drop_n);
    }

    let (user_message, ctx_metrics) = build_working_context(state, db, user_id, tail_window).await;
    // Optional debug dump of the full prompt — set
    // `AGENT_LOG_PROMPT=1` to see exactly what the agent receives.
    // Off by default because the prompt is sizable (multi-KB).
    if std::env::var("AGENT_LOG_PROMPT").ok().as_deref() == Some("1") {
        info!(
            user_id,
            prompt = %user_message,
            "agent prompt dump",
        );
    }
    let started = Instant::now();
    let ctx = ToolCtx {
        state: state.clone(),
        events_tx: events_tx.clone(),
        db: db.clone(),
        user_id: user_id.to_string(),
    };

    // Provider dispatch. Each arm constructs its own tools (cheap
    // Arc clones via ctx) and runs the prompt loop.
    let result = match &llm.backend {
        LlmBackend::Bedrock { client, model_id } => {
            let agent = client
                .agent(model_id)
                .preamble(SYSTEM_PROMPT)
                .tool(PushHighlight(ctx.clone()))
                .tool(ReplaceHighlights(ctx.clone()))
                .tool(PushAction(ctx.clone()))
                .tool(PushOpenQuestion(ctx.clone()))
                .tool(FetchArtifactSummary(ctx.clone()))
                .tool(FetchArtifact(ctx))
                .build();
            agent.prompt(user_message.clone()).max_turns(3).await
        }
        LlmBackend::OpenAI { client, model_id } => {
            let agent = client
                .agent(model_id.as_str())
                .preamble(SYSTEM_PROMPT)
                .tool(PushHighlight(ctx.clone()))
                .tool(ReplaceHighlights(ctx.clone()))
                .tool(PushAction(ctx.clone()))
                .tool(PushOpenQuestion(ctx.clone()))
                .tool(FetchArtifactSummary(ctx.clone()))
                .tool(FetchArtifact(ctx))
                .build();
            agent.prompt(user_message.clone()).max_turns(3).await
        }
        LlmBackend::Anthropic { client, model_id } => {
            let agent = client
                .agent(model_id.as_str())
                .preamble(SYSTEM_PROMPT)
                .tool(PushHighlight(ctx.clone()))
                .tool(ReplaceHighlights(ctx.clone()))
                .tool(PushAction(ctx.clone()))
                .tool(PushOpenQuestion(ctx.clone()))
                .tool(FetchArtifactSummary(ctx.clone()))
                .tool(FetchArtifact(ctx))
                .build();
            agent.prompt(user_message.clone()).max_turns(3).await
        }
    };

    let latency_ms = started.elapsed().as_millis() as u64;
    let prompt_chars = (SYSTEM_PROMPT.len() + user_message.len()) as u64;
    match result {
        Ok(response) => {
            // Per-fire `llm_call` log mirrors the per-extract logs
            // from `extract_with_prompt`. Increments the per-user
            // counter so `llm_usage_at_stop` reflects agent spend.
            // response_chars is the model's final-turn text, which
            // for tool-calling responses is usually empty (the
            // useful work happens in tool calls, not the text). The
            // tool calls themselves don't surface here — we'd need
            // a PromptHook to capture them; deferred to v1.1.
            let response_chars = response.len() as u64;
            llm.record_usage(user_id, prompt_chars, response_chars);
            info!(
                user_id,
                provider = ?llm.provider(),
                call = "agent_fire",
                prompt_chars,
                response_chars,
                latency_ms,
                "llm_call",
            );
            info!(
                user_id,
                provider = ?llm.provider(),
                new_chunks = new_chunks_count,
                tail_chunks = tail_window.len(),
                prompt_chars,
                attached_artifacts = ctx_metrics.attached_artifacts,
                existing_highlights = ctx_metrics.existing_highlights,
                existing_actions = ctx_metrics.existing_actions,
                existing_open_questions = ctx_metrics.existing_open_questions,
                latency_ms,
                "agent fire done",
            );
        }
        Err(e) => {
            // Still record the prompt_chars on failure — the call
            // hit the provider and we paid for it; the response is
            // just empty/error.
            llm.record_usage(user_id, prompt_chars, 0);
            warn!(
                user_id,
                provider = ?llm.provider(),
                error = %e,
                latency_ms,
                "agent fire failed",
            );
        }
    }
}

/// Render the working context the agent sees on each fire.
/// Sections in order:
///
/// 1. Meeting metadata (project / title / owner — flat key=value).
/// 2. Items-as-memory: current items in highlights / actions /
///    open_questions, each as a JSON line with id + flattened
///    fields. Per PLAN.md §3.4 implementation contract the
///    `meta` blob is flattened (`{id, text, owner, due}` not
///    `{id, text, meta: {...}}`) so the agent reads it cleanly.
/// 3. Tail-window transcript: the last N chunks verbatim with
///    `[Speaker N, mm:ss]` prefixes when diarization is available.
/// Counts of what landed in the prompt — emitted on `agent fire
/// done` so operators can verify (e.g.) that an attached artifact
/// actually made it into the working context for the fire that
/// followed an attach kick.
#[derive(Debug, Default)]
struct ContextMetrics {
    attached_artifacts: usize,
    existing_highlights: usize,
    existing_actions: usize,
    existing_open_questions: usize,
}

async fn build_working_context(
    state: &Arc<Mutex<ServerState>>,
    db: &sqlx::PgPool,
    user_id: &str,
    tail_window: &[TranscriptChunk],
) -> (String, ContextMetrics) {
    use std::fmt::Write;
    let mut buf = String::with_capacity(2048);
    let mut metrics = ContextMetrics::default();

    let (metadata, current_meeting_id, highlights, actions, open_questions) = {
        let s = state.lock().await;
        match s.user(user_id) {
            Some(u) => (
                u.metadata.clone(),
                u.current_meeting_id.clone(),
                u.items_per_mode
                    .get("highlights")
                    .cloned()
                    .unwrap_or_default(),
                u.items_per_mode.get("actions").cloned().unwrap_or_default(),
                u.items_per_mode
                    .get("open_questions")
                    .cloned()
                    .unwrap_or_default(),
            ),
            None => (Default::default(), None, Vec::new(), Vec::new(), Vec::new()),
        }
    };

    // Items-as-memory FIRST so the dedup signal is fresh when the
    // model considers the new transcript. This ordering matters:
    // small models (gpt-4.1-mini) routinely re-pushed duplicates
    // when items appeared after transcript in the prompt.
    metrics.existing_highlights = highlights.len();
    metrics.existing_actions = actions.len();
    metrics.existing_open_questions = open_questions.len();
    buf.push_str("# Already recorded — DO NOT push duplicates of these\n");
    write_items_section(&mut buf, "highlights", &highlights);
    write_items_section(&mut buf, "actions", &actions);
    write_items_section(&mut buf, "open_questions", &open_questions);
    buf.push('\n');

    // Attached artifacts pre-load. id + name + short_summary so the
    // agent can ground its reasoning on what the user attached
    // (PDF docs, images, etc.). The full content + long summary
    // come via fetch_artifact_summary / fetch_artifact tools (v1.2,
    // PLAN.md §3.8) — not yet wired. For now, short summary is
    // enough for the agent to know "this doc exists, here's its
    // gist" and reference it in highlights/actions.
    if let Some(mid) = current_meeting_id.as_deref() {
        let attached = crate::db::list_artifacts_for_meeting(db, mid)
            .await
            .unwrap_or_default();
        metrics.attached_artifacts = attached.len();
        if !attached.is_empty() {
            buf.push_str("# Attached artifacts (use these as context — the user uploaded them for this meeting)\n");
            for a in &attached {
                let summary = a.short_summary.as_deref().unwrap_or("(summary pending)");
                let _ = writeln!(
                    buf,
                    "  {{\"id\":\"{}\",\"name\":\"{}\",\"mime\":\"{}\",\"summary\":\"{}\"}}",
                    a.id,
                    escape_json_str(&a.name),
                    a.mime_type,
                    escape_json_str(summary),
                );
            }
            buf.push('\n');
        }
    }

    if !metadata.is_empty() {
        buf.push_str("# Meeting context\n");
        let mut keys: Vec<&String> = metadata.keys().collect();
        keys.sort();
        for k in keys {
            let _ = writeln!(buf, "  {k}: {}", metadata[k]);
        }
        buf.push('\n');
    }

    buf.push_str("# Transcript (oldest first, newest at the bottom)\n");
    if tail_window.is_empty() {
        buf.push_str("  (empty)\n");
    } else {
        for chunk in tail_window {
            let speaker = chunk
                .speaker
                .as_deref()
                .map(|s| format!("[Speaker {s}] "))
                .unwrap_or_default();
            let mm = chunk.t_start_ms / 60_000;
            let ss = (chunk.t_start_ms % 60_000) / 1000;
            let _ = writeln!(buf, "  {speaker}[{mm:02}:{ss:02}] {}", chunk.text);
        }
    }
    buf.push('\n');
    buf.push_str(
        "Process the transcript above. For EACH question (line ending in `?` or implying \
         uncertainty), emit push_open_question. For EACH commitment (\"I'll\", \"we'll\", named \
         deadline), emit push_action. For surprising/specific facts not already recorded, emit \
         push_highlight. Skip pleasantries and anything semantically equivalent to existing items.",
    );
    (buf, metrics)
}

/// Minimal JSON string escaper for the working-context renderer.
/// We're not deserializing this — it's a prompt for an LLM — so
/// quote/backslash/control-char handling is enough; full RFC 8259
/// is overkill.
fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push(' '),
            '\r' => out.push(' '),
            '\t' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Format one mode's items as `{id, text, ...flattened-meta}` JSON
/// lines. Empty modes render as `(none)` so the agent sees the
/// presence of the section even without data.
fn write_items_section(buf: &mut String, mode: &str, items: &[Item]) {
    use std::fmt::Write;
    let _ = writeln!(buf, "## {mode}");
    if items.is_empty() {
        buf.push_str("  (none)\n");
        return;
    }
    for item in items {
        let mut entry = serde_json::Map::new();
        entry.insert("id".into(), serde_json::Value::String(item.id.clone()));
        entry.insert("text".into(), serde_json::Value::String(item.text.clone()));
        if let Some(serde_json::Value::Object(map)) = &item.meta {
            for (k, v) in map {
                // Drop nulls AND empty strings so the agent doesn't
                // see noise like `"owner":""`. Models that emit
                // empty strings by mistake (gpt-4.1 does this) get
                // filtered at write time too — see the tool-call
                // normalization for the input side.
                if v.is_null() {
                    continue;
                }
                if let Some(s) = v.as_str() {
                    if s.is_empty() {
                        continue;
                    }
                }
                entry.insert(k.clone(), v.clone());
            }
        }
        let line = serde_json::to_string(&entry).unwrap_or_default();
        let _ = writeln!(buf, "  {line}");
    }
}

// ─── Kick channel ───────────────────────────────────────────────────────

/// Sent on `ServerHandle.agent_kick_tx` to ask the agent loop to
/// fire immediately for a specific user. Today there's one trigger
/// — an artifact was attached — but the type is open-ended so
/// future SystemNotice-style causes can use the same channel.
#[derive(Debug, Clone)]
pub struct AgentKick {
    pub user_id: String,
    pub reason: AgentKickReason,
}

#[derive(Debug, Clone, Copy)]
pub enum AgentKickReason {
    ArtifactAttached,
}

// ─── Helpers ────────────────────────────────────────────────────────────

/// Count complete sentences in `text` by tallying terminator
/// punctuation. ASCII `.!?` plus the CJK full-stop we already use
/// in soniox.rs's terminator detection. Trailing punctuation that
/// ends the string still counts (so "Hello." is one sentence).
fn count_sentences(text: &str) -> usize {
    text.chars()
        .filter(|c| matches!(c, '.' | '!' | '?' | '。'))
        .count()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Per PLAN.md §3.4, items-as-memory must flatten `meta` —
    /// agents see `{id, text, owner, due}`, not nested `{meta: ...}`.
    #[test]
    fn write_items_section_flattens_meta() {
        let item = Item {
            id: "a-1".into(),
            text: "Bob to send slides".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({"owner": "Bob", "due": "Friday"})),
        };
        let mut buf = String::new();
        write_items_section(&mut buf, "actions", &[item]);
        assert!(buf.contains("\"owner\":\"Bob\""));
        assert!(buf.contains("\"due\":\"Friday\""));
        // Should NOT have a "meta" nested key — that's the flattening contract.
        assert!(!buf.contains("\"meta\""));
    }

    #[test]
    fn write_items_section_drops_null_meta_fields() {
        let item = Item {
            id: "a-1".into(),
            text: "Some action".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({"owner": "Bob", "due": null})),
        };
        let mut buf = String::new();
        write_items_section(&mut buf, "actions", &[item]);
        assert!(buf.contains("\"owner\":\"Bob\""));
        // Null fields are dropped so the agent doesn't see noise.
        assert!(!buf.contains("\"due\""));
    }

    #[test]
    fn write_items_section_renders_none_for_empty() {
        let mut buf = String::new();
        write_items_section(&mut buf, "highlights", &[]);
        assert!(buf.contains("(none)"));
    }

    #[test]
    fn count_sentences_handles_multiple_terminators() {
        assert_eq!(count_sentences("Hello world."), 1);
        assert_eq!(count_sentences("Hello! How are you?"), 2);
        assert_eq!(
            count_sentences("This is a test. With three sentences. And one more."),
            3
        );
        assert_eq!(count_sentences(""), 0);
        assert_eq!(count_sentences("no punctuation here"), 0);
        // CJK full-stop counts.
        assert_eq!(count_sentences("こんにちは。さようなら。"), 2);
    }

    #[test]
    fn write_items_section_drops_empty_string_meta_fields() {
        // Models occasionally emit empty strings instead of omitting
        // optional fields. Those should be filtered the same way
        // null fields are — keeps the agent's view clean.
        let item = Item {
            id: "a-1".into(),
            text: "Some action".into(),
            detail: None,
            t: 0,
            meta: Some(serde_json::json!({"owner": "", "due": "Friday"})),
        };
        let mut buf = String::new();
        write_items_section(&mut buf, "actions", &[item]);
        assert!(buf.contains("\"due\":\"Friday\""));
        assert!(!buf.contains("\"owner\""));
    }
}
