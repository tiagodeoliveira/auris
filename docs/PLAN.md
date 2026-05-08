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
- Artifact subsystem (PDF / image / text upload, async LLM
  summarization, attach to meetings, 3-tier agent retrieval).
- Single stateful agent loop replacing the three per-mode
  summarizers — see [ADR-0011](adr/0011-agentic-summarizer-loop.md).
  Default model: Claude Opus 4.7 (1M context). Mode catalog now
  includes:
  - `transcript` (raw STT chunks, pass-through, no LLM),
  - `highlights` / `actions` / `open_questions` (agent tool
    emissions),
  - `summary` (running 3-5 sentence meeting summary, hybrid
    token-threshold + 5-min-ceiling refresh),
  - `chat` (ask the agent questions during a live meeting; Q+A
    bubble pairs replace each previous exchange — agent's
    conversation history is the context, no separate UI thread).
- Moment marks and artifact attaches both inject into the agent's
  conversation history as `[event]` blocks (immediate ack on
  creation, follow-up event when the moment-summary worker
  completes), so chat questions about a just-snapped moment work
  end-to-end.
- Container-shaped server (`docker compose up`) — same image for
  local dev and any future production host.

The roadmap from here is shaped by personal-use signal. Two forward
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

## 3. Horizontal scale (future)

Today the server runs as a single replica. Postgres takes care of the
durable surface, but a lot of _active_ meeting state still lives only
in the replica's RAM. Two scales worth distinguishing.

### 3.1 What pins a session to one replica today

Friends-and-family scale (one VM, vertical scale): fine. Postgres
covers the durable surface; the server itself just needs to keep
running.

The blockers to running N replicas behind a plain round-robin LB:

- **`ServerState` is per-process.** `HashMap<UserId, UserState>` —
  meeting state, items per mode, devices, rolling transcript,
  recalled context. Two browser tabs hitting different replicas would
  see different "active meeting" state.
- **Pipelines run in-process.** `start_meeting` spawns a Soniox WS,
  three summarizer tasks (transcript / agent / summary), an audio
  source, mnemo pusher/recaller — all on the receiving replica.
  Other replicas don't know it exists.
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

### 3.2 The cheap step — sticky sessions

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

### 3.3 The full step — stateless replicas

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

### 3.4 Decision triggers

Ship 3.2 when: a single $20/mo VM is regularly above 70% CPU OR a
single replica's memory growth puts a meeting at risk during long
sessions.

Ship 3.3 when: we have real concurrent users (>50 simultaneous
meetings) OR blue/green deploys without a 10-minute drain become a
business need.

---

## 4. Open follow-ups

Not blocking the next phase, but flagged for later resolution.

### 4.1 Quality / completeness

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
- **Agent prompt cost tracking.** Per-fire `agent fire done` lines
  log `input_tokens` / `output_tokens` / `cached_input_tokens` from
  rig's `PromptResponse.usage`, but `LlmClient::record_usage` still
  accumulates char counts (the older API shape). Wire the actual
  token numbers through to the per-user counter so
  `llm_usage_at_stop` reflects real Opus 4.7 spend, not a 4-char-
  per-token approximation. Small change in `llm.rs` + the agent
  fire site.
- **Rolling-summary compression.** With Opus 4.7's 1M context, a
  meeting can run hours before history exhausts the window. When
  (if) we hit the ceiling: compress the older portion of history
  into a synthetic summary message, keep the most recent N turns
  verbatim. Don't preempt — measure first.
- **Item delete/edit event injection.** The `AgentKickReason` enum
  already carries `ArtifactAttached`, `ChatMessage`,
  `MomentMarked`, and `MomentSummarized` — adding
  `ItemRemoved { mode, item_id, text }` /
  `ItemEdited { mode, item_id, old, new }` is mechanically the
  same pattern. Blocked on the user-facing delete/edit UI which
  doesn't exist yet (no contract intent, no PWA/Mac controls).
  When that lands, the agent-side wiring is ~20 lines.
- **Mode persistence to Postgres.** Today every mode's items live
  only in `UserState`'s in-memory map, so they're lost on server
  restart (transcript JSONL is the only persisted artifact —
  highlights, actions, open_questions, summary, chat are all
  ephemeral per meeting). Persisting them tied to `meeting_id`
  unlocks a "browse past meeting" view that shows what the agent
  produced, not just the raw transcript. Schema add: an
  `items(meeting_id, mode, id, text, meta_json, t_ms,
created_at)` table. Write on every `ItemsUpdate` broadcast;
  read on `GET /api/meetings/:id`. Don't persist `chat` mode in
  v1 (chat is working memory, not artifact) — start with the
  others.
- **Prompt caching.** rig 0.36 supports Anthropic prompt caching
  via `anthropic_beta`. The system prompt + early conversation
  history would cache cleanly. Real cost win on long meetings —
  revisit after a few weeks of usage data.

### 4.2 Cross-cutting

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
