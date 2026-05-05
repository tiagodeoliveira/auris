// AppModel.swift
// Single observable owner of app-wide state. Holds the user's
// settings + the live WebSocket connection + the current registered
// device identity. Views read derived state; menu actions call
// methods here.

import AppKit
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
    var permissionMonitor: PermissionMonitor
    var audioCapture: AudioCapture
    var audioStreamer: AudioStreamer

    /// This Mac's identity in the server's device registry. Set when
    /// the server replies with `device_registered`; cleared on
    /// disconnect. The full device list (including this one) is
    /// `availableDevices`.
    private(set) var ownDevice: Device?

    /// Snapshot of all registered devices, including this one. Updated
    /// from `snapshot.devices` and per-event `devices_changed`.
    private(set) var availableDevices: [Device] = []

    /// The capabilities this Mac advertises. Frozen at app start; will
    /// reflect granted permissions once 2d (permissions onboarding) lands.
    private let advertisedCapabilities: [Capability] = [
        .audioCapture,
        .screenCapture,
        .controlSurface,
        .systemAudio,
    ]

    init() {
        self.settings = AppSettings()
        self.webSocket = WebSocketClient()
        self.permissionMonitor = PermissionMonitor()
        self.audioCapture = AudioCapture()
        self.audioStreamer = AudioStreamer()
        self.webSocket.onMessage = { [weak self] event in
            self?.handle(event: event)
        }
        // Re-check permissions whenever the app comes back to the
        // foreground (the user may have toggled state in System
        // Settings). Tied to the model's lifetime via [weak self].
        Task { [weak self] in
            for await _ in NotificationCenter.default.notifications(
                named: NSApplication.didBecomeActiveNotification)
            {
                self?.permissionMonitor.refresh()
            }
        }
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
        case .connected:
            if let d = ownDevice {
                "Connected · registered as \(d.hostname)"
            } else {
                "Connected · registering…"
            }
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

    /// Open a WS connection using the current settings, then send a
    /// `register_device` intent so the server knows this Mac is
    /// available as an audio source.
    func connect() {
        webSocket.connect(serverURL: settings.serverURL, token: settings.token)

        let intent = RegisterDeviceIntent(
            hostname: Self.hostname(),
            capabilities: advertisedCapabilities
        )
        // URLSession buffers the send until the WS handshake
        // completes, so it's safe to fire-and-forget here.
        Task { [weak webSocket] in
            try? await webSocket?.send(intent: intent)
        }
    }

    /// Tear down the current WS connection. Server marks our device
    /// offline as a side effect of the close.
    func disconnect() {
        webSocket.disconnect()
        ownDevice = nil
        availableDevices = []
    }

    /// Phase 2e/2f₂ debug affordance: start audio capture AND stream
    /// the resulting frames to the server's /audio endpoint. The
    /// Phase 2g compose-meeting flow will replace this with proper
    /// meeting-bound start/stop.
    func toggleAudioCapture() async {
        switch audioCapture.state {
        case .stopped, .error:
            do {
                try await audioCapture.start()
                if let frames = audioCapture.output {
                    audioStreamer.start(
                        serverURL: settings.serverURL,
                        token: settings.token,
                        frames: frames)
                }
            } catch {
                // surfaced via audioCapture.state
                _ = error
            }
        case .running, .starting:
            audioStreamer.stop()
            audioCapture.stop()
        }
    }

    // MARK: - Event handling

    /// Apply a decoded server event to local state. Called from
    /// `WebSocketClient.onMessage` (set up in init).
    private func handle(event: TypedServerEvent) {
        switch event {
        case .snapshot(let payload):
            availableDevices = payload.devices
        case .deviceRegistered(let device):
            ownDevice = device
            // Keep availableDevices in sync in case the broadcast
            // hasn't landed yet.
            if !availableDevices.contains(where: { $0.id == device.id }) {
                availableDevices.append(device)
            }
        case .devicesChanged(let devices):
            availableDevices = devices
            // If our own device was removed (e.g., server-side
            // unregister), clear the local mirror.
            if let ours = ownDevice, !devices.contains(where: { $0.id == ours.id }) {
                ownDevice = nil
            }
        case .audioSourceDeviceChanged:
            // Phase 2g+ will react to the bound source; not needed yet.
            break
        case .unknown:
            // Unknown events fall through silently; we'll add cases
            // as we light up more flows.
            break
        }
    }

    // MARK: - Helpers

    /// Best-effort hostname for device registration. Falls back to a
    /// stable but human-readable label if Host info is unavailable.
    private static func hostname() -> String {
        if let host = Host.current().localizedName, !host.isEmpty {
            return host
        }
        if let name = ProcessInfo.processInfo.hostName.split(separator: ".").first {
            return String(name)
        }
        return "Mac"
    }
}
