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

    /// True when the Mac is mid-test-meeting (audio capture running).
    var isTestMeetingActive: Bool {
        switch audioCapture.state {
        case .running, .starting: true
        default: false
        }
    }

    /// True when starting a test meeting is meaningful — connected to
    /// the server, permissions in hand, no capture currently running.
    var canStartTestMeeting: Bool {
        webSocket.state == .connected
            && permissionMonitor.allGranted
            && audioCapture.state == .stopped
    }

    /// Debug affordance: starts a meeting end-to-end from the Mac
    /// alone, no PWA / websocat needed. Sequence is critical:
    ///
    ///   1. Start audio capture (creates the AsyncStream).
    ///   2. Open the /audio WS streamer; first frame installs the
    ///      receiver into the server's RemoteAudioSource slot.
    ///   3. Wait for the streamer to confirm streaming state.
    ///   4. Send start_meeting on the control WS — server takes the
    ///      receiver out of the slot at this point.
    ///
    /// Reversing 3↔4 leaves the meeting in a "no audio source bound"
    /// state — the server's NotConnected error path. Phase 2g's
    /// compose-window flow uses the same sequence with description +
    /// extract tags layered on top.
    func toggleTestMeeting() async {
        if isTestMeetingActive {
            await stopTestMeeting()
        } else {
            await startTestMeeting()
        }
    }

    private func startTestMeeting() async {
        guard canStartTestMeeting else { return }

        do {
            try await audioCapture.start()
        } catch {
            return  // surfaced via audioCapture.state
        }
        guard let frames = audioCapture.output else { return }

        audioStreamer.start(
            serverURL: settings.serverURL,
            token: settings.token,
            frames: frames)

        // Wait up to 2 s for the streamer to confirm the /audio WS
        // is open and at least one frame has shipped — only then is
        // it safe to send start_meeting (the server's
        // RemoteAudioSource needs the install() side to have run).
        let deadline = Date().addingTimeInterval(2.0)
        while audioStreamer.state != .streaming, Date() < deadline {
            if case .error = audioStreamer.state {
                audioCapture.stop()
                return
            }
            try? await Task.sleep(for: .milliseconds(50))
        }
        guard audioStreamer.state == .streaming else {
            print("[AppModel] audio streamer did not reach .streaming within 2s; aborting")
            audioStreamer.stop()
            audioCapture.stop()
            return
        }

        do {
            try await webSocket.send(intent: StartMeetingIntent())
            print("[AppModel] start_meeting sent")
        } catch {
            print("[AppModel] start_meeting send failed: \(error)")
            audioStreamer.stop()
            audioCapture.stop()
        }
    }

    private func stopTestMeeting() async {
        // Send stop_meeting first so the server tears down its
        // pipeline cleanly before we cut the audio source.
        do {
            try await webSocket.send(intent: StopMeetingIntent())
            print("[AppModel] stop_meeting sent")
        } catch {
            print("[AppModel] stop_meeting send failed: \(error)")
        }
        audioStreamer.stop()
        audioCapture.stop()
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
