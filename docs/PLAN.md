# Meeting Companion — Forward Plan

What's next, in order of priority. Past phases are captured in
git history and `docs/adr/`. The current shape of the system lives in
`docs/ARCHITECTURE.md`. This file churns; ADRs are durable.

---

## 1. Status

The local-first MVP is shipped and in personal-use rotation:

- Native Mac menu-bar app + browser PWA, both Auth0-authenticated.
- Per-user data isolation, Postgres persistence, boot recovery for
  meetings interrupted by a server crash.
- Server-mediated STT (the PWA and Mac dictation both flow through
  `/stt`, no provider keys leave the server).
- Moments with screenshots and vision-capable LLM summaries.
- mnemo memory integration (streaming push + recall at start).
- Container-shaped server (`docker compose up`) — same image for
  local dev and any future production host.

The roadmap from here is shaped by personal-use signal. Three forward
themes, in priority order.

---

## 2. Principles

1. **Local-first dev path stays alive forever.** Single-machine
   end-to-end is supported through every change. CI must run without
   cloud auth, mnemo, or external STT.
2. **Disabled-by-default for cloud features.** Local dev is unaffected
   when env vars are unset.
3. **Standalone-first, additive pairing.** Mac and PWA each work on
   their own; using both together is opt-in, never required.
4. **Capability over identity.** Clients declare capabilities
   (`audio_capture`, `screen_capture`, `control_surface`); a meeting's
   roles are filled by capability-bearers, not device types.
5. **Container-anywhere, deploy-portable.** The server is a single
   Docker image. Local and production use the same image; configuration
   is env-only.
6. **No throwaway code.** Each shipped piece feeds the next. Phases
   that ship get distilled into ADRs and lose their entry here.

---

## 3. Agentic summarizer loop (next)

Replace the three independent per-mode summarizers (highlights,
actions, open_questions) with a single agent that reasons about each
new transcript chunk and decides — if anything — what to push to
which mode. This is the next thing to build.

### 3.1 Why

Today: three tasks, each on its own heartbeat (15-20 s), each making
a fresh LLM call against the full rolling transcript every cycle, each
with a hand-tuned dedupe-by-exact-text gate. Three problems compound:

- **Triple LLM cost per cycle.** Every ~15 s we run three full-context
  prompts. Most cycles produce nothing new.
- **No cross-mode reasoning.** A topic the model surfaces as a
  highlight might also imply an action item. Today neither call sees
  the other's output, so they can't coordinate.
- **Heartbeat lag.** A decision spoken at t=5 s gets picked up at
  t=15-20 s on the next cycle. With chunk-driven reasoning the model
  could react within seconds of the chunk landing.
- **Dedupe is fragile.** Exact-text equality on action items / open
  questions misses paraphrases; a smarter agent that _remembers what
  it already extracted_ dedupes by intent, not string equality.

### 3.2 Shape

One `meeting_agent` task per active meeting (per user, same per-user
lifecycle as today's summarizers). Subscribes to the
`TranscriptChunk` broadcast the transcript summarizer already drains.

The agent reasons about new transcript material and decides — via
tool calls — whether to push, update, dismiss, or replace items in
any of the three LLM-driven modes (highlights, actions,
open_questions). Each tool call translates to the same
`Event::ItemsUpdate` we broadcast today; the wire shape and clients
are unchanged.

The transcript-mode pass-through summarizer stays as-is — it's
non-LLM and just promotes chunks to items.

Tool calling (not structured output) is the v1 contract with the
LLM: "do nothing this turn" is a free signal (just emit no tool
calls) and the model can fire multiple tool calls per turn (e.g.,
dismiss two stale items + push one new one to merge them).

### 3.3 Trigger model — hybrid

Per-chunk invocation is too expensive (5-10× current cost). The
agent fires when **any** of these conditions hits, whichever comes
first:

- **Token threshold.** The chunks accumulated since the last
  invocation reach ~`AGENT_TRIGGER_TOKENS` tokens (start: 200; tunes
  with real-meeting data).
- **Silence boundary.** ≥`AGENT_TRIGGER_SILENCE_MS` of no incoming
  chunks (start: 4000 ms — natural conversational pause).
- **Hard ceiling.** ≥`AGENT_TRIGGER_MAX_MS` since the last
  invocation (start: 30000 ms — caps latency on long monologues
  where neither token-threshold nor silence triggers).

All three thresholds live behind env vars so we can tune without a
rebuild during early personal use. After invocation the buffer
clears; the next batch starts accumulating.

### 3.4 Tool surface

Per LLM-driven mode, the agent gets push / update / dismiss. For
highlights, replace covers the "reorganize the whole list" case
(matches today's replace strategy). Eight tools total:

| Mode             | Tools                                                                 |
| ---------------- | --------------------------------------------------------------------- |
| `highlights`     | `push_highlight`, `replace_highlights`                                |
| `actions`        | `push_action`, `update_action`, `dismiss_action`                      |
| `open_questions` | `push_open_question`, `update_open_question`, `dismiss_open_question` |

Tool shapes:

- `push_highlight { text, importance? }`
- `replace_highlights { items: [{ text, importance? }] }`
- `push_action { text, owner?, due? }`
- `update_action { id, text?, owner?, due? }` — partial; only changed fields
- `dismiss_action { id, reason? }` — for retracted / completed items
- `push_open_question { question, kind?, context? }`
- `update_open_question { id, question?, kind?, context? }`
- `dismiss_open_question { id, reason? }`

Merge across duplicates is just `dismiss(id_a) + push(merged_text)` —
two tool calls in the same turn — so no explicit merge tool.

The retrieval tools (`fetch_artifact_summary`, `fetch_artifact`,
`mnemo_query`) live in §3.8 and ship with this same release.

**Implementation contracts:**

- **IDs on `push_*`.** Push tools take no `id` argument. The server
  mints the ID (`<mode-prefix>-<uuid>`, matching today's pattern)
  when processing the tool call. LLMs hallucinate UUIDs badly; this
  keeps the agent honest by removing the failure mode entirely.
- **Unknown IDs on `update_*` / `dismiss_*`.** When the LLM hands a
  non-existent `id` to an update or dismiss tool, the server returns
  a descriptive error tool result (`"no such item: a-fake"`) — not a
  silent no-op. rig's tool-result protocol surfaces the error back
  to the model, which self-corrects on its next turn. Silent
  failures break that feedback loop and the agent never learns.
- **Item shape in the prompt.** `Item.meta` carries action owner /
  due as a nested object on the wire. Flatten it when formatting
  items-as-memory for the agent — `{id, text, owner, due}`, not
  `{id, text, meta: { owner, due }}`. Same data, fewer tokens,
  cleaner for the LLM to reason about. Wire shape stays as-is; this
  is a presentation concern in the prompt assembler.

### 3.5 Working context

What the agent sees on every invocation:

| Component                        | Always | Notes                                                                                                                                                               |
| -------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Tail-window transcript           | ✓      | Recent N chunks verbatim, with `[Speaker N, mm:ss]` prefixes when diarization is available (see §3.7).                                                              |
| Rolled-up summary of older turns | ✓      | Empty until the tail crosses N; then a 1-paragraph compress of displaced chunks, re-compressed on each rollover.                                                    |
| Current items in all three modes | ✓      | Items-as-memory: dedup signal, refine-vs-push signal, cross-mode coordination signal.                                                                               |
| Meeting metadata                 | ✓      | Project, title, owner — frames the conversation.                                                                                                                    |
| `recalled_context` from mnemo    | ✓      | Same shape today's actions / open_questions summarizers consume. Capped at a token budget to avoid mnemo overload (start: 1500 tokens, drop oldest episodes first). |
| Tool descriptions                | ✓      | Required for tool calling.                                                                                                                                          |

Context rollover (when the tail crosses N):

- **Tail-window verbatim.** Keep the most recent N chunks in full
  (start with N=80, ~5-10 minutes of speech).
- **Summarized prefix.** Summarize the _displaced_ chunks into the
  rolling summary slot. Each rollover re-compresses summary +
  newly-displaced chunks into the same slot.
- **Items-as-memory.** The mode buffers themselves act as long-term
  memory; the agent doesn't need raw transcript to remember what's
  already been extracted.

The mnemo budget is the load-bearing knob here. mnemo can return
substantial prior-meeting episodes and we don't want them to
dominate the prompt; if the cap repeatedly clips relevant context,
that's a signal we want a new mnemo memory mode (separate work,
mnemo-side change).

**Trigger buffer composition.** The buffer that drives the trigger
thresholds (§3.3) is a tagged union, not a flat `Vec<TranscriptChunk>`:
it carries both transcript chunks (from the STT pipeline) and
`SystemNotice` items (e.g., the "user attached artifact X"
notification from §3.7). Both contribute to the token-threshold
count and both reset the silence timer; both appear in the
tail-window when the agent fires, with system notices formatted as
`[system, mm:ss] message` to distinguish them from speaker turns.
A small refactor of whatever channel / buffer shape the agent task
uses internally — called out so it isn't a surprise during
implementation.

### 3.6 Boot recovery

Agent state lives in memory only. On a server crash + boot recovery:

- The current items in each mode survive (rebroadcast from
  `UserState`).
- The rolled-up summary is gone.
- The tail-window verbatim is gone.

The agent restarts cold with items-as-memory as its only context.
Acceptable failure mode: the agent might double-push the most recent
item or two on a recovered meeting. Revisit only if quality
degrades — persisting the rolled summary to a transcript-blob
sidecar JSON is a small additive change when needed.

### 3.7 Artifact subsystem

Per-meeting documents and images, library-first. The user uploads
to a personal artifact library; only previously-uploaded library
artifacts can be attached to meetings. Always a two-step flow
from the user's perspective: upload, then attach.

#### Lifecycle

1. User uploads a file (Mac or PWA). Server stores bytes under
   `<DATA_DIR>/blobs/artifacts/<user_id>/<artifact_id>`.
2. Server async-summarizes via the LLM client. One call produces
   both the `short_summary` (~50 tokens, for pre-load) and the
   `long_summary` (~500 tokens, for the long-summary tool).
   Native attachment via rig's `Document` / `Image` content types
   — no pre-extraction needed for PDFs, markdown, or images.
3. Library row transitions `summary_status: pending → done` when
   the worker finishes (or `failed` if the LLM call gives up).
4. User selects from the library (pre-meeting compose or
   mid-meeting picker). Server writes a `meeting_artifacts` row.
5. Server broadcasts `Event::ArtifactAttached { artifact_id, name,
short_summary }` so all WS clients update their UI.
6. If the meeting is active, the server also pushes a synthetic
   system chunk into the agent's trigger buffer announcing the
   attachment (see §3.5 working context). The agent decides on
   its next trigger whether to act on it.

#### Schema

```sql
CREATE TABLE artifacts (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    mime_type       TEXT NOT NULL,
    asset_path      TEXT NOT NULL,            -- relative to <DATA_DIR>/blobs/
    short_summary   TEXT,                     -- ~50 tokens, pre-load
    long_summary    TEXT,                     -- ~500 tokens, fetch_artifact_summary
    summary_status  TEXT NOT NULL DEFAULT 'pending',  -- pending | done | failed
    size_bytes      BIGINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_artifacts_user ON artifacts(user_id, created_at DESC);

CREATE TABLE meeting_artifacts (
    meeting_id   TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    artifact_id  TEXT NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    attached_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (meeting_id, artifact_id)
);
```

#### Format support (v1)

Native via rig 0.36's `UserContent`:

| Family | MIME types                                                                 | rig type                                                              |
| ------ | -------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Text   | `text/plain`, `text/markdown`, `text/html`, `text/csv`, `application/json` | `UserContent::document` (string)                                      |
| PDF    | `application/pdf`                                                          | `UserContent::document_raw` with `DocumentMediaType::PDF`             |
| Image  | `image/png`, `image/jpeg`                                                  | `UserContent::image_base64` (reuses moment infra)                     |
| Code   | per-language MIME or extension                                             | `UserContent::document` with the matching `DocumentMediaType` variant |

Out of scope for v1: `.docx`, `.pptx`, Google Docs / Sheets links,
audio. Added if a real need shows up.

#### REST endpoints

- `POST /api/artifacts` — multipart upload. Returns the artifact
  row immediately with `summary_status: pending`. Server kicks off
  the summarization worker async.
- `GET /api/artifacts` — list the user's library, newest first.
- `GET /api/artifacts/:id` — one artifact's metadata + summaries.
- `DELETE /api/artifacts/:id` — remove from the library. Cascades
  to `meeting_artifacts`; past meetings lose the attachment but
  keep living. Blob deleted from disk.
- `POST /api/meetings/:id/artifacts` — attach a library artifact
  to a meeting. Body: `{ artifact_id }`. Server enforces that the
  artifact is `summary_status: done` before allowing attach.
- `DELETE /api/meetings/:id/artifacts/:artifact_id` — detach.

### 3.8 Retrieval tools

Three new tools available to the agent. All read-only against
external state (none mutate the LLM-driven mode buffers).

#### `fetch_artifact_summary { id }`

Returns the artifact's `long_summary` as the tool result text. No
conversation-history mutation. Cheap. The agent uses this for
"this attachment looks relevant, give me more than the pre-load
summary" without committing to the full doc.

Unknown id → `"no such artifact: <id>"` (same self-correction
contract as the mode tools — see §3.4).

#### `fetch_artifact { id }`

Side-effecting. Tool result text: `"Artifact 'X' attached to your
context. Reference it directly in your reasoning."`

The runtime intercepts this tool call: fetches the artifact's
bytes from `<DATA_DIR>/blobs/...`, then appends a synthetic
`Message::User` to the agent's conversation history with the
content as a `Document` (PDF / markdown / code) or `Image`. On the
agent's next trigger fire, the model sees the actual content
natively — PDF pages, image, markdown — not extracted text.

The attachment stays in conversation history for the rest of the
meeting. Eviction (e.g., LRU on attached docs older than N turns)
is additive; design it in only if cost instrumentation flags it.

#### `mnemo_query { ... }`

Passthrough to mnemo's recall API, mirroring its existing
parameter shape exactly:

```
mnemo_query {
  preferences?: bool,
  facts?: bool,
  episodes?: bool,
  about?: bool,             // mnemo's about-me mode (in flight)
  project?: string,         // narrow to /projects/{actorId}/{name}/
  task?: string,            // narrow to /tasks/{actorId}/{domain}/
  date?: string             // YYYY-MM-DD, /daily/{actorId}/{date}/
}
```

Multiple parameters combine in one call (`preferences=true +
project=helix`). Result text capped at ~1500 tokens — same budget
as the always-on `recalled_context`, oldest episodes dropped first
when over budget. The agent uses this for "what do we know about
project X" / "what was on yesterday's log" / "what does the user
prefer" queries beyond what's pre-loaded.

The `about` flag tracks mnemo's in-flight about-me work. The
client-side `RecallDimensions` struct in `mnemo/recall.rs` adds
the field; the tool exposes it; the field becomes useful when
mnemo populates that namespace. Not a blocker — landing the tool
without it is fine; flipping it on is a one-line change.

**Server-side change — `RecallDimensions` extension.** Today's
`packages/server/src/mnemo/recall.rs` defines `RecallDimensions`
with `preferences`, `facts`, `episodes`, `project`. The
`mnemo_query` tool requires extending this struct with three new
fields: `task: Option<String>`, `date: Option<String>`, and
`about: bool`. The `to_query_string()` impl gets matching cases.
Trivial in isolation (~10 lines) but called out so it isn't lost
inside the larger §3.12 ordering — it's a precondition for the
`mnemo_query` tool, not an implementation detail of it.

### 3.9 UX surfaces

#### Library management

- **Mac**: Settings → "Artifacts" tab. List view (name + short
  summary preview + size + `summary_status` badge), drag-and-drop
  or "Upload…" button, per-row delete.
- **PWA**: dedicated modal triggered from a new header icon (next
  to the meetings list icon). Same list / upload / delete flow,
  responsive to phone viewport.

#### Pre-meeting attach (compose panel)

A chip strip below the description textarea, mirroring the
existing metadata chips pattern:

- Chips show currently-attached artifacts by name. Tapping
  removes (with confirm on PWA to avoid fat-finger removals).
- A `+ Artifact` button opens a picker (Mac sheet / PWA
  sub-modal) showing the full library alphabetically. Multi-select
  with checkboxes; confirm to attach.
- Picker only allows selecting `summary_status: done` rows;
  `pending` items show a spinner and are unselectable; `failed`
  items show a retry button.

#### Mid-meeting attach (during a live meeting)

The overlay's `…` menu gets an "Attach artifact…" entry that
opens the same picker. Less discoverable than a dedicated icon
but cheaper UI real estate; promote to a 📎 paperclip icon in the
overlay toolbar if mid-meeting attach turns out to be a common
gesture in personal use.

#### Mid-meeting upload + attach

Always two steps:

1. Upload to the library (via Mac Settings → Artifacts or PWA
   header-icon modal, even while a meeting is live).
2. Open the mid-meeting picker, select the just-uploaded
   artifact, confirm.

Intentional: keeps a single upload code path, lets the async
summarizer finish before the agent can attach (so `pending` items
can't be attached prematurely), and the artifact's `pending →
done` transition is visible in the library list.

### 3.10 Precursor — Soniox speaker diarization

Soniox supports streaming speaker diarization; we just don't enable
it. The plumbing is already in place: `TranscriptChunk.speaker:
Option<String>`, `Token.speaker: Option<String>` deserialize from
the API response.

To turn it on:

1. Add `enable_speaker_diarization: true` to `ConfigFrame` in
   `packages/server/src/stt/soniox.rs`.
2. Aggregate per-token `speaker` values when emitting a chunk
   (most-common-token wins; multi-speaker turns get a `mixed` label
   or a `1→2` transition marker).
3. Thread the result into `TranscriptChunk.speaker` (currently
   hardcoded `None` at `soniox.rs:292`).

Soniox returns anonymous IDs (`"1"`, `"2"`, …) — exactly the
"distinguish without labeling" outcome we want. The agent's prompt
then formats the tail window as `[Speaker N, mm:ss] text`. Better
attribution for actions ("the person who proposed shipping by
Friday") at marginal token cost.

Lands as a separate small commit before the agent core ships —
cheap to verify on its own and the agent design assumes it's
present.

### 3.11 Cost instrumentation

Per-meeting LLM-call and token counters logged at meeting stop —
not for comparison against the per-mode summarizers (we know the
agent will be different; we're not A/B-testing) but as a permanent
operational signal. Useful for:

- Spotting trigger-threshold mistunes (a runaway meeting pushes
  cost 10× normal).
- Sanity-checking provider switches (Bedrock vs Anthropic-direct
  cost shape).
- Detecting artifact attachment cost drift (a fetched PDF that
  stays in context for a long meeting is a real cost driver).
- Future per-user budgeting if multi-tenancy ever hardens.

Implementation: a `LlmUsageCounter` in `llm.rs` that each
`extract_with_prompt[_and_image]` call increments. Emit
`tracing::info!(calls, tokens, "agent usage at stop")` on
`stop_meeting`. ~30 lines.

### 3.12 Bundled release ordering

The agent loop, artifact subsystem, and `mnemo_query` tool ship as
one bundled release — we're not versioning sub-features. v1 has
all three from day one, behind a single feature flag.

Order of work within the release:

1. **Soniox speaker diarization** (§3.10). Cheap precursor; the
   agent design assumes it. Lands independently first.
2. **Cost instrumentation** (§3.11). Independent quick add to
   `llm.rs`; useful from day one for tuning and operational
   awareness.
3. **Artifact subsystem backend** (§3.7). Schema migration, REST
   endpoints, blob storage, async summarization worker. Ships
   without UI; testable via `curl`. Adds the
   `Event::ArtifactAttached` wire event and the synthetic-chunk
   injection mechanism (see §3.5).
4. **Artifact UI** (§3.9 — Mac Settings tab + PWA modal +
   compose-panel chip strip + mid-meeting `…` attach). Fills in
   the user surface; backend already works.
5. **Agent core**. New `summarizer/agent.rs` with the mode tools
   from §3.4 (push / update / dismiss × 3 modes +
   `replace_highlights`). Spawn site is the existing
   `spawn_live_pipeline` in `ws.rs` — replaces the three
   LLM-driven summarizer-task spawns with one agent task when
   `MEETING_COMPANION_AGENT_SUMMARIZER=1`.
6. **Retrieval tools** (§3.8). `fetch_artifact_summary`,
   `fetch_artifact` (with the conversation-history append
   pattern), `mnemo_query` (passthrough to the existing mnemo
   client, with the `about` field added in `RecallDimensions`).
7. **Parallel run.** Behind the feature flag, default off. Run
   alongside the existing per-mode summarizers for a couple of
   weeks of personal use.
8. **Cleanup.** Delete the three per-mode summarizer modules
   (`summarizer/{actions,highlights,open_questions}.rs`). Keep
   `transcript.rs` (not LLM-driven) and `moment.rs` (separate
   pipeline, unaffected). The dedicated tests for each summarizer
   become agent-level integration tests.

Step 1 lands as a separate commit before the rest. Steps 2-6
overlap as makes sense; a sensible commit topology is one commit
per backend chunk, one per UI surface, then the agent core +
tools as a coherent unit, then the parallel-run flag flip.

---

## 4. Horizontal scale (future)

Today the server runs as a single replica. Postgres takes care of the
durable surface, but a lot of _active_ meeting state still lives only
in the replica's RAM. Two scales worth distinguishing.

### 4.1 What pins a session to one replica today

Friends-and-family scale (one VM, vertical scale): fine. Postgres
covers the durable surface; the server itself just needs to keep
running.

The blockers to running N replicas behind a plain round-robin LB:

- **`ServerState` is per-process.** `HashMap<UserId, UserState>` —
  meeting state, items per mode, devices, rolling transcript,
  recalled context. Two browser tabs hitting different replicas would
  see different "active meeting" state.
- **Pipelines run in-process.** `start_meeting` spawns a Soniox WS,
  four summarizer tasks, an audio source, mnemo pusher/recaller — all
  on the receiving replica. Other replicas don't know it exists.
- **`/audio` must hit the same replica as `start_meeting`.** Audio
  routes via an in-memory `audio_sources: HashMap<UserId, Arc<RemoteAudioSource>>`.
  Frames on the wrong replica get dropped.
- **In-process broadcast bus.** `events_tx` only fans out within one
  replica. A `mark_moment` on replica A doesn't reach the
  screenshot-capable Mac connected to replica B.
- **Blobs on local disk.** Transcript JSONL and moment screenshots
  live under `<DATA_DIR>/blobs/...` on the replica that wrote them.
- **Boot recovery double-spawns.** If both replicas restart together,
  each picks up unfinished meetings → duplicate STT sessions, billed
  twice.

### 4.2 The cheap step — sticky sessions

Cost: a couple of hours. Buys us N replicas where each user's traffic
always lands on the same replica.

- LB does consistent-hash on the JWT's `sub` (or on the `?token=`
  query param it terminates and inspects). Cloudflare, ALB, Traefik,
  Caddy all support this.
- Per-replica blob storage stays per-replica — but each user's
  blobs always live on their replica, so reads work.
- Boot recovery still races on full-fleet restart; mitigate with a
  Postgres advisory lock keyed on `user_id` so only one replica
  resurrects a given pipeline.

This is the right move when "one VM is at capacity" first becomes
real. Most of the code stays as-is.

### 4.3 The full step — stateless replicas

Cost: a couple of weeks. Replicas become disposable; any of them can
serve any user.

- **Blob storage moves to S3** (R2 in our cost profile). Implement
  `S3BlobStore` and switch by env. Local dev keeps `FilesystemBlobStore`.
- **Active state moves out of `ServerState`'s in-memory map.** Two
  options:
  - (a) Keep in-memory, but have replicas coordinate via Postgres
    advisory locks — only one replica is "owner" of a given user's
    pipeline at a time. Other replicas proxy the WS to the owner.
    Cheaper to build, more network hops.
  - (b) Push the active surface (rolling transcript, items per mode,
    recalled context) into Redis or Postgres. Replicas become
    stateless; any of them can serve any user. Higher-latency state
    reads, more writes per meeting, but truly horizontal.
- **Broadcast bus moves to Redis pub/sub or NATS.** Each replica
  subscribes to topics keyed on `user_id`; events from any replica
  fan out to all WS connections regardless of which replica they
  landed on.
- **Pipeline placement.** Decide per-user, with leader election or
  consistent hashing on `user_id`, which replica runs the
  STT+summarizer pipeline. The other replicas proxy.
- **Distributed boot recovery.** Postgres advisory lock per
  `user_id`; first replica to acquire it claims the pipeline.

The migration is _additive_ — each piece can land independently
behind a feature flag, and the single-replica path keeps working
throughout.

### 4.4 Decision triggers

Ship 4.2 when: a single $20/mo VM is regularly above 70% CPU OR a
single replica's memory growth puts a meeting at risk during long
sessions.

Ship 4.3 when: we have real concurrent users (>50 simultaneous
meetings) OR blue/green deploys without a 10-minute drain become a
business need.

---

## 5. Open follow-ups

Not blocking the next phase, but flagged for later resolution.

### 5.1 Quality / completeness

- **`expand_item` returns lorem ipsum.** `state.rs::synthesize_detail`
  returns a hardcoded "Detail for X: lorem ipsum dolor sit amet…"
  placeholder. The intent is plumbed end-to-end (PWA dispatches,
  server processes, item rebroadcasts with `detail` populated) — only
  the body is missing. Real implementation: an LLM call against the
  underlying transcript chunks (or whatever produced the item),
  prompted to expand on it in 2-3 sentences. Each summarizer mode
  knows what context was used to produce its items, so the right
  context is reachable; it's just not piped to a "detail" path yet.
- **Items-mirror DOM diffing.** `pwa/src/ui/items-mirror.ts` does
  `pane.innerHTML = ""` and rebuilds every row on every store
  change. The CSS `animation: items-fade` rule on `.item` was
  dropped because the full rebuild made it flicker on every update.
  Right fix: diff against existing DOM keyed by `item.id`, append
  only new rows, leave existing ones in place. That lets the fade
  return cleanly (only new items animate in). Small project — a
  100-line patch in `items-mirror.ts` plus restoring the CSS rule.
- **Agent prompt cost optimization.** v1 accepts higher token cost
  for clarity (see §3). Real meeting data from the §3.8 cost
  instrumentation may flag opportunities worth implementing:
  shortening item IDs (per-meeting counters or base62-encoded UUIDs
  in place of `<prefix>-<full-uuid>` — currently ~10-12 tokens per
  ID, ~300 tokens at peak item count), trimming items-as-memory to
  recent-N-per-mode rather than the full mode buffer, more
  aggressive compression of mnemo `recalled_context`, or further
  rollover-summary compression. Each needs the cost data to justify
  the wire-shape or prompt-assembly change. Don't preempt — measure
  first.

### 5.2 Cross-cutting

- **Wire-format codegen.** `contract.rs` (Rust), `Protocol.swift`
  (Mac), `contract.ts` (PWA) are hand-synced. Every wire change
  applies in three places; drift surfaces as runtime decode failures.
  Three options:
  - (a) **Keep hand-sync.** Cheap; risk grows with surface area.
  - (b) **protobuf + codegen** across all three (`prost` for Rust,
    `swift-protobuf` for Mac, `ts-proto` for PWA). One-time setup
    cost; pays back forever. JSON-on-the-wire stays as a debug aid
    via protobuf's JSON mapping.
  - (c) **TS-as-source + `quicktype`** to Rust + Swift. Easiest if
    you treat the PWA's `contract.ts` as canonical, but loses
    Rust/Swift's richer enum types.

  Lean (b) — write the ADR when the next non-trivial wire change
  comes due.

- **Per-user mnemo identity.** Depends on a mnemo-side change.
  Forward compatibility today (`attributes.meeting_id`) keeps the
  door open.
- **Production host pick.** When deploy time comes. Hetzner (cheapest +
  full control), Fly.io (zero-ops + container-native), Railway
  (simplest UX) are all viable. Same Docker image runs on any.
