//! Agentic summarizer loop — stateful, conversation-history-driven.
//!
//! One LLM agent per active meeting, holding the ONLY path from
//! transcript → items. The agent maintains a `Vec<Message>`
//! conversation history that grows across the entire meeting:
//! every fire appends only the delta (new transcript chunks +
//! optional event blocks) as the next user turn, and rig appends
//! the agent's reply (assistant turns + tool calls + tool results)
//! back onto the same history. The agent's tool-calling history
//! IS its memory of what was already recorded — there's no
//! separate items-as-memory section in the prompt.
//!
//! Trigger model — fires when ANY of:
//!   - new-token threshold (`AGENT_TRIGGER_TOKENS`, default 200)
//!   - new-sentence threshold (`AGENT_TRIGGER_SENTENCES`, default 4)
//!   - silence boundary (`AGENT_TRIGGER_SILENCE_MS`, default 4000)
//!   - hard ceiling (`AGENT_TRIGGER_MAX_MS`, default 30000)
//!   - kick (e.g., user attached an artifact)
//!
//! Per-fire user message structure:
//!   - first fire: `[meeting]` + `[attached artifacts]` (bootstrap)
//!     + `[transcript]` (new chunks). Subsequent fires: only
//!     `[transcript]` and/or `[event]` blocks.
//!   - kick events (e.g. mid-meeting artifact attach) are folded
//!     into the next user message as `[event]` blocks alongside
//!     any pending transcript.
//!
//! Tools:
//!   - `push_highlight`, `replace_highlights`,
//!   - `push_action`, `push_open_question`,
//!   - `fetch_artifact_summary`, `fetch_artifact` (3-tier artifact
//!     access: pre-loaded short summary, fetchable long summary,
//!     fetchable full text).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rig::agent::AgentBuilder;
use rig::completion::{Message as RigMessage, Prompt, ToolDefinition};
use rig::message::AssistantContent;
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

/// Rough chars-per-token estimate for the trigger threshold.
/// 4:1 is the well-known English ballpark. Provider-specific
/// tokenization is more accurate but rig doesn't surface it.
const CHARS_PER_TOKEN: usize = 4;

/// Max tool-call rounds per fire. The agent sometimes wants to
/// fetch_artifact_summary then act on it — that's 2 rounds. Allow
/// a few more headroom turns for fetch → reason → emit chains.
const MAX_TURNS_PER_FIRE: usize = 8;

/// Output-token ceiling per agent fire. Anthropic-direct *requires*
/// this be set on every request (rig raises `max_tokens must be set`
/// otherwise); Bedrock + OpenAI default it but we set it everywhere
/// for predictable behavior across providers. The agent emits a
/// handful of short tool calls plus optional chat replies — 4096 is
/// generous headroom without hitting Claude's 8192 default ceiling.
const AGENT_MAX_TOKENS: u64 = 4096;

const SYSTEM_PROMPT: &str = "You are an agent inside a real-time meeting note-taker. \
Your job: emit structured items via tool calls when transcript chunks contain something noteworthy.\n\
\n\
OUTPUT FORMAT — READ THIS FIRST\n\
There are TWO modes for your reply, decided by what's in the user message:\n\
\n\
A. NORMAL MODE — when the user message contains [transcript] / [event] blocks but NO [chat] block:\n\
- Tool calls are your ONLY useful output. Either emit one or more tool calls, or end your turn with empty text.\n\
- DO NOT respond with conversational text. NEVER say \"I'll keep listening\", \"Noted, I'll watch for…\", \
\"Thanks for attaching that\", \"I see\", \"Understood\", \"Let me know if…\".\n\
- Empty turn is the correct response when there's nothing to record.\n\
\n\
B. CHAT MODE — when the user message contains a [chat] block:\n\
- Reply with text. Your text response IS the answer the user sees in the chat panel — be direct, \
informative, and as concise as the question warrants. No \"Let me check…\" preamble.\n\
- Tool calls are still allowed and encouraged when the chat asks you to record something \
(\"capture this as an action,\" \"add a highlight,\" etc.). Emit the tool call AND a brief text \
confirmation in the same turn.\n\
- Use your conversation history (transcript + past tool calls + any attached artifacts) to ground \
the answer. If the user asks about something you don't have, say so honestly in one sentence.\n\
\n\
HOW THE CONVERSATION WORKS\n\
- Each user turn delivers one or more of: a [meeting] block (first turn only, structured key/value \
metadata such as title, host, type), a [context] block (first turn only, the user's freeform \
description — relationships, intent, expected outcomes; use it to interpret what's noteworthy), \
[event] blocks (e.g., \"User just attached artifact …\"), [chat] blocks (a user question/instruction), \
and a [transcript] block (new speech since the last turn).\n\
- Your past tool calls are visible in this conversation history. They are your memory of what's already \
been recorded — there is no separate \"existing items\" list.\n\
\n\
EMISSION RULES\n\
\n\
1. NO DUPLICATES. If you previously called push_action(text=\"Bob to send slides\") and the new transcript \
says \"Bob will share design\", DO NOT emit again — same intent, already captured. Treat dedup by intent, \
not by exact wording. Consult your prior tool calls in the conversation history before each emission.\n\
\n\
2. EMIT NOTHING WHEN THERE'S NOTHING NEW. Most turns produce zero tool calls — that's normal. End the turn \
with no text. When in doubt, skip the tool call AND skip the prose. The user prefers 5 high-signal items \
over 30 mediocre ones, and they ABSOLUTELY do not want a running commentary.\n\
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
\n\
OPEN_QUESTIONS — UNRESOLVED queries. Default to push_open_question when a question is asked AND its answer \
does not appear in the SAME [transcript] block. If the speaker asks \"How long ago did you leave?\" and the \
next sentence is \"31 August\", that's resolved — skip. If the question ends a transcript block with no \
answer following, push it.\n\
\n\
Trigger: any sentence ending in \"?\", or phrases like \"we need to figure out\", \"still TBD\", \"who's responsible for\", \
\"what are…\", \"how do we…\". Don't speculate that a future turn will answer it — push now; if it gets answered \
later you can dismiss it via UI.\n\
\n\
Examples:\n\
- \"Is it a migration or a new workload?\" (no answer in same block) → push_open_question\n\
- \"Who's responsible for access?\" → push_open_question\n\
- \"What are the biggest contributions Hopper made to computer science?\" (block ends here) → push_open_question\n\
- \"How tough was boot camp?\" → \"Mentally demanding.\" (answer in same block) → SKIP, resolved\n\
\n\
HIGHLIGHTS — DECISIONS, surprising facts, named entities, conclusions, specific numbers. \
Reserve for substance worth highlighting on a re-read. SKIP pleasantries, introductions, small talk, \
process commentary, and meta-commentary about the meeting itself.\n\
\n\
Examples:\n\
- \"The cutover target is January or February of next year\" → push_highlight (specific decision)\n\
- \"There's a Slack channel #oracle-database-at-aws for relevant posts\" → push_highlight (named resource)\n\
- SKIP: \"OK\", \"yeah\", \"I see\", \"Thank you for being here\"\n\
\n\
4. ACTIONS AND QUESTIONS ARE USUALLY MORE NUMEROUS THAN HIGHLIGHTS in working meetings. \
Expect 5-10 actions, 3-7 open_questions, 2-5 highlights for a 30-minute call. \
DO NOT default to push_highlight when something is really an action or question.\n\
\n\
5. ATTACHED ARTIFACTS. When a [event] block says the user attached an artifact, you receive its \
id + name + mime + short_summary. Use the retrieval tools to ground your reasoning when the transcript \
references it (\"per the agenda…\", \"as the design doc says…\"):\n\
- fetch_artifact_summary(id): LONG summary (~500 tokens). Cheap, use freely.\n\
- fetch_artifact(id): FULL text content. Use sparingly. Falls back to long summary for binary formats.\n\
\n\
The short summary in the [event] block is enough for ~70-80% of references; only fetch when you need \
specific facts the short summary doesn't capture.\n\
\n\
6. ATTACHED MEETINGS. The [meeting] block lists past meetings the user attached to the current one \
(their titles + ids). The transcript may refer back to them (\"like we talked about last sync…\", \
\"following up on the recruiter call…\"). Use the meeting-recall tools to pull context:\n\
- fetch_meeting_summary(id): scoped recall against an attached meeting — returns its action items, \
highlights, open_questions, plus moment summaries. Cheap; use when a reference is clearly to that \
meeting.\n\
- fetch_meeting(id): broader scoped recall (same scoping; mnemo decides verbosity). Use sparingly.\n\
\n\
Don't fetch a meeting just because it's attached — only when the transcript references it.\n\
\n\
7. Speak in the same language as the transcript. Don't translate.";

// ─── Tool surface ───────────────────────────────────────────────────────

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
struct ToolCtx {
    state: Arc<Mutex<ServerState>>,
    events_tx: broadcast::Sender<UserEvent>,
    db: sqlx::PgPool,
    user_id: String,
    mnemo: crate::mnemo::MnemoClient,
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

// ─── Meeting-recall tools ──────────────────────────────────────────────
//
// Per-meeting recall against mnemo. The user attaches past meetings
// to the current one (see `Snapshot.attached_meeting_ids` and the
// REST `/meetings/:id/attached_meetings` endpoints); each attached
// meeting becomes a meeting_id the agent can recall against
// on-demand. Filters mnemo's `/recall` by `attributes.meeting_id`,
// which every meeting's push events carry (see `mnemo/payload.rs`).

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct FetchMeetingArgs {
    /// The meeting id of an attached past meeting (from the
    /// "# Attached meetings" block in the working context).
    id: String,
}

/// `fetch_meeting_summary` — recall the curated summary-bundle of
/// an attached meeting. mnemo's summary_bundle events are pushed at
/// stop_meeting and contain the per-mode rollup (actions / highlights
/// / open_questions). Cheap when prompt caching is hot.
struct FetchMeetingSummary(ToolCtx);

impl Tool for FetchMeetingSummary {
    const NAME: &'static str = "fetch_meeting_summary";
    type Args = FetchMeetingArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description:
                "Fetch the curated SUMMARY of an attached past meeting (its action items, \
highlights, and open questions, plus any moment summaries). Use this when an attached meeting \
is the natural source of context for the current chunk — e.g. the user references \"last \
week's sync\" and the meeting is in the attached list. Cheap; prefer this over fetch_meeting \
for most ground-truth lookups."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Meeting id from the # Attached meetings list." }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: FetchMeetingArgs) -> Result<String, AgentToolError> {
        let params = crate::mnemo::recall::RecallParams::for_meeting_id(args.id.clone());
        tracing::info!(meeting_id = %args.id, "agent fetch_meeting_summary: recalling from mnemo");
        let recalled = self
            .0
            .mnemo
            .recall(&params)
            .await
            .map_err(|e| AgentToolError::Internal(format!("mnemo recall: {e}")))?;
        let s = recalled.summary();
        tracing::info!(
            meeting_id = %args.id,
            preferences = s.preferences,
            facts = s.facts,
            episodes = s.episodes,
            project_memories = s.project_memories,
            "agent fetch_meeting_summary: recall complete"
        );
        // Filter to summary_bundle-derived memories (the per-meeting
        // rollup) when present. Empty response = either the meeting
        // wasn't pushed to mnemo, or mnemo is disabled. Either way
        // we say so in plain text so the agent doesn't loop retrying.
        let body = recalled.format_for_prompt();
        if body.trim().is_empty() {
            Ok(format!(
                "No mnemo memories found for meeting {} (it may have ended before mnemo was \
                 enabled, or the attached id is unknown).",
                args.id
            ))
        } else {
            Ok(format!("Recall for meeting {}:\n\n{body}", args.id))
        }
    }
}

/// `fetch_meeting` — same shape as the summary tool but conceptually
/// "give me everything you have on this meeting" (including raw
/// transcript sentences that streamed in during the meeting, not
/// just the curated bundle). On mnemo's side this is the same scoped
/// recall — mnemo decides how much to return per dimension; we don't
/// filter further on the client side today.
struct FetchMeeting(ToolCtx);

impl Tool for FetchMeeting {
    const NAME: &'static str = "fetch_meeting";
    type Args = FetchMeetingArgs;
    type Output = String;
    type Error = AgentToolError;

    async fn definition(&self, _: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Fetch the FULL recall of an attached past meeting — same scoping as \
fetch_meeting_summary but mnemo will return more verbose context (e.g. raw transcript snippets \
in addition to the rollup). Use sparingly; the response can be large. Prefer fetch_meeting_summary \
when the rollup answers your question."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Meeting id from the # Attached meetings list." }
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(&self, args: FetchMeetingArgs) -> Result<String, AgentToolError> {
        // mnemo treats summary and full as the same scoped recall
        // today; the verbosity difference is on its end. We keep
        // both tools so the agent's prompt can express intent.
        let params = crate::mnemo::recall::RecallParams::for_meeting_id(args.id.clone());
        tracing::info!(meeting_id = %args.id, "agent fetch_meeting: recalling from mnemo");
        let recalled = self
            .0
            .mnemo
            .recall(&params)
            .await
            .map_err(|e| AgentToolError::Internal(format!("mnemo recall: {e}")))?;
        let s = recalled.summary();
        tracing::info!(
            meeting_id = %args.id,
            preferences = s.preferences,
            facts = s.facts,
            episodes = s.episodes,
            project_memories = s.project_memories,
            "agent fetch_meeting: recall complete"
        );
        let body = recalled.format_for_prompt();
        if body.trim().is_empty() {
            Ok(format!(
                "No mnemo memories found for meeting {} (it may have ended before mnemo was \
                 enabled, or the attached id is unknown).",
                args.id
            ))
        } else {
            Ok(format!("Full recall for meeting {}:\n\n{body}", args.id))
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
    mnemo: crate::mnemo::MnemoClient,
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

        // Stateful conversation: each meeting accumulates a single
        // Vec<Message> across all fires. Each fire sends only the
        // delta (new transcript chunks since last fire and any
        // pending event) — the agent's tool-calling history in
        // `history` is its memory of what it already pushed.
        let mut history: Vec<rig::completion::Message> = Vec::new();
        let mut bootstrapped = false;
        let mut buffer: Vec<TranscriptChunk> = Vec::new();
        let mut last_fire_at = Instant::now();
        let mut last_chunk_at: Option<Instant> = None;
        let mut transcript_rx = transcript_rx;
        let mut kick_rx = kick_rx;
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
                            let approx_tokens: usize = buffer
                                .iter()
                                .map(|c| c.text.len() / CHARS_PER_TOKEN)
                                .sum();
                            let sentences: usize = buffer
                                .iter()
                                .map(|c| count_sentences(&c.text))
                                .sum();
                            if approx_tokens >= token_threshold || sentences >= sentence_threshold {
                                fire(
                                    &state, &db, &events_tx, &user_id, &llm, &mnemo,
                                    &mut buffer, &mut history, &mut bootstrapped, None,
                                ).await;
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
                            info!(user_id = %user_id, reason = ?kick.reason, "agent loop kicked");
                            let kick_block = format_kick_event(&db, &kick).await;
                            fire(
                                &state, &db, &events_tx, &user_id, &llm, &mnemo,
                                &mut buffer, &mut history, &mut bootstrapped, kick_block,
                            ).await;
                            last_fire_at = Instant::now();
                            last_chunk_at = None;
                        }
                        Ok(_) => { /* kick for another user — ignore */ }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(lagged = n, user_id = %user_id, "agent loop kick channel lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
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
                        fire(
                            &state, &db, &events_tx, &user_id, &llm, &mnemo,
                            &mut buffer, &mut history, &mut bootstrapped, None,
                        ).await;
                        last_fire_at = now;
                        last_chunk_at = None;
                    }
                }
            }
        }
    });
}

/// Build a `rig::completion::Message::User` from a text body plus a
/// list of attachments. Text comes first, followed by image content
/// blocks in caller order. Empty / whitespace-only text is skipped
/// (Anthropic rejects empty text blocks; OpenAI is unhappy about
/// them too). The caller MUST guarantee that at least one of (text,
/// attachments) is non-empty — `OneOrMany::many` panics otherwise.
fn build_user_message(text: String, attachments: Vec<AttachmentPayload>) -> RigMessage {
    use base64::Engine as _;
    use rig::message::{DocumentSourceKind, Image as RigImage, ImageMediaType};

    let mut parts: Vec<rig::message::UserContent> = Vec::with_capacity(1 + attachments.len());
    if !text.trim().is_empty() {
        parts.push(rig::message::UserContent::Text(text.into()));
    }
    for a in attachments {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&a.bytes);
        parts.push(rig::message::UserContent::Image(RigImage {
            data: DocumentSourceKind::Base64(b64),
            media_type: Some(ImageMediaType::PNG),
            detail: None,
            additional_params: None,
        }));
    }
    RigMessage::User {
        content: rig::OneOrMany::many(parts).expect("at least one content part"),
    }
}

/// One agent invocation. Builds the next user-turn message from
/// (optional) bootstrap + (optional) kick block + new transcript
/// chunks, fires the agent through rig with the existing `history`
/// as chat context, and appends rig's returned `Vec<Message>` (the
/// new user/assistant/tool-result turns produced by this fire) back
/// onto `history` so the next fire sees them.
#[allow(clippy::too_many_arguments)]
async fn fire(
    state: &Arc<Mutex<ServerState>>,
    db: &sqlx::PgPool,
    events_tx: &broadcast::Sender<UserEvent>,
    user_id: &str,
    llm: &Arc<LlmClient>,
    mnemo: &crate::mnemo::MnemoClient,
    buffer: &mut Vec<TranscriptChunk>,
    history: &mut Vec<rig::completion::Message>,
    bootstrapped: &mut bool,
    kick_block: Option<KickBlock>,
) {
    let new_chunks: Vec<TranscriptChunk> = std::mem::take(buffer);
    if new_chunks.is_empty() && kick_block.is_none() && *bootstrapped {
        return;
    }
    let new_chunks_count = new_chunks.len();

    // Capture chat context up-front. We need the user_text for the
    // chat-mode item even if the fire fails or returns empty text;
    // and we use the boolean to gate trailing-text-strip and to
    // capture the agent's response as the chat reply.
    // Also capture attachments so we can thread image bytes through
    // `build_user_message` into the LLM call below.
    let (chat_user_text, chat_attachments): (Option<String>, Vec<AttachmentPayload>) =
        match &kick_block {
            Some(KickBlock::Chat {
                user_text,
                attachments,
            }) => (Some(user_text.clone()), attachments.clone()),
            _ => (None, Vec::new()),
        };
    let is_chat_fire = chat_user_text.is_some();
    // Same shape for expand — we need the (mode, item_id) pair to
    // know where to write the resulting detail back. Capture it
    // before we move kick_block into the section builder.
    let expand_target: Option<(String, String)> = match &kick_block {
        Some(KickBlock::Expand { mode, item_id, .. }) => Some((mode.clone(), item_id.clone())),
        _ => None,
    };
    let is_expand_fire = expand_target.is_some();

    // Compose this turn's user message. Sections, in order:
    //   [meeting] header (first fire only)
    //   [event] / [chat] block (kick payload — when set)
    //   [transcript] block (new chunks since last fire — when present)
    //
    // Each fire sends only the delta. The agent's tool-call history
    // is its memory of what was already pushed.
    let mut sections: Vec<String> = Vec::new();
    if !*bootstrapped {
        if let Some(boot) = build_bootstrap_section(state, db, user_id).await {
            sections.push(boot);
        }
    }
    if let Some(block) = &kick_block {
        sections.push(format!("[{}]\n{}", block.label(), block.body()));
    }
    if !new_chunks.is_empty() {
        sections.push(format!("[transcript]\n{}", format_chunks(&new_chunks)));
    }
    if sections.is_empty() {
        return;
    }
    let user_message = sections.join("\n\n");

    // When the chat fire carries images, wrap the composed user text +
    // images into a single typed Message::User. For text-only fires
    // (the common case for non-chat kicks AND chat without attachments)
    // we pass the String straight through — rig's `Into<Message>` impl
    // wraps it for us, preserving current behavior.
    let user_prompt: rig::completion::Message = if !chat_attachments.is_empty() {
        build_user_message(user_message.clone(), chat_attachments)
    } else {
        user_message.clone().into()
    };

    if std::env::var("AGENT_LOG_PROMPT").ok().as_deref() == Some("1") {
        info!(user_id, prompt = %user_message, "agent prompt");
    }

    let started = Instant::now();
    let ctx = ToolCtx {
        state: state.clone(),
        events_tx: events_tx.clone(),
        db: db.clone(),
        user_id: user_id.to_string(),
        mnemo: mnemo.clone(),
    };

    // Provider dispatch. Three near-identical arms — rig's
    // `Agent<M>` is generic over the provider's model type, so
    // there's no clean trait-object shortcut. Each arm builds
    // its own agent (cheap), passes the prior `history` via
    // `with_history`, and uses `extended_details()` to get the
    // accumulated `Vec<Message>` back so we can append it.
    let history_input = history.clone();
    let result = match &llm.backend {
        LlmBackend::Bedrock { client, model_id } => {
            // Prompt caching: Bedrock inserts CachePoint blocks after
            // the system prompt and after the last message. Cache
            // hits show up as `cache_read_input_tokens` (folded into
            // rig's `cached_input_tokens`) on subsequent fires. If
            // the cached prefix is too short or the model doesn't
            // support caching, Bedrock silently ignores the
            // checkpoint — no error path needed.
            let model = client.completion_model(model_id).with_prompt_caching();
            let agent = AgentBuilder::new(model)
                .preamble(SYSTEM_PROMPT)
                .max_tokens(AGENT_MAX_TOKENS)
                .tool(PushHighlight(ctx.clone()))
                .tool(ReplaceHighlights(ctx.clone()))
                .tool(PushAction(ctx.clone()))
                .tool(PushOpenQuestion(ctx.clone()))
                .tool(FetchArtifactSummary(ctx.clone()))
                .tool(FetchArtifact(ctx.clone()))
                .tool(FetchMeetingSummary(ctx.clone()))
                .tool(FetchMeeting(ctx))
                .build();
            agent
                .prompt(user_prompt.clone())
                .with_history(history_input)
                .max_turns(MAX_TURNS_PER_FIRE)
                .extended_details()
                .await
        }
        LlmBackend::OpenAI { client, model_id } => {
            let agent = client
                .agent(model_id.as_str())
                .preamble(SYSTEM_PROMPT)
                .max_tokens(AGENT_MAX_TOKENS)
                .tool(PushHighlight(ctx.clone()))
                .tool(ReplaceHighlights(ctx.clone()))
                .tool(PushAction(ctx.clone()))
                .tool(PushOpenQuestion(ctx.clone()))
                .tool(FetchArtifactSummary(ctx.clone()))
                .tool(FetchArtifact(ctx.clone()))
                .tool(FetchMeetingSummary(ctx.clone()))
                .tool(FetchMeeting(ctx))
                .build();
            agent
                .prompt(user_prompt.clone())
                .with_history(history_input)
                .max_turns(MAX_TURNS_PER_FIRE)
                .extended_details()
                .await
        }
        LlmBackend::Anthropic { client, model_id } => {
            // Same caching strategy as the Bedrock arm — rig's
            // `with_prompt_caching()` adds `cache_control: ephemeral`
            // breakpoints on the system prompt and last message;
            // Claude 3.5+ supports prompt caching as a GA feature
            // (no `anthropic_beta` header required).
            let model = client
                .completion_model(model_id.as_str())
                .with_prompt_caching();
            let agent = AgentBuilder::new(model)
                .preamble(SYSTEM_PROMPT)
                .max_tokens(AGENT_MAX_TOKENS)
                .tool(PushHighlight(ctx.clone()))
                .tool(ReplaceHighlights(ctx.clone()))
                .tool(PushAction(ctx.clone()))
                .tool(PushOpenQuestion(ctx.clone()))
                .tool(FetchArtifactSummary(ctx.clone()))
                .tool(FetchArtifact(ctx.clone()))
                .tool(FetchMeetingSummary(ctx.clone()))
                .tool(FetchMeeting(ctx))
                .build();
            agent
                .prompt(user_prompt.clone())
                .with_history(history_input)
                .max_turns(MAX_TURNS_PER_FIRE)
                .extended_details()
                .await
        }
    };

    let latency_ms = started.elapsed().as_millis() as u64;
    let prompt_chars = (SYSTEM_PROMPT.len() + user_message.len()) as u64;
    match result {
        Ok(resp) => {
            llm.record_usage(
                user_id,
                resp.usage.input_tokens,
                resp.usage.output_tokens,
                resp.usage.cached_input_tokens,
            );
            let raw_msg_count = resp.messages.as_ref().map(|m| m.len()).unwrap_or(0);
            let mut filtered = 0usize;
            if let Some(mut new_msgs) = resp.messages {
                // Strip trailing text-only assistant turns — UNLESS
                // this is a chat fire. For transcript-only fires,
                // letting the model's final prose into history
                // ("noted, I'll keep listening") teaches it that
                // chat-style replies are the precedent and pollutes
                // future fires. For chat fires the trailing text IS
                // the user-visible reply and must stay in history so
                // future turns see "I told them X" context.
                // Both chat AND expand fires want the prose retained
                // in history (it's the user-visible reply); transcript-
                // only fires strip it to avoid teaching the model that
                // chat-style replies are normal.
                let keep_text = is_chat_fire || is_expand_fire;
                if !keep_text {
                    while matches!(new_msgs.last(), Some(RigMessage::Assistant { content, .. })
                        if content.iter().all(|c| matches!(c, AssistantContent::Text(_) | AssistantContent::Reasoning(_))))
                    {
                        new_msgs.pop();
                        filtered += 1;
                    }
                }
                history.extend(new_msgs);
            }
            let new_msg_count = raw_msg_count.saturating_sub(filtered);
            *bootstrapped = true;

            // Surface the chat reply. The user's question + the
            // agent's text response replace the chat-mode items
            // atomically — UI sees the Q+A pair appear together.
            // If the agent returned empty text (e.g., it only emitted
            // tool calls), we still render the user's question with
            // a placeholder so the UI doesn't show a stale prior
            // reply; tool calls themselves are reflected in their
            // respective modes (highlights / actions / open_questions)
            // — chat mode doesn't echo them.
            if let Some(user_text) = chat_user_text {
                let reply = resp.output.trim();
                let assistant_text = if reply.is_empty() {
                    "(recorded — see other modes)".to_string()
                } else {
                    reply.to_string()
                };
                let user_item = Item {
                    id: format!("chat-q-{}", uuid::Uuid::new_v4()),
                    text: user_text,
                    detail: None,
                    t: 0,
                    meta: Some(serde_json::json!({"role": "user"})),
                };
                let assistant_item = Item {
                    id: format!("chat-a-{}", uuid::Uuid::new_v4()),
                    text: assistant_text,
                    detail: None,
                    t: 0,
                    meta: Some(serde_json::json!({"role": "assistant"})),
                };
                // Append the Q+A pair to chat-mode items. Sent in a
                // single ItemsUpdate so the client appends both
                // atomically — `push_item_for_mode` was called twice
                // because Append returns just the new item per call.
                let pair = vec![user_item.clone(), assistant_item.clone()];
                {
                    let mut s = state.lock().await;
                    let user_state = s.user_mut(user_id);
                    user_state.push_item_for_mode("chat", user_item);
                    user_state.push_item_for_mode("chat", assistant_item);
                }
                let _ = events_tx.send(UserEvent::new(
                    user_id.to_string(),
                    Event::ItemsUpdate {
                        mode: "chat".into(),
                        items: pair,
                    },
                ));
            }

            // Surface the expand reply. Find the targeted item, set
            // its `detail`, broadcast the updated item via
            // `Event::ItemUpdated` so clients can render the
            // expansion inline. Empty / failed reply: write a
            // placeholder that signals the round-trip completed but
            // didn't produce content (better than leaving the UI
            // stuck on "Loading…").
            if let Some((target_mode, target_item_id)) = expand_target {
                let reply = resp.output.trim();
                let detail_text = if reply.is_empty() {
                    "(no expansion produced)".to_string()
                } else {
                    reply.to_string()
                };
                let updated = {
                    let mut s = state.lock().await;
                    s.user_mut(user_id)
                        .set_item_detail(&target_mode, &target_item_id, &detail_text)
                };
                if let Some(item) = updated {
                    let _ = events_tx.send(UserEvent::new(
                        user_id.to_string(),
                        Event::ItemUpdated {
                            mode: target_mode,
                            item,
                        },
                    ));
                }
            }

            info!(
                user_id,
                provider = ?llm.provider(),
                new_chunks = new_chunks_count,
                history_len = history.len(),
                new_msg_count,
                stripped_text_turns = filtered,
                is_chat = is_chat_fire,
                is_expand = is_expand_fire,
                prompt_chars,
                input_tokens = resp.usage.input_tokens,
                output_tokens = resp.usage.output_tokens,
                cached_input_tokens = resp.usage.cached_input_tokens,
                latency_ms,
                "agent fire done",
            );
        }
        Err(e) => {
            // On failure rig doesn't surface usage — log zeros so
            // the call still increments the counter and the rest of
            // the meeting's tally stays accurate.
            llm.record_usage(user_id, 0, 0, 0);
            // On failure with a chat fire, surface a one-line error
            // bubble so the user sees their question + a "retry"
            // hint instead of a stuck pending placeholder. Same
            // append shape as the success path — prior chat history
            // stays in place.
            if let Some(user_text) = chat_user_text {
                let user_item = Item {
                    id: format!("chat-q-{}", uuid::Uuid::new_v4()),
                    text: user_text,
                    detail: None,
                    t: 0,
                    meta: Some(serde_json::json!({"role": "user"})),
                };
                let err_item = Item {
                    id: format!("chat-a-{}", uuid::Uuid::new_v4()),
                    text: "(chat failed — please retry)".to_string(),
                    detail: None,
                    t: 0,
                    meta: Some(serde_json::json!({"role": "assistant", "error": true})),
                };
                let pair = vec![user_item.clone(), err_item.clone()];
                {
                    let mut s = state.lock().await;
                    let user_state = s.user_mut(user_id);
                    user_state.push_item_for_mode("chat", user_item);
                    user_state.push_item_for_mode("chat", err_item);
                }
                let _ = events_tx.send(UserEvent::new(
                    user_id.to_string(),
                    Event::ItemsUpdate {
                        mode: "chat".into(),
                        items: pair,
                    },
                ));
            }
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

/// Render the new transcript chunks for a fire as
/// `[Speaker N] [mm:ss] text` lines, oldest first.
fn format_chunks(chunks: &[TranscriptChunk]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(chunks.len() * 80);
    for chunk in chunks {
        let speaker = chunk
            .speaker
            .as_deref()
            .map(|s| format!("[Speaker {s}] "))
            .unwrap_or_default();
        let mm = chunk.t_start_ms / 60_000;
        let ss = (chunk.t_start_ms % 60_000) / 1000;
        let _ = writeln!(out, "{speaker}[{mm:02}:{ss:02}] {}", chunk.text);
    }
    out.trim_end().to_string()
}

/// Bootstrap section — included only on the first fire of a
/// meeting. Carries the meeting metadata (title/description, etc.)
/// and any artifacts the user attached BEFORE the first transcript
/// chunk arrived. Subsequent attaches arrive as [event] blocks
/// during normal fires.
async fn build_bootstrap_section(
    state: &Arc<Mutex<ServerState>>,
    db: &sqlx::PgPool,
    user_id: &str,
) -> Option<String> {
    let (metadata, current_meeting_id, description) = {
        let s = state.lock().await;
        match s.user(user_id) {
            Some(u) => (
                u.metadata.clone(),
                u.current_meeting_id.clone(),
                u.description.clone(),
            ),
            None => (Default::default(), None, None),
        }
    };
    let (attached_artifacts, attached_meetings) = if let Some(mid) = current_meeting_id.as_deref() {
        let a = crate::db::list_artifacts_for_meeting(db, mid)
            .await
            .unwrap_or_default();
        let m = crate::db::list_attached_meetings_for_agent(db, mid, user_id)
            .await
            .unwrap_or_default();
        (a, m)
    } else {
        (Vec::new(), Vec::new())
    };
    format_bootstrap_section(
        &metadata,
        description.as_deref(),
        &attached_artifacts,
        &attached_meetings,
    )
}

/// Pure formatter for the agent's first-fire bootstrap message.
/// Emits up to three sections (in order):
///   - `[meeting]` — sorted key/value metadata fields.
///   - `[context]` — the user's freeform meeting description, when
///     non-empty. Distinct from `[meeting]` because it's prose
///     framing rather than structured fields, and the agent benefits
///     from the relationships and intent the user typed verbatim.
///   - `[attached artifacts]` — id/name/mime/summary one per row.
///
/// Returns `None` when every section would be empty (idle state, no
/// description, no artifacts).
fn format_bootstrap_section(
    metadata: &std::collections::HashMap<String, String>,
    description: Option<&str>,
    attached: &[crate::db::ArtifactRow],
    attached_meetings: &[crate::db::AttachedMeetingMeta],
) -> Option<String> {
    use std::fmt::Write;
    let mut sections: Vec<String> = Vec::new();

    if !metadata.is_empty() {
        let mut s = String::from("[meeting]\n");
        let mut keys: Vec<&String> = metadata.keys().collect();
        keys.sort();
        for k in keys {
            let _ = writeln!(s, "  {k}: {}", metadata[k]);
        }
        sections.push(s.trim_end().to_string());
    }

    if let Some(desc) = description.map(str::trim).filter(|s| !s.is_empty()) {
        let mut s = String::from("[context]\n");
        for line in desc.lines() {
            let _ = writeln!(s, "  {line}");
        }
        sections.push(s.trim_end().to_string());
    }

    if !attached.is_empty() {
        let mut s = String::from("[attached artifacts]\n");
        for a in attached {
            let summary = a.short_summary.as_deref().unwrap_or("(summary pending)");
            let _ = writeln!(
                s,
                "  id={} name={} mime={} summary={}",
                a.id, a.name, a.mime_type, summary,
            );
        }
        sections.push(s.trim_end().to_string());
    }

    if !attached_meetings.is_empty() {
        let mut s = String::from("[attached meetings]\n");
        for m in attached_meetings {
            let ended = m
                .ended_at
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "(in progress)".to_string());
            let _ = writeln!(s, "  id={} ended={} title={:?}", m.id, ended, m.title);
        }
        sections.push(s.trim_end().to_string());
    }

    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// One-of body produced from an `AgentKick`. Different kick reasons
/// produce different prompt-block kinds: `ArtifactAttached` becomes
/// an `[event]` block (one-way notification, agent doesn't reply);
/// `ChatMessage` becomes a `[chat]` block (the agent's text response
/// is captured and surfaced as the assistant-side reply in chat
/// mode). The fire function dispatches on this to format the right
/// block label and to decide whether to keep trailing assistant text
/// in history (chat needs it; events don't).
enum KickBlock {
    Event(String),
    Chat {
        user_text: String,
        attachments: Vec<AttachmentPayload>,
    },
    /// User asked the agent to expand on a specific item.
    /// Captures the mode + item_id so the fire path knows where to
    /// write back the resulting `detail` once the agent replies.
    Expand {
        mode: String,
        item_id: String,
        item_text: String,
    },
}

impl KickBlock {
    fn label(&self) -> &'static str {
        match self {
            KickBlock::Event(_) => "event",
            KickBlock::Chat { .. } => "chat",
            KickBlock::Expand { .. } => "expand",
        }
    }

    fn body(&self) -> String {
        match self {
            KickBlock::Event(text) => text.clone(),
            KickBlock::Chat { user_text, .. } => format!("User: {user_text:?}"),
            KickBlock::Expand {
                mode, item_text, ..
            } => format!(
                "User is asking you to expand on this {mode} item: {item_text:?}. \
                 Use your conversation history (transcript, attached artifacts, your prior \
                 tool calls) to give a 2-3 sentence expansion that adds context the bare \
                 item text doesn't carry — what was happening when this came up, who said \
                 what, why it matters. Keep it tight; the user is reading this in a small \
                 inline panel."
            ),
        }
    }
}

async fn format_kick_event(db: &sqlx::PgPool, kick: &AgentKick) -> Option<KickBlock> {
    match &kick.reason {
        AgentKickReason::ArtifactAttached { artifact_id } => {
            let body = match crate::db::get_artifact_for_user(db, &kick.user_id, artifact_id).await
            {
                Ok(Some(a)) => {
                    let summary = a.short_summary.as_deref().unwrap_or("(summary pending)");
                    format!(
                        "User just attached artifact: id={} name={} mime={} summary={}",
                        a.id, a.name, a.mime_type, summary,
                    )
                }
                _ => format!("User just attached artifact: id={artifact_id} (details unavailable)"),
            };
            Some(KickBlock::Event(body))
        }
        AgentKickReason::ChatMessage { text, attachments } => Some(KickBlock::Chat {
            user_text: text.clone(),
            attachments: attachments.clone(),
        }),
        AgentKickReason::ExpandItem {
            mode,
            item_id,
            item_text,
        } => Some(KickBlock::Expand {
            mode: mode.clone(),
            item_id: item_id.clone(),
            item_text: item_text.clone(),
        }),
        AgentKickReason::MomentMarked { t_ms, note } => {
            let ts = format_ms(*t_ms);
            let body = match note.as_deref().filter(|s| !s.trim().is_empty()) {
                Some(n) => format!("User marked a moment at {ts} with note: {n:?}. The moment summary will arrive as a follow-up event once the worker finishes (~15-22 s)."),
                None => format!("User marked a moment at {ts}. The moment summary will arrive as a follow-up event once the worker finishes (~15-22 s)."),
            };
            Some(KickBlock::Event(body))
        }
        AgentKickReason::MomentSummarized {
            moment_id,
            t_ms,
            summary,
        } => {
            let ts = format_ms(*t_ms);
            Some(KickBlock::Event(format!(
                "Moment at {ts} summarized (id={moment_id}): {summary}"
            )))
        }
        AgentKickReason::MeetingAttached {
            attached_meeting_id,
        } => {
            // Resolve the attached meeting's title so the agent's
            // [event] block reads naturally ("meeting 'Q1 review'"
            // rather than "meeting 49388fb8-…"). Falls back to id
            // alone if the row is missing or the user doesn't own it.
            let body = match crate::db::get_meeting_summary_for_user(
                db,
                attached_meeting_id,
                &kick.user_id,
            )
            .await
            {
                Ok(Some(s)) => format!(
                    "User attached past meeting: id={} title={:?}. You can recall its summary \
                     via fetch_meeting_summary(id) when the transcript references it.",
                    s.id, s.title,
                ),
                _ => format!(
                    "User attached past meeting: id={attached_meeting_id} (details unavailable). \
                     You can still recall via fetch_meeting_summary(id) if it's referenced.",
                ),
            };
            Some(KickBlock::Event(body))
        }
    }
}

fn format_ms(t_ms: i64) -> String {
    let total_secs = (t_ms.max(0) / 1000) as u64;
    let mm = total_secs / 60;
    let ss = total_secs % 60;
    format!("{mm:02}:{ss:02}")
}

// ─── Kick channel ───────────────────────────────────────────────────────

/// Bytes + mime for a chat attachment. Owns the bytes so it can
/// travel through the `AgentKick` broadcast channel without
/// dipping back into the DB or filesystem.
///
/// Custom `Debug` redacts the byte vector — `tracing::info!(?kick)`
/// elsewhere in the agent loop must not spill base64 into logs.
#[derive(Clone)]
pub struct AttachmentPayload {
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for AttachmentPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachmentPayload")
            .field("mime", &self.mime)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

/// Sent on `ServerHandle.agent_kick_tx` to ask the agent loop to
/// fire immediately for a specific user, optionally with an event
/// payload that gets folded into the agent's next user-turn message
/// (so the agent sees what just happened in the conversation, not
/// just "fire now").
#[derive(Debug, Clone)]
pub struct AgentKick {
    pub user_id: String,
    pub reason: AgentKickReason,
}

#[derive(Debug, Clone)]
pub enum AgentKickReason {
    /// User attached an artifact to the active meeting. The agent
    /// task loads the artifact by id to render its name + summary
    /// into the [event] block on the next fire.
    ArtifactAttached { artifact_id: String },
    /// User sent a chat message. The agent's text response becomes
    /// the assistant-side reply, rendered alongside the user's
    /// question in chat mode (Replace strategy, single Q+A pair).
    /// Tool calls are still allowed during a chat fire — if the
    /// user asks "record this as an action," the agent emits the
    /// tool call AND the text reply.
    ///
    /// `attachments` (added 2026-05-12) carries any screenshots the
    /// user attached via the Mac compose strip. Empty for text-only
    /// chats. Bytes are loaded by the WS handler before kicking; the
    /// agent task threads them as `UserContent::Image` blocks.
    ChatMessage {
        text: String,
        attachments: Vec<AttachmentPayload>,
    },
    /// User just bookmarked a moment. Sent immediately on creation
    /// so the agent knows about it before the (15-22 s) summary
    /// worker finishes; lets users chat about a moment they just
    /// snapped without a stale "I don't know what you mean" reply.
    /// The richer summary lands as a separate `MomentSummarized`
    /// event when the worker completes.
    MomentMarked { t_ms: i64, note: Option<String> },
    /// Moment summary worker finished. Carries the summary text the
    /// LLM produced (transcript window ± screenshot synthesis) so
    /// the agent has detailed context for any later question about
    /// what was happening at that moment.
    MomentSummarized {
        moment_id: String,
        t_ms: i64,
        summary: String,
    },
    /// User clicked the chevron on an item to ask the agent to
    /// expand on it. The agent's text reply becomes the item's
    /// `detail` field, broadcast via `Event::ItemUpdated`. Same
    /// fire shape as chat — text response captured from
    /// `resp.output`, trailing-text-strip skipped.
    ExpandItem {
        mode: String,
        item_id: String,
        item_text: String,
    },
    /// User attached a past meeting to the active meeting. The agent
    /// receives the attached meeting's id + title in the [event]
    /// block; it should NOT auto-fetch on attach, just absorb the
    /// fact that the meeting is now available via the
    /// `fetch_meeting_summary` / `fetch_meeting` tools when the
    /// transcript references it.
    MeetingAttached { attached_meeting_id: String },
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
    use crate::stt::TranscriptChunk;

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

    fn chunk(t_ms: u64, speaker: Option<&str>, text: &str) -> TranscriptChunk {
        TranscriptChunk {
            id: format!("c-{t_ms}"),
            user_id: "u".into(),
            t_start_ms: t_ms,
            t_end_ms: t_ms + 1000,
            text: text.into(),
            speaker: speaker.map(Into::into),
        }
    }

    #[test]
    fn format_chunks_renders_speaker_when_present() {
        let chunks = vec![chunk(0, Some("1"), "Hello"), chunk(75_000, None, "world")];
        let out = format_chunks(&chunks);
        assert!(out.contains("[Speaker 1]"));
        assert!(out.contains("[00:00] Hello"));
        // 75s → 01:15.
        assert!(out.contains("[01:15] world"));
        // No leading "[Speaker " on the second line — speaker was None.
        assert_eq!(out.matches("[Speaker").count(), 1);
    }

    use std::collections::HashMap;

    fn artifact(id: &str, name: &str, summary: Option<&str>) -> crate::db::ArtifactRow {
        crate::db::ArtifactRow {
            id: id.into(),
            user_id: "u".into(),
            name: name.into(),
            mime_type: "text/plain".into(),
            asset_path: format!("/blobs/{id}"),
            short_summary: summary.map(Into::into),
            long_summary: None,
            summary_status: "ready".into(),
            size_bytes: 100,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn format_bootstrap_section_returns_none_when_all_empty() {
        let metadata = HashMap::new();
        assert!(format_bootstrap_section(&metadata, None, &[], &[]).is_none());
    }

    #[test]
    fn format_bootstrap_section_emits_meeting_block_for_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("project".into(), "helix".into());
        metadata.insert("host".into(), "Susan".into());
        let out = format_bootstrap_section(&metadata, None, &[], &[]).unwrap();
        assert!(out.contains("[meeting]"), "got: {out}");
        assert!(out.contains("host: Susan"));
        assert!(out.contains("project: helix"));
        // Sorted by key: host < project alphabetically.
        let host_pos = out.find("host:").unwrap();
        let project_pos = out.find("project:").unwrap();
        assert!(host_pos < project_pos);
    }

    #[test]
    fn format_bootstrap_section_emits_context_block_when_description_present() {
        let metadata = HashMap::new();
        let out = format_bootstrap_section(
            &metadata,
            Some("Quarterly review with Acme. Susan + 2 engineers."),
            &[],
            &[],
        )
        .unwrap();
        assert!(out.contains("[context]"), "got: {out}");
        assert!(out.contains("Quarterly review with Acme. Susan + 2 engineers."));
    }

    #[test]
    fn format_bootstrap_section_omits_context_when_description_none() {
        let mut metadata = HashMap::new();
        metadata.insert("project".into(), "helix".into());
        let out = format_bootstrap_section(&metadata, None, &[], &[]).unwrap();
        assert!(!out.contains("[context]"));
    }

    #[test]
    fn format_bootstrap_section_orders_meeting_then_context_then_artifacts() {
        let mut metadata = HashMap::new();
        metadata.insert("host".into(), "Susan".into());
        let attached = vec![artifact("a1", "spec.pdf", Some("Q3 plan"))];
        let out = format_bootstrap_section(
            &metadata,
            Some("Discussion of Q3 incident."),
            &attached,
            &[],
        )
        .unwrap();
        let meeting_pos = out.find("[meeting]").unwrap();
        let context_pos = out.find("[context]").unwrap();
        let artifacts_pos = out.find("[attached artifacts]").unwrap();
        assert!(meeting_pos < context_pos, "got: {out}");
        assert!(context_pos < artifacts_pos, "got: {out}");
    }

    #[test]
    fn format_bootstrap_section_indents_multiline_description() {
        let metadata = HashMap::new();
        let out =
            format_bootstrap_section(&metadata, Some("first line\nsecond line"), &[], &[]).unwrap();
        // Each prose line should be indented to match the block's
        // visual style — the agent reads the block by looking for
        // `[context]` and the indented body underneath.
        assert!(out.contains("  first line"));
        assert!(out.contains("  second line"));
    }
}

#[cfg(test)]
mod build_user_message_tests {
    use super::*;
    use rig::completion::Message;
    use rig::message::{DocumentSourceKind, ImageMediaType, UserContent};

    #[test]
    fn text_only_produces_single_text_part() {
        let msg = build_user_message("hello".to_string(), vec![]);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 1);
                assert!(matches!(parts[0], UserContent::Text(_)));
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn attachments_only_produces_image_parts() {
        let attachments = vec![
            AttachmentPayload {
                mime: "image/png".into(),
                bytes: vec![0, 1, 2],
            },
            AttachmentPayload {
                mime: "image/png".into(),
                bytes: vec![3, 4, 5],
            },
        ];
        let msg = build_user_message("".to_string(), attachments);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 2);
                for p in &parts {
                    assert!(matches!(p, UserContent::Image(_)));
                }
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn mixed_produces_text_then_images_in_order() {
        let attachments = vec![
            AttachmentPayload {
                mime: "image/png".into(),
                bytes: vec![1],
            },
            AttachmentPayload {
                mime: "image/png".into(),
                bytes: vec![2],
            },
        ];
        let msg = build_user_message("compare these".to_string(), attachments);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0], UserContent::Text(_)));
                assert!(matches!(parts[1], UserContent::Image(_)));
                assert!(matches!(parts[2], UserContent::Image(_)));
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn whitespace_only_text_skipped() {
        let attachments = vec![AttachmentPayload {
            mime: "image/png".into(),
            bytes: vec![0],
        }];
        let msg = build_user_message("   \n  ".to_string(), attachments);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                assert_eq!(parts.len(), 1, "whitespace text dropped, image kept");
                assert!(matches!(parts[0], UserContent::Image(_)));
            }
            _ => panic!("expected Message::User"),
        }
    }

    #[test]
    fn image_uses_base64_png_media_type() {
        let attachments = vec![AttachmentPayload {
            mime: "image/png".into(),
            bytes: vec![0xAA, 0xBB, 0xCC],
        }];
        let msg = build_user_message("".to_string(), attachments);
        match msg {
            Message::User { content } => {
                let parts: Vec<UserContent> = content.into_iter().collect();
                match &parts[0] {
                    UserContent::Image(img) => {
                        assert_eq!(img.media_type, Some(ImageMediaType::PNG));
                        assert!(matches!(img.data, DocumentSourceKind::Base64(_)));
                    }
                    _ => panic!("expected Image part"),
                }
            }
            _ => panic!("expected Message::User"),
        }
    }
}
