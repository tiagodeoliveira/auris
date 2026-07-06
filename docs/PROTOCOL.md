# Wire Protocol

The four clients (server, Mac, PWA, mobile) communicate over a single
WebSocket plus a REST API on port 7331. The contract lives in four
hand-maintained files kept in sync by review:

- Server (Rust, source of truth): [`packages/server/src/contract.rs`](../packages/server/src/contract.rs)
- PWA (TypeScript): [`packages/pwa/src/contract.ts`](../packages/pwa/src/contract.ts)
- Mac (Swift): [`packages/mac/Sources/Auris/Net/Protocol.swift`](../packages/mac/Sources/Auris/Net/Protocol.swift)
- Mobile (TypeScript): [`packages/mobile/src/wire/contract.ts`](../packages/mobile/src/wire/contract.ts)

See [ADR-0004](adr/0004-websocket-protocol.md) for the rationale. A
shared codegen-from-Rust path is the long-term plan; until then,
adding a field requires editing all four.

## Endpoints

```
GET  /                                                   control WS — Intent ↔ Event
GET  /audio                                              binary PCM frames from a capture-capable client
GET  /stt                                                server-mediated STT — PCM in, transcript JSON out
GET  /meetings, /artifacts, /meetings/:id/...            REST endpoints (see "REST API" below)
```

All endpoints are JWT-authenticated (see Authentication). REST and
WS share the same port (7331) and the same router; there is no
separate `/api/` prefix.

## Authentication

Auth0-issued JWT, validated server-side against Auth0's JWKS. Clients
authenticate per request:

- **WebSocket**: `?token=<JWT>` on the handshake URL. The server
  rejects connections with a missing, expired, or invalid JWT
  immediately after the upgrade.
- **REST**: `Authorization: Bearer <JWT>` header.

The server caches JWKS keys by `kid` with a refetch-on-miss cooldown
to resist forged-`kid` flooding (see `packages/server/src/auth.rs`).

For local dev / CI, set `AURIS_AUTH_DISABLED=1`. The
server then synthesises a stable user (`auth0_sub = dev-user`) and
the JWT is ignored entirely.

Auth0 client distribution:

| Client | Auth0 app type | Notes                                                                                                                            |
| ------ | -------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Server | (none)         | Validates JWTs against the configured audience.                                                                                  |
| PWA    | SPA            | Auth0 Universal Login redirect flow. Token persisted via the bridge.                                                             |
| Mac    | Native         | Authorization Code + PKCE, refresh token in Keychain.                                                                            |
| Mobile | Native         | Authorization Code + PKCE via `expo-auth-session`, refresh token in `expo-secure-store`. Currently shares the Mac's `client_id`. |

The server's per-user state is keyed on `auth0_sub`. First sign-in
mints a row in `users` with a fresh UUID; subsequent sign-ins from
any client land on the same row.

## Versioning

A `protocol_version: u32` constant lives in all four contract files
(today: `1`). On connect the server emits a `Snapshot` carrying its
compiled-against version. Clients refuse to operate when their
compile-time expectation doesn't match, surfacing a non-dismissable
error overlay.

Bump the constant when changing intent or event semantics in a way
that isn't backwards-compatible. Adding optional fields (e.g.
`#[serde(skip_serializing_if = "Option::is_none", default)]` in Rust)
does NOT require a bump.

## Snapshot

The first message from the server on every connection. Contains the
full per-user state needed to render any client without prior context.

```jsonc
{
  "type": "snapshot",
  "protocol_version": 1,
  "meeting_state": "idle" | "active" | "paused",
  "meeting_id": "<uuid, omitted when idle>",
  "available_modes": [
    { "id": "...", "label": "...", "update_strategy": "append" | "replace" }
  ],
  "mode": "<current mode id>",
  "display_tag": "<optional, mode-specific badge>",
  "metadata": { "<key>": "<value>", ... },
  "items": [{ "id": "...", "text": "...", "t": <ms>, "meta": { ... } }],
  "status": { "listening": <bool>, "paused": <bool>, "error": "<optional>" },
  "prior_context": {
    "preferences": <int>, "facts": <int>, "episodes": <int>, "project_memories": <int>
  },
  "devices": [
    { "id": "...", "hostname": "...", "capabilities": [...], "online": <bool> }
  ],
  "audio_source_device_id": "<id of device feeding audio, omitted if none>"
}
```

`prior_context` is omitted when no mnemo recall has populated it.
`meeting_id` and `audio_source_device_id` are omitted when their
values are `None` (idle / unbound).

## Intents (client → server)

Every intent is a JSON object with a snake-case `type` discriminator.
Unknown types produce an `error` event with code `unknown_intent`;
known types with malformed payloads produce code `bad_payload`.

| Type               | Fields                                                                                                             | Effect                                                                                                                                                                                                           |
| ------------------ | ------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `start_meeting`    | `description?: string`, `metadata?: Record<string,string>`, `audio_source_device_id?: string`                      | Idle → Active. If `description` is present and `metadata` empty/absent, server fires LLM extraction in background. `audio_source_device_id` binds a registered device's `/audio` stream as the meeting's source. |
| `stop_meeting`     | none                                                                                                               | Active/Paused → Idle. Clears items, metadata, recalled context, in-flight extraction. Pushes the summary bundle to mnemo.                                                                                        |
| `pause`            | none                                                                                                               | Active → Paused. Audio + STT continue but the agent idles.                                                                                                                                                       |
| `resume`           | none                                                                                                               | Paused → Active.                                                                                                                                                                                                 |
| `set_mode`         | `mode: string`                                                                                                     | Switch the current mode. Mode tab clicks on any client.                                                                                                                                                          |
| `set_metadata`     | `key: string`, `value: string \| null`                                                                             | Insert/replace key. `null` deletes. Always emits `metadata_changed`.                                                                                                                                             |
| `extract_metadata` | `description: string`                                                                                              | Trigger LLM extraction in idle without changing state. Result merges with existing via "manual wins". See [ADR-0010](adr/0010-extract-metadata-flow.md).                                                         |
| `register_device`  | `hostname: string`, `capabilities: ("audio_capture" \| "screen_capture" \| "control_surface" \| "system_audio")[]` | Capability-bearing client (Mac, future glasses) declares itself. Server replies `device_registered` to the sender and `devices_changed` to the user's other clients.                                             |
| `mark_moment`      | `t: number`, `note?: string`                                                                                       | Inserts a moment row. If a `screen_capture`-capable device is bound, server sends it `capture_moment_screenshot`; the device uploads via REST. Vision-LLM summarizer fires.                                      |
| `expand_item`      | `item_id: string`                                                                                                  | Asks the agent to fill the item's `detail` field. Agent emits `item_updated` for the current mode with the enriched item.                                                                                        |
| `chat`             | `text: string`                                                                                                     | User question for the agent. Allowed only when `meeting_state` is `active` or `paused`. Renders as a Q+A bubble pair in chat mode (Replace strategy, single pair).                                               |

## Events (server → client)

| Type                          | Fields                                                        | Notes                                                                                                                                     |
| ----------------------------- | ------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `snapshot`                    | (see above)                                                   | First message after connect.                                                                                                              |
| `meeting_state_changed`       | `meeting_state`, `meeting_id?`                                | After every lifecycle transition. `meeting_id` present going to `active`/`paused`, omitted going to `idle`.                               |
| `mode_changed`                | `mode`, `display_tag?`, `items`                               | After `set_mode`.                                                                                                                         |
| `display_tag_changed`         | `tag?`                                                        | Mode-specific small status string.                                                                                                        |
| `metadata_changed`            | `metadata`                                                    | Full metadata after any change.                                                                                                           |
| `prior_context_changed`       | `summary: { preferences, facts, episodes, project_memories }` | After mnemo recall completes. All-zero counts means cleared.                                                                              |
| `items_update`                | `mode`, `items`                                               | Per-mode item list. Update strategy depends on the mode (`append` or `replace`).                                                          |
| `item_updated`                | `mode`, `item`                                                | One item changed in-place — replace by id within the mode's list. Today's only producer is `expand_item` (agent writing a detail).        |
| `transcript_interim`          | `text`                                                        | Live in-flight transcript chunk between sentence boundaries. Clients render as the dim "live row" in transcript mode.                     |
| `status`                      | `status: { listening, paused, error? }`                       | Heartbeat-driven (~3s while active); also used for transient extraction errors.                                                           |
| `error`                       | `code`, `message`, `intent_ref?`                              | Per-intent rejection. Codes: `bad_json`, `unknown_intent`, `bad_payload`, `unknown_mode`, `unknown_item`. Delivered to sender only.       |
| `device_registered`           | `device`                                                      | Sent only to the connection that just registered. Carries the assigned `device_id`.                                                       |
| `devices_changed`             | `devices`                                                     | Broadcast on registration / disconnect / capability change. Full current list — clients replace their cache.                              |
| `audio_source_device_changed` | `device_id?`                                                  | Audio-source binding for the active meeting changed. `None` means unbound.                                                                |
| `artifacts_changed`           | `artifact_ids`                                                | Attached-artifact set for the active meeting changed. Full current id set — clients overwrite their local mirror.                         |
| `capture_moment_screenshot`   | `meeting_id`, `moment_id`, `t_ms`                             | **Point-to-point** to the bound `screen_capture`-capable device only. Recipient uploads via `POST /meetings/:id/moments/:mid/screenshot`. |
| `moment_summarized`           | `moment_id`, `meeting_id`, `t_ms`, `summary`, `note?`         | Vision-LLM summarizer wrote the moment's summary. Currently consumed by the mnemo pusher; clients ignore.                                 |

## Modes

The mode catalog is server-defined and currently fixed at compile
time (`packages/server/src/state.rs`):

| ID               | Label          | Update strategy | Source                                                                                                         |
| ---------------- | -------------- | --------------- | -------------------------------------------------------------------------------------------------------------- |
| `transcript`     | Transcript     | `append`        | Soniox sentence-flush via STT adapter (no LLM).                                                                |
| `highlights`     | Highlights     | `replace`       | Agent loop tool-call (`replace_highlights` — single Replace per fire so the list stays compact).               |
| `actions`        | Actions        | `append`        | Agent loop tool-call (`push_action`).                                                                          |
| `open_questions` | Open Questions | `append`        | Agent loop tool-call (`push_open_question`).                                                                   |
| `summary`        | Summary        | `replace`       | `summary` summarizer (single-item Replace). Hybrid trigger: token threshold OR 5-min ceiling, whichever first. |
| `chat`           | Chat           | `replace`       | `chat` intent renders user bubble + agent reply as a single Q+A pair. Replace, single pair — see ADR-0011.     |

`transcript` is the default. The agent loop drives
highlights / actions / open_questions / chat; see
[ADR-0011](adr/0011-agentic-summarizer-loop.md) for the design.

## Item shape

```jsonc
{
  "id": "<server-generated, mode-prefixed>",
  "text": "<surface text shown in the items list>",
  "detail": "<optional long-form text, populated lazily on expand_item>",
  "t": <milliseconds since meeting start>,
  "meta": { /* mode-specific extras (e.g. role, owner, due, importance, kind) */ }
}
```

Item IDs are stable for the meeting's lifetime; clients use them for
`expand_item` requests, for client-side keying, and for matching
`item_updated` events back to a row.

For the `chat` mode, `meta.role` is `"user"` for the question bubble
and `"assistant"` for the answer. PWA and mobile use this to style
the bubbles.

## Errors

```jsonc
{
  "type": "error",
  "code": "<bad_json | unknown_intent | bad_payload | unknown_mode | unknown_item>",
  "message": "<human-readable detail>",
  "intent_ref": "<optional — the offending intent type, when known>",
}
```

`error` is delivered to the originating client only (not broadcast).
The PWA / Mac / mobile each surface it as a toast with the code prefix.

## Heartbeat

The server emits a `status` event every ~3s when the meeting is
active, carrying the live `listening` / `paused` flags and any
transient error string. Used by clients to detect a hung connection
and trigger reconnection.

## Audio + STT WS endpoints

Two binary WS endpoints share the JWT-on-query-string auth model:

- **`/audio`** — capture-capable client streams S16LE 16 kHz mono PCM
  frames into the server. Server mixes / forwards to the active
  meeting's STT pipeline. Frame layout: raw little-endian samples,
  no header. The client sends a brief JSON header on connect
  declaring its `device_id` and audio configuration; on
  authorization the server replies with a JSON ack and the client
  flips to binary mode.
- **`/stt`** — server-mediated STT for the dictation flow (Mac
  description mic, PWA listening flow). Client streams PCM exactly as
  for `/audio`; server runs its own Soniox session and pushes back a
  JSON-text stream of `{type: "ready"|"interim"|"final"|"error", ...}`
  messages. No provider keys leave the server.

See [ADR-0006](adr/0006-live-audio-stt-pipeline.md) for the audio
pipeline rationale.

## REST API

JWT in `Authorization: Bearer <JWT>` for all endpoints. Per-user
scoping is enforced server-side via the JWT's `auth0_sub`.

| Method | Path                                           | Purpose                                                                                                 |
| ------ | ---------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| GET    | `/meetings`                                    | List the user's meetings, newest first. Returns summaries.                                              |
| GET    | `/meetings/:id`                                | Full detail for one meeting: description, metadata, transcript, moments, attached artifacts, LLM usage. |
| DELETE | `/meetings/:id`                                | Delete a meeting and its blobs.                                                                         |
| GET    | `/meetings/:id/moments/:moment_id/screenshot`  | Fetch a moment's screenshot bytes.                                                                      |
| POST   | `/meetings/:id/moments/:moment_id/screenshot`  | Upload a moment's screenshot (multipart). Sent by the device that received `capture_moment_screenshot`. |
| DELETE | `/moments/:moment_id`                          | Delete a single moment.                                                                                 |
| GET    | `/artifacts`                                   | List the user's artifacts.                                                                              |
| POST   | `/artifacts`                                   | Upload an artifact (multipart). Server writes the blob and schedules an async LLM summarizer.           |
| GET    | `/artifacts/:id`                               | Fetch an artifact's bytes.                                                                              |
| DELETE | `/artifacts/:id`                               | Delete an artifact. Implicitly detaches from any active meeting.                                        |
| POST   | `/meetings/:meeting_id/artifacts`              | Attach an artifact to the meeting (artifact id in JSON body). Emits `artifacts_changed`.                |
| DELETE | `/meetings/:meeting_id/artifacts/:artifact_id` | Detach an artifact from the meeting. Emits `artifacts_changed`.                                         |
