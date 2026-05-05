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
/// free-text prompt entered in the compose window; `nil` is a
/// valid value (server treats it as an unlabeled meeting).
/// Phase 2g-2 will add the metadata field once Extract Tags lands.
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

// MARK: - Events (Server → Mac)

/// Decoded form of incoming frames. Only the events the Mac currently
/// cares about are typed. Everything else falls through as
/// `.unknown(type)` so we don't break on event types we haven't
/// modeled yet (e.g., items_update).
enum TypedServerEvent: Sendable {
    case snapshot(SnapshotPayload)
    case deviceRegistered(Device)
    case devicesChanged([Device])
    case audioSourceDeviceChanged(String?)
    /// Live, in-flight transcript preview from the STT provider.
    /// Replaces the previous interim text wholesale on each event.
    case transcriptInterim(String)
    /// Finalised utterance from the STT provider — append to the
    /// rolling transcript view. Emitted by the server when its
    /// internal buffer is flushed (punctuation / length cap /
    /// silence). Distinct from `transcriptInterim` which is the
    /// still-mutable preview.
    case transcriptCommitted(String)
    case unknown(type: String)
}

/// Minimal snapshot decode — only the fields the Mac uses today.
/// Phase 2g+ adds more as the meeting flow lights up.
struct SnapshotPayload: Decodable, Sendable {
    let protocolVersion: Int
    let devices: [Device]
    let audioSourceDeviceId: String?

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
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
    case "transcript_interim":
        struct Wrap: Decodable { let text: String }
        let w = try decoder.decode(Wrap.self, from: data)
        return .transcriptInterim(w.text)
    case "transcript_committed":
        struct Wrap: Decodable { let text: String }
        let w = try decoder.decode(Wrap.self, from: data)
        return .transcriptCommitted(w.text)
    default:
        return .unknown(type: envelope.type)
    }
}
