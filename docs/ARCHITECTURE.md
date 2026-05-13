# Architecture

Real-time meeting summarization with **three independent client
surfaces** and a shared Rust server. A native Mac menu-bar app, a
browser PWA (which also drives Even Realities G2 glasses), and an
Expo iOS/Android mobile app each connect to the same server, which
owns audio capture, streaming STT, and the mode catalog (transcript,
highlights, actions, open_questions, summary, chat). A single
tool-calling agent loop (Claude Opus 4.7, stateful conversation
history per meeting) reasons about the live transcript and decides
what to push to which mode via tool calls.

```
┌──────────────┐                 ┌─────────────────────────┐                ┌──────────────┐
│  Mac app     │◀━━ WebSocket ━━▶│  Server (Rust, Docker)  │◀━━━━━━━━━━━━━━▶│  Phone PWA   │
│  (SwiftUI,   │   :7331         │                         │   :7331        │  (TS, EvenHub│
│   menu bar)  │                 │  • audio ingest         │                │   WebView)   │
│              │                 │  • streaming STT (Soniox)│               │              │
│  • capture   │                 │  • mode catalog (×6)    │                │  • control   │
│  • dictation │                 │  • agent loop (Opus 4.7)│                │  • items     │
│  • overlay   │                 │  • moments + screenshots│                │    mirror    │
│              │                 │  • artifacts + summaries│                │              │
│  • Sparkle   │                 │  • mnemo memory         │                │  • Auth0 SPA │
│    auto-     │                 │  • Auth0 / per-user     │                │              │
│    update    │                 │  • Postgres + blobs     │                └──────┬───────┘
│  • Auth0     │                 │                         │                       │
│    PKCE      │                 │                         │                       │ BLE
└──────────────┘                 └────────────┬────────────┘                       ▼
                                              │                            ┌──────────────┐
                                              │ WebSocket :7331            │  G2 Glasses  │
                                              ▼                            │  (E.R. App)  │
                                  ┌──────────────────────┐                 │  thin display│
                                  │  Mobile app          │                 └──────────────┘
                                  │  (Expo, iOS+Android) │
                                  │                      │
                                  │  • control surface   │
                                  │  • Auth0 PKCE        │
                                  │  • EAS Build/Update  │
                                  └──────────────────────┘
```

All three clients are bidirectional WebSocket peers — each can send
intents and receive events for their authenticated user. Pairing is
additive: any client works alone. The glasses are PWA-only (LVGL
pages built in `packages/pwa/src/glasses/`, shipped to the Even
Realities companion app over BLE).

**Distribution.** Server image to GHCR, Mac `.app` to GitHub
Releases (Sparkle-signed for auto-update), iOS/Android via EAS Build
+ EAS Update. See `.github/workflows/` and the per-package READMEs
for the full pipeline.

---

## 1. Components

### Server — `packages/server/` (Rust)

The source of truth for active meeting state, per user. Single binary,
single process, but multi-tenant in shape: state is sharded by `user_id`
in a `HashMap<UserId, UserState>` and JWT-authenticated WS / REST
endpoints route every request to its owner.

Modules:

- `ws.rs` — WebSocket entry points (`/`, `/audio`, `/stt`), intent
  dispatch, per-user pipeline lifecycle, the `spawn_live_pipeline`
  function that wires up STT + transcript summarizer + agent loop
  per active meeting.
- `api.rs` — REST endpoints: `GET/DELETE /meetings`, `GET
  /meetings/:id`, moment screenshot upload/fetch routes,
  artifact CRUD + attach/detach. Mounted at the root next to the
  WS endpoints — no `/api/` prefix.
- `auth.rs` — Auth0 JWT validation. JWKS fetched lazily, cached by
  `kid`, refetch-on-miss with cooldown to resist forged-`kid` floods.
  `AURIS_AUTH_DISABLED=1` provides a synthetic dev user
  for local / CI.
- `state.rs` — `ServerState` (the multi-user shard), `UserState`
  (per-user meeting + mode buffers + devices + recalled context),
  `IntentOutcome` (the state machine's reply shape).
- `contract.rs` — `Intent`, `Event`, `UserEvent` envelope, all wire
  types, `PROTOCOL_VERSION`.
- `audio/` — ScreenCaptureKit local capture (legacy / dev-only path)
  + `RemoteAudioSource` that ingests PCM frames from the `/audio`
  WebSocket.
- `stt/` — `SttAdapter` trait + `SonioxAdapter` (production) +
  `MockAdapter` (offline / CI).
- `stt_ws.rs` — server-mediated STT endpoint (`/stt`). The Mac
  dictation mic and the PWA's listening flow both push PCM through
  this; the server owns the Soniox session and broadcasts transcript
  updates back. No provider keys leave the server.
- `summarizer/` — five pipelines, six modes:
  - `transcript` (pass-through, no LLM; emits each finalized chunk
    as a transcript-mode item).
  - `agent` (single tool-calling LLM loop per active meeting, with
    stateful `Vec<rig::Message>` conversation history; emits items
    into highlights/actions/open_questions via `push_*` /
    `replace_highlights` tools, reads attached artifacts via
    `fetch_artifact_summary` / `fetch_artifact`, and replies to
    chat questions in chat mode via Q+A bubble pairs). Anthropic
    prompt caching enabled. See ADR-0011.
  - `summary` (running 3-5 sentence meeting summary, single-item
    Replace strategy; hybrid trigger — fires on a token threshold
    OR a 5-min ceiling whichever first).
  - `moment` (vision-LLM summarizer for screenshots taken on
    `mark_moment`).
  - `artifact` (one-shot async summarizer for uploaded
    PDF/image/text artifacts: `short_summary` + `long_summary`).
  - The moment + artifact workers both kick the agent on
    completion so chat questions about a just-snapped moment or a
    just-attached artifact have full context.
- `llm.rs` — `LlmClient` with multi-provider support via `rig`
  (Bedrock / OpenAI / Anthropic). Vision path
  (`extract_with_prompt_and_image`) base64-encodes screenshots and
  routes through the same provider abstraction.
- `extraction.rs` — metadata extraction from the meeting description
  (free-form text → `{ project, title, owner, … }`).
- `db.rs` — Postgres connection pool, query helpers, redacted URL
  logging.
- `persistence.rs` — transcript JSONL writer. Subscribes to the
  `events_tx` broadcast and appends each finalized transcript item to
  `<DATA_DIR>/blobs/meetings/<meeting_id>/transcription.jsonl`.
- `mnemo/` — memory integration. `client.rs` (HTTP), `payload.rs`
  (pure builders), `pusher.rs` (sentence streaming + summary at stop),
  `recall.rs` (recall + prompt formatting).

### Mac app — `packages/mac/` (SwiftUI)

A native macOS menu-bar app. `LSUIElement = true` (accessory app, no
Dock entry). Two surfaces: the menu bar dropdown and a floating
overlay panel during meetings.

Files of note:

- `AurisApp.swift` — App entry, App Delegate.
- `AppModel.swift` — central observable model. Mirrors server state:
  `availableModes`, `currentMode`, `itemsByMode`, `transcriptInterim`,
  device list. Reduces incoming events; sends intents back.
- `MenuBarContent.swift` — menu dropdown UI. Status, Start / Stop
  meeting, Meetings…, Settings…, Permissions….
- `MeetingOverlayView.swift` — floating overlay panel during meetings.
  Mode tabs (TRANSCRIPT / HIGHLIGHTS / ACTIONS / QUESTIONS), live items
  list, Live indicator, dictation mic, mark-moment, stop.
- `DictationController.swift` — mic-capture → `SttSession` → compose
  panel description binding for the dictation flow.
- `Net/`:
  - `WebSocketClient.swift` — main control / event WS.
  - `Auth0Client.swift` — native Auth0 OAuth flow, token storage in
    Keychain.
  - `MeetingsAPI.swift` — REST client for `/meetings`.
  - `Protocol.swift` — wire types (hand-synced with `contract.rs`).
  - `SttSession.swift` — `/stt` WS client for the dictation mic path.
- `Audio/` — `AudioCapture` + `MicCapture` + `AudioStreamer`
  (system + mic mixer at ~50 fps), `DictationMicCapture` (mic-only
  for dictation, runs on the realtime audio thread, `@unchecked
  Sendable` because the tap fires off the main actor).
- `Capture/ScreenshotCapture.swift` — on-demand screenshot for moments.
- `Settings/`, `Permissions/` — onboarding, microphone + screen
  recording prompts, server URL display.

### PWA — `packages/pwa/` (TypeScript)

The user's control surface on the phone. Runs inside the EvenHub
Flutter WebView (via `@evenrealities/even_hub_sdk`'s
`waitForEvenAppBridge`) or inside the EvenHub simulator on a laptop
during dev.

Layered roughly as:

- `main.ts` — boot, `waitForEvenAppBridge`, store creation, WS open.
- `boot.ts` — settings load (bridge + `localStorage` fallback), env
  defaults, Auth0 SPA flow, first-paint state.
- `auth.ts` — Auth0 SPA client, token storage, getAccessToken hook.
- `server-url.ts` — `SERVER_URL` constant from
  `import.meta.env.VITE_SERVER_URL` (build-time, not runtime; matches
  the Mac app's `AppSettings.serverURLDefault`).
- `store.ts` — typed `Store<AppState>` with selector subscriptions.
- `types.ts` — `AppState`, glasses view enum, derived helpers.
- `ws.ts` — `ReconnectingSocket`. Backoff-driven reconnect with
  status callbacks.
- `ws-handlers.ts` — server → store reducer. One case per event type.
- `glasses/` — page-container builder. Translates the store's items
  into LVGL-friendly text containers via the EvenHub SDK.
- `input/` — gesture and lifecycle event routers (temple taps, ring
  taps, foreground/background).
- `listening.ts` — `/stt` WS client for the dictation flow (matches
  the Mac path; PCM through server).
- `meetings-api.ts` — REST client for `/meetings`.
- `ui/` — DOM components. One file per surface; all self-hide based
  on `meetingState`.
- `state-machine.ts` — small reducer that maps bridge events to
  glasses view transitions.

Detailed UX in [`UX.md`](UX.md).

### Mobile app — `packages/mobile/` (Expo + React Native)

A native iOS and Android client built on Expo SDK 51 with Expo
Router (file-based routing). Same wire types as the PWA, hand-synced
into `src/wire/contract.ts`. Same Auth0 tenant; uses the Mac's
Native client_id for personal-use simplicity.

Files of note:

- `app/` — file-routed screens. `_layout.tsx` (root Stack with auth
  bootstrap + auto-WS-on-identity), `login.tsx` (Auth0 sign-in),
  `(tabs)/index.tsx` (compose), `(tabs)/history.tsx` (past
  meetings, bucketed), `(tabs)/artifacts.tsx` (artifact list),
  `(tabs)/settings.tsx`, `meeting.tsx` (live-meeting fullscreen
  modal), `meeting/[id].tsx` (past-meeting detail).
- `src/wire/` — the protocol layer. `contract.ts` (types),
  `ws.ts` (`ReconnectingSocket` ported verbatim from PWA),
  `meetings-api.ts`, `artifacts-api.ts` (REST clients).
- `src/auth/auth0.ts` — PKCE flow via `expo-auth-session`,
  refresh token in `expo-secure-store`, `getAccessToken` with
  silent refresh, identity subscription API.
- `src/store/index.ts` — Zustand store mirroring PWA's
  `defaultAppState` reducer; owns the `ReconnectingSocket`
  lifecycle.
- `src/audio/audio-capture.ts` — currently stubbed (no-op).
  expo-audio per-buffer PCM is gated on Expo SDK 52+; SDK 51 here
  forces a deferred PCM path. Phase 3 of MOBILE-PLAN.
- `src/lib/meetings.ts` — pure helpers (`pickMeetingTitle`,
  `relativeBucket`, `formatDateLong`, `formatTokens`) shared with
  the PWA's display logic.

Distribution: EAS Build (matrixed `[ios, android]`) for binaries,
EAS Update for OTA JS bundles via channels. See `eas.json` and
`.github/workflows/mobile-{build,update}.yml`.

Phases 0-5 of [`MOBILE-PLAN`](MOBILE-PLAN.md) shipped. Deferred:
PCM streaming (3.3+), camera-attached moments (4.5+), moment image
rendering (5.4), artifact uploads (5.7+).

### Glasses — Even Realities G2 (display only)

Renders page containers built by the PWA via
`bridge.createStartUpPageContainer` and `bridge.rebuildPageContainer`.
No business logic on-glasses; the layout-builder code in
`packages/pwa/src/glasses/` is the entire contract surface. Hardware
constraints (ADR-0002):

- 576 × 288 px, 4-bit greyscale (16 levels).
- `ListContainer` cannot be incrementally updated — every change is a
  full page rebuild.
- No long-press event from the SDK (ADR-0001) — lifecycle gestures
  live entirely on the phone.
- `bridge.setLocalStorage` is the only reliable persistence surface
  inside the Flutter WebView, with browser `localStorage` as a fallback
  for dev / simulator (ADR-0003).

---

## 2. Wire protocol

Three WebSocket endpoints + a REST API on a single port (7331), all
JWT-authenticated:

- `GET /` — control WS. Carries `Intent` (client → server) and `Event`
  (server → client) messages, snake-case `type` discriminator,
  `PROTOCOL_VERSION` versioned.
- `GET /audio` — binary PCM frames from a capture-capable client into
  the server's audio pipeline.
- `GET /stt` — server-mediated STT. Client sends PCM frames, server
  owns the Soniox session, broadcasts JSON transcript updates back
  (`Ready` / `Interim` / `Final` / `Error`).
- `GET /api/...` — REST endpoints for meetings, moments, blobs.

JWT is passed as `?token=<JWT>` on the WS handshake or as
`Authorization: Bearer <JWT>` on REST. Auth0 issues the JWT; the
server validates against Auth0's JWKS. A bypass mode for local dev
(`AURIS_AUTH_DISABLED=1`) substitutes a synthetic user.

A two-stage intent dispatch produces named errors (`bad_json` /
`unknown_intent` / `bad_payload`) instead of generic deserialize
failures.

Full reference in [`PROTOCOL.md`](PROTOCOL.md). Decision in
[ADR-0004](adr/0004-websocket-protocol.md).

---

## 3. Data flow during a live meeting

```
ScreenCaptureKit ─┐                                      ┌── /audio WS ──┐
   (system audio) │                                      │   from Mac    │
                  ├─▶ Audio Mixer ─▶ Soniox WS ─▶ TranscriptChunk ─┐
   (microphone)   │   (50 fps,      (streaming,     (sentence-       │
ScreenCaptureKit ─┘    timestamp-     finalized +   flushed on        │
                       aligned)       interim       3 s idle +        │
                                      tokens)       soft-boundary)    │
                                                                      │
                              ┌───────────────────────────────────────┘
                              ▼
                    rolling_transcript ◀── UserState (per user)
                              │
                              ├─▶ Transcript pass-through (no LLM, raw chunks → items)
                              │
                              ├─▶ Summary loop (single-item Replace; full
                              │                 transcript every fire,
                              │                 token-threshold + 5-min ceiling)
                              │
                              └─▶ Agent loop (one task per active meeting,
                                              stateful Vec<Message> history,
                                              hybrid trigger: tokens / sentences
                                              / silence / hard cap / kick)
                                              │
                                              ├─ tools: push_highlight,
                                              │         replace_highlights,
                                              │         push_action,
                                              │         push_open_question,
                                              │         fetch_artifact_summary,
                                              │         fetch_artifact
                                              ├─ kick events: artifact attached,
                                              │               chat message,
                                              │               moment marked,
                                              │               moment summarized
                                              ▼
                       items_per_mode  ─▶  Event::ItemsUpdate  ─▶  per-user broadcast bus
                                              │                     │
                                              │                     ├─▶ all WS clients (Mac + PWA)
                                              │                     ├─▶ persistence task → JSONL blob
                                              │                     └─▶ glasses page rebuilds (PWA)
                                              ▼
                                     mnemo pusher (per sentence: user-role turn)
                                                  (at stop: assistant-role bundle)
```

Each transcript sentence triggers:

- Append to that user's `rolling_transcript`.
- An `Event::ItemsUpdate { mode: "transcript", items }` broadcast to
  the user's WS clients.
- A streaming push to mnemo (one `user`-role turn).
- An append to the per-meeting `transcription.jsonl` blob.

The agent fires when any of: ~200 new tokens accumulate,
4 sentences accumulate, 4 s of silence, 30 s since last fire, or
a kick (e.g., user attached an artifact). Each fire:

- Drains the chunk buffer and composes the next user-turn message
  with new transcript chunks, optional `[event]` blocks, and
  (first fire only) a `[meeting]` + `[attached artifacts]` header.
- Calls `agent.prompt(...).with_history(history.clone())
  .extended_details()` via rig — passes the full prior history,
  receives back the new turns produced this fire (assistant +
  tool-call + tool-result messages).
- Trailing text-only assistant turns are stripped before the new
  messages are appended onto history (keeps the agent emitting
  tool calls, not chat).
- Tool calls execute their side effects: `push_*` /
  `replace_highlights` mutate state via `push_item_for_mode` /
  `replace_items_for_mode`, broadcasting `Event::ItemsUpdate`.
  `fetch_artifact_summary` / `fetch_artifact` read from the
  artifacts table to ground reasoning.

At meeting Active, the recaller fires one `GET /recall` to mnemo and
populates `state.recalled_context`. Re-fires if the user edits the
project tag mid-meeting. At meeting Idle, the pusher emits one final
`POST /events` bundling actions / highlights / open_questions as
`assistant`-role turns.

**Moments** are a parallel side-pipeline. On `MarkMoment`, the server
inserts a moments row, asks a `screen_capture`-capable device for a
fresh frame, persists the screenshot under
`<DATA_DIR>/blobs/meetings/<meeting_id>/moments/<moment_id>.jpg`, then
schedules an async vision-LLM summarizer (`summarizer/moment.rs`) that
reads back the screenshot bytes and produces a structured note. The
moment appears in the meeting detail view via `/meetings/:id`.

ADRs covering this flow:

- [0006 — Live audio + STT pipeline](adr/0006-live-audio-stt-pipeline.md)
- [0007 — Summarizer architecture](adr/0007-summarizer-architecture.md) (transcript / moment portion; superseded for highlights/actions/open_questions)
- [0008 — mnemo memory integration](adr/0008-mnemo-memory-integration.md)
- [0011 — Agentic summarizer loop](adr/0011-agentic-summarizer-loop.md)

---

## 4. Meeting lifecycle

```
                ┌────────────────────────────────────────┐
                ▼                                        │
              IDLE                                       │
              │                                          │
   description typed                                     │
              │                                          │
              ├─▶ ExtractMetadata intent                 │
              │   ├─▶ LLM extraction (idle, async)       │
              │   └─▶ MetadataChanged                    │
              │                                          │
   StartMeeting (preserves metadata)                     │
              │                                          │
              ▼                                          │
            ACTIVE ─── Pause ──▶ PAUSED ── Resume ──▶ ACTIVE
              │                       │                  │
              │                       └──────────────────┤
              │                                          │
              StopMeeting                                │
              │                                          │
              ▼                                          │
              IDLE  (clears metadata, items,             │
                     recalled_context, in-flight         │
                     extraction)                         │
              │                                          │
              └──────────────────────────────────────────┘
```

Two cancellation tokens orthogonalize the two lifetimes:

- `meeting_cancel` covers the audio task, STT adapter, transcript
  summarizer, and agent loop. Scoped to a single Active session.
- `extraction_cancel` covers in-flight LLM-extraction calls and
  in-flight mnemo recalls. Independent — an idle-time
  `ExtractMetadata` survives `start_meeting` (the user shouldn't lose
  the chips just because they hit Start), but `stop_meeting` cancels
  any in-flight extraction so a stale recall can't pollute the next
  idle's empty state.

**Boot recovery.** A meeting that was Active when the server crashed
remains in Postgres with `ended_at IS NULL`. On startup, the server
scans for these rows (cheap via the partial index
`idx_meetings_active`), respawns the live pipeline for each, and
broadcasts a synthetic state-change event so reconnecting clients see
`Active`. The previous WS audio source is gone — the recovered meeting
sits idle until a capture-capable client reconnects and binds.

`ExtractMetadata` decision in [ADR-0010](adr/0010-extract-metadata-flow.md).

---

## 5. PWA UX model

Store-driven, self-hiding components. Mount order *is* the layout.

- A single typed `Store<AppState>` is the source of truth.
- Each UI component subscribes to a slice via a string selector and
  re-renders on change.
- Each component toggles its own `display: none` based on
  `meetingState`. The parent (`ui/index.ts`) is unaware of visibility
  logic.
- The metadata chip strip is mounted *between* idle compose and idle
  Start, so it stays in the same layout slot when a meeting starts.

Full UX reference in [`UX.md`](UX.md). Decision in
[ADR-0009](adr/0009-pwa-ux-design-system.md).

---

## 6. mnemo memory layer

The companion is one of several producers feeding [mnemo](https://github.com/tiagodeoliveira/mnemo),
a personal memory layer backed by AWS Bedrock AgentCore Memory. The
integration:

- **Streams transcript** sentence-by-sentence as `user`-role turns
  during a meeting.
- **Pushes a summary bundle** as `assistant`-role turns at meeting stop
  (actions / highlights / open questions, omitting empty modes).
- **Recalls prior context** (preferences + facts + episodes +
  optional project) at meeting start; result populates
  `state.recalled_context`.
- **Per-mode toggle** for prior-context consumption: actions and
  open_questions read it; highlights doesn't (its signal is local).
- **Direct HTTP** with `x-api-key` header — no CLI shell-out.
- **Disabled by default** when env vars are unset; CI / unit tests
  unaffected.

Configured via three env vars:

- `AURIS_MNEMO_URL`
- `AURIS_MNEMO_API_KEY`
- `AURIS_MNEMO_WORKSTATION` (optional, falls back to
  `gethostname()`)

Decision and constraints in [ADR-0008](adr/0008-mnemo-memory-integration.md).

---

## 7. LLM extraction

Four consumers of one client:

- **Metadata extraction** from the meeting description (free-form text
  → structured `{ project, title, owner, … }`). Goes through
  `LlmClient::extract_with_prompt::<Schema>` for typed output.
- **Agent loop** (live transcript → tool calls). Goes through rig's
  `Agent.prompt(...).with_history(...).extended_details()` directly
  (not the typed extractor); a tool surface drives item emission. See
  ADR-0011.
- **Moment summaries** (transcript context + screenshot → structured
  note). The vision-capable path base64-encodes the screenshot bytes
  and routes through `extract_with_prompt_and_image`.
- **Artifact summaries** (uploaded PDF/image/text → short + long
  summary). One-shot async worker; routes through
  `extract_with_prompt_and_document_pdf` for PDFs,
  `extract_with_prompt_and_image` for images, plain
  `extract_with_prompt` for text.

All paths share `rig::providers::{bedrock, openai, anthropic}`,
routed by `AURIS_LLM_PROVIDER`. The agent loop's default
model is Claude Opus 4.7 (1M context).

Vision moments use a longer timeout (30 s) than text-only calls (8 s)
because vision providers are noticeably slower.

Decision in [ADR-0005](adr/0005-multi-provider-llm.md).

---

## 8. Persistence

Three persistence surfaces, each with a clear job:

### 8.1 Postgres (relational state)

Schema in `packages/server/migrations/0001_initial_schema.sql`:

- `users` — `id` (UUID v4 we mint), `auth0_sub` (Auth0's stable id),
  `email` / `name` (best-effort from JWT claims), timestamps.
- `meetings` — `id`, `user_id` (CASCADE), `started_at`, `ended_at`
  (NULL while active), `description`, `metadata` (JSON-as-TEXT).
  Composite index on `(user_id, started_at DESC)` for the dominant
  list-meetings read pattern. Partial index on active meetings for
  cheap boot-recovery scans.
- `moments` — `id`, `meeting_id` (CASCADE), `kind`, `t` (millisecond
  offset from `started_at`), `note`, `asset_path` (relative path
  under `<DATA_DIR>/blobs`), `summary`, `summary_status`
  (`pending` / `done` / `failed`).

`metadata` is intentionally JSON-as-TEXT (not `JSONB`) because the
access pattern is "load the blob, hand it to the client" — there are
no server-side filters into the JSON yet. One-line `ALTER` if that
ever changes.

### 8.2 Blob storage (transcript JSONL + moment screenshots)

`<DATA_DIR>/blobs/meetings/<meeting_id>/`:

- `transcription.jsonl` — one JSON-encoded `Item` per line, appended
  by `persistence.rs` as the transcript-mode broadcast fires. The
  ground truth for a meeting; highlights / actions / open_questions
  are derived from it and could be re-run if lost.
- `moments/<moment_id>.jpg` — screenshot for each moment.

Local dev uses the filesystem directly. The shape is intentionally
S3-compatible (one prefix per meeting) so `S3BlobStore` is an additive
swap when horizontal scale arrives (see `PLAN.md` §4).

### 8.3 mnemo (cross-session memory)

See §6. The summary bundle pushed at meeting stop is the source of
truth for cross-meeting recall.

### 8.4 What's NOT persisted in the DB

- **Items per mode** (highlights / actions / open_questions). They
  live in `UserState` in memory only. Re-derivable from the transcript
  by replaying the summarizers if we ever build a "review meeting"
  feature.
- **Devices.** Registered per WS connection, in-memory only.
- **`recalled_context`.** Fetched from mnemo at meeting start; not
  cached server-side beyond the active meeting.
- **PWA settings** (server URL, last metadata) — in the PWA, via
  `bridge.setLocalStorage` with browser `localStorage` fallback.
  Per-key debounced writes, ~500 ms.

### 8.5 Crash semantics

A meeting Active when the server crashes remains in Postgres with
`ended_at IS NULL`. On restart the server respawns its live pipeline
(see §4 Boot recovery). Items per mode are lost — they were in-memory
only. The transcript JSONL on disk is intact, so summarizers replay
from a clean state and items rebuild as the conversation continues.

---

## 9. Identity & multi-tenancy

The server is multi-tenant in shape. Two clients share the same
identity model:

- **Auth0 as the IdP.** Mac uses the Auth0 native flow with
  Authorization Code + PKCE; tokens stored in Keychain. PWA uses the
  Auth0 SPA flow; tokens persisted via `bridge.setLocalStorage` with
  `localStorage` fallback.
- **JWT validation.** Every WS / REST request carries the JWT.
  `auth.rs` validates against Auth0's JWKS, caching keys by `kid` with
  a refetch-on-miss cooldown to resist forged-`kid` flooding.
- **Per-user state shard.** `ServerState` holds a `HashMap<UserId,
  UserState>`. Each user's `UserState` has its own
  `rolling_transcript`, items per mode, devices, recalled context,
  and in-flight cancellation tokens.
- **Per-user broadcast.** Events go through a shared bus wrapped in a
  `UserEvent { user_id, event }` envelope. Clients only see events
  for their own `user_id`.
- **Cross-user isolation.** STT, audio frames, summarizers, and
  persistence are all keyed on `user_id` end-to-end. Cross-user
  contamination is structurally prevented (the `state` API never
  exposes a "global" mutator).
- **Bypass for dev / CI.** `AURIS_AUTH_DISABLED=1`
  substitutes a synthetic user so the local dev path runs without
  cloud auth.

---

## 10. Decision records

The non-obvious decisions live in [`adr/`](adr/). Read them when "why
did we do it this way?" comes up:

- [0001 — Gesture map](adr/0001-gesture-map.md)
- [0002 — Active-list rendering](adr/0002-active-list-rendering.md)
- [0003 — Persistence via bridge](adr/0003-persistence-via-bridge.md)
- [0004 — WebSocket protocol](adr/0004-websocket-protocol.md)
- [0005 — Multi-provider LLM](adr/0005-multi-provider-llm.md)
- [0006 — Live audio + STT pipeline](adr/0006-live-audio-stt-pipeline.md)
- [0007 — Summarizer architecture](adr/0007-summarizer-architecture.md)
- [0008 — mnemo memory integration](adr/0008-mnemo-memory-integration.md)
- [0009 — PWA UX design system](adr/0009-pwa-ux-design-system.md)
- [0010 — `ExtractMetadata` flow](adr/0010-extract-metadata-flow.md)
