# Meeting Companion — PWA Component Spec (v1)

> **Status:** Draft, pending review.
> **Last updated:** 2026-05-02.
> **Companion to:** [`ARCHITECTURE.md`](../ARCHITECTURE.md) (system spec, especially §11 SDK Reality Corrections).
> **Sibling spec:** [`server.md`](server.md) (the WebSocket contract this PWA consumes).
>
> This document is the contract for `packages/pwa/` — the TypeScript
> Even Hub plugin that runs in the Even Realities App's WebView,
> renders the glasses display, handles G2/R1 input, captures the
> spoken meeting description via local STT, and stays in sync with
> the laptop server over WebSocket.
>
> Major design decisions are captured in:
>
> - [ADR-0001](../adr/0001-gesture-map.md) — phone-only lifecycle gestures for Phase 0.
> - [ADR-0002](../adr/0002-active-list-rendering.md) — TextContainer with formatted lines for the active-list body.
> - [ADR-0003](../adr/0003-persistence-via-bridge.md) — `bridge.setLocalStorage` for all persistence.

## 1. Purpose & scope

### 1.1 What this component owns

- The glasses display, rendered through `@evenrealities/even_hub_sdk`
  container APIs (`createStartUpPageContainer`, `rebuildPageContainer`,
  `textContainerUpgrade`).
- The PWA's local UI state (which glasses view is active, the
  highlight cursor in the active list, the current scroll viewport,
  optimistic UI overlays).
- Routing of bridge input events (G2 temple touchpads, R1 ring) into
  WebSocket intents.
- The meeting-description capture flow: enabling the G2 mic via
  `bridge.audioControl(true)`, streaming PCM frames to the Soniox
  Streaming API for STT, rendering the running transcript to the
  glasses live, and committing the final transcript via the
  `start_meeting` intent.
- The phone-screen UI (status bar, mode dropdown, metadata KV editor,
  Start/Pause/Stop CTAs, items mirror, settings modal).
- Persistent settings storage via `bridge.setLocalStorage`.
- WebSocket reconnect logic with exponential backoff and snapshot-on-connect
  reconciliation.
- The `app.json` manifest declaring permissions, edition, and entry point.

### 1.2 What this component does NOT own

- The WebSocket message contract — that lives in [`docs/specs/server.md`](server.md)
  §2.6, mirrored on this side as TypeScript types in
  `packages/pwa/src/contract.ts` (which absorbs the temporary
  `packages/contract/` package per [§7.5](#75-inline-contract-types-delete-packagescontract)).
- Real meeting audio capture (the laptop captures system audio via
  ScreenCaptureKit; the PWA never sees meeting audio).
- LLM metadata extraction, summarization, mode logic — all server-side.
- Bluetooth pairing, glasses connectivity, app store distribution — all
  handled by the host Even Realities App.
- IMU data interpretation (the SDK exposes `imuControl` and IMU events,
  but Phase 0 does not use them).

### 1.3 Phases referenced by this document

This spec describes the **Phase 0 PWA** (per [`ARCHITECTURE.md` §10](../ARCHITECTURE.md#10-build-order--simulator-first)
as amended by [§11.9](../ARCHITECTURE.md#119-net-effect-on-10-build-order)).
Where a behavior is explicitly deferred to a later phase, that's called
out inline. The on-the-wire contract with the server (§2.1) is identical
across phases — only PWA internals evolve.

## 2. Public interfaces

The PWA is the consumer of three external interfaces. It does not expose
its own API; it is a leaf in the system.

### 2.1 Server WebSocket protocol

Defined in [`docs/specs/server.md`](server.md) §2. Summary:

- Connect to `ws[s]://<host>:<port>/?token=<token>` (URL + token from
  settings, see §7.1).
- Server sends one `snapshot` event on connect; PWA validates
  `protocol_version === 1` (see §8.1) and reconciles its local state.
- Server sends ongoing events: `meeting_state_changed`,
  `metadata_changed`, `mode_changed`, `display_tag_changed`,
  `items_update`, `status`, `error`.
- PWA sends intents: `start_meeting`, `stop_meeting`, `pause`, `resume`,
  `set_mode`, `set_metadata`, `mark_moment`, `expand_item`.
- `available_modes_changed` is reserved (the stub server never emits
  it; the PWA must tolerate it as a no-op until a future server
  populates it).

### 2.2 Even Hub bridge (SDK)

Provided by `@evenrealities/even_hub_sdk` ≥ `0.0.10`. The PWA uses the
following methods (full reference: [skill `everything-evenhub:sdk-reference`](#)):

| Method                                | Used for                                                      |
|---------------------------------------|---------------------------------------------------------------|
| `waitForEvenAppBridge()`              | Boot — must await before any other bridge call.               |
| `bridge.createStartUpPageContainer()` | One-shot startup. Renders the initial idle layout (Layout A). |
| `bridge.rebuildPageContainer()`       | Layout transitions (idle ↔ listening ↔ active list ↔ active detail). |
| `bridge.textContainerUpgrade()`       | Content updates within a layout (live transcript, items list, header text). |
| `bridge.audioControl(true/false)`     | Enable/disable G2 mic for description capture.                |
| `bridge.onEvenHubEvent(cb)`           | Subscribe to all hub events (input, audio PCM, lifecycle).    |
| `bridge.onLaunchSource(cb)`           | Single-fire on app open: `'appMenu'` or `'glassesMenu'`.      |
| `bridge.onDeviceStatusChanged(cb)`    | Real-time BLE/battery/wearing updates.                        |
| `bridge.setLocalStorage(key, value)`  | Persist settings (see ADR-0003 + §7.1).                       |
| `bridge.getLocalStorage(key)`         | Read settings on boot.                                        |
| `bridge.shutDownPageContainer(0)`     | Optional clean-exit on tear-down.                             |

Methods explicitly **not used** in Phase 0:

- `bridge.imuControl()` and IMU events — no motion-aware behavior planned.
- `bridge.updateImageRawData()` — no image content.
- `bridge.getDeviceInfo()` / `bridge.getUserInfo()` — not required;
  `onDeviceStatusChanged` provides what we need (battery indicator,
  wearing detection).
- `bridge.callEvenApp()` — escape hatch, no current need.

### 2.3 Soniox Streaming API

The PWA streams 16 kHz signed-16-bit-LE-mono PCM frames (the format
`bridge.audioControl(true)` delivers via `audioEvent.audioPcm`) directly
to Soniox's WebSocket Streaming endpoint, authenticated with the user's
Soniox API key. Soniox returns `interim` and `final` transcripts as
JSON; the PWA renders interims live to the glasses and accumulates
finals into the description that becomes `start_meeting.description`.

**Auth surface:** the Soniox API key lives in `bridge.setLocalStorage`
under the `mc.sonioxKey` namespace (see §7.1) and travels only between
the PWA and Soniox — it is **never** sent to the laptop server.

**Frame cadence:** the PWA forwards each 100 ms PCM chunk as it arrives
(no buffering). Soniox tolerates this rate.

**Endpoint URL** is hard-coded to Soniox's documented streaming endpoint;
the API key is the only user-configurable value.

## 3. State

### 3.1 `AppState`

The PWA holds a single in-memory `AppState` object, consumed by both the
glasses-rendering layer and the phone-screen UI:

```ts
interface AppState {
  // Settings (loaded from bridge storage at boot, mutated via settings modal)
  settings: Settings;

  // Connection
  wsStatus: "connecting" | "open" | "reconnecting" | "closed" | "error";
  wsLastEventAt: number | null;       // ms since epoch, for heartbeat-loss detection
  protocolVersionMatched: boolean;    // false until snapshot validates

  // Server-mirrored state (last snapshot + applied events)
  meetingState: "idle" | "active" | "paused";
  availableModes: ModeOption[];
  currentMode: string;
  displayTag: string | null;
  metadata: Record<string, string>;
  items: Item[];
  status: { listening: boolean; paused: boolean; error: string | null };

  // PWA-local UI state
  glassesView: GlassesView;
  highlightIndex: number;             // index into items[]
  viewportStart: number;              // index of first item rendered on glasses
  detailItemId: string | null;        // when glassesView === "active_detail"
  listeningTranscript: string;        // accumulated final + current interim
  listeningInterim: string;           // current Soniox interim, separated for highlighting
  listeningStartedAt: number | null;  // ms since epoch, for the 25s soft cap

  // Even Hub lifecycle
  appForegrounded: boolean;
  bleConnected: boolean;
  batteryLevel: number | null;
  wearing: boolean;

  // Phone UI state
  settingsModalOpen: boolean;
  toasts: Toast[];                    // transient notifications, see §8.2
  errorOverlay: ErrorOverlay | null;  // full-screen halt, see §8.1
}

interface Toast {
  id: string;
  text: string;
  level: "info" | "warn" | "error";
  expiresAt: number;
}

interface ErrorOverlay {
  title: string;
  message: string;
  dismissable: boolean;
}
```

### 3.2 `GlassesView` enum

Per ADR-0001, `confirm_stop` is no longer a glasses-side view (stop
confirmation moved to the phone). The four states the glasses can be in:

```ts
type GlassesView = "idle" | "listening" | "active_list" | "active_detail";
```

Transition rules are in §5.2.

### 3.3 `Settings` (persisted)

```ts
interface Settings {
  serverUrl: string;       // e.g. "ws://laptop.local:7331"
  serverToken: string;
  sonioxKey: string;
  lastMetadata: Record<string, string>;  // pre-fill for next meeting
}
```

Storage layer (`packages/pwa/src/storage.ts`) is a thin typed wrapper
over `bridge.setLocalStorage` / `bridge.getLocalStorage` per ADR-0003.
See §7 for keys, env-var seeds, and load order.

### 3.4 State store

A hand-rolled minimal event-emitter store at `packages/pwa/src/store.ts`
holds the `AppState` object behind a `subscribe(selector, fn)` /
`update(patch)` API. ~50 lines, zero dependencies, easy to mock in
tests. The store is the single source of truth in the PWA — both the
glasses renderer and the phone UI subscribe to selected slices.

Mutations follow a strict pattern: a single `dispatch(action)` updates
state and then notifies subscribers. Selectors compare references to
decide whether to re-fire. The store is synchronous; async work
(WebSocket I/O, bridge calls) happens in the action handlers, not in
the reducer.

## 4. Boot sequence

```text
1. Page loads. Vite serves index.html with bundled JS.
2. [register] bridge.onLaunchSource(cb) — must be early, fires once.
3. await waitForEvenAppBridge() — resolves with the bridge instance.
4. [parallel] Load settings:
     await loadSettings()  // reads mc.serverUrl, mc.serverToken,
                           // mc.sonioxKey, mc.lastMetadata from bridge
                           // storage; falls back to VITE_DEFAULT_*
                           // env vars if a key is empty.
5. If settings.serverUrl is empty:
     state.update({ settingsModalOpen: true });
     // user enters values, presses Save, then continue.
6. createStartUpPageContainer with Layout A (idle) — must succeed
   (returns StartUpPageCreateResult.success === 0). On failure, halt
   with ErrorOverlay (see §8.1).
7. Subscribe to bridge.onEvenHubEvent(cb) — handles input, audio,
   lifecycle.
8. Subscribe to bridge.onDeviceStatusChanged(cb) — updates
   state.bleConnected / batteryLevel / wearing.
9. Open the WebSocket: connectToServer(state.settings.serverUrl,
   state.settings.serverToken). Subsequent state changes trigger
   automatic glasses re-renders via store subscription.
10. Mount the phone-screen UI into document.body.
```

`onLaunchSource` is registered before `waitForEvenAppBridge` because the
SDK reference notes it fires only once and may fire as the bridge
becomes ready. The launch source value (`appMenu` vs `glassesMenu`) is
stored in `state.launchSource` for later use; in Phase 0 it informs no
behavior, but is logged.

If `createStartUpPageContainer` returns a non-success code (1 invalid,
2 oversize, 3 outOfMemory), the PWA halts with an `ErrorOverlay`
explaining the code and asking the user to file a bug. None should
trigger in normal use — Layout A is small.

## 5. Behavior

### 5.1 Connection lifecycle

#### 5.1.1 `ReconnectingSocket` wrapper

`packages/pwa/src/ws.ts` exposes a typed wrapper around the native
`WebSocket` API:

```ts
class ReconnectingSocket {
  constructor(opts: { url: string; token: string; onEvent: (evt: Event) => void; onStatus: (s: WsStatus) => void });
  send(intent: Intent): void;        // queues if not open
  close(): void;
  // Backoff: initial 1000 ms, factor 2, max 30 s, jitter ±20 %.
  // Heartbeat-loss: if no event of any type in 25 s, closes + reconnects.
}
```

- On `WebSocket.onopen`: set `wsStatus = "open"`, drain any queued
  intents.
- On `WebSocket.onmessage`: parse JSON, call `onEvent(event)`.
- On `WebSocket.onclose`: set `wsStatus = "reconnecting"`, schedule
  reconnect with backoff.
- On `WebSocket.onerror`: set `wsStatus = "error"`, surface a toast
  (see §8.2), proceed to onclose handling.

The 25 s heartbeat-loss threshold is 2.5× the server's 10 s heartbeat
cadence (see [`server.md` §4.9](server.md)). On loss, the wrapper
closes the underlying socket, which triggers the standard reconnect
path.

#### 5.1.2 Event reconciliation

For each event from the server, the PWA dispatches the matching action:

| Server event              | Action                                                                     |
|---------------------------|----------------------------------------------------------------------------|
| `snapshot`                | Validate `protocol_version`, replace mirrored state slices, reset glasses view to match `meeting_state`. See §5.1.3. |
| `meeting_state_changed`   | Update `state.meetingState`. Trigger glasses-view transition (see §5.2).   |
| `available_modes_changed` | Replace `state.availableModes`. Re-render mode dropdown. (Stub never emits.) |
| `mode_changed`            | Update `state.currentMode`, `state.displayTag`, `state.items`. Reset `highlightIndex = 0`, `viewportStart = 0`. Re-render active-list body. |
| `display_tag_changed`     | Update `state.displayTag`. Re-render header.                               |
| `metadata_changed`        | Replace `state.metadata`. Re-render KV editor (preserving any in-flight edits, see §5.6). |
| `items_update`            | Apply per current mode's `update_strategy` (see §5.4). Re-render body.     |
| `status`                  | Update `state.status`. Update `wsLastEventAt`. Touch the heartbeat watchdog. |
| `error`                   | Show a toast (see §8.2). No state change.                                  |

Every event also updates `state.wsLastEventAt = Date.now()` so the
heartbeat watchdog has a fresh timestamp.

#### 5.1.3 Snapshot-driven reconciliation

On `snapshot`:

1. If `protocol_version !== 1`: trigger `ErrorOverlay` per §8.1 and halt.
2. Replace `state.availableModes`, `state.currentMode`, `state.displayTag`,
   `state.metadata`, `state.items`, `state.status`, `state.meetingState`.
3. Recompute `glassesView` from `meetingState`:
   - `idle` → `idle`
   - `active` → `active_list` (highlight reset to 0)
   - `paused` → `active_list` (no items will arrive until resume)
4. Discard PWA-local state that doesn't survive reconnect:
   - If `glassesView` was `listening` before this reconnect, cancel
     listening (close audio, clear interim, return to `idle`) per
     [ARCHITECTURE.md §7](../ARCHITECTURE.md#7-end-to-end-flow).
   - Clear toasts (they're transient anyway).
5. Trigger a glasses re-render against the new state.

### 5.2 Glasses view state machine

```text
                ┌─────────────────────────────────────────┐
                │                                         │
                │       (server: meeting_state_changed)   │
                │                  to active              │
                ▼                                         │
       ┌────────────────┐  user taps "Start meeting"   ┌──┴──────────────┐
       │      idle      │ ─────────────────────────►   │  active_list    │
       └────────────────┘   (server: meeting_state_     └─────────────────┘
                ▲           changed → active)              │      ▲
                │                                          │      │
                │ user taps                                │      │
                │ "Describe meeting" (idle)                │      │ ring tap
                │ from phone                               │      │ (return to list)
                │                                          ▼      │
       ┌────────────────┐                         ┌────────────────┐
       │   listening    │                         │ active_detail  │
       └────────────────┘                         └────────────────┘
                │
                │ user taps "Cancel" / "Commit" (phone)
                │ OR VAD silence ≥ 2.5 s OR 25 s elapsed (force-commit)
                ▼
       (sends start_meeting intent;
        on meeting_state_changed → active,
        moves to active_list)
```

Transitions are PWA-driven for the lifecycle ones (idle → listening,
listening → idle/active) per ADR-0001. Server events drive the
in-meeting transitions (active → idle on stop_meeting, etc.).

| From            | Trigger                                                 | To              | Glasses call                                    |
|-----------------|---------------------------------------------------------|-----------------|-------------------------------------------------|
| `idle`          | User taps "Describe meeting" (phone)                    | `listening`     | `rebuildPageContainer` → Layout E.              |
| `idle`          | User taps "Start meeting" (phone, no description)       | `idle`*         | None — sends `start_meeting{}` intent; `active_list` enters when server confirms. |
| `listening`     | User taps "Commit" (phone)                              | `idle`*         | `audioControl(false)`; sends `start_meeting{description}`; `active_list` enters when server confirms. |
| `listening`     | User taps "Cancel" (phone)                              | `idle`          | `audioControl(false)`; `rebuildPageContainer` → Layout A. |
| `listening`     | VAD silence ≥ 2.5 s after at least 0.5 s of speech     | `idle`*         | Same as Commit (auto-commit).                   |
| `listening`     | 25 s elapsed since `listeningStartedAt`                | `idle`*         | Same as Commit (force-commit per architecture §8 risk). |
| `idle` / `paused` | Server: `meeting_state_changed { active }`            | `active_list`   | `rebuildPageContainer` → Layout B.              |
| `active_list`   | User ring tap on highlighted item                       | `active_detail` | `rebuildPageContainer` → Layout C.              |
| `active_list`   | Server: `mode_changed`                                  | `active_list`   | `rebuildPageContainer` → Layout B with new mode header + reset highlight. |
| `active_detail` | User ring tap                                           | `active_list`   | `rebuildPageContainer` → Layout B.              |
| `active` (any)  | Server: `meeting_state_changed { idle }`               | `idle`          | `rebuildPageContainer` → Layout A.              |
| `active` (any)  | Server: `meeting_state_changed { paused }`             | `active_list`   | (Stay in Layout B; status indicator on phone shows paused.) |
| `paused`        | Server: `meeting_state_changed { active }` (resume)    | `active_list`   | (Stay in Layout B.)                             |

`*` "idle*" means the glasses view is idle until the server confirms the
state change; the phone CTA changes immediately to give optimistic feedback.

### 5.3 Listening flow

```text
1. User taps "Describe meeting" on the phone.
2. PWA dispatches: glassesView = "listening", listeningStartedAt = Date.now(),
   listeningTranscript = "", listeningInterim = "".
3. PWA calls bridge.audioControl(true) — starts G2 mic.
4. PWA opens a Soniox WS to the streaming endpoint, sending the API key
   in the handshake config message.
5. PWA calls bridge.rebuildPageContainer(Layout E) — header "⌁ Listening…  ●",
   body empty.
6. As bridge.onEvenHubEvent fires with audioEvent.audioPcm, PWA forwards
   each chunk to the Soniox WS.
7. Soniox responds with messages containing { interim, final } strings:
   - On interim: state.listeningInterim = interim; trigger body re-render.
   - On final: state.listeningTranscript += final; state.listeningInterim = "";
     trigger body re-render.
   Body content is computed as `tail(transcript + interim, MAX_BODY_CHARS)`,
   showing the most recent text (truncated to fit Layout E body).
8. VAD: PWA tracks silence by counting consecutive PCM frames whose
   absolute amplitude is below SILENCE_THRESHOLD. If silence ≥ 2.5 s
   AND we have ≥ 0.5 s of accumulated speech, auto-commit.
9. 25 s force-cap: if Date.now() - listeningStartedAt ≥ 25 000, commit
   regardless of VAD.
10. On commit:
    - bridge.audioControl(false)
    - Close Soniox WS
    - Send WebSocket intent: start_meeting { description: state.listeningTranscript,
      metadata: state.settings.lastMetadata }
    - PWA sets glassesView = "idle"; the active_list view enters when the
      server confirms via meeting_state_changed.
11. On cancel:
    - Same as commit but no intent sent; glassesView = "idle";
      rebuildPageContainer → Layout A.
```

**Constants:**

- `SILENCE_THRESHOLD = 800` (signed 16-bit absolute amplitude — empirical;
  Phase 0 task to tune against the simulator).
- `VAD_SILENCE_MS = 2500`
- `VAD_MIN_SPEECH_MS = 500` (must accumulate this much speech before
  silence can trigger commit, prevents instant-commit on accidental tap).
- `LISTENING_FORCE_COMMIT_MS = 25_000`
- `MAX_BODY_CHARS = 600` (fits Layout E body comfortably; Phase 0 task
  to confirm against [`everything-evenhub:font-measurement`](#)).

**Mirror on phone:** the same `listeningTranscript + listeningInterim`
text is rendered in the phone-screen UI in a scrollable text area
(see §6.1), with the interim portion in a lighter colour. This gives
the user a clearer view than the truncated glasses display.

### 5.4 Active-list rendering (per ADR-0002)

The active list (Layout B) is rendered as a single TextContainer with
formatted multi-line content. The PWA owns the highlight cursor and
viewport scrolling.

#### 5.4.1 Item application

For each `items_update` event, look up the current mode's
`update_strategy` and apply:

- `replace`: `state.items = event.items;` (atomic replacement)
- `append`: for each incoming item, find by id in `state.items`; if
  found, replace in place (preserves order); if not, push to end.

After applying, recompute the formatted body and re-emit via
`textContainerUpgrade`.

#### 5.4.2 Body formatter

```ts
function formatActiveListBody(
  items: Item[],
  highlightIndex: number,
  viewportStart: number,
  linesPerScreen: number,
  charsPerLine: number,
): string {
  const visible = items.slice(viewportStart, viewportStart + linesPerScreen);
  return visible
    .map((item, offset) => {
      const idx = viewportStart + offset;
      const cursor = idx === highlightIndex ? "▶ " : "  ";
      const text = truncate(item.text, charsPerLine - 2); // -2 for cursor
      return cursor + text;
    })
    .join("\n");
}

function truncate(s: string, max: number): string {
  return s.length <= max ? s : s.slice(0, max - 1) + "…";
}
```

`linesPerScreen` and `charsPerLine` are constants derived from
[`everything-evenhub:font-measurement`](#) and the available container
width × height. Phase 0 starts with placeholder values
(`linesPerScreen = 5`, `charsPerLine = 60`) and a Vitest snapshot test
locks the formatter behavior. Phase 1 hardware tasks recalibrate.

#### 5.4.3 Cursor + viewport

- Ring `SCROLL_TOP_EVENT` (swipe up): `highlightIndex = max(0, highlightIndex - 1)`.
- Ring `SCROLL_BOTTOM_EVENT` (swipe down): `highlightIndex = min(items.length - 1, highlightIndex + 1)`.
- After a cursor move, adjust viewport:
  - If `highlightIndex < viewportStart`: `viewportStart = highlightIndex`.
  - If `highlightIndex >= viewportStart + linesPerScreen`: `viewportStart = highlightIndex - linesPerScreen + 1`.
- After applying an `items_update` in `append` mode, if the new tail item
  is appended AND `highlightIndex === items.length - 2` (was at the end),
  bump `highlightIndex` to the new last index — i.e., follow-mode for the
  newest item. Otherwise leave the cursor where it is.

### 5.5 Detail view

Triggered by ring `CLICK_EVENT` while in `active_list`:

1. PWA dispatches `glassesView = "active_detail"`, `detailItemId = items[highlightIndex].id`.
2. If `items[highlightIndex].detail` is present (already loaded from a
   prior expand): render Layout C with the existing detail.
3. If `detail` is absent: render Layout C with body text "Loading…",
   then send `expand_item { item_id }` intent. When the server replies
   via `items_update` (per [`server.md` §4.7](server.md)), the upserted
   item now has `detail` populated; PWA re-renders the body.

Ring `CLICK_EVENT` again while in `active_detail` returns to
`active_list` (PWA-side, no server roundtrip). Ring scroll while in
`active_detail` is a no-op (Phase 0); could be used for in-detail scroll
in a future revision.

### 5.6 Mode + metadata interactions

#### 5.6.1 Mode dropdown

The phone-screen UI renders a `<select>` populated from
`state.availableModes`. On change, the PWA sends `set_mode { mode }`.
The state update happens optimistically: `state.currentMode` updates
immediately so the dropdown reflects the user's selection without
flicker, but `state.items` does not change until the server's
`mode_changed` event arrives. If the server returns
`error { code: "unknown_mode" }`, the dropdown reverts and a toast
explains the rejection.

Glasses-side mode cycling: ring swipe-left and swipe-right (using
`SCROLL_TOP_EVENT` / `SCROLL_BOTTOM_EVENT` as proxies — these are the
only swipe events available; the SDK doesn't distinguish swipe
directions beyond top/bottom semantics) is **deferred to Phase 1**
because it conflicts with cursor scroll. Phase 0: glasses scroll moves
the highlight; mode cycling is phone-only.

#### 5.6.2 Metadata KV editor

The phone-screen UI renders a table of `state.metadata` rows, plus a
`+` row for adding new keys. Edits are debounced (500 ms after last
keystroke) and committed via `set_metadata { key, value }` intent.
While an edit is in flight, the local input value is preserved even if
a server `metadata_changed` event arrives — the action handler skips
overwriting any key currently being edited.

Setting `value` to empty string in the input field calls
`set_metadata { key, value: null }` (delete). A confirm tap is required
to delete; no inline `×` button (avoids accidental deletion).

### 5.7 Optimistic UI policies

Per the design decision: optimistic UI is reserved for actions where
the PWA has authoritative information about the immediate consequence
and the server's confirmation is just acknowledgment.

| Action                            | Optimistic? | Why                                                      |
|-----------------------------------|-------------|----------------------------------------------------------|
| Ring scroll moves highlight       | Yes         | Cursor lives in PWA state; server doesn't track it.      |
| Listening interim transcript      | Yes         | Live audio capture; server has no view of this.          |
| Mode dropdown selection           | Yes         | Reverts on `error{unknown_mode}`.                        |
| Phone Start button                | Yes (CTA only) | Button text changes to "Stop"/"Pause" immediately; glasses view waits for server. |
| `mark_moment` ring double-tap     | No          | Server emits `status` event as ack; glasses just shows it. |
| `expand_item` ring tap            | Hybrid      | Glasses view changes immediately to detail with "Loading…"; body fills when server returns. |
| `start_meeting` / `stop_meeting`  | No          | These are gated by deliberate user confirmation; the latency between intent and confirmation is acceptable. |

When the server contradicts an optimistic update (e.g. `error{unknown_mode}`),
the PWA reverts the local change and surfaces a toast (§8.2).

### 5.8 Even Hub lifecycle handlers

Per [`ARCHITECTURE.md` §11.8](../ARCHITECTURE.md#118-pwa-local-lifecycle):

```text
event.sysEvent.eventType === FOREGROUND_EXIT_EVENT (5):
  state.appForegrounded = false;
  if state.glassesView === "listening":
    bridge.audioControl(false); cancel listening; glassesView = "idle";
  Pause: highlight scroll animation, optimistic UI timers.
  WS stays connected — events queued in state.

event.sysEvent.eventType === FOREGROUND_ENTER_EVENT (4):
  state.appForegrounded = true;
  Force a snapshot reconciliation: if WS is open, trigger a
    no-op intent (e.g. set_metadata noop) to elicit a status response,
    OR close + reopen the WS to receive a fresh snapshot.
  Resume timers.

event.sysEvent.eventType === ABNORMAL_EXIT_EVENT (6):
  Surface a dismissable toast: "Glasses connection lost".
  state.bleConnected = false (also via onDeviceStatusChanged).
  PWA continues; events still flow over WS to phone, but glasses
  rendering pauses until BLE reconnects.

event.sysEvent.eventType === SYSTEM_EXIT_EVENT (7):
  Tear down: bridge.audioControl(false); ws.close(); unsubscribe all.
```

Mic and listeners are cleaned up in a `beforeunload` hook as a
safety net per the device-features skill guidance.

## 6. Phone-screen UI

### 6.1 Layout

```text
┌─────────────────────────────────┐
│ ● WS  ● BLE   State: idle    ⚙  │  Status bar (40 px tall, fixed top)
├─────────────────────────────────┤
│ Mode: [Highlights ▾]    <tag>   │
├─────────────────────────────────┤
│ Metadata                        │
│   project   helix      [✕]      │
│   title     Q1 review  [✕]      │
│   client    -          [✕]      │
│   [+ key]   [+ value]   [add]   │
├─────────────────────────────────┤
│  ┌───────────────────────────┐  │
│  │   Describe meeting        │  │  Primary CTA (large, full width)
│  └───────────────────────────┘  │  text changes per state, see below
│  ┌───────────────────────────┐  │
│  │      Start meeting        │  │
│  └───────────────────────────┘  │
├─────────────────────────────────┤
│ Items so far ▼                  │  Collapsible, mirrors glasses body
│   ▶ Tiago raised concern …      │
│     Decision: ship feature X    │
│     Open question: who owns …   │
└─────────────────────────────────┘
```

#### CTA region by `meetingState` × `glassesView`

| meetingState | glassesView      | CTAs (top → bottom)                                                          |
|--------------|------------------|------------------------------------------------------------------------------|
| `idle`       | `idle`           | `Describe meeting` (secondary), `Start meeting` (primary).                   |
| `idle`       | `listening`      | Live transcript area (read-only, ~150 px tall), `Cancel` (secondary), `Commit` (primary). |
| `active`     | `active_list` / `active_detail` | `Pause` (secondary), `Stop` (primary, requires Confirm-stop two-tap, see §6.1.1). |
| `paused`     | `active_list`    | `Resume` (primary), `Stop` (secondary, two-tap confirm).                     |

#### 6.1.1 Stop confirmation (replacing Layout D)

Per ADR-0001 stop confirmation moves to the phone. Implementation:

- First tap of `Stop` button: button text changes to "Tap again to confirm",
  background flashes red, 3-second timer starts.
- Second tap within 3 s: send `stop_meeting` intent, button reverts.
- 3 s expires without second tap: button reverts to "Stop".

This mirrors the architecture's auto-dismiss confirm-stop semantic
without needing a glasses-side layout.

### 6.2 Settings modal

Triggered by the gear icon in the status bar. Full-screen modal (95 %
viewport) with:

- Header: "Settings" + close X.
- Form fields (all `bridge.setLocalStorage`-backed):
  - **Server URL** (`text`, e.g. `ws://laptop.local:7331`)
  - **Server token** (`password`)
  - **Soniox API key** (`password`)
- Footer: `Save & Reconnect` (primary), `Cancel` (secondary).

On `Save & Reconnect`: write all values via `bridge.setLocalStorage`,
update `state.settings`, close + reopen the WebSocket with the new
URL/token. Soniox key is just stored (used on next listening session).

Validation: empty fields are flagged; `Save` is disabled until all
required fields are filled (server URL + token; Soniox optional — if
missing, the listening flow shows an error and falls back to
`start_meeting{}` with no description).

### 6.3 Visual style

- Single `style.css` ~150 lines, no CSS framework.
- Dark theme: `--bg: #0a0a0c`, `--fg: #e8e8ea`, `--accent: #4ea1ff`,
  `--success: #4ec76e`, `--warn: #f0a020`, `--error: #f04747`.
- System font stack (`system-ui, -apple-system, sans-serif`).
- Buttons: 48 px tall minimum (fat-finger), 16 px font, 8 px border
  radius.
- Inputs: 44 px tall, monospace for token/key fields.
- Status indicators: 8 px filled circles, colour by state.

CSS lives at `packages/pwa/src/style.css`, imported once from
`main.ts`. No CSS-in-JS, no preprocessor.

### 6.4 First-run UX

On boot if `state.settings.serverUrl === ""`:

1. PWA renders the idle UI as normal but with the settings modal
   pre-opened, focused on the Server URL field.
2. The CTA region is replaced with a brief "Configure your server first"
   message in lieu of the Start/Describe buttons.
3. After successful save, the modal closes and normal boot continues
   (WebSocket connects).

If `VITE_DEFAULT_SERVER_URL`, `VITE_DEFAULT_SERVER_TOKEN`, and
`VITE_DEFAULT_SONIOX_KEY` are all set in `.env.local` at build time,
the PWA seeds the storage on first run (only if the keys are empty in
storage) and skips the settings modal — useful for dev ergonomics.

## 7. Configuration & persistence

### 7.1 Storage keys

All keys are namespaced `mc.` to avoid collision with other Even Hub
plugins sharing host storage. Implementation in
`packages/pwa/src/storage.ts`:

| Key                    | Type                              | Description                                          |
|------------------------|-----------------------------------|------------------------------------------------------|
| `mc.serverUrl`         | `string`                          | WebSocket URL.                                       |
| `mc.serverToken`       | `string`                          | Server auth token.                                   |
| `mc.sonioxKey`         | `string`                          | Soniox API key.                                      |
| `mc.lastMetadata`      | `Record<string, string>` (JSON)   | Pre-fill for next meeting's `start_meeting.metadata`. |

`bridge.getLocalStorage(key)` returns `""` for unset keys; the storage
wrapper translates this to `null`/`undefined` for typed access.

`bridge.setLocalStorage(key, value)` accepts only strings; non-string
values (like `lastMetadata`) are JSON-encoded by the wrapper.

### 7.2 Vite environment variable seeds

Read at build time via `import.meta.env`:

| Env var                       | Description                                                        |
|-------------------------------|--------------------------------------------------------------------|
| `VITE_DEFAULT_SERVER_URL`     | Default WS URL if `mc.serverUrl` is empty in storage.              |
| `VITE_DEFAULT_SERVER_TOKEN`   | Default token if `mc.serverToken` is empty.                        |
| `VITE_DEFAULT_SONIOX_KEY`     | Default Soniox key if `mc.sonioxKey` is empty.                     |
| `VITE_PROTOCOL_VERSION`       | Built-in expected protocol version, defaulted to `1`.              |

These live in `packages/pwa/.env.local` (git-ignored). A
`packages/pwa/.env.example` ships in git documenting the names.

The seed pattern: after `loadSettings()` reads the bridge storage, any
key that came back empty is filled from the corresponding env var (if
set). If both are empty, the value remains `""`.

### 7.3 `app.json` template

`packages/pwa/app.json` (committed):

```json
{
  "package_id": "com.tiago.meetingcompanion",
  "edition": "202601",
  "name": "Meeting Companion",
  "version": "0.1.0",
  "min_app_version": "2.0.0",
  "min_sdk_version": "0.0.10",
  "entrypoint": "index.html",
  "permissions": [
    {
      "name": "g2-microphone",
      "desc": "Capture an optional spoken meeting description from the G2 mic before each meeting starts."
    },
    {
      "name": "network",
      "desc": "Connect to the Meeting Companion server on the user's local network for real-time meeting state.",
      "whitelist": ["http://localhost:7331"]
    }
  ],
  "supported_languages": ["en"]
}
```

A dev script `packages/pwa/scripts/sync-whitelist.ts` reads
`VITE_DEFAULT_SERVER_URL` from `.env.local` and patches the `whitelist`
array before `evenhub pack` runs (extracting the origin part of the URL,
normalizing `ws://` ↔ `http://`). The patch happens on a copy of
`app.json` produced at pack time, not the committed file — this keeps
personal LAN IPs out of git.

### 7.4 Vite config

`packages/pwa/vite.config.ts`:

```ts
import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  build: {
    target: "es2022",
    sourcemap: true,
  },
  define: {
    "import.meta.env.VITE_PROTOCOL_VERSION": JSON.stringify(1),
  },
  server: {
    port: 5173,
    strictPort: true,
  },
});
```

`base: './'` ensures the `.ehpk` works regardless of host-injected
origin. `strictPort` guarantees the simulator's `evenhub-simulator
http://localhost:5173` URL resolves.

### 7.5 Inline contract types, delete `packages/contract/`

The Rust server's contract types live in `packages/server/src/contract.rs`;
the PWA's mirror lives in `packages/pwa/src/contract.ts`. Both reference
[`server.md` §2.6](server.md) as the single source of truth.

Implementation order:

1. Copy current `packages/contract/src/index.ts` content into
   `packages/pwa/src/contract.ts` (verbatim — types are stable).
2. Update `packages/server/src/contract.rs` doc comment cross-reference
   from "mirrors `packages/contract/src/index.ts`" to "mirrors
   `packages/pwa/src/contract.ts`."
3. Remove `packages/contract` from `pnpm-workspace.yaml` `packages` glob
   (it's already covered by `packages/*`, but the package itself is
   deleted).
4. `git rm -r packages/contract`.
5. `pnpm install` to refresh the lockfile.

This becomes one of the early tasks in the PWA implementation plan; no
separate cleanup commit needed.

## 8. Errors & failure modes

### 8.1 Full-screen `ErrorOverlay`

Halts the PWA. Triggered by:

- `protocol_version !== 1` on snapshot. Title: "Incompatible server".
  Message: explains the version mismatch and asks to update one side.
  Not dismissable — the user must restart the app after resolving.
- `createStartUpPageContainer` returns non-success. Title: "Failed to
  initialize glasses display". Message includes the `StartUpPageCreateResult`
  code and instructs the user to file a bug.
- `waitForEvenAppBridge()` times out (rejects after 30 s). Title: "Even
  Realities App not detected". Message instructs the user to ensure
  they opened the PWA from inside the Even Realities companion app.

### 8.2 Toasts

Transient, dismiss after 4 s, max 3 stacked. Triggered by:

- Server `error` events: text = `"<code>: <message>"`, level = warn.
- WebSocket connect failure (after 3 retries): "Server unreachable.
  Check Settings.", level = error, with "Open Settings" action button.
- `bridge.audioControl(true)` rejection: "Microphone access denied.",
  level = error, listening flow cancels.
- Soniox auth failure (HTTP 401 in handshake): "Soniox API key invalid.
  Check Settings.", level = error.
- `mark_moment` while idle (server ignores silently): no toast — the
  PWA UI shouldn't allow this in the first place.
- Optimistic UI revert: "Server rejected mode change.", level = info.

### 8.3 Status indicators (subtle errors)

- `wsStatus = "reconnecting"`: WS dot turns yellow, status bar shows
  "Reconnecting…" with backoff countdown.
- `wsStatus = "error"` or `"closed"`: WS dot turns red.
- `bleConnected = false`: BLE dot turns grey, status bar shows
  "Glasses disconnected" — glasses-side rendering pauses (the bridge
  calls still succeed; they just don't reach the glasses).
- Heartbeat-loss (no events in 25 s while supposedly connected):
  triggers a reconnect; surfaces as `wsStatus = "reconnecting"`.

### 8.4 Failure-mode summary

| Failure                                  | User-visible  | Action                                                  |
|------------------------------------------|---------------|---------------------------------------------------------|
| Bad protocol version                     | ErrorOverlay  | Halt; restart required.                                 |
| `createStartUpPageContainer` fails       | ErrorOverlay  | Halt; bug report.                                       |
| `waitForEvenAppBridge` times out         | ErrorOverlay  | Halt; check host app.                                   |
| WS connect fails repeatedly              | Toast + Settings shortcut | User checks server URL/token.            |
| Server emits `error` event               | Toast         | Auto-dismiss; logged.                                   |
| Optimistic update rejected               | Toast (info)  | Local state reverts.                                    |
| Mic permission denied                    | Toast         | Listening cancels back to idle.                         |
| Soniox auth fails                        | Toast         | Settings page red banner; listening commits with empty description if user retries. |
| BLE disconnect                           | Status bar    | Glasses pauses; auto-recovers on reconnect.             |
| `FOREGROUND_EXIT_EVENT`                  | None visible  | Audio + timers paused; resumes on FG enter.             |
| `ABNORMAL_EXIT_EVENT`                    | Toast         | "Glasses connection lost"; auto-recovers.               |

## 9. Concurrency model

The PWA is single-threaded JavaScript. Concurrency comes from:

- The browser event loop (DOM events, `setTimeout`, `setInterval`).
- WebSocket callbacks (`onmessage`, `onclose`, etc.).
- Bridge `Promise<T>` returns from the SDK.
- Bridge subscriptions (`onEvenHubEvent`, `onDeviceStatusChanged`).

The store's `dispatch(action)` is the only place state mutates. Action
handlers may be async — they perform I/O, then dispatch sub-actions.
This keeps the state machine reasoning sequential even though the I/O
is concurrent.

**Re-entrancy:** dispatch may be called from within a subscriber. The
store handles this by queueing the second dispatch and processing it
after the first completes. (~10 lines of guard logic.)

**Bridge call serialization:** `updateImageRawData` requires serial
calls per the SDK. We don't use it in Phase 0, but the store has a
generic `bridgeQueue` slot that future image work would route through.

`textContainerUpgrade` and `rebuildPageContainer` calls are **not**
strictly serialized in Phase 0 — they're idempotent at the destination
(a later call wins). If race-induced flicker is observed in testing
we'll add a queue.

**Bridge subscriptions** are registered once at boot and never
unsubscribed mid-session. The cleanup happens only on
`SYSTEM_EXIT_EVENT` and `beforeunload`.

## 10. Test strategy

### 10.1 Unit tests (Vitest)

Located in `packages/pwa/src/**/*.test.ts`. Each module exports its
pure functions or class as testable units.

| Module                              | Tests                                                                                                                                                                |
|-------------------------------------|----------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `storage.ts`                        | Mocks `bridge.setLocalStorage` / `getLocalStorage`. Verifies key namespacing, JSON encoding, env-var seed fallback, missing-key → empty string handling.            |
| `store.ts`                          | Subscribe/dispatch/notify. Selector reference-equality. Re-entrant dispatch.                                                                                          |
| `state-machine.ts` (glasses view)   | Every cell of §5.2's transition table. Listening cancel vs commit. Force-commit on 25 s.                                                                              |
| `format-active-list.ts`             | `formatActiveListBody()` snapshots: empty list, < linesPerScreen, > linesPerScreen, highlight at top/middle/bottom, viewport scroll, item truncation.                |
| `apply-items-update.ts`             | Replace strategy semantics. Append upsert by id (existing replaced; new pushed).                                                                                     |
| `vad.ts`                            | Detects silence after speech; respects min-speech threshold; force-commit at cap.                                                                                    |
| `gesture-router.ts`                 | Maps `OsEventTypeList` × current `glassesView` → action dispatched.                                                                                                  |
| `ws.ts` (`ReconnectingSocket`)      | Mocks `WebSocket`. Backoff schedule. Heartbeat-loss timeout. Send-queue while reconnecting. Token in URL.                                                            |
| `soniox.ts`                         | Mocks Soniox WS. Forwards PCM frames. Parses interim/final messages. Auth error surfaces.                                                                            |
| `protocol-version.ts`               | Mismatch triggers ErrorOverlay action.                                                                                                                                |
| `optimistic-revert.ts`              | Mode change reverts on `error{unknown_mode}`.                                                                                                                        |

### 10.2 Bridge mock

`packages/pwa/src/__test__/mock-bridge.ts` provides an
`EvenAppBridge`-shaped class with:

- All bridge methods stubbed with `vi.fn()` (Vitest mocks).
- Settable mock storage for `setLocalStorage` / `getLocalStorage`.
- `simulateEvent(event)` helper that triggers all `onEvenHubEvent`
  subscribers.
- `simulateDeviceStatus(status)` helper.

Used in unit tests via dependency injection (the bridge instance is
passed as an arg to module functions, not imported globally).

### 10.3 Integration tests via simulator HTTP API

Located in `packages/pwa/tests/integration/` (separate from `src/`
unit tests).

The `evenhub-simulator` exposes an automation HTTP API on `:9898`:

- `POST /api/input` — inject Up / Down / Click / Double Click.
- `GET /api/screenshot/glasses` — 576×288 RGBA PNG of the rendered display.
- `GET /api/console` — read in-WebView console logs since last clear.

Test pattern:

```ts
import { spawn } from "node:child_process";
import { test, expect } from "vitest";

test("idle layout shows ready text", async () => {
  // Vite dev server is assumed running on :5173 (caller's responsibility).
  const sim = spawn("evenhub-simulator", ["http://localhost:5173", "--automation-port", "9898"]);
  await waitForReady("http://localhost:9898/api/ping");
  await waitForConsoleLine(/app-ready/);
  const png = await fetch("http://localhost:9898/api/screenshot/glasses").then(r => r.arrayBuffer());
  expect(detectGreenPixels(png)).toBeGreaterThan(0);
  // Optionally: OCR the PNG to verify text content.
  sim.kill();
});
```

Coverage in Phase 0:

- `idle_initial_render`: PWA boots, snapshot received from a running
  stub server, glasses show Layout A.
- `start_meeting_via_phone`: synthesize a phone-button click (in
  practice via the test harness dispatching the action directly), assert
  WS intent sent + screenshot transitions to Layout B.
- `items_update_appears`: server emits items, screenshot shows them.
- `expand_item`: simulate ring `CLICK_EVENT` via `/api/input`, assert
  Layout C rendered, server intent sent.

The integration tests require a running stub server (the user spawns
it manually; tests skip if `MEETING_COMPANION_TEST_SERVER_URL` env var
is unset).

### 10.4 Manual hardware checklist

Lives in `packages/pwa/README.md`. Phase 1 sideload step:

1. Sideload PWA to G2 via `evenhub qr --url http://<lan-ip>:5173`.
2. Verify Layout A renders.
3. Tap "Describe meeting" on phone → glasses Layout E appears.
4. Speak; verify transcript appears.
5. Wait for VAD silence → meeting starts; glasses Layout B appears.
6. Verify mock items arrive every ~3 s.
7. Ring scroll → highlight moves.
8. Ring tap → Layout C; verify detail loads.
9. Phone Stop → first tap shows confirm; second commits.
10. **Discovery task per ADR-0001**: log raw `bridge.onEvenHubEvent`
    payloads on temple tap, ring tap, swipe — identify which field
    distinguishes G2 left/right temple from R1 ring (likely
    `event.sysEvent.eventSource` per `EventSourceType` enum: `1` = G2
    right, `2` = R1, `3` = G2 left). Document findings in this spec.

## 11. Build & packaging

### 11.1 Repo layout (`packages/pwa/`)

```
packages/pwa/
├── package.json
├── tsconfig.json
├── vite.config.ts
├── index.html
├── app.json
├── .env.example
├── README.md
├── public/                       (any static assets)
├── scripts/
│   └── sync-whitelist.ts         (patches app.json for evenhub pack)
├── src/
│   ├── main.ts                   (boot sequence per §4)
│   ├── style.css
│   ├── contract.ts               (mirrors server.md §2.6)
│   ├── store.ts                  (state store)
│   ├── storage.ts                (bridge.setLocalStorage wrapper)
│   ├── ws.ts                     (ReconnectingSocket)
│   ├── stt/
│   │   ├── soniox.ts             (Soniox client)
│   │   └── vad.ts                (silence detection)
│   ├── glasses/
│   │   ├── render.ts             (orchestrator: dispatches to layout renderers)
│   │   ├── layout-idle.ts        (Layout A)
│   │   ├── layout-listening.ts   (Layout E)
│   │   ├── layout-active-list.ts (Layout B)
│   │   ├── layout-active-detail.ts (Layout C)
│   │   └── format-active-list.ts (the body formatter)
│   ├── input/
│   │   ├── gesture-router.ts     (event → action)
│   │   └── lifecycle.ts          (FG/BG handlers)
│   ├── ui/
│   │   ├── status-bar.ts
│   │   ├── mode-dropdown.ts
│   │   ├── kv-editor.ts
│   │   ├── cta-region.ts
│   │   ├── items-mirror.ts
│   │   ├── settings-modal.ts
│   │   └── toast.ts
│   └── __test__/
│       └── mock-bridge.ts
└── tests/
    └── integration/
        └── *.test.ts             (simulator HTTP tests)
```

### 11.2 `package.json` scripts

```json
{
  "scripts": {
    "dev": "vite",
    "dev:sim": "concurrently -n vite,sim -c blue,green \"vite\" \"wait-on http://localhost:5173 && evenhub-simulator http://localhost:5173 --automation-port 9898\"",
    "dev:qr": "evenhub qr --url http://$(ipconfig getifaddr en0):5173",
    "build": "vite build",
    "pack": "tsx scripts/sync-whitelist.ts && evenhub pack app.json dist -o meeting-companion.ehpk",
    "typecheck": "tsc --noEmit",
    "test": "vitest run",
    "test:watch": "vitest",
    "test:integration": "vitest run tests/integration"
  }
}
```

devDependencies include `vite`, `typescript`, `vitest`, `concurrently`,
`wait-on`, `tsx`, plus the Even Hub SDK / CLI / simulator already
required by §11.3.

### 11.3 Dependency manifest

```json
{
  "dependencies": {
    "@evenrealities/even_hub_sdk": "^0.0.10"
  },
  "devDependencies": {
    "@evenrealities/evenhub-cli": "^0.1.12",
    "@evenrealities/evenhub-simulator": "^0.7.2",
    "concurrently": "^8.2.0",
    "tsx": "^4.7.0",
    "typescript": "^5.4.0",
    "vite": "^5.2.0",
    "vitest": "^1.6.0",
    "wait-on": "^7.2.0"
  }
}
```

Versions track the latest stable as of 2026-05-02 — update during
implementation if newer compatible releases exist.

### 11.4 Production packaging

```bash
pnpm -F @meeting-companion/pwa build      # vite build → dist/
pnpm -F @meeting-companion/pwa pack       # syncs whitelist + evenhub pack
                                          # → meeting-companion.ehpk
```

The `.ehpk` file is uploaded to the Even Hub developer portal for
distribution. For personal use, sideload via QR (`pnpm -F
@meeting-companion/pwa dev:qr`).

## 12. Out of scope

- IMU-driven interactions (head-shake to dismiss, etc.). The SDK
  exposes IMU but Phase 0 does not consume it.
- Image content on the glasses (no `updateImageRawData` use). Future
  enhancement: a small avatar / status icon in the corner.
- Mid-meeting re-describe via the same listening flow (architecture
  §9 deferral).
- Pause / resume from glasses gestures. Phone-only per ADR-0001.
- Speaker labels. Will surface via `display_tag` from the server in
  Phase 2; no special client-side handling needed.
- Soniox provider abstraction. Direct Soniox client only; if we want
  to swap engines later we extract the interface at that point.
- Multi-language support. `supported_languages: ["en"]` in `app.json`;
  the listening flow assumes English STT.
- Offline mode. PWA requires an active WS connection to be useful;
  no local-only mode.
- Push notifications, badge counts, dashboard widgets. The Overview
  guide describes these as future Even Hub platform features; not used.
- TLS termination on the laptop server. Production deployment via
  Tailscale Funnel / cloudflared is documented in the README, not
  implemented in code.

## 13. Open questions

None at time of writing. Remaining unknowns (G2-vs-R1 source
distinction field, exact LVGL font measurements, simulator audio
support) are explicitly Phase 0 / Phase 1 discovery tasks and tracked
inline (§5.4.2, §10.4) rather than as spec-blocking gaps.
