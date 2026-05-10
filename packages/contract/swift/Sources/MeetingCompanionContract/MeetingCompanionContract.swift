// Public surface for the contract Swift package.
//
// The protobuf-generated types live under `Generated/` and are
// re-exported transitively via `@_exported import`. Consumers add
// `import MeetingCompanionContract` and get the full v1 type
// surface.
//
// Wire format is binary protobuf over WebSocket binary frames:
//   let bytes = try intent.serializedData()           // → Data
//   let parsed = try Meeting_Companion_V1_Intent(serializedData: bytes)
//
// SwiftProtobuf prefixes types with the proto package, so
// `meeting_companion.v1.Intent` becomes `Meeting_Companion_V1_Intent`.
// Callers can typealias for shorter local names if they prefer.

@_exported import SwiftProtobuf

/// The wire-protocol version this package's generated types speak.
/// Mirrors `Snapshot.protocolVersion` for callers that need the
/// constant outside an event payload.
public let MeetingCompanionContractProtocolVersion: UInt32 = 1
