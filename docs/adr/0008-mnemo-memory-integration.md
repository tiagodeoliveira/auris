# ADR-0008: mnemo memory integration — streaming push, summary at stop, recall at start

**Status:** Accepted
**Date:** 2026-05-04
**Context for:** server-side `mnemo/` module; PWA memory badge in
[`UX.md`](../UX.md).

## Context

The author runs a personal memory layer ([mnemo](https://github.com/tiagodeoliveira/mnemo))
that backs onto AWS Bedrock AgentCore Memory. mnemo extracts preferences,
facts, episodes, and project-scoped memories from conversational data
across all the user's tools (Claude Code, Codex, etc.). The meeting
companion is a natural producer and consumer of this same memory.

Three integration questions had to be answered:

1. **Granularity of pushes.** One per meeting? One per mode? One per
   sentence? Each has different latency, cost, and recall-quality
   tradeoffs.
2. **Recall scope per meeting.** Which mnemo dimensions (preferences /
   facts / episodes / project / task) does an in-progress meeting want
   as context? Which ones make the LLM extractors _better_ vs. _noisier_?
3. **mnemo API stability.** mnemo's read API is shaped around pre-defined
   dimensions and doesn't yet support per-record metadata filtering.
   Pushing meeting-specific metadata is possible (mnemo has a generic
   `attributes` bag on ingest), but exploiting it on read requires a
   new mnemo extraction strategy.

## Decision

- **Streaming push, sentence-by-sentence.** When the soniox adapter
  promotes a buffered transcript span to an `Item`, the mnemo pusher
  diffs against the previously-pushed count and fires one
  `POST /events` per new sentence with a single `user`-role turn. All
  pushes for one meeting share the same `sessionId` (a UUID generated
  at meeting Active).
- **One final summary push at meeting stop.** Bundle non-empty mode
  outputs (`actions`, `highlights`, `open_questions`) into one event of
  `assistant`-role turns. Transcript is omitted from the bundle — those
  items already streamed. No push when every mode is empty.
- **Recall at meeting Active.** One `GET /recall?facts=true&preferences=true&episodes=true&project=<if any>`
  populates `state.recalled_context`. Failures log warn and leave the
  field empty; summarizers degrade to "no prior context" prompts
  unchanged.
- **Re-recall on project change mid-meeting.** Tracked in the recaller
  task's local state; if the user edits `metadata.project` during an
  active meeting and the new value differs, fire a fresh recall. Other
  metadata edits do not trigger.
- **Per-mode prior-context consumption is hard-coded.** Actions and
  open_questions read `state.recalled_context` and prepend a "Prior
  context" preamble to their LLM input. Highlights does not — local
  signal only.
- **Direct HTTP, no CLI shell-out.** The Rust server uses `reqwest`
  with `x-api-key` header, mirroring mnemo's CLI request shape but
  bypassing the Node.js binary.
- **Generic `attributes` bag passes all metadata through unchanged.**
  This is mnemo's chosen extension point: future mnemo strategies can
  consume the bag without an API contract change. The companion's only
  promotion-into-typed-fields is `metadata.project → context.project`
  (so mnemo's existing project-scoped extraction works out of the box).
- **Two cancellation tokens.** `meeting_cancel` covers audio, STT,
  summarizers; `extraction_cancel` covers in-flight LLM-extraction calls.
  An idle-time `ExtractMetadata` survives `start_meeting`; an
  `extraction_cancel` is taken on stop so a stale recall doesn't pollute
  the next meeting's empty state.
- **Disabled by default.** When `AURIS_MNEMO_URL` /
  `AURIS_MNEMO_API_KEY` are unset, the client returns
  `Disabled` and `spawn_tasks` early-returns. No HTTP, no broadcast
  subscriber, zero overhead.

## Consequences

**Positive:**

- Recall populates the LLM extractors with real cross-meeting context.
  Actions and open*questions visibly improve at extracting what's \_new*
  vs. recapping what mnemo already knew.
- Per-sentence streaming + one summary push composes naturally with
  Bedrock AgentCore's extraction model: raw turns become facts and
  episodes; distilled summaries become more focused episode records.
- The `attributes` bag means we can ship richer metadata today (title,
  owner, …) and reap the benefits when mnemo grows attribute-aware
  extraction; no code change on the meeting side will be needed.
- Failure modes are bounded: an HTTP timeout drops one sentence push
  but doesn't stop the meeting; a bad recall leaves `recalled_context = None`
  and summarizers run their plain prompts.
- Disabled-by-default makes the integration trivially opt-in. CI and
  unit tests don't see mnemo at all.

**Negative:**

- mnemo currently has no `meeting_id` dimension, so per-meeting recall
  ("show me everything from the SMB demo") isn't possible without a
  mnemo strategy change. The companion ships `attributes.meeting_id` for
  forward compatibility but cannot exploit it on read yet.
- Each meeting produces O(N) HTTP calls where N = sentences. Typical
  rate ~10/min, well below mnemo's 10 req/s limit. A high-cadence meeting
  could stress this.
- The `extraction_cancel` slot is a second, parallel cancellation
  lifeline; conceptually clean but easy to mismanage if a third
  extraction-like task is added later.

**Accepted risks:**

- mnemo's API shape is owned by another repo. A breaking change on
  mnemo's side requires coordinated work. Mitigated by matching the
  CLI's exact wire shape, so we evolve together.
- Bedrock AgentCore extraction is best-effort over conversational data;
  the _quality_ of recall depends on its strategy decisions, not ours.

## Alternatives considered

### (a, chosen) Streaming push + summary push + recall at start, all generic

See above.

### (b) Single push at meeting stop with everything

Bundle the full transcript + all summaries into one `POST /events` at
stop. Rejected: doesn't leverage AgentCore's streaming-friendly
ingestion model; loses the ability for cross-meeting recall during a
long meeting; one big payload is harder to debug than many small ones.

### (c) Push only summaries, never transcript

Cheaper, less noisy memory. Rejected: AgentCore's facts/episodes
extractors operate over raw conversational data — a summary alone is
already-distilled and produces shallower extraction. Raw turns are
where the value lives.

### (d) Run mnemo as a sub-process via the CLI

Shell out to `mnemo push`. Rejected: adds a Node.js dependency to the
Rust server's runtime; the CLI's value-add (cursor file dedup) is
covered in-process by our `transcript_pushed` count. Direct HTTP keeps
the deployment simple.

### (e) Meeting-specific extraction via a new mnemo strategy

A `meetings/{actorId}/{meetingId}/` namespace with its own extraction
lambda. Rejected for _this_ phase: mnemo's strategy layer is complex
enough that a meeting-specific addition couldn't ship in one pass with
the Auris changes. The companion is forward-compatible
(`attributes.meeting_id` is sent today) so the strategy can be added
later without a contract change.

## Follow-ups

- Meeting slug as `attributes.meeting_id`: a stable, human-readable
  identifier (`YYYY-MM-DD-<project>-<title-slug>`) that survives
  AgentCore extraction by appearing inline in turn content. Useful as a
  bridge until mnemo supports per-meeting recall properly.
- mnemo's strategy layer: when it learns to read `attributes`, the
  Auris can stop content-embedding the slug.
- Recall on a longer time window: today the recall is a single call at
  meeting Active. Re-recall on project change is implemented; mid-meeting
  refresh on transcript-content drift could be added if the existing
  context becomes stale during a long meeting.
