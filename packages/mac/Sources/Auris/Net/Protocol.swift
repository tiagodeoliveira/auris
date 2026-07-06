// Protocol.swift
// Server-contract types used over the WebSocket. Mirrors the wire
// shapes defined in `packages/server/src/contract.rs` and
// `packages/pwa/src/contract.ts`. Hand-maintained for now (3-language
// codegen is a Phase-2-end decision).
//
// Conventions:
// - Snake_case enum raw values to match the wire format.
// - Decoders use a JSONDecoder with default key strategy (no
//   conversion); we map struct properties to wire keys explicitly
//   when they differ.

import Foundation

// MARK: - Capability

/// What a device can do for a meeting. Drives the audio-source picker
/// (filters by `audioCapture`) and the screenshot trigger (Phase 5,
/// filters by `screenCapture`).
enum Capability: String, Codable, Sendable {
    case audioCapture = "audio_capture"
    case screenCapture = "screen_capture"
    case controlSurface = "control_surface"
    /// Sub-capability of `audioCapture` indicating this device can
    /// grab system-wide audio output (not just a microphone).
    case systemAudio = "system_audio"
}

// MARK: - Device

/// A registered device. The server assigns the `id` on registration
/// and broadcasts `DevicesChanged` when the registry changes.
struct Device: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let hostname: String
    let capabilities: [Capability]
    let online: Bool
}

// MARK: - Modes & Items

/// How a mode's item list should be merged when an `items_update`
/// arrives. Mirrors `crate::contract::UpdateStrategy`.
///
/// - `replace`: payload IS the new full list (capped at 10 server-side).
/// - `append`: payload is one or more items to append onto the buffer.
enum UpdateStrategy: String, Codable, Sendable {
    case replace, append
}

/// One available mode the user can select (transcript, highlights,
/// actions, open_questions). Static set, snapshot-delivered.
struct ModeOption: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let label: String
    let updateStrategy: UpdateStrategy

    enum CodingKeys: String, CodingKey {
        case id
        case label
        case updateStrategy = "update_strategy"
    }
}

/// Open-ended metadata bag attached to an item. Server-side this is
/// `serde_json::Value`, so future fields can land without breaking
/// the decode. We only pluck the keys the Mac actually renders; the
/// rest of the JSON is silently ignored.
struct ItemMeta: Codable, Sendable, Equatable {
    /// Speaker label from the STT provider (currently Soniox). Only
    /// present on transcript-mode items. Used as a small chip-style
    /// prefix in the overlay so multi-speaker meetings stay
    /// readable.
    let speaker: String?
    /// Role on chat-mode items: `"user"` for the question bubble,
    /// `"assistant"` for the agent's reply. Drives bubble
    /// alignment + tint in the overlay.
    let role: String?
    /// Importance label on highlights-mode items ("high" / "medium" /
    /// "low"). Rendered as the meta chip beneath the item text.
    let importance: String?
    /// Owner on actions-mode items — whoever was named or
    /// self-referenced as responsible.
    let owner: String?
    /// Stated deadline on actions-mode items. Free-form ("Friday",
    /// "next week", "by EOM").
    let due: String?
    /// Kind label on open_questions-mode items ("factual",
    /// "decision", "follow-up", etc.). Server may emit any short
    /// string; the overlay just uppercases it.
    let kind: String?
    /// Optional context blurb on open_questions-mode items —
    /// extra detail rendered in the meta chip after KIND.
    let context: String?
    /// Sub-type tag on assist-mode items: `"definition"` / `"question"`
    /// / `"memory"` / `"coach"`. Decoded from the server's
    /// `meta.type` field; renamed to `assistType` here to avoid
    /// colliding with Swift's `type(of:)`. Drives the emoji chip
    /// prefixed onto the assist item's body text in the overlay.
    let assistType: String?
    /// Streaming flag on chat-mode assistant bubbles. True while the
    /// server's agent is emitting deltas for this bubble; flipped to
    /// false on the terminal ItemUpdated. The overlay renders typing
    /// dots while empty + streaming, and locks the chat input across
    /// the whole streaming phase so the user can't fire a second
    /// chat mid-stream.
    let streaming: Bool?
    /// Attachment ids on chat-mode `user` bubbles — present when the
    /// message rode one or more screenshots. The overlay renders a
    /// small photo glyph (+ count when >1); keeping the ids (not just
    /// a count) lets a future build fetch/preview the actual images.
    let attachmentIds: [String]?

    private enum CodingKeys: String, CodingKey {
        case speaker, role, importance, owner, due, kind, context, streaming
        case assistType = "type"
        case attachmentIds = "attachment_ids"
    }
}

/// One row inside a mode's items list. `meta` is decoded
/// loosely — see `ItemMeta`.
struct Item: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let text: String
    let detail: String?
    let t: UInt64
    let meta: ItemMeta?
}

// MARK: - Intents (Mac → Server)

/// Encoded with `type: "register_device"` to match the snake_case
/// discriminator the server expects.
struct RegisterDeviceIntent: Encodable {
    let type: String = "register_device"
    let hostname: String
    let capabilities: [Capability]
    /// Stable device id persisted across reconnects (UserDefaults).
    /// The server reuses it instead of minting a fresh UUID, so this
    /// Mac keeps its identity — and its audio-source binding — when
    /// the socket drops and reconnects. Without it, every reconnect
    /// looked like a brand-new device and silently unbound the mic.
    let deviceId: String

    init(hostname: String, capabilities: [Capability], deviceId: String) {
        self.hostname = hostname
        self.capabilities = capabilities
        self.deviceId = deviceId
    }

    enum CodingKeys: String, CodingKey {
        case type
        case hostname
        case capabilities
        case deviceId = "device_id"
    }
}

/// Begin a meeting on the server. `description` is the user's
/// free-text prompt entered in the overlay compose panel; `nil`
/// is a valid value (server treats it as an unlabeled meeting).
/// Metadata is intentionally omitted by the Mac start path so the
/// server preserves any chips already set via `set_metadata`, and so
/// it can auto-extract from the description when no chips exist.
struct StartMeetingIntent: Encodable {
    let type: String = "start_meeting"
    let description: String?
    /// Device id the server should bind as the audio source. The
    /// chosen device sees the resulting `audio_source_device_changed`
    /// event and starts streaming `/audio`. Mac fills this with its
    /// own `ownDevice.id` when initiating; PWA passes a user-picked
    /// id (or `nil` for a silent meeting).
    let audioSourceDeviceId: String?
    /// Per-meeting assist surface sensitivity. `nil` falls back to
    /// the server's default (Moderate), matching the historical
    /// behavior pre-feature.
    let assistSensitivity: AssistSensitivity?

    init(
        description: String? = nil,
        audioSourceDeviceId: String? = nil,
        assistSensitivity: AssistSensitivity? = nil
    ) {
        self.description = description
        self.audioSourceDeviceId = audioSourceDeviceId
        self.assistSensitivity = assistSensitivity
    }

    enum CodingKeys: String, CodingKey {
        case type
        case description
        case audioSourceDeviceId = "audio_source_device_id"
        case assistSensitivity = "assist_sensitivity"
    }
}

struct StopMeetingIntent: Encodable {
    let type: String = "stop_meeting"
}

/// Mid-meeting assist sensitivity flip. Server validates state
/// (no-op when idle); broadcasts `assist_sensitivity_changed` so
/// every connected surface stays in sync.
struct SetAssistSensitivityIntent: Encodable {
    let type: String = "set_assist_sensitivity"
    let value: AssistSensitivity
}

/// Three-step assist-surface sensitivity. Mirrors the server's
/// `protocol::AssistSensitivity` (snake_case wire). `Codable` so
/// it ships in `StartMeetingIntent` + `SetAssistSensitivityIntent`
/// AND decodes from the snapshot / `assist_sensitivity_changed`
/// event surface.
enum AssistSensitivity: String, Codable, Sendable, CaseIterable {
    case aggressive
    case moderate
    case minimal

    /// Display label for the SwiftUI segmented picker. Title-cased
    /// for the macOS conventions (the wire stays lowercase).
    var displayName: String {
        switch self {
        case .aggressive: return "Aggressive"
        case .moderate: return "Moderate"
        case .minimal: return "Minimal"
        }
    }
}

struct SetMetadataIntent: Encodable {
    let type: String = "set_metadata"
    let key: String
    let value: String?
}

// SetModeIntent removed: `currentMode` is now per-surface UI
// state; Mac doesn't send `set_mode` to the server. The server
// keeps the intent variant as a logged no-op for legacy clients
// during the rollout window.

/// Upsert one of the user's saved "quick asks" — a labeled prompt
/// they can fire as chat with one tap. `id` is client-minted UUID
/// (reused on edits). The server stores label / text / position
/// per-user and broadcasts the new library on `quick_asks` mode.
struct UpsertQuickAskIntent: Encodable {
    let type: String = "upsert_quick_ask"
    let id: String
    let label: String
    let text: String
    let position: Int32
}

struct DeleteQuickAskIntent: Encodable {
    let type: String = "delete_quick_ask"
    let id: String
}

/// Bookmark a moment in the active meeting at offset `t` (ms since
/// meeting start). Server inserts the row; if a `screen_capture`-
/// capable device is bound as the audio source, server emits
/// `capture_moment_screenshot` to that device which then uploads
/// the PNG via REST. Optional free-text `note` rides along.
struct MarkMomentIntent: Encodable {
    let type: String = "mark_moment"
    let t: Int64
    let note: String?

    init(t: Int64, note: String? = nil) {
        self.t = t
        self.note = note
    }
}

/// User-typed question to the agent during an active meeting. The
/// server validates active/paused state, kicks the agent, and the
/// resulting Q+A pair lands in chat-mode `items_update` events.
///
/// `attachmentIds` (added 2026-05-12) carries chat-attachment ids
/// previously returned by `POST /meetings/:id/chat_attachments`.
/// Default `[]` matches today's text-only chats.
struct ChatIntent: Encodable {
    let type: String = "chat"
    let text: String
    let attachmentIds: [String]

    init(text: String, attachmentIds: [String] = []) {
        self.text = text
        self.attachmentIds = attachmentIds
    }

    enum CodingKeys: String, CodingKey {
        case type
        case text
        case attachmentIds = "attachment_ids"
    }
}

/// Ask the agent to expand on a specific item by id. The agent's
/// text reply lands as the item's `detail` via `Event::ItemUpdated`.
/// Server kicks the agent through the same channel chat uses.
struct ExpandItemIntent: Encodable {
    let type: String = "expand_item"
    let item_id: String
}

// MARK: - Events (Server → Mac)

/// Decoded form of incoming frames. Only the events the Mac currently
/// cares about are typed. Everything else falls through as
/// `.unknown(type)` so we don't break on event types we haven't
/// modeled yet (e.g., items_update).
enum TypedServerEvent: Sendable {
    case snapshot(SnapshotPayload)
    /// Meeting lifecycle transition. `meetingId` is `Some` when going
    /// to `active` / `paused`, `None` when going to `idle`. The Mac
    /// uses this to track the current meeting's id between snapshots.
    case meetingStateChanged(state: String, meetingId: String?)
    case deviceRegistered(Device)
    case devicesChanged([Device])
    case audioSourceDeviceChanged(String?)
    /// Server is asking *this* device (per-connection routing on the
    /// server side) to capture a screenshot for a moment that already
    /// exists in the DB. The matching Mac uploads the PNG via
    /// `POST /meetings/:id/moments/:moment_id/screenshot`.
    case captureMomentScreenshot(
        meetingId: String,
        momentId: String,
        tMs: Int64
    )
    case metadataChanged([String: String])
    /// Live, in-flight transcript preview from the STT provider.
    /// Replaces the previous interim text wholesale on each event.
    case transcriptInterim(String)
    /// User switched modes (or the server-side meeting started in a
    /// non-default mode). `items` is the full list for the new mode.
    case modeChanged(mode: String, displayTag: String?, items: [Item])
    /// New / replacement items for a mode. Merge per the mode's
    /// `UpdateStrategy` — `replace` payloads are the full list,
    /// `append` payloads are deltas.
    case itemsUpdate(mode: String, items: [Item])
    /// One item updated in-place (used today by `expand_item` to
    /// land the agent's expansion in `item.detail`). Decoders
    /// replace the matching item by id in their per-mode list.
    case itemUpdated(mode: String, item: Item)
    /// Server-authoritative list of artifact IDs attached to the
    /// user's active meeting. Carried whenever attach/detach happens
    /// on EITHER client so the Mac and PWA stay in sync without
    /// polling. The overlay's mid-meeting picker pre-checks rows
    /// against this set.
    case artifactsChanged(artifactIds: [String])
    /// Server-authoritative list of past-meeting IDs attached to the
    /// user's active meeting. Same shape/role as `artifactsChanged`
    /// — fired whenever a meeting attach/detach happens on either
    /// client, so all surfaces stay in sync.
    case attachedMeetingsChanged(meetingIds: [String])
    /// Active meeting's assist sensitivity changed (mid-meeting flip
    /// from any surface, or initial broadcast from `start_meeting`).
    /// Mac mirrors into `AppModel.assistSensitivity` so the picker
    /// reflects the canonical value.
    case assistSensitivityChanged(AssistSensitivity)
    case error(code: String, message: String)
    case unknown(type: String)
}

/// Initial state for a freshly-connected client. `items` is for the
/// current mode only; other modes hydrate lazily via `mode_changed`
/// or `items_update` as the server pushes them.
///
/// `meetingState` is critical on *reconnect*: if the server has
/// restarted, it boots into "idle" and the snapshot will say so —
/// the Mac uses that as the signal to tear down any locally-running
/// meeting that the server no longer knows about.
struct SnapshotPayload: Decodable, Sendable {
    let protocolVersion: Int
    let meetingState: String
    /// Server-assigned id of the active meeting. `nil` when idle.
    /// Used to link to history (`GET /meetings/<id>`) and to
    /// reconcile across reconnects (same id = same meeting).
    let meetingId: String?
    let availableModes: [ModeOption]
    let mode: String
    let displayTag: String?
    let metadata: [String: String]
    let items: [Item]
    let devices: [Device]
    let audioSourceDeviceId: String?
    /// IDs of past meetings attached to the active meeting. Snapshot
    /// itself ships an empty list today; the server fires a synthetic
    /// `AttachedMeetingsChanged` immediately after the snapshot when
    /// the active meeting has attachments. Optional decode for
    /// forward-compat with older server builds.
    let attachedMeetingIds: [String]?
    /// Active meeting's assist sensitivity. Optional decode for
    /// forward-compat with older server builds; absent / NULL
    /// rows decode to `nil` and the caller applies the default.
    let assistSensitivity: AssistSensitivity?

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case meetingState = "meeting_state"
        case meetingId = "meeting_id"
        case availableModes = "available_modes"
        case mode
        case displayTag = "display_tag"
        case metadata
        case items
        case devices
        case audioSourceDeviceId = "audio_source_device_id"
        case attachedMeetingIds = "attached_meeting_ids"
        case assistSensitivity = "assist_sensitivity"
    }
}

// MARK: - Decoding

/// Decode an incoming WS text frame into a `TypedServerEvent`.
/// Returns `nil` on malformed JSON; throws on a decode error within a
/// known type (so we surface contract drift as a real failure).
func decodeServerEvent(from text: String) throws -> TypedServerEvent? {
    guard let data = text.data(using: .utf8) else { return nil }
    let decoder = JSONDecoder()

    // Read the discriminator first.
    struct Envelope: Decodable {
        let type: String
    }
    let envelope: Envelope
    do {
        envelope = try decoder.decode(Envelope.self, from: data)
    } catch {
        return nil  // not a typed event
    }

    switch envelope.type {
    case "snapshot":
        // Snapshot is a wrapped envelope — fields live at the top level.
        let payload = try decoder.decode(SnapshotPayload.self, from: data)
        return .snapshot(payload)
    case "meeting_state_changed":
        struct Wrap: Decodable {
            let meeting_state: String
            let meeting_id: String?
        }
        let w = try decoder.decode(Wrap.self, from: data)
        return .meetingStateChanged(state: w.meeting_state, meetingId: w.meeting_id)
    case "device_registered":
        struct Wrap: Decodable { let device: Device }
        let w = try decoder.decode(Wrap.self, from: data)
        return .deviceRegistered(w.device)
    case "devices_changed":
        struct Wrap: Decodable { let devices: [Device] }
        let w = try decoder.decode(Wrap.self, from: data)
        return .devicesChanged(w.devices)
    case "audio_source_device_changed":
        struct Wrap: Decodable { let device_id: String? }
        let w = try decoder.decode(Wrap.self, from: data)
        return .audioSourceDeviceChanged(w.device_id)
    case "capture_moment_screenshot":
        struct Wrap: Decodable {
            let meeting_id: String
            let moment_id: String
            let t_ms: Int64
        }
        let w = try decoder.decode(Wrap.self, from: data)
        return .captureMomentScreenshot(
            meetingId: w.meeting_id,
            momentId: w.moment_id,
            tMs: w.t_ms
        )
    case "metadata_changed":
        struct Wrap: Decodable { let metadata: [String: String] }
        let w = try decoder.decode(Wrap.self, from: data)
        return .metadataChanged(w.metadata)
    case "transcript_interim":
        struct Wrap: Decodable { let text: String }
        let w = try decoder.decode(Wrap.self, from: data)
        return .transcriptInterim(w.text)
    case "mode_changed":
        struct Wrap: Decodable {
            let mode: String
            let display_tag: String?
            let items: [Item]
        }
        let w = try decoder.decode(Wrap.self, from: data)
        return .modeChanged(mode: w.mode, displayTag: w.display_tag, items: w.items)
    case "items_update":
        struct Wrap: Decodable {
            let mode: String
            let items: [Item]
        }
        let w = try decoder.decode(Wrap.self, from: data)
        return .itemsUpdate(mode: w.mode, items: w.items)
    case "item_updated":
        struct Wrap: Decodable {
            let mode: String
            let item: Item
        }
        let w = try decoder.decode(Wrap.self, from: data)
        return .itemUpdated(mode: w.mode, item: w.item)
    case "artifacts_changed":
        struct Wrap: Decodable { let artifact_ids: [String] }
        let w = try decoder.decode(Wrap.self, from: data)
        return .artifactsChanged(artifactIds: w.artifact_ids)
    case "attached_meetings_changed":
        struct Wrap: Decodable { let meeting_ids: [String] }
        let w = try decoder.decode(Wrap.self, from: data)
        return .attachedMeetingsChanged(meetingIds: w.meeting_ids)
    case "assist_sensitivity_changed":
        struct Wrap: Decodable { let value: AssistSensitivity }
        let w = try decoder.decode(Wrap.self, from: data)
        return .assistSensitivityChanged(w.value)
    case "error":
        struct Wrap: Decodable {
            let code: String
            let message: String
        }
        let w = try decoder.decode(Wrap.self, from: data)
        return .error(code: w.code, message: w.message)
    default:
        return .unknown(type: envelope.type)
    }
}
