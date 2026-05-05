// AppModel.swift
// Single observable owner of app-wide state. Holds the user's
// settings + the live WebSocket connection. Views read derived
// state; menu actions call methods here.

import Foundation
import Observation

@MainActor
@Observable
final class AppModel {
    // `var` (not `let`) so SwiftUI `@Bindable` can project bindings
    // through to nested observable state (`$model.settings.serverURL`
    // in SettingsView, etc.). We never actually reassign these.
    var settings: AppSettings
    var webSocket: WebSocketClient

    init() {
        self.settings = AppSettings()
        self.webSocket = WebSocketClient()
    }

    // MARK: - Derived state for views

    /// SF Symbol name shown as the menu bar icon. Reflects the
    /// connection state at a glance.
    var statusSystemImageName: String {
        switch webSocket.state {
        case .disconnected: settings.isConfigured ? "circle" : "circle.dashed"
        case .connecting: "circle.dotted"
        case .connected: "circle.fill"
        case .error: "exclamationmark.circle.fill"
        }
    }

    /// Human-readable status line for the dropdown header.
    var statusLine: String {
        switch webSocket.state {
        case .disconnected:
            settings.isConfigured ? "Not connected" : "Not signed in"
        case .connecting: "Connecting…"
        case .connected: "Connected"
        case .error(let message): "Error: \(message)"
        }
    }

    /// True when the user can press "Connect" — i.e., settings exist
    /// and we're not already connecting/connected.
    var canConnect: Bool {
        settings.isConfigured && webSocket.state == .disconnected
    }

    /// True when the user can press "Disconnect".
    var canDisconnect: Bool {
        switch webSocket.state {
        case .connecting, .connected: true
        default: false
        }
    }

    // MARK: - Intent

    /// Open a WS connection using the current settings.
    func connect() {
        webSocket.connect(serverURL: settings.serverURL, token: settings.token)
    }

    /// Tear down the current WS connection.
    func disconnect() {
        webSocket.disconnect()
    }
}
