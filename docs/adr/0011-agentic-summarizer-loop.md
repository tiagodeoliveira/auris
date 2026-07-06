# ADR-0011: Agentic summarizer loop — single stateful agent replaces per-mode summarizers

**Status:** Accepted
**Date:** 2026-05-07
**Context for:** server-side summarization. Supersedes [ADR-0007: Per-mode summarizer architecture](0007-summarizer-architecture.md) for the highlights / actions / open_questions modes (transcript and moment summarizers are unaffected).

## Context

ADR-0007 split the summarizer surface across three independent
heartbeat-driven LLM tasks (highlights / actions / open_questions),
each calling an Extractor on its own cadence and writing back to
state with mode-specific update strategies. After several months of
personal-use rotation, four problems compounded:

- **Triple cost per cycle.** Every ~15-20 s, three full-context
  prompts hit the provider. Most cycles produced no new items.
- **No cross-mode reasoning.** A topic surfaced as a highlight might
  also imply an action. The three calls couldn't see each other's
  output, so a stated commitment landed in highlights when it should
  have been an action.
- **Heartbeat lag.** A decision spoken at t=5s waited until the next
  cycle (~t=20s) to surface.
- **Brittle dedup.** Exact-text equality missed paraphrases. Each
  summarizer rebuilt the universe from raw transcript every cycle and
  re-extracted similar items repeatedly.

A `Vec<Item>` rendered into the prompt as items-as-memory was tried
as a fix and helped on bigger models, but small tool-calling models
(gpt-4.1-mini) still re-pushed paraphrases despite seeing them
inline. The cleanest fix was to give the agent real _conversation
memory_ — not a re-rendered list, but its own past tool calls in
chat history.

## Decision

- **One async task per active meeting**, spawned at start via
  `spawn_meeting_agent` in `summarizer/agent.rs`. Same per-user
  lifecycle as the old summarizer tasks; same `CancellationToken`
  parent.
- **Stateful conversation history.** Each meeting carries one
  `Vec<rig::completion::Message>` accumulated across the meeting's
  lifetime. Each fire passes the prior history via rig's
  `agent.prompt(...).with_history(history.clone()).extended_details()`
  and appends the returned `resp.messages` (new user/assistant/tool
  turns from this fire) back onto history. The agent's tool-calling
  history _is_ its memory of what was already pushed — no separate
  items-as-memory rendering in the prompt.
- **Delta-only fires.** Each fire's user message contains only what's
  new since the last fire: a `[transcript]` block of newly-arrived
  chunks, an `[event]` block when triggered by a kick (e.g., artifact
  attached), and a one-time `[meeting]` + `[attached artifacts]`
  bootstrap header on the first fire. No tail window, no rolling
  re-render.
- **Hybrid trigger model.** The agent fires when _any_ of these hits:
  - new-token threshold (`AGENT_TRIGGER_TOKENS`, default 200)
  - new-sentence threshold (`AGENT_TRIGGER_SENTENCES`, default 4)
  - silence boundary (`AGENT_TRIGGER_SILENCE_MS`, default 4000)
  - hard ceiling (`AGENT_TRIGGER_MAX_MS`, default 30000)
  - kick channel (e.g., user attached an artifact)
- **Tool surface, six tools:** `push_highlight`, `replace_highlights`,
  `push_action`, `push_open_question` (the items-mutating tools), plus
  `fetch_artifact_summary` and `fetch_artifact` (3-tier retrieval —
  pre-loaded short summary, fetchable long summary, fetchable full
  text for text-format artifacts).
- **Strip trailing assistant text from history.** rig's
  `extended_details` returns every message produced this fire,
  including the model's final prose. Letting that prose into history
  teaches the agent its own pattern is "respond conversationally,"
  and subsequent fires emit chat instead of tool calls. The fire
  filters trailing text-only assistant turns before appending to
  history; tool-calling reasoning chains stay intact.
- **Default model: `claude-opus-4-7`.** 1M context window at standard
  Opus pricing — the growing per-meeting history wouldn't crowd
  Sonnet's 200k budget on a long meeting; 1M gives headroom and lets
  us defer rolling-summary compression until proven needed.
- **Prompt rule: tool calls or empty turn, never prose.** The system
  prompt's first section explicitly forbids conversational
  acknowledgments. Combined with the history-strip filter, this kept
  the agent on-task across a real test meeting.

The transcript-mode pass-through summarizer stays as-is (no LLM,
just re-emits chunks as items). The moment summarizer is unaffected
(separate one-shot pipeline).

## Consequences

**Wins:**

- Single LLM call per meaningful state change (instead of three on
  every heartbeat).
- Cross-mode reasoning: the agent decides "this is an action, not a
  highlight" with one global view.
- Sub-second reaction to chunks (vs ~15s heartbeat lag).
- Dedup is a natural property of stateful reasoning; the prior
  Jaccard fallback was deleted.
- Mid-meeting events (artifact attach, future delete/edit) are
  first-class conversation turns the agent reasons about, not just
  triggers.

**Costs / risks:**

- Cost-per-fire grows with history. A 60-min meeting fires ~200
  times; later fires carry more context tokens. Opus 4.7 input is
  $5/Mtok — a chatty meeting might end at $1-3, vs ~$0.30 on the old
  per-mode flow. Acceptable for personal use.
- One bug type traded for another: the old flow could miss things the
  new flow's "EMIT NOTHING WHEN NOTHING NEW" instinct also skips.
  Calibrating the prompt's emit-vs-skip bias is content-dependent.
- Provider dispatch in `fire` is verbose — three near-identical
  match arms over `LlmBackend` for Bedrock / OpenAI / Anthropic.
  rig's `Agent<M>` is generic over the model type, so trait-object
  shortcuts don't compose cleanly. Accepted as the cost of the
  multi-provider abstraction.

## Alternatives considered

- **Keep per-mode summarizers, just upgrade the model.** Cheaper to
  ship but doesn't fix cross-mode reasoning or heartbeat lag.
- **Per-mode summarizers + shared Extractor.** Splits the cost
  problem partially but keeps the lag.
- **Single Extractor returning all three modes.** Considered in
  ADR-0007 and rejected then for the same reason it would still be
  wrong: replace-vs-append semantics differ per mode, and each mode
  has a natural cadence the combined call can't honor.
- **Custom completion loop instead of rig's
  `.prompt().extended_details()`.** Considered to gain finer control
  over message handling. Rejected: rig already handles the empty-
  terminal-turn edge case and tool-result message construction
  correctly. Re-implementing those is footgun-prone for marginal
  gain.
- **Pass full chat history every fire vs items-as-memory.**
  Items-as-memory was the previous compromise (compress past output
  into a small structured list). Rejected for the new design because
  the bigger 1M-context model can afford full history, and full
  history is qualitatively better — the agent sees its own reasoning
  process, not just its outputs.

## Follow-ups

- **Event injection for user delete/edit.** Originally scoped as
  "Option C": when the user removes or edits an item via UI, inject
  `[event] User removed action #abc: '…'` into the agent's history
  so its memory stays in sync with reality. Deferred — no
  delete/edit UI exists today. The `AgentKick` channel is the
  hook-point when that UI ships.
- **Rolling-summary compression.** When (if) a meeting's history
  approaches 1M tokens, compress the older portion into a summary
  message. Not built; revisit when real usage hits the ceiling.
- **Prompt caching.** rig 0.36's Anthropic provider supports
  `prompt-caching` via `anthropic_beta`. The system prompt + early
  history would cache cleanly. Real cost win on long meetings.
  Revisit after we have a few weeks of usage data.
- **Token-accurate cost tracking.** `LlmClient::record_usage`
  currently takes char counts; rig's `PromptResponse.usage` carries
  real token numbers we already log. Wire those through the per-user
  counter for accurate budget reporting.
- **Content-mode prompt variants.** The current prompt is calibrated
  for working meetings (5-10 actions, 3-7 questions, 2-5 highlights
  for a 30-min call). Interview-style content produces mostly
  highlights and few questions/actions because Q&A resolves
  questions inline. A future "meeting type" hint could swap prompt
  variants. Defer until personal-use signal warrants.
