# ADR-0007: Per-mode summarizer architecture

**Status:** Partially superseded by [ADR-0011](0011-agentic-summarizer-loop.md) (highlights/actions/open_questions modes only; transcript and moment summarizers continue as decided here).
**Date:** 2026-05-03

## Context

A meeting produces a continuous stream of consolidated transcript
sentences. The user wants four distinct surfaces over that stream:

- **Transcript** — append-only list of consolidated sentences. No LLM.
- **Highlights** — 3–5 most important points so far. Replace each cycle.
- **Actions** — imperative-mood action items. Append, dedup by text.
- **Open questions** — pending Qs + clarification opportunities. Append,
  dedup by text.

Each surface has different update semantics, different LLM prompts,
different output schemas, and different cadences. Lumping them into a
single "extractor that returns a struct of everything" was rejected
during early design because:

- Replace-vs-append semantics differ per mode; one return type that mixes
  them is awkward.
- A single big prompt is harder to iterate on than three focused ones.
- Each mode has a natural cadence: actions every 15 s, highlights every
  20 s, open_questions every 15 s. A combined extractor must pick one.
- A failure in one mode shouldn't stall the others.

## Decision

- **One async task per mode**, spawned at meeting start via the
  `summarizer/{mode}.rs` modules. Each runs a `tokio::time::interval` at
  its own cadence (`HEARTBEAT_DEFAULT_MS` per module, env-overridable).
- **Each summarizer reads `state.rolling_transcript_text()` under the
  state lock**, builds a mode-specific prompt, calls
  `LlmClient::extract_with_prompt::<ModeSchema>`, and writes the result
  back through one of two state methods:
  - `push_item_for_mode(mode, item)` — append semantics (transcript,
    actions, open_questions). Internal dedup by exact text equality.
  - `replace_items_for_mode(mode, items)` — replace semantics
    (highlights). The whole list is swapped each cycle.
- **Each push emits an `Event::ItemsUpdate { mode, items }` on the broadcast
  channel.** The PWA, the mnemo pusher, and any future consumer all
  subscribe to the same event.
- **State lock is dropped before the LLM call.** The pattern is: lock,
  read transcript + existing items + recalled context, drop lock, call
  LLM, lock, write. Holding the lock across an HTTP call would block the
  WS handler and the audio pipeline.
- **Cancellation by token.** Each summarizer task takes a child of the
  meeting's `CancellationToken`. Stop_meeting cancels the parent;
  summarizers exit cleanly without flushing partial results.
- **Per-mode prior context toggle is hard-coded.** Actions and
  open_questions read `state.recalled_context` and prepend a "Prior
  context" preamble to their LLM input. Highlights does not — its
  signal is local to the current meeting.
- **Mode catalog is server-defined.** The list of modes
  (`transcript`, `highlights`, `actions`, `open_questions`) is a
  compile-time constant on the server. Modes carry server-side prompts
  and schemas; the PWA receives them via `Snapshot.available_modes` and
  renders a tab per mode.

## Consequences

**Positive:**

- Iterating on a single summarizer's prompt is a single-file change
  with its own tests. No risk of breaking the others.
- Failure isolation: a Bedrock throttle on the actions extractor doesn't
  delay highlights. Each summarizer logs and skips its own cycle.
- Cadences are tuned independently. Actions are cheap-ish (small
  schema), highlights are heavier; running them on the same cadence
  would waste tokens.
- Append vs. replace semantics live in the state methods, not in the
  summarizers. A new mode picks one.

**Negative:**

- Three concurrent LLM calls running on staggered cadences mean three
  times the API spend of a combined extractor for partly-redundant
  context. Real measurement: O($0.10) per minute of meeting at
  Bedrock rates. Acceptable for a personal project.
- The "modes are server-defined" decision means adding a new mode is a
  server release; the PWA can't add a custom mode at runtime.
- Tasks-per-meeting scales with mode count, not connection count. Four
  tasks today; if we ever shipped 20 modes, this design would feel it.

## Alternatives considered

### (a, chosen) One task per mode, separate prompts and cadences

See above.

### (b) Single combined extractor, returns a struct of all-modes

One LLM call per cycle returns `{ highlights, actions, questions }`.
Rejected: replace-vs-append can't both be expressed in a single return
shape; cadences become uniform; one failure stalls all surfaces; prompt
becomes monolithic and hard to evolve.

### (c) Mode catalog defined by the PWA

User picks which extractors run; PWA sends a `set_modes` intent. Rejected:
mode prompts are server-side and tightly coupled to the LLM client setup;
making the PWA the source of truth would require shipping prompt strings
over the wire and managing prompt versioning across both repos. The
trade-off didn't justify the flexibility.

### (d) Mode catalog dynamically loaded from a config file

Each mode is a YAML file with prompt + schema; server scans a directory
on boot. Rejected for now (this was the deferred "step 17" in the
original Phase 2 plan): introduces a runtime extension surface that
this project hasn't grown into. Revisit if the modes meaningfully
multiply or if non-author contributors start shipping their own.

## Follow-ups

- Per-mode adaptive cadence: if the transcript hasn't grown since the
  last cycle, skip the call. Highlights already does this via
  `last_seen_len`; actions and open_questions could too.
- Cost monitoring: emit token counts on each `extract_with_prompt`
  call, surface via a debug endpoint or log filter.
