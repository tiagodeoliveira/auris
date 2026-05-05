// AppSettings.swift
// User-configurable settings persisted in UserDefaults. The Mac app's
// only persistent state today; OAuth tokens land here in Phase 3.

import Foundation
import Observation

@Observable
final class AppSettings {
    private enum Keys {
        static let serverURL = "meetingCompanion.serverURL"
        static let token = "meetingCompanion.token"
    }

    /// WebSocket server URL. `ws://` for local dev, `wss://` for prod.
    var serverURL: String {
        didSet { UserDefaults.standard.set(serverURL, forKey: Keys.serverURL) }
    }

    /// Shared-secret token expected by the server's `?token=` query.
    /// Replaced by a JWT issued via OAuth in Phase 3.
    var token: String {
        didSet { UserDefaults.standard.set(token, forKey: Keys.token) }
    }

    init() {
        let defaults = UserDefaults.standard
        self.serverURL = defaults.string(forKey: Keys.serverURL) ?? "ws://localhost:7331"
        self.token = defaults.string(forKey: Keys.token) ?? ""
    }

    /// True when both fields look usable enough to attempt a connection.
    var isConfigured: Bool {
        !serverURL.isEmpty && !token.isEmpty
    }
}
