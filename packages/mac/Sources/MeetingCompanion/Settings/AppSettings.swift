// AppSettings.swift
// User-configurable settings persisted in UserDefaults. The Mac app's
// only persistent state today; OAuth tokens land here in Phase 3.

import Foundation
import Observation

@Observable
final class AppSettings {
    /// WebSocket server URL. Hardcoded for now — future builds will
    /// substitute this at compile time (e.g., dev vs prod targets each
    /// shipping their own constant baked into the binary). The user
    /// shouldn't be configuring this in-app.
    static let serverURLDefault = "ws://localhost:7331"

    /// Read-only convenience so call sites stay readable
    /// (`settings.serverURL`) and we can swap to a per-build value
    /// without touching every caller.
    var serverURL: String { Self.serverURLDefault }

    init() {}
}
