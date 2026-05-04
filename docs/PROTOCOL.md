# Wire Protocol

The PWA and the Rust server communicate over a single WebSocket. The
contract lives in two hand-maintained files kept in sync by review:

- Server: [`packages/server/src/contract.rs`](../packages/server/src/contract.rs)
- PWA: [`packages/pwa/src/contract.ts`](../packages/pwa/src/contract.ts)

See [ADR-0004](adr/0004-websocket-protocol.md) for the rationale.

## Connection

```
ws://<server-host>:7331/?token=<MEETING_COMPANION_TOKEN>
```

`token` is a shared secret. The server rejects connections with a
missing or wrong token immediately after the WebSocket upgrade.

## Versioning

A `protocol_version: u32` constant lives in both contract files (today:
`1`). On connection the server emits a `Snapshot` carrying the version
the server is built against. The PWA refuses to operate when this
doesn't match its compile-time expectation, surfacing a non-dismissable
error overlay.

Bump the constant when changing intent or event semantics in a way that
isn't backwards-compatible. Adding optional fields to existing variants
does not require a bump.

## Snapshot

The first message from the server on every connection. Contains the
full state needed to render the PWA without prior context.

```jsonc
{
  "type": "snapshot",
  "protocol_version": 1,
  "meeting_state": "idle" | "active" | "paused",
  "available_modes": [{ "id": "...", "label": "...", "update_strategy": "append" | "replace" }],
  "mode": "<current mode id>",
  "display_tag": "<optional, mode-specific badge>",
  "metadata": { "<key>": "<value>", ... },
  "items": [{ "id": "...", "text": "...", "t": <ms>, "meta": { ... } }],
  "status": { "listening": <bool>, "paused": <bool>, "error": "<optional>" },
  "prior_context": {
    "preferences": <int>, "facts": <int>, "episodes": <int>, "project_memories": <int>
  }
}
```

`prior_context` is omitted when no mnemo recall has populated it for
the current session.

## Intents (PWA → Server)

Every intent is a JSON object with a `type` discriminator. Unknown
types produce an `error` event with code `unknown_intent`; well-known
types with malformed payloads produce code `bad_payload`.

| Type               | Fields                                                     | Effect                                                                                                                                                                                                                   |
| ------------------ | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `start_meeting`    | `description?: string`, `metadata?: Record<string,string>` | Idle → Active. If `description` is present and `metadata` is absent or empty, server fires LLM extraction in background. If `metadata` is present, server replaces; if absent, server preserves existing state.metadata. |
| `stop_meeting`     | none                                                       | Active/Paused → Idle. Clears items, metadata, recalled context, in-flight extraction.                                                                                                                                    |
| `pause`            | none                                                       | Active → Paused. Audio + STT continue but summarizers idle.                                                                                                                                                              |
| `resume`           | none                                                       | Paused → Active.                                                                                                                                                                                                         |
| `set_mode`         | `mode: string`                                             | Switch the current mode. PWA tab clicks.                                                                                                                                                                                 |
| `set_metadata`     | `key: string`, `value: string \| null`                     | Insert/replace key. `null` deletes. Always emits `metadata_changed`.                                                                                                                                                     |
| `extract_metadata` | `description: string`                                      | Trigger LLM extraction in idle without changing state. Result merges with existing metadata via "manual wins"; new keys are added, conflicts keep the user's value. See [ADR-0010](adr/0010-extract-metadata-flow.md).   |
| `mark_moment`      | `t: number`, `note?: string`                               | (Phase-1+) Marker timestamp during active meeting.                                                                                                                                                                       |
| `expand_item`      | `item_id: string`                                          | Request the server to fill an item's `detail` field; server emits an `items_update` for the current mode with the enriched item.                                                                                         |

## Events (Server → PWA)

| Type                      | Fields                                                        | Notes                                                                                                              |
| ------------------------- | ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `snapshot`                | (see above)                                                   | First message after connect.                                                                                       |
| `meeting_state_changed`   | `meeting_state: "idle" \| "active" \| "paused"`               | After every lifecycle transition.                                                                                  |
| `available_modes_changed` | `available_modes: ModeOption[]`                               | Catalog change (today: never; reserved).                                                                           |
| `mode_changed`            | `mode: string`, `display_tag?: string`, `items: Item[]`       | After `set_mode`.                                                                                                  |
| `display_tag_changed`     | `tag?: string`                                                | Mode-specific small status.                                                                                        |
| `metadata_changed`        | `metadata: Record<string,string>`                             | Full metadata after any change.                                                                                    |
| `prior_context_changed`   | `summary: { preferences, facts, episodes, project_memories }` | After mnemo recall completes. All-zeros means cleared.                                                             |
| `items_update`            | `mode: string`, `items: Item[]`                               | Per-mode item list. Update strategy depends on the mode (`append` or `replace`).                                   |
| `transcript_interim`      | `text: string`                                                | Live in-flight transcript chunk between sentence boundaries. PWA renders as the dim "live row" in transcript mode. |
| `status`                  | `status: { listening, paused, error? }`                       | Heartbeat-driven; also used for transient extraction errors.                                                       |
| `error`                   | `code: string`, `message: string`, `intent_ref?: string`      | Per-intent rejection. Codes: `bad_json`, `unknown_intent`, `bad_payload`, `unknown_mode`, `unknown_item`.          |

## Modes

The mode catalog is server-defined and currently fixed at compile time:

| ID               | Label          | Update strategy | Source                                                    |
| ---------------- | -------------- | --------------- | --------------------------------------------------------- |
| `transcript`     | Transcript     | `append`        | Soniox sentence-flush via STT adapter                     |
| `highlights`     | Highlights     | `replace`       | LLM summarizer (~20 s heartbeat)                          |
| `actions`        | Actions        | `append`        | LLM summarizer (~15 s heartbeat) with text-equality dedup |
| `open_questions` | Open Questions | `append`        | LLM summarizer (~15 s heartbeat) with text-equality dedup |

`transcript` is the default. See [ADR-0007](adr/0007-summarizer-architecture.md)
for the per-mode design.

## Item shape

```jsonc
{
  "id": "<server-generated, mode-prefixed>",
  "text": "<surface text shown in the PWA list>",
  "detail": "<optional long-form text, populated lazily on expand_item>",
  "t": <milliseconds since meeting start>,
  "meta": { /* mode-specific extras (e.g. owner, due, importance, kind) */ }
}
```

Item IDs are stable for the meeting's lifetime; the PWA can use them
for `expand_item` requests and for client-side keying.

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
The PWA surfaces it as a toast with the code prefix.

## Heartbeat

The server emits a `status` event every ~3 s when the meeting is
active, carrying the live `listening` / `paused` flags and any
transient error string. Used by the PWA to detect a hung connection
and trigger reconnection.
