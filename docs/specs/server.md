# Meeting Companion — Server Component Spec (v1)

> **Status:** Draft, pending review.
> **Last updated:** 2026-05-01.
> **Companion to:** [`ARCHITECTURE.md`](../ARCHITECTURE.md) (system-level spec).
>
> This document is the contract for `apps/server/` — the Rust binary that owns
> meeting state and serves the WebSocket endpoint defined in architecture §6.
> The implementation plan in `docs/superpowers/plans/` derives mechanically
> from this spec; the contract types in `packages/contract/` are the literal
> typed output of §2.6.

## 1. Purpose & scope

### 1.1 What this component owns

- The meeting lifecycle state machine: `idle` / `active` / `paused`.
- The `available_modes` catalog and the currently selected mode.
- Per-mode item lists for the active meeting.
- The metadata KV map for the active meeting.
- The WebSocket endpoint, its protocol, and its authentication.
- Event broadcast to all connected clients.
- Mock content generation in Phase 0 (placeholder for the real STT /
  summarizer pipeline that lands in Phase 2).

### 1.2 What this component does NOT own

- Real audio capture via ScreenCaptureKit — replaced by mock generator.
  Lands in Phase 2.
- Real STT / summarization pipeline — out of scope per architecture §9.
- Real LLM metadata extraction — replaced by deterministic stub in §8.4.
  Lands in Phase 2.
- Memory-system enrichment — Phase 2 per architecture §10 step 18.
- Persistence across server restarts. State is in-memory only; restart is
  equivalent to "fresh idle".
- TLS termination. The server speaks plain `ws://` on the LAN. Off-LAN
  reachability is provided by Tailscale or `cloudflared` upstream.
- Per-user accounts. Auth is a single shared-secret token.
- The phone PWA, the Even Realities bridge, and the glasses display.
  See `pwa.md` (forthcoming).

### 1.3 Phases referenced by this document

The architecture doc defines three phases (§10). This spec describes the
**Phase 0 stub server**, with explicit pointers to which behaviors get
upgraded in Phase 2. The §6 wire contract is identical across phases —
only the internals change.

## 2. Public interface — WebSocket protocol

### 2.1 Endpoint & handshake

- URL: `ws://<host>:<port>/?token=<token>`
- HTTP upgrade to WebSocket on path `/`.
- `token` query param required; absence is treated as an auth failure.
- Server bind defaults to `0.0.0.0:7331`. See §5.2 for overrides.
- No subprotocol negotiation. No compression. No fragmentation requirements.
- Frames: text, JSON-encoded UTF-8. One JSON message per text frame.
- Maximum incoming frame size: 64 KiB. Larger frames cause the server to
  close the connection with code `1009` ("message too big").

### 2.2 Authentication

The shared secret is provided at server startup via the env var
`MEETING_COMPANION_TOKEN`. The server **refuses to start** if this var is
unset or empty.

On WebSocket handshake:

1. Server parses `token` from the URL query string.
2. Server compares it to the env-var value via constant-time comparison
   (`subtle::ConstantTimeEq` or equivalent).
3. **Match** → upgrade completes; the connection becomes a normal subscriber.
4. **Mismatch or missing** → upgrade completes (the WS handshake itself is
   accepted), then the server immediately sends a close frame with code
   `1008` ("policy violation") and reason `"invalid token"`, and drops the
   connection. No `error` event is emitted (the connection isn't subscribed
   yet).

Rationale for closing post-upgrade rather than rejecting at the HTTP layer:
keeps the auth path uniform across implementations and avoids leaking auth
state via HTTP status codes. WS clients see a clean close they can surface
to UI.

### 2.3 Message envelope

All messages in both directions are JSON objects with a `type` discriminator:

```json
{ "type": "<message_type>", "...": "..." }
```

Implemented in Rust with `#[serde(tag = "type", rename_all = "snake_case")]`
on both the `Intent` and `Event` enums. Mirrored in TypeScript in
`packages/contract/`.

Unknown discriminators on inbound intents trigger an `error` event with code
`unknown_intent` (see §6.1). Unknown discriminators on outbound events
should never happen (server is the only writer); if observed in tests it
indicates a contract drift bug.

### 2.4 Inbound — Intents (PWA → server)

| `type`           | Payload                                                          | Validity                       | Effect                                                                                      |
|------------------|------------------------------------------------------------------|--------------------------------|---------------------------------------------------------------------------------------------|
| `start_meeting`  | `{ description?: string, metadata?: Record<string,string> }`     | Only when `idle`.              | `idle → active`. Replace metadata with `intent.metadata`. Spawn extraction task. Spawn mock generator. |
| `stop_meeting`   | `{}`                                                             | Only when `active` or `paused`. | `→ idle`. Cancel mock generator. Clear items + metadata.                                    |
| `pause`          | `{}`                                                             | Only when `active`.            | `active → paused`. Cancel mock generator. Retain items + metadata.                          |
| `resume`         | `{}`                                                             | Only when `paused`.            | `paused → active`. Restart mock generator.                                                  |
| `set_mode`       | `{ mode: string }`                                               | Any state. `mode` must be in catalog. | Update `current_mode`. Emit `mode_changed` carrying the new mode's items.            |
| `set_metadata`   | `{ key: string, value: string \| null }`                         | Any state.                     | Set or delete the key. Emit `metadata_changed` with full map.                               |
| `mark_moment`    | `{ t: number, note?: string }`                                   | Only when `active`.            | Log + emit `status` event as ack.                                                           |
| `expand_item`    | `{ item_id: string }`                                            | Any state. Item must exist in current mode. | Synthesize detail; emit `items_update` shaped per current mode strategy.       |

**Validity** column maps to error handling. Intents that fail validity
are handled per §6:

- *State* invalid (e.g., `pause` while `idle`): silent ignore + WARN log.
- *Payload* invalid (e.g., `set_mode "bogus"`, `expand_item "unknown"`,
  malformed JSON): emit `error` event to the originating client.

### 2.5 Outbound — Events (server → clients)

| `type`                       | Payload                                                      | Trigger                                                                  | Routing                          |
|------------------------------|--------------------------------------------------------------|--------------------------------------------------------------------------|----------------------------------|
| `snapshot`                   | full state (see §2.6)                                        | Sent once on each new connection, after auth.                            | Per-connection (not broadcast).  |
| `meeting_state_changed`      | `{ meeting_state: MeetingState }`                            | After any state machine transition.                                      | Broadcast.                       |
| `available_modes_changed`    | `{ available_modes: ModeOption[] }`                          | (Reserved.) Stub server never emits — catalog is fixed at compile time.  | Broadcast.                       |
| `mode_changed`               | `{ mode, display_tag?, items: Item[] }`                      | After successful `set_mode` or as part of the `start_meeting` startup sequence. | Broadcast.               |
| `display_tag_changed`        | `{ tag?: string }`                                           | (Reserved.) Stub server never emits — see §8.5.                          | Broadcast.                       |
| `metadata_changed`           | `{ metadata: Record<string,string> }`                        | After `set_metadata`, or after extraction merge.                         | Broadcast.                       |
| `items_update`               | `{ items: Item[] }`                                          | Mock generator tick or `expand_item` reply. Shape per current mode strategy. | Broadcast.                   |
| `status`                     | `{ listening, paused, error? }`                              | Heartbeat (10s) and on `mark_moment` ack.                                | Broadcast.                       |
| `error`                      | `{ code, message, intent_ref? }`                             | Protocol error (see §6.1).                                               | Per-connection (originator only).|

### 2.6 Concrete schemas

```ts
const PROTOCOL_VERSION = 1;

type MeetingState = "idle" | "active" | "paused";
type UpdateStrategy = "replace" | "append";

interface ModeOption {
  id: string;
  label: string;
  update_strategy: UpdateStrategy;
}

interface Item {
  id: string;            // UUIDv4 generated server-side
  text: string;          // human-readable summary line
  detail?: string;       // populated only after expand_item
  t: number;             // milliseconds since meeting_started_at (monotonic)
  meta?: Record<string, unknown>;
}

interface Status {
  listening: boolean;    // true iff meeting_state == "active"
  paused: boolean;       // true iff meeting_state == "paused"
  error?: string;        // transient error message; absent when no error
}

type Intent =
  | { type: "start_meeting"; description?: string;
      metadata?: Record<string, string> }
  | { type: "stop_meeting" }
  | { type: "pause" }
  | { type: "resume" }
  | { type: "set_mode"; mode: string }
  | { type: "set_metadata"; key: string; value: string | null }
  | { type: "mark_moment"; t: number; note?: string }
  | { type: "expand_item"; item_id: string };

type Event =
  | { type: "snapshot";
      protocol_version: number;
      meeting_state: MeetingState;
      available_modes: ModeOption[];
      mode: string;
      display_tag?: string;
      metadata: Record<string, string>;
      items: Item[];
      status: Status }
  | { type: "meeting_state_changed"; meeting_state: MeetingState }
  | { type: "available_modes_changed"; available_modes: ModeOption[] }
  | { type: "mode_changed"; mode: string;
      display_tag?: string; items: Item[] }
  | { type: "display_tag_changed"; tag?: string }
  | { type: "metadata_changed"; metadata: Record<string, string> }
  | { type: "items_update"; items: Item[] }
  | { type: "status"; status: Status }
  | { type: "error"; code: string; message: string; intent_ref?: string };
```

### 2.7 Protocol versioning

- `PROTOCOL_VERSION = 1` carried in every `snapshot`.
- **Additive changes are free**: new event types, new optional fields, new
  mode IDs, new metadata keys, new `update_strategy` values, new `error`
  codes. PWA must tolerate unknown event types and unknown fields.
- **Breaking changes bump the version.** During transition the server
  retains backward compatibility for at least one prior version.
- PWA validates `protocol_version` on every `snapshot`. Major mismatch
  surfaces a connection error and halts further processing.
- The server is the only writer of the contract. Clients never echo or
  re-export it.

## 3. State

### 3.1 `ServerState` struct

```rust
pub struct ServerState {
    meeting_state: MeetingState,                       // idle | active | paused
    available_modes: Vec<ModeOption>,                  // fixed catalog (§8.3)
    current_mode: String,                              // id from available_modes
    items_per_mode: HashMap<String, Vec<Item>>,        // keyed by mode id
    metadata: HashMap<String, String>,
    meeting_started_at: Option<Instant>,               // anchor for Item.t
}
```

All fields are private. Mutations go through methods on `ServerState` that
return `Vec<Event>` describing what to broadcast.

### 3.2 Invariants

The following must hold at every observable point (after every method
returns, before every method is called):

- `current_mode` is always present in `available_modes`.
- `items_per_mode` has exactly one entry per mode in `available_modes`.
  Empty entries are present, not omitted.
- `meeting_started_at.is_some()` iff `meeting_state != idle`.
- When `meeting_state == idle`: `metadata` is empty AND every entry in
  `items_per_mode` is empty AND `meeting_started_at == None`.
- `Item.id` is unique within `items_per_mode[mode]` for any mode.

Invariants are checked by an internal `debug_assert!` helper called at the
end of every mutating method. They are also asserted by the unit tests
in §10.1.

### 3.3 Ownership

The struct lives behind a single `Arc<Mutex<ServerState>>` shared across
all server tasks. See §7 for concurrency discipline.

## 4. Behavior

### 4.1 Meeting state machine

| Current state | Intent          | Next state | Side effects (in order)                                                                                                                                                            | Validity            |
|---------------|-----------------|------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|---------------------|
| `idle`        | `start_meeting` | `active`   | (1) `meeting_started_at = now`. (2) `metadata = intent.metadata.unwrap_or_default()`. (3) `current_mode = "highlights"`. (4) Broadcast `meeting_state_changed{active}`, then `metadata_changed{<current>}`, then `mode_changed{"highlights", items: []}`. (5) Spawn mock generator. (6) If `description` non-empty: spawn extraction task (§8.4). | Valid.              |
| `active`      | `start_meeting` | `active`   | None.                                                                                                                                                                              | Invalid (state).    |
| `paused`      | `start_meeting` | `paused`   | None.                                                                                                                                                                              | Invalid (state).    |
| `active`      | `stop_meeting`  | `idle`     | (1) Cancel mock generator. (2) Cancel any in-flight extraction task. (3) Clear `items_per_mode` (re-init empty per mode). (4) Clear `metadata`. (5) `meeting_started_at = None`. (6) `current_mode = "highlights"`. (7) Broadcast `meeting_state_changed{idle}`. | Valid.              |
| `paused`      | `stop_meeting`  | `idle`     | Same as `active → idle`.                                                                                                                                                           | Valid.              |
| `idle`        | `stop_meeting`  | `idle`     | None.                                                                                                                                                                              | Invalid (state).    |
| `active`      | `pause`         | `paused`   | (1) Cancel mock generator. (2) Broadcast `meeting_state_changed{paused}`.                                                                                                          | Valid.              |
| `paused`      | `pause`         | `paused`   | None.                                                                                                                                                                              | Invalid (state).    |
| `idle`        | `pause`         | `idle`     | None.                                                                                                                                                                              | Invalid (state).    |
| `paused`      | `resume`        | `active`   | (1) Restart mock generator. (2) Broadcast `meeting_state_changed{active}`.                                                                                                         | Valid.              |
| `active`      | `resume`        | `active`   | None.                                                                                                                                                                              | Invalid (state).    |
| `idle`        | `resume`        | `idle`     | None.                                                                                                                                                                              | Invalid (state).    |
| any           | `set_mode`      | unchanged  | If `mode ∈ available_modes`: update `current_mode`, broadcast `mode_changed{mode, display_tag: None, items: items_per_mode[mode]}`. If not: emit `error{unknown_mode, intent_ref: mode}` to originator. | Valid (any state).  |
| any           | `set_metadata`  | unchanged  | If `value: Some`: insert/replace. If `value: None`: remove key (no-op if absent). Broadcast `metadata_changed{full map}`.                                                          | Valid (any state).  |
| `active`      | `mark_moment`   | `active`   | (1) Log INFO. (2) Broadcast `status` event with current state.                                                                                                                     | Valid.              |
| not `active`  | `mark_moment`   | unchanged  | None.                                                                                                                                                                              | Invalid (state).    |
| any           | `expand_item`   | unchanged  | If item exists in `items_per_mode[current_mode]`: synthesize detail (§8.6), set on item, broadcast `items_update` shaped per current mode strategy. If not: emit `error{unknown_item, intent_ref: item_id}` to originator. | Valid (any state).  |

"Invalid (state)" rows: silent ignore, WARN log. See §6.2.

### 4.2 `start_meeting` startup sequence (atomic emission order)

Within the lock:

1. `meeting_started_at = Some(Instant::now())`.
2. `meeting_state = active`.
3. `metadata = intent.metadata.unwrap_or_default()`.
4. `current_mode = "highlights"`.
5. `items_per_mode = {<each mode>: empty vec}` (already empty by invariant
   §3.2 #4 since we were in `idle`).

After the lock is dropped, broadcast in order:

1. `meeting_state_changed { meeting_state: "active" }`
2. `metadata_changed { metadata: <current_map> }`
3. `mode_changed { mode: "highlights", display_tag: None, items: [] }`

Then spawn:

1. Mock generator task (§8.1).
2. If `description` is `Some` and non-empty: extraction task (§8.4).

### 4.3 `stop_meeting` teardown

Within the lock:

1. `meeting_state = idle`.
2. Clear `metadata`.
3. Re-init `items_per_mode` to `{<each mode>: empty vec}`.
4. `meeting_started_at = None`.
5. `current_mode = "highlights"` (default).

After the lock is dropped:

1. Cancel the mock generator's `CancellationToken`.
2. Cancel any in-flight extraction task's `CancellationToken`.
3. Broadcast `meeting_state_changed { meeting_state: "idle" }`.

Note: only `meeting_state_changed` fires on stop. The PWA infers cleared
items / metadata / mode from the `idle` state and clears its UI accordingly.
Avoiding `metadata_changed { {} }` and `mode_changed` on stop keeps the
event trail focused; reconnecting clients always get a fresh snapshot.

### 4.4 `set_mode` semantics

- Validate `intent.mode ∈ available_modes` (by id). If not: emit `error`
  with `code: "unknown_mode"`, `intent_ref: intent.mode`. No state change.
- Otherwise: update `current_mode`. Compute `display_tag` (always `None` in
  stub — see §8.5). Broadcast `mode_changed { mode, display_tag, items: items_per_mode[mode].clone() }`.
- Allowed in any meeting state. While `idle`, this presets the mode for the
  next `start_meeting`. While `paused`, the mode is switched immediately
  but no items generate until `resume`.

### 4.5 `set_metadata` semantics

- If `value: Some`: insert or replace `metadata[key]`.
- If `value: None`: remove `metadata[key]`. No-op if not present.
- Broadcast `metadata_changed { metadata: <full current map> }` — always
  the full map, never a diff. Clients overwrite their local state.
- Allowed in any state. While `idle`, the metadata is preserved into the
  next `start_meeting` only if the PWA includes it in the intent's
  `metadata` field (since `start_meeting` replaces the map per §4.2).
- The architecture's "manual KV wins on conflict" rule applies at extraction
  time (§8.4), not at every `set_metadata` call.

### 4.6 `mark_moment` semantics

- Only valid when `meeting_state == active`. Otherwise silent + WARN.
- Action: log `INFO mark_moment t=<...> note=<...>`.
- Side effect: broadcast a `status` event with current state. This serves
  as both an ack and an explicit "I heard you" signal to the PWA.
- Real persistence (writing the moment to the meeting record) lands in
  Phase 2. Stub does not retain moments.

### 4.7 `expand_item` semantics

- Look up `intent.item_id` in `items_per_mode[current_mode]`.
- If absent: emit `error { code: "unknown_item", intent_ref: item_id }` to
  the originator only.
- If present: synthesize a detail string (§8.6) and set it on the item.
- Broadcast `items_update` shaped per the current mode's `update_strategy`:
  - `replace`: full vec including the augmented item.
  - `append`: single-item vec containing only the augmented item (PWA
    upserts by id, replacing the previous undecorated copy in place).
- Repeated `expand_item` for the same item is idempotent: detail is
  overwritten with a freshly synthesized string. (In Phase 2 with real
  detail synthesis, this would re-fetch from cache.)

### 4.8 Reconnect

- Each new connection receives a `snapshot` immediately after auth.
- The snapshot reflects current global state: `meeting_state`, full
  `available_modes`, `current_mode`, `items` for the current mode,
  `metadata`, `display_tag`, and `status`.
- The server keeps no per-client session state. Slow or dropped connections
  are dropped silently and their broadcast subscribers naturally pruned
  when the per-connection task ends.
- A PWA reconnecting mid-meeting sees the active state and resumes
  rendering. PWA-local UI (the `listening` view) does not survive
  reconnect — the PWA cancels listening on reconnect per architecture §7.

### 4.9 Heartbeat

- A global background task driven by `tokio::time::interval(Duration::from_secs(10))`.
- Each tick: lock state, build a `Status` from current values, drop lock,
  broadcast `status { status: <built> }`.
- `Status.listening = (meeting_state == active)`.
- `Status.paused = (meeting_state == paused)`.
- `Status.error = None` in stub. (Phase 2 will populate this with transient
  pipeline errors; the field clears on the next tick if no current error.)
- Heartbeat continues regardless of meeting state — even in `idle` it
  serves as a "server still alive" signal so PWAs can detect dead
  connections and trigger reconnect.

## 5. Configuration

### 5.1 Environment variables

| Var                       | Required | Default | Description                                                                 |
|---------------------------|----------|---------|-----------------------------------------------------------------------------|
| `MEETING_COMPANION_TOKEN` | yes      | —       | Shared secret for WS auth. Server refuses to start if unset or empty.       |
| `RUST_LOG`                | no       | `info`  | Standard `tracing-subscriber::EnvFilter` syntax (e.g., `info,server=debug`). |

### 5.2 CLI args

| Flag      | Default     | Description                  |
|-----------|-------------|------------------------------|
| `--port`  | `7331`      | TCP port to bind.            |
| `--bind`  | `0.0.0.0`   | Bind address.                |

Parsed via `clap` derive. `--help` prints usage.

### 5.3 Compile-time defaults

- `PROTOCOL_VERSION = 1`.
- Default `available_modes`: see §8.3.
- Default `current_mode` after server boot and after `stop_meeting`:
  `"highlights"`.
- Heartbeat interval: 10 seconds.
- Mock generator interval: 3 seconds.
- Extraction simulator delay: 1500 milliseconds.
- Replace-strategy item cap (FIFO): 10.
- Broadcast channel capacity: 64.
- Max inbound frame size: 64 KiB.
- Graceful shutdown drain timeout: 2 seconds.

## 6. Errors & failure modes

### 6.1 Protocol error event

Wire format:

```json
{
  "type": "error",
  "code": "<code>",
  "message": "<human-readable description>",
  "intent_ref": "<optional contextual reference, e.g., a mode id or item id>"
}
```

Defined codes:

| `code`            | Trigger                                                                        | `intent_ref`               |
|-------------------|--------------------------------------------------------------------------------|----------------------------|
| `bad_json`        | Inbound frame is not valid JSON or not a JSON object.                          | absent                     |
| `unknown_intent`  | Intent's `type` discriminator is not in the recognized set.                    | the unrecognized type str  |
| `bad_payload`     | Intent type is recognized but a required field is missing or has wrong type.   | the field name (if known)  |
| `unknown_mode`    | `set_mode` with a mode id not in `available_modes`.                            | the mode id                |
| `unknown_item`    | `expand_item` with an item id not in `items_per_mode[current_mode]`.           | the item id                |

`error` events are sent **only to the originating client** (per-connection,
not broadcast). Other connected clients see nothing.

### 6.2 State errors (silent ignore)

The following intents in the wrong meeting state are **silently ignored**:

| Intent          | Wrong state(s)            |
|-----------------|---------------------------|
| `start_meeting` | `active`, `paused`        |
| `stop_meeting`  | `idle`                    |
| `pause`         | `idle`, `paused`          |
| `resume`        | `idle`, `active`          |
| `mark_moment`   | `idle`, `paused`          |

Action on encountering one:

1. Log at WARN with intent type and current state.
2. No state change.
3. No event emitted (no `error`, no acknowledgement).

Rationale: the PWA UX should make these unreachable (Start button disabled
when active, etc.). If a state-invalid intent arrives, it indicates a PWA
bug worth surfacing in server logs but not worth a user-facing error event.

### 6.3 Auth failures

- Missing `token` query param OR mismatch with `MEETING_COMPANION_TOKEN`:
  WebSocket upgrade completes, then the server immediately sends a close
  frame with code `1008` ("policy violation") and reason `"invalid token"`.
- No `error` event is emitted (the connection is not yet a subscriber).
- The auth attempt is logged at WARN with the peer address.

### 6.4 Connection failures

- **Slow consumer** (broadcast `Receiver` lags beyond capacity 64):
  detected as `RecvError::Lagged` in the per-connection write loop. Action:
  log at WARN with peer address and lag count, close the WS with code
  `1011` ("internal error") and reason `"client lagging"`. The client
  reconnects per its own backoff policy and gets a fresh snapshot.
- **Frame too large** (>64 KiB inbound): close with code `1009`.
- **TCP-level disconnect**: the per-connection task ends naturally; the
  broadcast `Receiver` is dropped, pruning the subscriber.

### 6.5 Server startup failures

| Condition                                  | Behavior                                |
|--------------------------------------------|-----------------------------------------|
| `MEETING_COMPANION_TOKEN` unset or empty   | Print error to stderr, exit code 2.     |
| Bind address already in use                | Print error to stderr, exit code 1.     |
| Other unrecoverable boot error             | Log + exit code 1.                      |

### 6.6 Graceful shutdown

Triggered by SIGINT or SIGTERM:

1. Stop accepting new connections.
2. Cancel the heartbeat task.
3. Cancel the mock generator (if running) and any extraction task.
4. Send a WS close frame with code `1001` ("going away") to every
   connected client.
5. Wait up to 2 seconds for clean close acks.
6. Drop all connections (forceful close).
7. Exit code 0.

## 7. Concurrency model

### 7.1 Tasks

| Task                    | Lifetime                          | Purpose                                                                          |
|-------------------------|-----------------------------------|----------------------------------------------------------------------------------|
| Acceptor                | server lifetime                   | Accepts TCP connections, performs WS handshake + auth, spawns per-connection.    |
| Per-connection (×N)     | per WebSocket connection          | Reads inbound frames, dispatches intents; writes broadcast events to the socket. |
| Heartbeat               | server lifetime                   | Emits `status` every 10s.                                                        |
| Mock generator          | per active meeting                | Emits `items_update` every 3s while meeting is active. Spawned on `start_meeting`, cancelled on `stop_meeting` and `pause`. |
| Extraction simulator    | per `start_meeting` invocation    | After 1500ms, computes extracted metadata and broadcasts `metadata_changed`. Cancellable. |
| Shutdown listener       | server lifetime                   | Catches SIGINT/SIGTERM, signals all other tasks via the shutdown channel.        |

### 7.2 Synchronization primitives

- `state: Arc<tokio::sync::Mutex<ServerState>>` — single mutex protecting
  all state.
- `events_tx: tokio::sync::broadcast::Sender<Event>` — capacity **64**.
  Slow consumers receive `RecvError::Lagged` and are disconnected (§6.4).
- `shutdown: tokio::sync::watch::Sender<bool>` — toggled to true on
  signal; tasks observe the receiver and exit.
- `meeting_lifecycle: Mutex<Option<CancellationToken>>` — held by the
  state module. Replaced on each `start_meeting` (with a fresh token) and
  cancelled on `stop_meeting` / `pause`. The mock generator and extraction
  simulator hold child tokens and observe cancellation.

### 7.3 Mutex discipline

**Lock-then-act-then-emit, never await under the lock.**

1. Acquire `state` lock.
2. Mutate state.
3. Build the `Vec<Event>` to broadcast (clone what's needed).
4. Drop the lock.
5. `events_tx.send(event)` for each event.
6. (If applicable) spawn or cancel background tasks.

The build-then-drop pattern keeps the lock hold time bounded by struct
mutation work — never by network I/O.

### 7.4 Per-connection lifecycle

```text
client connects (TCP)
    ↓
WS handshake (libp2p / tokio-tungstenite handles upgrade)
    ↓
Server checks token → if bad: close(1008), end.
    ↓
Spawn per-connection task:
    1. Subscribe to events_tx (Receiver)
    2. Lock state, build Snapshot, drop lock
    3. Send Snapshot to client
    4. select! loop:
        - inbound frame → decode + dispatch intent (locks state internally)
        - outbound event from broadcast → serialize + send
        - shutdown signal → break
    5. On exit: log disconnect, drop Receiver
```

### 7.5 Single active meeting

The state machine is global. There is exactly one current meeting at any
time. Multiple connected clients are equal peers — every event broadcasts
to all. There is no notion of "primary" or "owner" client.

## 8. Mock content (Phase 0)

### 8.1 Mock generator task

Lifetime: spawned on `idle → active` transition; cancelled on transition
out of `active`. (On `paused → active` via `resume`, a fresh task is
spawned.)

Loop:

```text
interval = tokio::time::interval(3 seconds)
loop {
    select! {
        _ = interval.tick() => emit_one_tick()
        _ = cancellation_token.cancelled() => break
    }
}
```

`emit_one_tick()`:

1. Lock state.
2. Read `current_mode` and its `update_strategy` from `available_modes`.
3. Append a new fake item (§8.6) to `items_per_mode[current_mode]`.
4. If strategy is `replace` and length > 10: drop oldest entries until
   length == 10 (FIFO).
5. Determine the broadcast payload:
   - `replace`: full clone of `items_per_mode[current_mode]`.
   - `append`: single-element `vec![new_item.clone()]`.
6. Drop lock.
7. Broadcast `items_update { items: <payload> }`.

If the user switches mode mid-meeting, the next tick generates content for
the new mode. Items previously generated in other modes remain in their
respective entries and reappear when the user switches back via
`set_mode → mode_changed { items: <retained> }`.

### 8.2 Cap policy for `replace` modes

- Cap: 10 items.
- Eviction: FIFO (drop the oldest).
- Rationale: keeps the glasses-side display bounded without truncating
  meaningful content. Real `replace` modes in Phase 2 will manage their
  own caps via summarizer logic.

### 8.3 Default mode catalog

Hardcoded at compile time:

```rust
fn default_modes() -> Vec<ModeOption> {
    vec![
        ModeOption {
            id: "highlights".into(),
            label: "Highlights".into(),
            update_strategy: UpdateStrategy::Replace,
        },
        ModeOption {
            id: "transcript".into(),
            label: "Transcript".into(),
            update_strategy: UpdateStrategy::Append,
        },
        ModeOption {
            id: "actions".into(),
            label: "Actions".into(),
            update_strategy: UpdateStrategy::Append,
        },
    ]
}
```

The catalog covers both update strategies (one `replace`, two `append`),
which lets the PWA exercise both code paths in Phase 0.

### 8.4 Simulated LLM extraction

Triggered when `start_meeting.description` is present and non-empty.

Pseudocode:

```text
spawn async {
    select! {
        _ = sleep(1500ms) => {
            extracted = HashMap::from([
                ("title",   first_n_words(description, 8)),
                ("project", "sim-extracted"),
            ])
            lock state.
            if meeting_state != active { drop lock; return }    // raced with stop
            manual = state.metadata.clone()
            merged = extracted ∪ manual                          // manual wins on conflict
            state.metadata = merged.clone()
            drop lock.
            broadcast metadata_changed { metadata: merged }
        }
        _ = cancellation_token.cancelled() => return
    }
}
```

If `description` is empty or absent, no extraction task is spawned; the
only `metadata_changed` event for that meeting is the immediate one in
the startup sequence (§4.2).

If the meeting is stopped or paused before the 1500ms elapses, the
cancellation token aborts the task without broadcasting.

The merge rule: extracted fields **fill in missing keys**; manual fields
**win on conflict**. This matches the architecture's stated behavior in §3.

### 8.5 `display_tag` in stub

Always `None` / omitted. The wire format supports it on `mode_changed`,
`display_tag_changed`, and `snapshot`; Phase 2 (real STT) will populate it
with speaker labels via `display_tag_changed`. The PWA must already handle
the optional / null case correctly.

### 8.6 Item content templates

Each new item is generated with:

- `id`: fresh UUIDv4.
- `t`: `(now - meeting_started_at).as_millis() as u64` (cast to `f64`/`number` for JSON).
- `text`: rotating per-mode template; see below.
- `detail`: `None` until `expand_item`.
- `meta`: `None`.

Per-mode rotating templates. Each generator tick advances the index modulo
the array length:

**`highlights`:**
```
[
  "Tiago raised concern about Q1 budget overrun",
  "Decision: ship feature X by end of sprint",
  "Open question: who owns the migration",
  "Action item: schedule follow-up with vendor",
  "Aline highlighted the dependency on the auth team",
  "Push the launch date by two weeks",
  "Concern: test coverage gap in the new module",
  "Confirmed: customer is OK with the proposed timeline",
]
```

**`transcript`:**
```
[
  "Speaker A: I think we should delay the launch by two weeks.",
  "Speaker B: Acknowledged. Let me check with engineering.",
  "Speaker A: The dependency on the auth team is the blocker.",
  "Speaker C: I can take the auth conversation offline.",
  "Speaker A: Great. What about the migration plan?",
  "Speaker B: Draft is ready, sending tonight.",
  "Speaker C: Are we testing against staging first?",
  "Speaker A: Yes, full staging soak before prod.",
  "Speaker B: Agreed. We'll set up the soak window.",
  "Speaker A: Anything else? OK, ending here.",
]
```

**`actions`:**
```
[
  "Tiago: Draft proposal by Friday",
  "Aline: Confirm vendor availability",
  "Speaker C: Sync with auth team on dependency",
  "Speaker B: Send migration draft tonight",
  "Speaker A: Schedule staging soak window",
  "Tiago: Update launch date in roadmap",
]
```

**Detail synthesis** (used by `expand_item`):

Format: `"Detail for '<item.text>': lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam."`

Static across calls — repeated `expand_item` for the same item produces
the same detail. This makes Phase 0 deterministic and easy to test.

## 9. Logging & observability

### 9.1 Framework

- `tracing` crate.
- `tracing-subscriber::fmt::layer()` writing human-readable to stderr.
- `EnvFilter::from_default_env()` for level control.
- Default level: `info`. Override via `RUST_LOG`.

### 9.2 What gets logged

| Event                                | Level | Context fields                             |
|--------------------------------------|-------|--------------------------------------------|
| Server boot                          | INFO  | port, bind, version, protocol_version      |
| Connection accepted                  | INFO  | peer_addr                                  |
| Auth failure                         | WARN  | peer_addr, reason (`missing` / `mismatch`) |
| Connection closed (normal)           | INFO  | peer_addr, duration_ms                     |
| Connection closed (error)            | WARN  | peer_addr, error                           |
| Intent received                      | DEBUG | peer_addr, intent_type                     |
| Event broadcast                      | DEBUG | event_type, n_subscribers                  |
| State transition                     | INFO  | from, to, intent                           |
| State error (silent ignore)          | WARN  | intent_type, current_state                 |
| Protocol error (sent to client)      | WARN  | peer_addr, code, message, intent_ref       |
| Mock generator tick                  | TRACE | mode, item_id                              |
| Heartbeat tick                       | TRACE | n_subscribers                              |
| Extraction completed                 | INFO  | merged_keys                                |
| Extraction cancelled                 | DEBUG | reason                                     |
| Shutdown signal                      | INFO  | signal                                     |

### 9.3 No metrics, no tracing exporter

Phase 0 does not export Prometheus or OpenTelemetry. Stderr logs only.
Phase 2 may add metrics if the meeting pipeline grows enough complexity to
justify it.

## 10. Test strategy

### 10.1 Unit tests

In-source `#[cfg(test)] mod tests`:

- **`contract.rs`** — serde round-trip:
  - Every `Intent` variant: encode → decode → compare.
  - Every `Event` variant: encode → decode → compare.
  - Optional fields: `description?` absent vs empty string; `display_tag?`
    absent vs string; `error?` absent vs string.
  - `set_metadata` `value: null` round-trips correctly.
  - Unknown discriminator on inbound: returns a `bad_json`-equivalent
    decode error that the caller turns into an `error` event.

- **`state.rs`** — state machine:
  - Every cell of the §4.1 transition table is exercised by a test.
  - Each test starts from a known state, applies one intent, asserts the
    resulting state and the returned `Vec<Event>`.
  - Invariants from §3.2 are asserted at the end of every test.
  - `expand_item` for both strategies: replace returns full vec, append
    returns single-element vec, both with `detail` set.
  - `set_metadata` with `null` value removes the key.

### 10.2 Integration tests

In `apps/server/tests/`. Each test spins up a real server bound to port 0,
reads the actual `SocketAddr`, connects via `tokio_tungstenite::connect_async`,
exchanges messages, asserts.

| Test                              | Scenario                                                                                                     |
|-----------------------------------|--------------------------------------------------------------------------------------------------------------|
| `handshake_token_match`           | Connect with matching token → snapshot received within 1s.                                                   |
| `handshake_token_mismatch`        | Connect with bad token → close with code 1008.                                                               |
| `handshake_token_missing`         | Connect with no token query param → close with code 1008.                                                    |
| `snapshot_initial_state`          | First connect: snapshot has `idle`, default modes, empty items, empty metadata, `protocol_version: 1`.       |
| `start_stop_meeting_events`       | Send `start_meeting` → receive `meeting_state_changed`, `metadata_changed`, `mode_changed` in order. Send `stop_meeting` → receive `meeting_state_changed{idle}`. |
| `start_meeting_with_metadata`     | `start_meeting { metadata: {project: helix} }` → first `metadata_changed` carries `{project: helix}`.        |
| `pause_resume_events`             | `start → pause → resume → stop`: each transition emits exactly one `meeting_state_changed`.                  |
| `set_mode_valid`                  | After start: `set_mode "transcript"` → receive `mode_changed { mode: "transcript", items: [] }`.             |
| `set_mode_unknown`                | `set_mode "bogus"` → receive `error { code: "unknown_mode", intent_ref: "bogus" }`. State unchanged.         |
| `set_mode_in_idle`                | Without starting: `set_mode "transcript"` → `mode_changed` received. (Allowed in any state.)                 |
| `set_metadata_basic`              | `set_metadata foo=bar` → `metadata_changed { foo: "bar" }`.                                                  |
| `set_metadata_delete`             | After `foo=bar`: `set_metadata foo=null` → `metadata_changed {}`.                                            |
| `extraction_merge_manual_wins`    | `start_meeting { description: "Q1 budget", metadata: { project: "helix" } }` → first `metadata_changed` has `{project: helix}`; second (~1.5s later) has `{title: "Q1 budget", project: "helix"}` (manual wins). |
| `extraction_no_description`       | `start_meeting { metadata: {project: helix} }` (no description) → exactly one `metadata_changed` for the meeting; no second event after 2s. |
| `extraction_cancelled_on_stop`    | `start_meeting { description: "..." }`; immediately `stop_meeting`. Wait 2.5s → no `metadata_changed` from extraction. |
| `mock_items_replace`              | `start_meeting`, mode=highlights, wait 4s → receive at least one `items_update` whose payload is full vec.   |
| `mock_items_append`               | `start_meeting`, `set_mode transcript`, wait 4s → receive `items_update`s each with single-item vecs.        |
| `mock_items_cap`                  | `start_meeting`, mode=highlights, wait 35s → no `items_update` payload exceeds 10 items.                     |
| `mock_stops_on_pause`             | `start`, wait 4s, `pause`, wait 6s → no `items_update` events received during the pause window.              |
| `mock_resumes_on_resume`          | `start`, `pause`, `resume`, wait 4s → at least one `items_update` after resume.                              |
| `mock_stops_on_stop`              | `start`, wait 4s, `stop`, wait 6s → no `items_update` events received after stop.                            |
| `expand_item_append`              | `start`, `set_mode transcript`, wait for an item; `expand_item` that id → receive `items_update` with single item carrying `detail`. |
| `expand_item_replace`             | `start` (mode=highlights), wait for item; `expand_item` id → receive `items_update` with full list, target item has `detail`. |
| `expand_item_unknown`             | `expand_item "no-such-id"` → receive `error { code: "unknown_item", intent_ref: "no-such-id" }`.            |
| `mark_moment_active`              | `start`; `mark_moment t=123` → receive `status` event.                                                       |
| `mark_moment_idle`                | (Without starting) `mark_moment t=0` → no event received within 1s.                                          |
| `bad_json`                        | Send a frame with invalid JSON (`"not json"`) → receive `error { code: "bad_json" }`.                        |
| `unknown_intent`                  | Send `{"type": "fly_to_moon"}` → receive `error { code: "unknown_intent" }`.                                 |
| `bad_payload`                     | Send `{"type": "set_mode"}` (missing `mode`) → receive `error { code: "bad_payload" }`.                      |
| `error_only_to_originator`        | Connect A and B; A sends bad intent → only A receives `error`; B receives nothing.                           |
| `two_clients_broadcast`           | Connect A and B; A sends `start_meeting` → both A and B receive `meeting_state_changed{active}`.             |
| `reconnect_snapshot_active`       | Connect A, `start_meeting`, disconnect; reconnect A → snapshot has `meeting_state: active` and current mode/items. |
| `heartbeat_idle`                  | Connect, wait 11s → at least one `status` event with `listening:false`, `paused:false`.                      |
| `heartbeat_active`                | Connect, `start_meeting`, wait 11s → `status` events have `listening:true`.                                  |
| `lagged_client_disconnect`        | Connect A, then send 200 events rapidly without consuming → A's connection eventually closes with code 1011. |
| `graceful_shutdown`               | Connect A; send SIGINT to server → A receives close frame with code 1001.                                    |

### 10.3 Test conventions

- Helper `spawn_test_server() -> (SocketAddr, ShutdownHandle)`: spawns the
  server with a fixed token (`"test-token"`) on port 0, returns the bound
  address and a handle whose `Drop` triggers shutdown.
- Helper `connect(addr, token) -> (Sink, Stream)`: opens WS, returns the
  split sink/stream pair.
- Helper `next_event(stream, timeout) -> Event`: reads one frame, decodes
  to `Event`, fails the test on timeout or decode error.
- Helper `assert_event_matches!(actual, pattern)`: macro for pattern-style
  assertions on event variants.
- Default per-test timeout: 5 seconds (via `tokio::time::timeout`).
- Time-sensitive tests (`mock_*`, `extraction_*`, `heartbeat_*`) explicitly
  call `tokio::time::sleep` rather than mocking time, to keep the tests
  exercising the real timing paths. The cadence constants (3s mock, 1.5s
  extraction, 10s heartbeat) are defined as `const` so tests can reference
  them rather than duplicate them as magic numbers.

### 10.4 Manual smoke

A `Justfile` recipe `just smoke`:

1. Starts the server in the background with `MEETING_COMPANION_TOKEN=dev`.
2. Uses `websocat` to connect: `websocat 'ws://localhost:7331/?token=dev'`.
3. Pipes a series of intents via stdin.
4. Pretty-prints incoming events.
5. Stops the server cleanly (sends SIGTERM).

Used during development to eyeball the contract end-to-end without
spinning up the PWA.

## 11. Out of scope

- Real audio capture via ScreenCaptureKit. Phase 2; architecture §3, §10
  step 15.
- Real STT pipeline. Architecture §9.
- Real LLM metadata extraction. Phase 2; architecture §10 step 16.
- Real summarization / mode-specific item generation. Architecture §9.
- Memory-system enrichment. Phase 2; architecture §10 step 18.
- Persistence across server restarts. No DB. In-memory only. Restart =
  back to fresh `idle`.
- TLS / WSS termination. Plain `ws://` on LAN. Tunneling via Tailscale or
  cloudflared handles TLS upstream.
- Multiple concurrent meetings. Single active meeting at a time.
- Multiple separate users. Single shared-secret model; no per-user auth.
- Token rotation, audit logs, rate limiting on bad-token attempts.
- Metrics or telemetry exports.
- macOS Screen Recording permission handling. Phase 2.
- `available_modes_changed` event emission. Reserved in the contract for
  future dynamic mode catalogs. Stub catalog is fixed at compile time.
- `display_tag_changed` event emission. Reserved in the contract; stub
  always omits `display_tag`.

## 12. Open questions

None at time of writing. All §1–§11 decisions resolved during the
2026-04-30 spec brainstorm. New questions surfaced during implementation
should be added here and resolved before the implementation plan is
revised.
