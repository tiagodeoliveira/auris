# Architecture

Real-time meeting summarization on Even Realities G2 glasses. A
laptop-hosted Rust server captures audio, runs streaming STT, and
distills four parallel "modes" (transcript, highlights, actions, open
questions) via per-mode LLM summarizers. A phone-hosted PWA — running
inside the EvenHub Flutter WebView — is the user's control surface and
the conduit to the glasses display.

```
┌──────────────────┐   WebSocket   ┌──────────────────────┐    BLE    ┌──────────────┐
│  Laptop Server   │◀━━━━━━━━━━━━▶│  Phone PWA           │◀━━━━━━━━━▶│  G2 Glasses  │
│  (Rust)          │  :7331         │  (TS, EvenHub plugin)│   E.R. App│  thin display │
│                  │                │                      │           │              │
│  • audio capture │                │  • control surface   │           │  • renders    │
│  • streaming STT │                │  • mirrors items     │           │    page      │
│  • summarizers   │                │  • dictation client  │           │    containers│
│  • LLM extraction│                │  • settings + auth   │           │              │
│  • mnemo memory  │                │                      │           │              │
└──────────────────┘                └──────────────────────┘           └──────────────┘
```

Two transports: a single WebSocket between server and PWA carries
state; BLE between phone and glasses (managed by the Even Realities
companion app, hidden behind the EvenHub SDK) carries the rendered UI.

---

## 1. Components

### Server — `packages/server/` (Rust)

Source of truth for meeting state. Owns audio capture, STT, LLM
extraction, summarizers, and the mnemo integration. Single binary,
single process, single connected WS client at a time.

Modules:

- `ws.rs` — WebSocket entry point, intent dispatch, connection lifetime,
  spawn-on-boot side tasks (heartbeat, mnemo pusher, mnemo recaller).
- `state.rs` — `ServerState` and `IntentOutcome`. The state machine.
  Lock granularity is a single `tokio::sync::Mutex<ServerState>`.
- `contract.rs` — `Intent`, `Event`, all wire types, `PROTOCOL_VERSION`.
- `audio/` — ScreenCaptureKit capture + 50 fps in-process mixer.
  macOS-only.
- `stt/` — `SttAdapter` trait + `SonioxAdapter` (production) +
  `MockAdapter` (offline / CI).
- `summarizer/` — one module per mode (`transcript`, `highlights`,
  `actions`, `open_questions`). Each is an async heartbeat task.
- `llm.rs` — `LlmClient` with multi-provider support via `rig`
  (Bedrock / OpenAI / Anthropic).
- `extraction.rs` — metadata extraction from the meeting description.
- `mnemo/` — memory integration. `client.rs` (HTTP), `payload.rs`
  (pure builders), `pusher.rs` (sentence streaming + summary at stop),
  `recaller.rs` (recall at start), `recall.rs` (response types and
  prompt formatting).

### PWA — `packages/pwa/` (TypeScript)

The user's control surface. Runs inside the EvenHub Flutter WebView on
the phone, or inside the EvenHub simulator on a laptop during dev.
~37 KB gzipped.

Layered roughly as:

- `main.ts` — boot, `waitForEvenAppBridge`, store creation, WS open.
- `store.ts` — typed `Store<AppState>` with selector subscriptions.
- `types.ts` — `AppState`, glasses view enum, derived helpers.
- `ws.ts` — `ReconnectingSocket`. Backoff-driven reconnect with
  status callbacks.
- `ws-handlers.ts` — server → store reducer. One case per event type.
- `boot.ts` — settings load (bridge + `localStorage` fallback), env
  defaults, first-paint state.
- `glasses/` — page-container builder. Translates the store's items
  into LVGL-friendly text containers via the EvenHub SDK.
- `input/` — gesture and lifecycle event routers (temple taps, ring
  taps, foreground/background).
- `listening.ts` — Soniox client for the meeting-description dictation
  flow (mic only; runs in the user's browser context).
- `ui/` — DOM components. One file per surface; all self-hide based
  on `meetingState`.
- `state-machine.ts` — small reducer that maps bridge events to glasses
  view transitions.

Detailed UX in [`UX.md`](UX.md).

### Glasses — Even Realities G2 (display only)

Renders page containers built by the PWA via `bridge.createStartUpPageContainer`
and `bridge.rebuildPageContainer`. No business logic on-glasses; the
layout-builder code in `packages/pwa/src/glasses/` is the entire
contract surface. Hardware constraints (ADR-0002):

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

A single WebSocket on `:7331/?token=<MEETING_COMPANION_TOKEN>`. Two
hand-maintained contract files (Rust + TS) kept in sync by review.
Snake-case `type` discriminator, opt-in optional fields, versioned via
`PROTOCOL_VERSION`. A two-stage intent dispatch produces named errors
(`bad_json` / `unknown_intent` / `bad_payload`) instead of generic
deserialize failures.

Full reference in [`PROTOCOL.md`](PROTOCOL.md). Decision in
[ADR-0004](adr/0004-websocket-protocol.md).

---

## 3. Data flow during a live meeting

```
ScreenCaptureKit ─┐
   (system audio) │
                  ├─▶ Audio Mixer ─▶ Soniox WS ─▶ TranscriptChunk ─┐
   (microphone)   │   (50 fps,      (streaming,     (sentence-       │
ScreenCaptureKit ─┘    timestamp-     finalized +   flushed on        │
                       aligned)       interim       3 s idle +        │
                                      tokens)       soft-boundary)    │
                                                                      │
                              ┌───────────────────────────────────────┘
                              ▼
                    rolling_transcript ◀── ServerState
                              │
                              ├─▶ Highlights summarizer (20 s heartbeat)
                              ├─▶ Actions summarizer    (15 s heartbeat) ─── reads recalled_context
                              ├─▶ Open Questions       (15 s heartbeat) ─── reads recalled_context
                              │
                              ▼
                       items_per_mode  ─▶  Event::ItemsUpdate  ─▶  PWA store  ─▶  glasses + items mirror
                                              │
                                              ▼
                                     mnemo pusher (per sentence: user-role turn)
                                                  (at stop: assistant-role bundle)
```

Each transcript sentence triggers:

- Append to `state.rolling_transcript`.
- An `Event::ItemsUpdate { mode: "transcript", items }` broadcast to
  the WS client.
- A streaming push to mnemo (one `user`-role turn).

Every 15–20 s, each LLM summarizer:

- Reads `rolling_transcript_text()`, existing same-mode items, and
  (for actions / open_questions) `recalled_context` under the state
  lock.
- Drops the lock, calls `LlmClient::extract_with_prompt`.
- Re-locks, applies the result via `push_item_for_mode` (append +
  dedup) or `replace_items_for_mode` (replace).
- Broadcasts `Event::ItemsUpdate { mode, items }`.

At meeting Active, the recaller fires one `GET /recall` to mnemo and
populates `state.recalled_context`. Re-fires if the user edits the
project tag mid-meeting. At meeting Idle, the pusher emits one final
`POST /events` bundling actions/highlights/open_questions as
`assistant`-role turns.

ADRs covering this flow:

- [0006 — Live audio + STT pipeline](adr/0006-live-audio-stt-pipeline.md)
- [0007 — Summarizer architecture](adr/0007-summarizer-architecture.md)
- [0008 — mnemo memory integration](adr/0008-mnemo-memory-integration.md)

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

- `meeting_cancel` covers audio task, STT adapter, summarizer tasks.
  Scoped to a single Active session.
- `extraction_cancel` covers in-flight LLM-extraction calls and
  in-flight mnemo recalls. Independent — an idle-time
  `ExtractMetadata` survives `start_meeting` (the user shouldn't lose
  the chips just because they hit Start), but `stop_meeting` cancels
  any in-flight extraction so a stale recall can't pollute the next
  idle's empty state.

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

- `MEETING_COMPANION_MNEMO_URL`
- `MEETING_COMPANION_MNEMO_API_KEY`
- `MEETING_COMPANION_MNEMO_WORKSTATION` (optional, falls back to
  `gethostname()`)

Decision and constraints in [ADR-0008](adr/0008-mnemo-memory-integration.md).

---

## 7. LLM extraction

Two consumers of one client:

- Metadata extraction from the meeting description (free-form text →
  structured `{ project, title, owner, … }`).
- Per-mode summarizers (rolling transcript → mode-specific schema).

Both go through `LlmClient::extract_with_prompt::<Schema>`, which is
backed by [`rig`](https://github.com/0xPlaygrounds/rig) and routes to
Bedrock, OpenAI, or Anthropic-direct based on
`MEETING_COMPANION_LLM_PROVIDER`.

Decision in [ADR-0005](adr/0005-multi-provider-llm.md).

---

## 8. Persistence

Two state surfaces survive a process restart:

- **Settings** (server URL, server token, last metadata) — in the PWA,
  via `bridge.setLocalStorage` with browser `localStorage` fallback.
  Per-key debounced writes, ~500 ms.
- **mnemo memories** — in mnemo. AgentCore-managed, not under the
  companion's direct control.

The server itself is stateless across restarts. A meeting in flight
when the server crashes is lost; the PWA reconnects, sees Idle, and
the user starts again. Acceptable for the personal-project scope.

---

## 9. Status and roadmap

**Phase 0 (simulator-first stub) — complete.** Server (110 tests),
PWA (66 unit tests + 2 simulator-gated integration tests skipped on no
sim) green. Four glasses layouts, ReconnectingSocket reconciliation,
Soniox dictation flow, end-to-end against EvenHub simulator.

**Phase 1 (real hardware sideload) — pending.** Manual checklist in
`packages/pwa/README.md`. Open items: empirical source-distinction for
G2 vs R1 events (ADR-0001 follow-up), full-restart persistence
verification (ADR-0003 follow-up), LVGL font metrics calibration.

**Phase 2 (real audio + LLM extraction + memory) — complete.**

- Step 16 — LLM metadata extraction, multi-provider via `rig`
  (ADR-0005).
- Step 15 — live ScreenCaptureKit audio + Soniox STT + parallel mode
  summarizers (ADR-0006, ADR-0007).
- Step 18 — mnemo memory integration: streaming push, summary at stop,
  recall at start, per-mode prior-context toggle (ADR-0008).

Step 17 (dynamic mode catalog) was deferred indefinitely — see
[ADR-0007](adr/0007-summarizer-architecture.md) for why.

**Future directions** (not currently scheduled): meeting-specific
mnemo namespace (`/meetings/{actorId}/{meetingId}/`) when mnemo's
strategy layer can support it; multi-meeting browse / recap UI;
calendar integration.

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
