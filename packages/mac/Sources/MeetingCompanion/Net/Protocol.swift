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

    init(hostname: String, capabilities: [Capability]) {
        self.hostname = hostname
        self.capabilities = capabilities
    }
}

/// Begin a meeting on the server. `description` is the user's
/// free-text prompt entered in the overlay compose panel; `nil`
/// is a valid value (server treats it as an unlabeled meeting).
/// Metadata is intentionally omitted by the Mac start path so the
/// server preserves any reviewed chips from `extract_metadata` and
/// `set_metadata`.
struct StartMeetingIntent: Encodable {
    let type: String = "start_meeting"
    let description: String?
    /// Device id the server should bind as the audio source. The
    /// chosen device sees the resulting `audio_source_device_changed`
    /// event and starts streaming `/audio`. Mac fills this with its
    /// own `ownDevice.id` when initiating; PWA passes a user-picked
    /// id (or `nil` for a silent meeting).
    let audioSourceDeviceId: String?

    init(description: String? = nil, audioSourceDeviceId: String? = nil) {
        self.description = description
        self.audioSourceDeviceId = audioSourceDeviceId
    }

    enum CodingKeys: String, CodingKey {
        case type
        case description
        case audioSourceDeviceId = "audio_source_device_id"
    }
}

struct StopMeetingIntent: Encodable {
    let type: String = "stop_meeting"
}

struct ExtractMetadataIntent: Encodable {
    let type: String = "extract_metadata"
    let description: String
}

struct SetMetadataIntent: Encodable {
    let type: String = "set_metadata"
    let key: String
    let value: String?
}

struct SetModeIntent: Encodable {
    let type: String = "set_mode"
    let mode: String
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
