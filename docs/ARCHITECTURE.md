# Meeting Companion — Architecture (v12)

Personal project. Real-time meeting summarization on Even Realities G2 via a
laptop-hosted summarizer (Rust), a phone-hosted PWA, and the glasses as a thin
display. Iterates against the Even Hub simulator before touching real hardware.

## 0. Status

- **Phase 0 (simulator-first stub) — complete.** Both components end-to-end
  testable against the EvenHub simulator. `packages/server/` (79 tests) and
  `packages/pwa/` (64 unit tests + 2 simulator-gated integration tests) are
  green. The `bridge.setLocalStorage` settings flow, the four glasses
  layouts, the ReconnectingSocket reconciliation, the listening flow with
  Soniox STT + VAD, and the phone-screen UI all work end-to-end against the
  mock content the server produces every 3 seconds.
- **Phase 1 (real hardware sideload) — pending.** See the manual checklist in
  [`packages/pwa/README.md`](../packages/pwa/README.md). Notable open tasks:
  - Discover the `EventSourceType` field path on bridge events (ADR-0001
    follow-up).
  - Confirm `bridge.setLocalStorage` persists across a full Even Realities
    App restart (ADR-0003 follow-up).
  - Recalibrate `LINES_PER_SCREEN` / `CHARS_PER_LINE` in `layout-active-list`
    against the real LVGL font metrics.
- **Phase 2 (real audio + extraction pipeline) — pending.** See §10 steps
  15-18. Mock content generator + simulated extraction get replaced by
  ScreenCaptureKit + STT/summarizer + LLM-based metadata extraction.

The §6 wire contract is identical across phases — only internals evolve.

The "SDK Reality Corrections" appendix (§11) supersedes parts of §4 / §5 / §10
that were drafted before the Even Hub SDK was fully read; it is the binding
text where it conflicts with the original sections.

Major design decisions that aren't obvious from the code or that have real
alternatives are recorded as ADRs under [`docs/adr/`](adr/):

- [ADR-0001 — Gesture map](adr/0001-gesture-map.md): phone-only lifecycle
  gestures for Phase 0; SDK has no long-press event.
- [ADR-0002 — Active-list rendering](adr/0002-active-list-rendering.md):
  TextContainer with formatted lines; `ListContainer` cannot be updated
  in place.
- [ADR-0003 — Persistence via bridge](adr/0003-persistence-via-bridge.md):
  `bridge.setLocalStorage` only; browser `localStorage` is unreliable in the
  Flutter WebView.

## 1. System Topology

Three components, two transports.

```
┌──────────────────┐  WebSocket   ┌──────────────────────┐   BLE    ┌──────────────┐
│  Laptop Server   │◄────────────►│  Phone PWA           │◄────────►│  G2 Glasses  │
│  (Rust)          │  intents ↑   │  (Even Realities App │  (Even   │  (display +  │
│  state owner     │  events ↓    │   WebView)           │   App    │   ring / pad │
└──────────────────┘              │   + local STT for    │   owns)  │   + 4-mic    │
        ▲                         │     description      │          │     array)   │
        │ ScreenCaptureKit        └──────────────────────┘          └──────────────┘
        │   → STT
        │   → summarizer
        │   → memory system
        │ + LLM extraction on start_meeting.description
        ▼
   (existing pipeline + memory system — out of scope for this doc)
```

**Meeting state machine.** Top-level state lives on the server:

```
        start_meeting              stop_meeting
   idle ─────────────► active ───────────────► idle
                       ▲    │
                       │    │ pause
                       │    ▼
              resume   paused
                ◄──────
```

- `idle` — no audio capture, glasses prompt user to start.
- `active` — capturing audio, summarizing, emitting events.
- `paused` — capture stopped but state retained; resume continues the same meeting.

**Glasses-local sub-state during idle (PWA-driven).** Before transitioning to
active, the PWA can put the glasses in a `listening` view to capture an
optional spoken meeting description via the G2 mic. This is local UI state,
not a server-side meeting state. The server only sees the result via
`start_meeting { description }`.

```
        long-press           VAD-silence / ring-tap
  idle ──────────────► listening ────────────────────► (start_meeting fires)
   │                       │
   │                       │ ring long-press / phone Cancel
   └────── cancel ◄────────┘
```

**Input-surface convention (glasses).**

- **Ring** — in-flow actions: scroll, expand, mark, mode cycle.
- **Temples** — lifecycle: start (with optional description), stop. Distinct
  physical motion makes these gestures intentional.
- **Phone** — explicit, deliberate controls always available as a parallel path.

Lifecycle is consistently bound to **left temple long-press**: in idle it
opens the listening view; in active/paused it triggers stop confirmation.

## 2. Design principles

- **Stable contract, mutable internals.** The WebSocket message contract is
  the only thing the PWA depends on. Audio capture, STT, summarization,
  memory enrichment, LLM extraction, available modes, mode labels, display
  tags, item update strategies — all swappable behind it.
- **No bespoke content types.** The PWA and glasses know nothing about
  "highlights", "actions", or "transcript" by name. They know about *modes*
  generically.
- **Single source of truth.** Server owns meeting state. PWA holds only
  ephemeral UI state (current view, listening transcript buffer,
  highlightedIndex, scroll, connection indicators). Glasses hold no state.
- **Audio stays local where it can.** The meeting description is captured
  on G2 mic, transcribed on the phone (PWA), and only the resulting text
  crosses the WS to the server. Meeting audio is captured by the laptop
  directly via ScreenCaptureKit — never re-routed through phone or glasses.
- **Data-driven UI.** Mode list, mode labels, update strategies, header
  display tags, available metadata fields — all sent over WS.
- **Optimistic UI, reconcile via events.** Phone-side gestures get an
  immediate glasses ack. Server events confirm or correct.
- **Confirmation for destructive actions only.** Stop requires deliberate
  confirmation when triggered from the glasses. Start does not.
- **Snapshot on connect.** No replay logs.
- **Versioned protocol, additive evolution.**

## 3. Component — Laptop Server (Rust)

Owns meeting state. Captures system audio via ScreenCaptureKit. Hosts the WS
endpoint. PWA and glasses are renderers driven by its events.

### Why Rust
- `screencapturekit` crate provides safe bindings to Apple's framework.
- Single binary, single language, no Node/Swift split.
- Calls out to existing STT/LLM services over HTTP — that part is trivial in any language.

### Crates
- `screencapturekit` — system audio capture
- `tokio` + `tokio-tungstenite` — async runtime + WebSocket server
- `serde` + `serde_json` — message serialization
- (your choice) STT and LLM clients via `reqwest` or `aws-sdk-bedrockruntime`

### Responsibilities

- Hold meeting state machine: `idle` / `active` / `paused`.
- Hold the catalog of `available_modes` (id, label, update_strategy).
- Compute and emit the `display_tag` for the current mode.
- On `start_meeting` intent:
  1. Request Screen Recording permission (first run).
  2. Start `SCStream`, wire PCM frames into STT pipeline.
  3. If `description` is present: run LLM extraction prompt on the text to
     pull structured metadata fields. Merge with `metadata` from the same
     intent (extraction values fill missing keys; explicit values from the
     intent override extraction on conflict — manual wins).
  4. Emit `meeting_state_changed { active }`, `metadata_changed { merged }`,
     `mode_changed { default mode }`.
- On `stop_meeting` intent: stop `SCStream`, finalize state, optionally archive.
- Run STT and summarization (existing pipeline; orchestrated from Rust).
- Maintain per-meeting state: items per mode, status, metadata.
- For each mode: produce `items_update` events at the cadence/shape that
  mode needs.
- On `expand_item` intent: produce or fetch detail; reply via single-item
  upsert in `items_update`.
- Read metadata KV pairs and pass relevant keys (e.g., `project`) to the
  memory system / summarizer pipeline.
- Expose authenticated WebSocket on LAN.
- Receive intents from PWA → mutate state → emit events.
- Broadcast events to all connected PWA clients.

### macOS permissions
- `Info.plist` (or runtime equivalent): `NSScreenCaptureUsageDescription`.
- First run: macOS prompts for Screen Recording permission. Permission attaches
  to the *binary* (or your terminal/IDE if running via `cargo run`).

### Inbound: Intents (PWA → Server)

| Intent           | Payload                                                       | Effect                                                  |
|------------------|---------------------------------------------------------------|---------------------------------------------------------|
| `start_meeting`  | `{description?: string, metadata?: Record<string,string>}`     | idle → active; LLM extract from description; start audio capture |
| `stop_meeting`   | `{}`                                                          | active/paused → idle; stop capture, archive state        |
| `pause`          | `{}`                                                          | active → paused; stop capture, retain state              |
| `resume`         | `{}`                                                          | paused → active; restart capture                         |
| `set_mode`       | `{mode: string}`                                              | Switch active mode (must be in `available_modes`)        |
| `set_metadata`   | `{key: string, value: string \| null}`                        | Set/update a metadata key; null deletes it               |
| `mark_moment`    | `{t: number, note?: string}`                                  | Tag current timestamp as significant                     |
| `expand_item`    | `{item_id: string}`                                           | Request detail for an item; server responds via items_update |

### Outbound: Events (Server → PWA)

| Event                      | Payload                                                  | Notes                                          |
|----------------------------|----------------------------------------------------------|------------------------------------------------|
| `snapshot`                 | full current state (see §6)                              | Sent on connect/reconnect                      |
| `meeting_state_changed`    | `{meeting_state}`                                         | idle ↔ active ↔ paused transitions             |
| `available_modes_changed`  | `{available_modes: ModeOption[]}`                         | Mode catalog or strategies changed             |
| `mode_changed`             | `{mode, display_tag?, items: Item[]}`                     | Atomic switch: new mode + its current state    |
| `display_tag_changed`      | `{tag?: string}`                                          | Tag updated for current mode                   |
| `metadata_changed`         | `{metadata: Record<string,string>}`                       | Full metadata after manual or extraction-driven update |
| `items_update`             | `{items: Item[]}`                                         | Generic content update for current mode (see below) |
| `status`                   | `{listening, paused, error?}`                             | Heartbeat + state                              |

`items_update` semantics determined by mode's `update_strategy`:

- **`replace`** → PWA replaces the entire list with the payload.
- **`append`** → PWA upserts each item by `id`. New IDs append; existing IDs
  update in place. Naturally supports detail enrichment.

### Connection semantics

- Shared-secret token in URL: `ws://laptop.local:7331/?token=...`
- On client connect: server immediately emits `snapshot` for the active state.
- Heartbeat every 10s; client reconnects with exponential backoff.
- Events idempotent by `id`. Last-write-wins for state.

## 4. Component — Phone PWA

Hosted as a PWA, opened inside the Even Realities App via the Hub PWA route.
Routes between ring/temple gestures, phone-screen controls, websocket events,
glasses bridge calls, and **local STT for meeting description**.

### Responsibilities

- Connect to laptop WebSocket; auto-reconnect; show connection status.
- Validate `protocol_version` from snapshot on connect.
- Populate mode dropdown from server-provided `available_modes`.
- Subscribe to `bridge.onEvenHubEvent` for ring/touchpad gestures.
- Translate gestures → intents based on current `meeting_state` AND current
  glasses view.
- Translate WS events → glasses bridge calls.
- On `items_update`: look up current mode's `update_strategy` and apply.
- Track local glasses view state: `list` | `detail` | `confirm_stop` | `listening`.
- Provide phone-screen UI: metadata KV editor, mode selector, settings,
  Start/Stop/Cancel-listening buttons.
- **Run local STT** for the meeting description: activate G2 mic via
  `bridge.audioControl(true)`, consume PCM frames from
  `event.audioEvent.audioPcm`, feed to a chosen STT (Soniox, Whisper, etc),
  emit incremental transcription to the glasses, deactivate mic on
  VAD-silence or user gesture.

### Phone-screen layout

```
┌─────────────────────────────────┐
│ ● WS  ● BLE   State: idle       │  status row
├─────────────────────────────────┤
│ Mode: [<label> ▾]      <tag>    │
├─────────────────────────────────┤
│ Metadata                        │
│   project   helix      [✕]      │
│   title     Q1 review  [✕]      │
│   client    -          [✕]      │
│   [+ key]   [+ value]   [add]   │
├─────────────────────────────────┤
│ [Start meeting]                 │  starts directly, no listening view
│   (becomes [Pause][Stop] when   │
│    meeting active;              │
│    becomes [Cancel] when        │
│    listening)                   │
├─────────────────────────────────┤
│ Items so far:                   │
│  • <text>                       │
└─────────────────────────────────┘
```

### Listening flow (G2 mic + local STT)

Triggered when user enters the listening view from idle. The phone Start
button bypasses this entirely — phone Start fires `start_meeting` immediately
with no description.

1. Entry: PWA receives left temple long-press in idle.
2. PWA calls `bridge.audioControl(true)` — G2 4-mic array activates,
   PCM frames begin streaming over BLE → bridge → PWA.
3. PWA sets `glassesView = "listening"`, rebuilds glasses to Layout E.
4. PWA pipes incoming PCM into local STT. As text arrives:
   - PWA accumulates the running transcription buffer.
   - PWA pushes the latest text to the glasses body via
     `textContainerUpgrade()` so the user sees what's being captured.
5. VAD detects ~2.5s of silence → end of speech.
6. PWA calls `bridge.audioControl(false)` → mic off.
7. PWA sends `start_meeting { description: <accumulated text>, metadata: <current KV> }`.
8. Server processes, returns `meeting_state_changed`, `metadata_changed`,
   `mode_changed`. PWA transitions glasses to Layout B.

**Skip / commit / cancel paths during listening:**

- Ring single tap → commit early. PWA stops mic, sends `start_meeting`
  with whatever transcript exists (could be empty if user tapped immediately).
- Ring long press → cancel. PWA stops mic, discards transcript, returns
  glasses to Layout A.
- Phone screen Cancel button → same as ring long press.
- Auto VAD-silence → commit (the normal path).

### Glasses view state

Four views: `list` (default in active), `detail`, `confirm_stop`, `listening`.

- `listening` is only reachable from idle.
- View resets to `list` on `mode_changed`, `meeting_state_changed`,
  reconnect, or auto-dismiss timeout.
- While in `detail`, incoming `items_update` events still mutate underlying
  list state.
- `confirm_stop` auto-dismisses to previous view after ~3 seconds.

### Gesture map

**When `meeting_state == idle` AND `view == "list"` (Layout A):**

| Surface              | Gesture            | Action                                          |
|----------------------|--------------------|-------------------------------------------------|
| Left temple          | long press         | Enter `listening` view (Layout E)                |
| Ring                 | (any)              | (no-op)                                          |

**When `meeting_state == idle` AND `view == "listening"` (Layout E):**

| Surface              | Gesture            | Action                                                  |
|----------------------|--------------------|---------------------------------------------------------|
| Ring                 | single tap         | Commit early: stop mic, send `start_meeting` with current transcript |
| Ring                 | long press         | Cancel: stop mic, discard transcript, back to Layout A   |
| Left temple          | long press         | Cancel (toggle off — same as ring long press)            |
| (auto VAD)           | 2.5s silence       | Commit: stop mic, send `start_meeting`                   |

**When `meeting_state == active` AND `view == "list"`:**

| Surface              | Gesture            | Action                                                  |
|----------------------|--------------------|---------------------------------------------------------|
| Ring                 | scroll up/down     | Local highlight move                                     |
| Ring                 | single tap         | If item has `detail`: enter detail view.<br/>If not: send `expand_item` and enter detail view with loading state. |
| Ring                 | double tap         | `mark_moment`                                            |
| Ring                 | long press         | Cycle mode locally, then `set_mode`                      |
| Left temple          | long press         | Enter `confirm_stop` (Layout D)                          |

**When `meeting_state == active` AND `view == "detail"`:**

| Surface              | Gesture            | Action                                                  |
|----------------------|--------------------|---------------------------------------------------------|
| Ring                 | single tap         | Back to list view                                        |
| Ring                 | double tap         | `mark_moment`                                            |
| Ring                 | long press         | Back to list view (alt path)                             |
| Left temple          | long press         | Enter `confirm_stop` (Layout D)                          |

**When `meeting_state == active` AND `view == "confirm_stop"`:**

| Surface              | Gesture            | Action                                                  |
|----------------------|--------------------|---------------------------------------------------------|
| Ring                 | single tap         | Confirm: send `stop_meeting`, return to Layout A         |
| Ring                 | scroll / long press | Cancel: return to previous view                          |
| Left temple          | long press         | Cancel (toggle off)                                      |
| (auto)               | 3s timeout         | Cancel: return to previous view                          |

### Acknowledgment policy

Optimistic for ring/temple gestures. The listening view's live transcription
*is* the ack — user sees their words appearing as they speak.

## 5. Component — Glasses Display

Five layouts: idle, listening, active-list, active-detail, stop-confirm.

### Layout A — Idle

```
┌────────────────────────────────────────┐
│ ⌁ Ready                  <display_tag> │  TextContainer #1 (header)
├────────────────────────────────────────┤  containerID: 1, no focus
│                                        │
│  Long-press left temple to start       │  TextContainer #2 (body)
│                                        │  containerID: 2, isEventCapture: 1
└────────────────────────────────────────┘
```

### Layout E — Listening (idle sub-state)

```
┌────────────────────────────────────────┐
│ ⌁ Listening…                  ●        │  TextContainer #1 (header)
├────────────────────────────────────────┤  containerID: 1, no focus
│ <running transcription, updates as     │
│  text arrives from local STT>          │  TextContainer #2 (body)
│ ...                                    │  containerID: 2, isEventCapture: 1
└────────────────────────────────────────┘
```

- Header `●` indicates active mic capture (server provides nothing here —
  it's PWA-driven UI).
- Body shows incremental transcript via `textContainerUpgrade()` calls as
  STT produces text. Full body refresh on each update is fine — the text is
  short and the cadence is roughly word-rate.
- When transcript exceeds visible area, PWA truncates to show the most recent
  portion (tail) — user wants to see what they just said, not the start.

### Layout B — Active / List

```
┌────────────────────────────────────────┐
│ ⌁ <mode label>            <display_tag>│  TextContainer #1 (header)
├────────────────────────────────────────┤  containerID: 1, no focus
│ ▶ <item text 1>                       │
│   <item text 2>                       │  ListContainer #2 (body)
│   <item text 3>                       │  containerID: 2, isEventCapture: 1
│   ...                                 │
└────────────────────────────────────────┘
```

### Layout C — Active / Detail

```
┌────────────────────────────────────────┐
│ ⌁ <mode label>            <display_tag>│  TextContainer #1 (header)
├────────────────────────────────────────┤  containerID: 1, no focus
│ <item.text>                            │
│ ──────────                             │  TextContainer #2 (body)
│ <item.detail>                          │  containerID: 2, isEventCapture: 1
└────────────────────────────────────────┘
```

### Layout D — Stop confirmation

```
┌────────────────────────────────────────┐
│ ⌁ Confirm Stop?                        │  TextContainer #1 (header)
├────────────────────────────────────────┤  containerID: 1, no focus
│ ▶ Yes, stop meeting                   │
│                                        │  ListContainer #2 (single item)
│   (auto-cancels in 3s)                │  containerID: 2, isEventCapture: 1
└────────────────────────────────────────┘
```

### Update strategy

- `textContainerUpgrade()` for content updates (flicker-free).
- `rebuildPageContainer()` on layout transitions (idle ↔ listening ↔ active;
  list ↔ detail ↔ confirm_stop; mode change).
- Body in Layout B is updated on `items_update` per mode strategy.
- Body in Layout C is updated only when the current detail item is upserted.
- Body in Layout E is updated by the PWA from local STT — no server events.
- Layout D is purely PWA-driven.
- Diff before sending — only push on actual content delta.

### Display constraints

- 576×288 px per eye; both eyes render the same image.
- 4-bit greyscale (16 shades of green); single built-in font.
- Max 4 containers per page; exactly one with `isEventCapture: 1`.
- SDK quirks: `borderRdaius` is the correct (typo'd) property name (string,
  not number). `CLICK_EVENT (0)` is normalized to `undefined` — handle both.

## 6. Message contract — concrete shapes

```ts
type MeetingState = "idle" | "active" | "paused"
type UpdateStrategy = "replace" | "append"   // append = upsert by id

type ModeOption = {
  id: string
  label: string
  update_strategy: UpdateStrategy
}

type Item = {
  id: string
  text: string
  detail?: string
  t: number
  meta?: Record<string, unknown>
}

type Status = {
  listening: boolean
  paused: boolean
  error?: string
}

type Intent =
  | { type: "start_meeting"
      description?: string                  // transcript of spoken description (optional)
      metadata?: Record<string, string> }   // already-set KV pairs from PWA
  | { type: "stop_meeting" }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "set_mode"; mode: string }
  | { type: "set_metadata"; key: string; value: string | null }
  | { type: "mark_moment"; t: number; note?: string }
  | { type: "expand_item"; item_id: string }

type Event =
  | { type: "snapshot"
      protocol_version: number
      meeting_state: MeetingState
      available_modes: ModeOption[]
      mode: string
      display_tag?: string
      metadata: Record<string, string>
      items: Item[]
      status: Status }
  | { type: "meeting_state_changed"; meeting_state: MeetingState }
  | { type: "available_modes_changed"; available_modes: ModeOption[] }
  | { type: "mode_changed"; mode: string; display_tag?: string; items: Item[] }
  | { type: "display_tag_changed"; tag?: string }
  | { type: "metadata_changed"; metadata: Record<string, string> }
  | { type: "items_update"; items: Item[] }
  | { type: "status"; status: Status }
```

### Contract evolution rules

- **Current version: 1.** Every snapshot carries `protocol_version`.
- **Additive changes are free.** New event types, optional fields, mode IDs,
  metadata keys, update strategies.
- **Breaking changes bump the version.** Server keeps backward compatibility
  for at least one prior version during transition.
- **PWA validates on snapshot.** Major mismatch → warning + halt processing.
- **Server is the only writer of the contract.**

## 7. End-to-end flow

Happy-path including describe-via-G2-mic → start, items flow, expand,
mode switch, and stop.

```mermaid
sequenceDiagram
    actor User
    participant Glasses as G2 Glasses (incl. mic)
    participant PWA as PWA (incl. local STT)
    participant Server as Laptop Server (Rust)

    %% --- Connect ---
    Note over PWA,Server: Connect & initial render
    PWA->>Server: WS connect (token in URL)
    Server-->>PWA: snapshot { ..., meeting_state: "idle", metadata: {} }
    PWA->>Glasses: rebuildPageContainer (Layout A — idle)
    Glasses-->>User: "Long-press left temple to start"

    %% --- Listening (description) ---
    Note over User,Server: Optional: describe meeting via G2 mic
    User->>Glasses: left temple long press
    Glasses-->>PWA: bridge event (LONG_PRESS, source=left_temple)
    PWA->>Glasses: bridge.audioControl(true)
    PWA->>PWA: glassesView = "listening"; init local STT
    PWA->>Glasses: rebuildPageContainer (Layout E — listening)

    loop while user speaks
        Glasses-->>PWA: audioEvent.audioPcm (PCM frames)
        PWA->>PWA: feed PCM to local STT
        PWA->>Glasses: textContainerUpgrade (body — running transcript)
    end

    alt VAD silence ~2.5s
        Note over PWA: auto-commit
    else ring single tap
        Note over PWA: early commit
    else ring long press / phone Cancel
        Note over PWA: cancel — discard transcript<br/>back to Layout A, abort flow
    end

    PWA->>Glasses: bridge.audioControl(false)
    PWA->>Server: { type: "start_meeting",<br/>description: "<final transcript>",<br/>metadata: <current KV> }

    %% --- Server processes start ---
    Server->>Server: idle → active; start SCStream + STT
    Server->>Server: LLM extract metadata from description
    Server->>Server: merge: manual KV wins on conflict
    Server-->>PWA: meeting_state_changed { "active" }
    Server-->>PWA: metadata_changed { merged metadata }
    Server-->>PWA: mode_changed { default mode, display_tag, items: [] }
    PWA->>PWA: phone KV editor re-renders w/ extracted fields
    PWA->>PWA: glassesView = "list"
    PWA->>Glasses: rebuildPageContainer (Layout B)

    %% --- Items flow ---
    Note over Server,Glasses: Items flow as content is produced
    Server-->>PWA: items_update { items }
    alt mode strategy = "replace"
        PWA->>Glasses: textContainerUpgrade (body — replace)
    else mode strategy = "append" (upsert by id)
        PWA->>Glasses: textContainerUpgrade (body — upsert)
    end

    %% --- Expand ---
    User->>Glasses: ring single tap (highlighted item)
    Glasses-->>PWA: bridge event (TAP, source=ring)
    PWA->>Glasses: rebuildPageContainer (Layout C — detail)

    User->>Glasses: ring single tap
    PWA->>Glasses: rebuildPageContainer (Layout B — list)

    %% --- Switch mode ---
    User->>Glasses: ring long press
    Glasses-->>PWA: bridge event (LONG_PRESS, source=ring)
    PWA->>Server: { type: "set_mode", mode: "<next>" }
    Server-->>PWA: mode_changed { mode, display_tag, items }
    PWA->>Glasses: rebuildPageContainer (Layout B with new mode)

    %% --- Stop ---
    Note over User,Server: Stop meeting
    User->>Glasses: left temple long press
    Glasses-->>PWA: bridge event (LONG_PRESS, source=left_temple)
    PWA->>PWA: glassesView = "confirm_stop" (3s timeout)
    PWA->>Glasses: rebuildPageContainer (Layout D)

    alt user confirms
        User->>Glasses: ring single tap
        PWA->>Server: { type: "stop_meeting" }
        Server-->>PWA: meeting_state_changed { "idle" }
        PWA->>Glasses: rebuildPageContainer (Layout A — idle)
    else cancel (gesture or timeout)
        PWA->>Glasses: rebuildPageContainer (back to previous view)
    end
```

Notes on the flow:

- **Audio for description never leaves the laptop's LAN.** PCM streams from
  G2 → phone over BLE; STT runs on the phone; only resulting text crosses
  the WS to the server.
- **Description and start are atomic.** `start_meeting` carries the
  description. Server runs LLM extraction inline and emits metadata before
  the meeting is fully running. Manual KV values from the phone editor
  override extracted values on conflict.
- **Phone Start button bypasses listening.** Going through the phone is the
  "I don't want to describe anything, just start" path.
- **Skip-description mid-listening (ring tap)** sends `start_meeting` with
  whatever's transcribed so far (possibly empty string). Server treats empty
  description as "no extraction" and just starts the meeting.
- **Cancel-listening returns to idle.** Discard the transcript, no intent
  fired.
- **`glassesView` is PWA-local.** Server never sees the listening view at all.
- **Reconnect mid-listening would be awkward.** If the WS drops while the
  PWA is in listening view, the PWA should cancel the listening flow on
  reconnect (snapshot will say `idle`) and return to Layout A.

## 8. Constraints and risks to track

- **iOS WebView lifecycle.** PWA inside the Even Realities App may be
  throttled or suspended on background/lock. Validate session continuity
  early. Escape hatches: BLE-direct from laptop, or native iOS shim.
- **Input source disambiguation.** Lifecycle gestures assume the SDK's
  bridge events expose input source (ring/left temple/right temple).
  Validate in Phase 0.
- **G2 mic via `bridge.audioControl(true)`.** Validate that the legacy
  30-second cap from the original Even AI flow does not apply to
  third-party SDK use, or design around it (e.g., enforce a 25s soft cap
  on description length and force-commit if exceeded).
- **Local STT in WebView.** Whichever engine is chosen (Soniox, Deepgram,
  Whisper-in-browser, etc), validate it runs in the iOS WebView under the
  Even Realities App. WASM-heavy options may have memory issues.
- **VAD reliability.** Silence-based end-of-speech detection is the default
  commit path. If the user pauses mid-sentence the system might commit
  early. The 2.5s threshold matches Even's existing tooling — start there,
  tune from real use.
- **macOS permissions.** Screen Recording permission attaches to the
  binary. During `cargo run`, attaches to terminal/IDE.
- **ScreenCaptureKit audio scope.** Captures whatever's playing through the
  active output device.
- **Display refresh.** Container text updates are fast; full-frame bitmaps
  1–3 Hz.
- **Display tag length.** Keep `display_tag` short (≤ ~16 chars).
- **Detail length.** TextContainer overflow on G2 not validated. Keep
  `Item.detail` ≤ ~8 lines / ~200 chars; fallback to ListContainer.
- **Replace-strategy churn.** Prefer `append` for high-cadence modes.
- **LLM extraction latency.** STT on phone is fast (running text), LLM
  extraction on server takes a few seconds. The user sees the listening
  view dismiss → glasses jump to Layout B (empty body) → metadata appears
  in phone UI a beat later when extraction finishes. Acceptable; the
  meeting itself starts immediately.
- **LLM extraction errors.** If extraction fails, server emits empty
  metadata diff and an `error` in `status`. PWA shows it in status row.
  Manual KV editor still works.
- **Description privacy.** The transcribed description goes to whatever
  LLM the server uses for extraction. Same posture as meeting summarization
  — but worth noting that even a brief description can leak project context
  to an LLM provider.
- **Network reachability.** Laptop ↔ phone over LAN. Tailscale/cloudflared
  for travel.
- **Auth.** Shared secret token in WS URL.

## 9. Out of scope

- STT and summarization for the meeting itself (existing pipeline;
  orchestrated from the Rust server but logic lives elsewhere). Speaker
  labels, if produced, surface via `display_tag`.
- LLM metadata extraction prompt/logic. Server-internal.
- Memory-system integration for project-aware summaries.
- Local STT engine selection for the description (Soniox, Whisper, etc.).
  PWA-internal — choose what works.
- Native iOS shim (only if PWA lifecycle proves blocking).
- Mode-specific tag computation logic.
- Mode-specific update logic.
- Detail content authoring.
- Pause/resume from glasses (deferred — phone screen suffices for now).
- Mid-meeting re-describe via the same listening flow. Possible future
  enhancement; would reuse the same path with `set_metadata`-style merge
  on the resulting extraction.

## 10. Build order — simulator first

Phase 0 — simulator-only iteration loop, no hardware required.

1. **Stub server.** Rust binary with WS handling, fake event timer, fake
   `available_modes`, fake state machine. Stubs `start_meeting`: ignores
   `description` for now, just transitions state. **Keep this stub
   permanently** as the contract reference.
2. **Validate input source discriminator.** Log raw bridge events for
   ring tap, ring long-press, left temple long-press, right temple long-press.
3. **Validate G2 mic + `bridge.audioControl`.** Log audioPcm event flow.
   Check session-length cap (30s legacy?). Test in simulator first if it
   supports mic injection; otherwise defer to Phase 1.
4. **Validate STT engine in WebView.** Pick one (Soniox is well-documented
   in the Even toolkit), run it inside the simulator/WebView, confirm
   end-to-end audio → text.
5. **Minimal PWA against `evenhub-simulator`.** Connect, validate
   `protocol_version`, populate mode dropdown, render Layout A. Wire left
   temple long-press → enter listening view (Layout E).
6. **Listening flow.** Activate mic, run STT, render running transcript,
   detect VAD silence, commit `start_meeting { description }`. Wire ring
   tap (commit early) and ring long-press (cancel) paths.
7. **Generic `items_update` handling** with strategy lookup.
8. **Detail view (Layout C) and `expand_item` flow.**
9. **Stop confirmation (Layout D) flow.**
10. **`mark_moment`** with optimistic glasses ack overlay.
11. **Mode cycling** (ring long-press + phone dropdown).
12. **Metadata KV editor** with `set_metadata` round-trips.

Phase 1 — real hardware.

13. **QR-sideload the PWA to actual G2.** Verify all five layouts and
    gesture handling. Test G2 mic and STT end-to-end. Measure latency
    from speech → glasses transcript update.
14. **Validate iOS WebView lifecycle** including mic permission
    propagation through the host app.

Phase 2 — real audio + extraction pipeline.

15. **Real meeting audio capture.** Wire `screencapturekit` audio capture
    to the existing STT/summarizer.
16. **Real LLM metadata extraction.** Wire the extraction prompt; merge
    with manual metadata.
17. **Real available_modes catalog.**
18. **Memory-system enrichment.** Server reads `metadata.project` for
    project-aware modes.

Each phase is independently testable. Stub server stays in the repo
permanently. Don't move forward until the previous phase's contract is solid.

## 11. SDK Reality Corrections (appendix)

The PWA-related sections of this document (§4 Phone PWA, §5 Glasses
Display, §10 build order steps tied to gestures) were drafted before
the Even Hub SDK surface had been fully read. Subsequent investigation
of the [Even Hub developer docs](https://hub.evenrealities.com/docs/)
and the `@evenrealities/even_hub_sdk` TypeScript definitions surfaced
hard constraints that contradict parts of the original design. This
appendix supersedes those bits where it conflicts; the rest of the
document stands.

The full implications and the alternatives we considered are recorded
as Architecture Decision Records under [`docs/adr/`](adr/).

### 11.1 No long-press event in the SDK

The [Input & Events guide](https://hub.evenrealities.com/docs/guides/input-events)
enumerates exactly four input event types from both the G2 temple
touchpads and the R1 ring: `CLICK_EVENT (0)`, `DOUBLE_CLICK_EVENT (3)`,
`SCROLL_TOP_EVENT (1)`, `SCROLL_BOTTOM_EVENT (2)`. **There is no
long-press event.**

§4 (Gesture map) and §1 (Input-surface convention) therefore cannot be
implemented as written. Resolution per
[ADR-0001](adr/0001-gesture-map.md): for Phase 0, **lifecycle gestures
move entirely to the phone screen**. Glasses-side gestures are reserved
for in-flow controls (scroll, expand, mark moment, mode cycle via
swipe). Promotion of `DOUBLE_CLICK_EVENT` to a lifecycle role is left
open until real hardware is available and a long-form usability check
can be done.

The G2-vs-R1 source distinction is described as "now possible" in the
guide but the field on the event payload is undocumented; this will be
discovered empirically in Phase 0 by logging raw events.

### 11.2 ListContainer cannot be updated in place

The [Display & UI System guide](https://hub.evenrealities.com/docs/guides/display)
makes clear that `textContainerUpgrade` is the only flicker-free update
path on hardware, and it operates **only on `TextContainer`**. List
containers cannot be updated incrementally — any change to the items
requires a full `rebuildPageContainer`, which the same guide flags as
causing brief flicker.

§5 Layout B describes the active-list body as a `ListContainer` updated
via `textContainerUpgrade`, which is not a valid SDK call combination.
Resolution per [ADR-0002](adr/0002-active-list-rendering.md): **render
the active-list body as a single `TextContainer`** with formatted
multi-line content (one item per line, plus a cursor glyph for the
highlighted entry). The PWA tracks the highlight cursor locally and
re-emits the formatted text via `textContainerUpgrade` on every items
update or scroll. This trades native firmware-level scroll for
flicker-free updates at the cadence the meeting summarizer produces
items.

### 11.3 Container limits

§5 says "Max 4 containers per page; exactly one with `isEventCapture: 1`."
The actual limit per the Display & UI guide is **4 image containers
plus 8 other (text/list) containers per page**, with exactly one
container holding `isEventCapture: 1`. The "exactly one event capture"
half is correct as written; the cap should read 4 + 8.

### 11.4 Persistence: bridge storage, not browser storage

§4 doesn't specify where PWA-side state (server URL, token, Soniox
credentials, last-used metadata) is persisted. The
`@evenrealities/even_hub_sdk` reference is unambiguous on this point:
**browser `localStorage` and `IndexedDB` are not reliably persistent
across app restarts** in the Flutter WebView the Even Realities App
provides. Only `bridge.setLocalStorage` / `bridge.getLocalStorage`
survive restart cycles.

Resolution per
[ADR-0003](adr/0003-persistence-via-bridge.md): all PWA-side persistent
state goes through `bridge.setLocalStorage` / `bridge.getLocalStorage`.
Vite `import.meta.env.VITE_*` variables seed first-run defaults so
developers can avoid retyping credentials on a fresh install.

### 11.5 Permissions and `app.json`

§4 doesn't address how the PWA declares its capabilities to the host.
Every Even Hub plugin ships an `app.json` manifest with a
`permissions` array. Two permissions matter for this app:

- `g2-microphone` — required for `bridge.audioControl(true)` to capture
  the spoken meeting description from the G2 mic array. We deliberately
  do **not** request `phone-microphone`; the phone mic is never used.
- `network` — required to reach the laptop server, with a per-origin
  `whitelist` (full origins, no wildcards). For LAN dev:
  `http://localhost:7331` plus the developer's LAN address. For
  production: a TLS-terminated address (Tailscale Funnel, cloudflared,
  etc.) since the WebView origin is `https://` and would otherwise hit
  mixed-content blocks.

The simulator skips the whitelist gate; real glasses enforce it. CORS
headers must be set correctly on the server for both surfaces (see the
[Networking guide](https://hub.evenrealities.com/docs/guides/networking)).

### 11.6 PWA hosting and build pipeline

§4 says the PWA is "hosted as a PWA, opened inside the Even Realities
App via the Hub PWA route." The concrete shape of this is:

- Built with **Vite + TypeScript** using the `vanilla-ts` template.
  Runtime dep `@evenrealities/even_hub_sdk`; dev deps
  `@evenrealities/evenhub-cli` and `@evenrealities/evenhub-simulator`.
- Dev loop: `vite dev` on `:5173`; `evenhub-simulator http://localhost:5173`
  to render the glasses view in a desktop window.
- Sideloading to real glasses: `evenhub qr --url http://<lan-ip>:5173`,
  scan the QR in the Even Realities companion app.
- Production: `vite build` then `evenhub pack app.json dist -o
  meeting-companion.ehpk`. The `.ehpk` is uploaded via the Even Hub
  developer portal.

`min_sdk_version: "0.0.10"`, `edition: "202601"`, `package_id` lowercase
no-hyphens reverse-domain (e.g. `com.tiago.meetingcompanion`).

### 11.7 Property-name typo

§5 notes "`borderRdaius` is the correct (typo'd) property name." The
TypeScript SDK exposes the correctly-spelled `borderRadius` field on
container property classes; the typo is preserved only in the wire-level
protobuf, which the SDK shields callers from. Use `borderRadius` in
PWA code.

### 11.8 PWA-local lifecycle

The Even App reports `FOREGROUND_ENTER_EVENT (4)`,
`FOREGROUND_EXIT_EVENT (5)`, and `ABNORMAL_EXIT_EVENT (6)` to the PWA
when the user backgrounds, restores, or loses the app. §4 doesn't
discuss these. The PWA must:

- On `FOREGROUND_EXIT_EVENT`: pause local timers (highlight scroll,
  optimistic UI animations); stop `audioControl(true)` if active.
- On `FOREGROUND_ENTER_EVENT`: re-validate WS connection (reconnect if
  needed; the snapshot reconciles state); resume timers.
- On `ABNORMAL_EXIT_EVENT`: surface a dismissable "BLE disconnected"
  status; PWA state still mirrors what the server sent on its last
  snapshot.

### 11.9 Net effect on §10 build order

The simulator-first phases in §10 still hold, but the gesture-bound
items shift:

- §10 step 6 (listening flow) and step 11 (mode cycling via ring
  long-press) are reframed as **phone-driven** in Phase 0 per ADR-0001.
  The simulator's input control plane only emits Press / Double Press /
  Swipe / Scroll, so the lifecycle gestures couldn't have been
  validated end-to-end against the simulator anyway.
- §10 step 9 (stop confirmation) is also phone-driven (Cancel / Confirm
  buttons in the PWA UI; no glasses-side temple long-press).
- §10 step 13 (real hardware) gains an explicit task: **discover the
  G2-vs-R1 input source field** and decide whether to promote
  double-press to a lifecycle gesture (deferred ADR-0001 follow-up).
