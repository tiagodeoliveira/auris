//! Agentic summarizer loop (PLAN.md §3, step 5 — v1 lean).
//!
//! Replaces the three LLM-driven per-mode summarizers
//! (highlights / actions / open_questions) with a single agent that
//! reasons per-`TranscriptChunk` batch and decides — via tool calls
//! — what to push to which mode. Lives behind the
//! `MEETING_COMPANION_AGENT_SUMMARIZER=1` env flag; default off so
//! the existing per-mode flow keeps working during the parallel-run
//! window.
//!
//! v1 scope (intentional simplifications):
//!
//! - Four tools only: `push_highlight`, `replace_highlights`,
//!   `push_action`, `push_open_question`. The `update_*` /
//!   `dismiss_*` tools from PLAN.md §3.4 require a wire-shape
//!   addition (per-event `strategy: append|replace`) for
//!   append-mode clients to handle in-place edits cleanly. Land
//!   that in v1.1.
//! - Hybrid trigger: token threshold OR silence boundary OR hard
//!   ceiling. All three env-tunable
//!   (`AGENT_TRIGGER_TOKENS=200`, `AGENT_TRIGGER_SILENCE_MS=4000`,
//!   `AGENT_TRIGGER_MAX_MS=30000`).
//! - Working context: tail-window verbatim transcript +
//!   items-as-memory (current items in all three modes) +
//!   meeting metadata. Skips mnemo `recalled_context` and
//!   attached-artifact pre-load for v1; both come in v1.1
//!   (PLAN.md §3.5).
//! - Single-turn prompt (`multi_turn(1)`) so cost is bounded:
//!   one LLM call per fire, all tool calls executed, no chained
//!   reasoning rounds. Tools just return "ok" anyway, so chain
//!   value is low at this stage.
//! - No SystemNotice trigger-buffer entries yet (e.g., "user just
//!   attached artifact X" injection). The next agent fire will see
//!   the artifact in items-as-memory once that lands; no
//!   real-time announcement.

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
5. Speak in the same language as the transcript. Don't translate.";

// ─── Tool surface ───────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum AgentToolError {
    #[error("internal: {0}")]
    Internal(String),
}

/// Shared dependencies every tool needs: the user state, the WS
/// broadcast channel, and the user_id to scope mutations and
/// emissions to. Cloning is cheap (Arc + Sender + String).
#[derive(Clone)]
struct ToolCtx {
    state: Arc<Mutex<ServerState>>,
    events_tx: broadcast::Sender<UserEvent>,
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

/// Server-side defensive dedup. Models routinely re-push the same
/// item across fires despite items-as-memory in the prompt — the
/// instruction-following just isn't strong enough on small tool-
/// calling models. We compute Jaccard similarity over the
/// significant words of the new text vs each existing item; if any
/// pair crosses the threshold, this is a duplicate. Returns the
/// `id` of the item we matched against (so the tool can return a
/// descriptive "skipped: duplicate of X" result, giving the model
/// feedback for next turn).
/// Jaccard threshold for "this is a duplicate." Calibrated against
/// the real-world paraphrase pattern observed in early agent runs:
/// 4-5 shared keywords out of ~10 total → ~0.4 Jaccard. Higher
/// thresholds (0.5+) miss obvious dupes; lower (0.3) catches false
/// positives across distinct items that happen to share generic
/// verbs ("send", "discuss"). 0.4 is the working sweet spot for
/// personal-use meeting transcripts.
const DUPLICATE_SIMILARITY_THRESHOLD: f64 = 0.4;

fn find_duplicate(new_text: &str, existing: &[Item]) -> Option<String> {
    let new_words = significant_words(new_text);
    if new_words.is_empty() {
        return None;
    }
    for item in existing {
        let item_words = significant_words(&item.text);
        if item_words.is_empty() {
            continue;
        }
        let inter = new_words.intersection(&item_words).count();
        let union = new_words.union(&item_words).count();
        if union == 0 {
            continue;
        }
        let jaccard = inter as f64 / union as f64;
        if jaccard >= DUPLICATE_SIMILARITY_THRESHOLD {
            return Some(item.id.clone());
        }
    }
    None
}

/// Lowercased words ≥4 chars. Short words ("the", "a", "of") are
/// noise for dedup; skipping them sharpens the Jaccard signal.
fn significant_words(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4)
        .map(|w| w.to_string())
        .collect()
}

/// Pull the current items in `mode` for `user_id`. Used by the
/// push_* tools' dedup check. Returns empty vec if the user has
/// no state yet (shouldn't happen when meeting is live, but safe).
async fn current_items(ctx: &ToolCtx, mode: &str) -> Vec<Item> {
    let s = ctx.state.lock().await;
    s.user(&ctx.user_id)
        .and_then(|u| u.items_per_mode.get(mode).cloned())
        .unwrap_or_default()
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
        let existing = current_items(&self.0, "highlights").await;
        if let Some(dup_id) = find_duplicate(&args.text, &existing) {
            return Ok(format!(
                "skipped: duplicate of existing highlight {dup_id} — do not push semantically-equivalent items"
            ));
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
        let existing = current_items(&self.0, "actions").await;
        if let Some(dup_id) = find_duplicate(&args.text, &existing) {
            return Ok(format!(
                "skipped: duplicate of existing action {dup_id} — do not push semantically-equivalent items"
            ));
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
        let existing = current_items(&self.0, "open_questions").await;
        if let Some(dup_id) = find_duplicate(&args.question, &existing) {
            return Ok(format!(
                "skipped: duplicate of existing question {dup_id} — do not push semantically-equivalent items"
            ));
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

// ─── Trigger loop ───────────────────────────────────────────────────────

/// Spawn the agent task. Runs for the meeting's lifetime; cancels
/// when the per-meeting cancel token fires (matches the per-mode
/// summarizer task lifecycle).
pub fn spawn_meeting_agent(
    state: Arc<Mutex<ServerState>>,
    transcript_rx: broadcast::Receiver<TranscriptChunk>,
    events_tx: broadcast::Sender<UserEvent>,
    user_id: String,
    llm: Arc<LlmClient>,
    cancel: CancellationToken,
) {
    let token_threshold = env_usize("AGENT_TRIGGER_TOKENS", AGENT_TRIGGER_TOKENS_DEFAULT);
    let silence_ms = env_u64("AGENT_TRIGGER_SILENCE_MS", AGENT_TRIGGER_SILENCE_MS_DEFAULT);
    let max_ms = env_u64("AGENT_TRIGGER_MAX_MS", AGENT_TRIGGER_MAX_MS_DEFAULT);

    tokio::spawn(async move {
        info!(
            user_id = %user_id,
            token_threshold,
            silence_ms,
            max_ms,
            "agent loop started",
        );

        let mut buffer: Vec<TranscriptChunk> = Vec::new();
        let mut tail_window: Vec<TranscriptChunk> = Vec::new();
        let mut last_fire_at = Instant::now();
        let mut last_chunk_at: Option<Instant> = None;
        let mut transcript_rx = transcript_rx;
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
                            // Token-threshold check immediately after buffer push.
                            let approx_tokens: usize = buffer
                                .iter()
                                .map(|c| c.text.len() / CHARS_PER_TOKEN)
                                .sum();
                            if approx_tokens >= token_threshold {
                                fire(&state, &events_tx, &user_id, &llm, &mut buffer, &mut tail_window).await;
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
                _ = tick.tick() => {
                    if buffer.is_empty() { continue; }
                    let now = Instant::now();
                    let silent = last_chunk_at
                        .map(|t| now.duration_since(t) >= Duration::from_millis(silence_ms))
                        .unwrap_or(false);
                    let aged = now.duration_since(last_fire_at) >= Duration::from_millis(max_ms);
                    if silent || aged {
                        fire(&state, &events_tx, &user_id, &llm, &mut buffer, &mut tail_window).await;
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

    let user_message = build_working_context(state, user_id, tail_window).await;
    let started = Instant::now();
    let ctx = ToolCtx {
        state: state.clone(),
        events_tx: events_tx.clone(),
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
                .tool(PushOpenQuestion(ctx))
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
                .tool(PushOpenQuestion(ctx))
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
                .tool(PushOpenQuestion(ctx))
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
async fn build_working_context(
    state: &Arc<Mutex<ServerState>>,
    user_id: &str,
    tail_window: &[TranscriptChunk],
) -> String {
    use std::fmt::Write;
    let mut buf = String::with_capacity(2048);

    let (metadata, highlights, actions, open_questions) = {
        let s = state.lock().await;
        match s.user(user_id) {
            Some(u) => (
                u.metadata.clone(),
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
            None => (Default::default(), Vec::new(), Vec::new(), Vec::new()),
        }
    };

    // Items-as-memory FIRST so the dedup signal is fresh when the
    // model considers the new transcript. This ordering matters:
    // small models (gpt-4.1-mini) routinely re-pushed duplicates
    // when items appeared after transcript in the prompt.
    buf.push_str("# Already recorded — DO NOT push duplicates of these\n");
    write_items_section(&mut buf, "highlights", &highlights);
    write_items_section(&mut buf, "actions", &actions);
    write_items_section(&mut buf, "open_questions", &open_questions);
    buf.push('\n');

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
    buf
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

// ─── Helpers ────────────────────────────────────────────────────────────

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

    fn item(id: &str, text: &str) -> Item {
        Item {
            id: id.into(),
            text: text.into(),
            detail: None,
            t: 0,
            meta: None,
        }
    }

    #[test]
    fn find_duplicate_catches_paraphrase() {
        // Same intent, different wording — Jaccard over significant
        // words (≥4 chars) catches it. Real-world example from the
        // first agent run where this was pushed 4 times.
        let existing = vec![item(
            "a-1",
            "Send the rebuttal exchanges related to Dell and AWS Outpost",
        )];
        let new = "Share rebuttal exchanges between Dell and AWS Outpost with the team";
        assert_eq!(find_duplicate(new, &existing), Some("a-1".into()));
    }

    #[test]
    fn find_duplicate_misses_unrelated_text() {
        let existing = vec![item("h-1", "NVIDIA reference architecture for physical AI")];
        let new = "T-Mobile is launching soft phones next week";
        assert_eq!(find_duplicate(new, &existing), None);
    }

    #[test]
    fn find_duplicate_handles_empty_text() {
        let existing = vec![item("a-1", "Long action with several words")];
        // Empty text has no significant words → no duplicate signal.
        assert_eq!(find_duplicate("", &existing), None);
        assert_eq!(find_duplicate("a", &existing), None);
    }

    #[test]
    fn find_duplicate_returns_first_match() {
        // When multiple existing items match, return the id of the
        // first one — caller doesn't need a "best match" picker.
        let existing = vec![
            item("a-1", "Testing results expected next week"),
            item("a-2", "Next week we will have testing results"),
        ];
        assert_eq!(
            find_duplicate("Testing results next week", &existing),
            Some("a-1".into())
        );
    }

    #[test]
    fn significant_words_drops_short_tokens() {
        // The/of/a noise gets stripped so dedup focuses on content.
        let words = significant_words("The cat sat on the mat");
        assert!(words.contains("cat") || !words.contains("the"));
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
