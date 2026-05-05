// AppModel.swift
// Single observable owner of app-wide state. Everything in the app
// reads from here; phases add fields as features land.

import Foundation
import Observation

@Observable
final class AppModel {
    enum ConnectionState: Equatable {
        case signedOut
        case connecting
        case connected
        case error(String)
    }

    /// Server we connect to. Configurable via the (future) Settings
    /// window. Defaults to local dev.
    var serverURL: String = "ws://localhost:7331"

    /// Connection / auth state. Drives the menu bar icon and the
    /// status line in the dropdown.
    var connectionState: ConnectionState = .signedOut

    /// SF Symbol name shown as the menu bar icon. Reflects the
    /// connection state at a glance.
    var statusSystemImageName: String {
        switch connectionState {
        case .signedOut: "circle"
        case .connecting: "circle.dotted"
        case .connected: "circle.fill"
        case .error: "exclamationmark.circle.fill"
        }
    }

    /// Human-readable status line for the dropdown header.
    var statusLine: String {
        switch connectionState {
        case .signedOut: "Not signed in"
        case .connecting: "Connecting…"
        case .connected: "Connected"
        case .error(let message): "Error: \(message)"
        }
    }
}
