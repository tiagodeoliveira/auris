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
lifecycle as today's summarizers). Subscribes to the same
`TranscriptChunk` broadcast the transcript summarizer already drains.

On each chunk:

1. Append the chunk to the agent's working context.
2. Invoke the LLM with system prompt + working context. Model has
   access to four tools:
   - `push_highlight { text, importance }` — append to highlights mode.
   - `push_action { text, owner, due }` — append to actions mode.
   - `push_open_question { question, kind, context }` — append to
     open_questions mode.
   - `replace_highlights { items: [...] }` — full-replace when the
     model decides existing highlights are stale.
3. Each tool call translates to the same `Event::ItemsUpdate` we
   broadcast today. Wire shape unchanged — only the producer changes.
4. The model is also free to do nothing this turn.

The transcript-mode pass-through summarizer stays as-is — it's
non-LLM and just promotes chunks to items.

### 3.3 Context rollover strategy

Working context grows linearly with meeting length and will exceed
budget on long calls. Strategy:

- **Tail-window verbatim.** Keep the most recent N chunks in full
  (start with N=80, ~5-10 minutes of speech).
- **Summarized prefix.** When the tail crosses N, summarize the
  _displaced_ chunks into a 1-paragraph rolling summary and keep that
  summary in the system prompt. Each rollover compounds into the
  same summary slot (re-summarize summary + newly-displaced chunks).
- **Items-as-memory.** The four mode buffers themselves act as
  long-term memory: the agent's prompt includes the current state of
  highlights / actions / open_questions, so it doesn't need raw
  transcript to remember what's already been extracted.

Roughly the "running summary + sliding window" pattern; formalize
once we have real meeting data showing where context gets unwieldy.

### 3.4 Open questions

- **Cost.** Per-chunk LLM calls could be 5-10× current spend. Mitigate
  by coalescing chunks (don't fire until the buffer has N tokens or
  silence ≥ M seconds). Worth measuring on a real meeting before
  optimizing.
- **Tool-calling vs structured output.** rig supports both. Tool
  calling reads naturally for "decide which mode to update"; the
  "do nothing this turn" path is a free signal there. Structured
  output forces a synthetic no-op variant in the schema. Lean tool
  calling unless instrumentation says otherwise.
- **Backwards compatibility.** Wire shape stays the same
  (`Event::ItemsUpdate { mode, items }`). Clients don't need to know
  the producer changed. Roll out via a feature flag and run the agent
  in parallel with the existing summarizers for a few meetings before
  cutting over.
- **mnemo recaller integration.** Today the per-mode summarizers fold
  prior-meeting context into their prompt. The agent needs the same —
  drop the prior-context block into its system prompt on meeting
  start, same shape as today.
- **Persistence.** Today's summarizers are stateless across server
  restarts (state rebuilds from items*per_mode + recalled_context).
  The agent's \_working context* is just the post-rollover summary +
  tail-window — both rebuildable from the persisted transcript JSONL
  and the existing `recalled_context` if we want crash recovery.
  Probably keep it in memory for v1; revisit if recovery matters.

### 3.5 Migration

- Land behind `MEETING_COMPANION_AGENT_SUMMARIZER=1` env flag;
  default off so existing behavior is preserved.
- New module `summarizer/agent.rs` parallel to today's per-mode
  files. Spawn site is the existing `spawn_live_pipeline` in `ws.rs`
  — replaces the four summarizer-task spawns with one agent task
  when the flag is on.
- Delete the three per-mode summarizers once the agent runs cleanly
  for a couple of weeks of personal use. Keep `transcript.rs` (it's
  not LLM-driven). The dedicated tests for each summarizer become
  agent-level integration tests.

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
