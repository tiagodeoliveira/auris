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

/// One row inside a mode's items list. `meta` is `serde_json::Value`
/// server-side; we don't need to introspect it yet (speaker tagging
/// is the only known consumer; deferred), so it stays out of this
/// struct entirely. Adding it later is a non-breaking decode change.
struct Item: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let text: String
    let detail: String?
    let t: UInt64
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

    init(description: String? = nil) {
        self.description = description
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

// MARK: - Events (Server → Mac)

/// Decoded form of incoming frames. Only the events the Mac currently
/// cares about are typed. Everything else falls through as
/// `.unknown(type)` so we don't break on event types we haven't
/// modeled yet (e.g., items_update).
enum TypedServerEvent: Sendable {
    case snapshot(SnapshotPayload)
    case meetingStateChanged(String)
    case deviceRegistered(Device)
    case devicesChanged([Device])
    case audioSourceDeviceChanged(String?)
    case metadataChanged([String: String])
    /// Live, in-flight transcript preview from the STT provider.
    /// Replaces the previous interim text wholesale on each event.
    case transcriptInterim(String)
    /// Finalised utterance from the STT provider — same content
    /// also flows as a transcript-mode item via `itemsUpdate`, so
    /// the overlay no longer needs this directly. Kept decoded so
    /// future consumers (e.g. system-level captions) don't have to
    /// re-add the variant.
    case transcriptCommitted(String)
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
        struct Wrap: Decodable { let meeting_state: String }
        let w = try decoder.decode(Wrap.self, from: data)
        return .meetingStateChanged(w.meeting_state)
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
    case "metadata_changed":
        struct Wrap: Decodable { let metadata: [String: String] }
        let w = try decoder.decode(Wrap.self, from: data)
        return .metadataChanged(w.metadata)
    case "transcript_interim":
        struct Wrap: Decodable { let text: String }
        let w = try decoder.decode(Wrap.self, from: data)
        return .transcriptInterim(w.text)
    case "transcript_committed":
        struct Wrap: Decodable { let text: String }
        let w = try decoder.decode(Wrap.self, from: data)
        return .transcriptCommitted(w.text)
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
