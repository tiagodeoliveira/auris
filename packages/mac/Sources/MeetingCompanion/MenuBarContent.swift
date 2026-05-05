// MenuBarContent.swift
// The dropdown shown when the menu bar icon is clicked.

import SwiftUI

struct MenuBarContent: View {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        // Header
        Text("Meeting Companion")
            .font(.headline)
        Text(model.statusLine)
            .foregroundStyle(.secondary)
        if let device = model.ownDevice {
            Text("Device id: \(device.id.prefix(8))…")
                .foregroundStyle(.secondary)
                .font(.caption)
        } else if let preview = model.webSocket.lastMessagePreview, !preview.isEmpty {
            Text("Last frame: \(preview)")
                .foregroundStyle(.secondary)
                .font(.caption)
        }

        Divider()

        // Connect / disconnect — drives the WebSocket
        if model.canConnect {
            Button("Connect") { model.connect() }
        }
        if model.canDisconnect {
            Button("Disconnect") { model.disconnect() }
        }
        if !model.settings.isConfigured {
            Button("Open Settings to sign in…") {
                openSettings()
            }
        }

        Divider()

        // Meeting lifecycle — wired in Phase 2g (compose) and 2f (control)
        Button("Start meeting…") {
            // TODO Phase 2g: open compose window, then send start_meeting
        }
        .disabled(true)

        Button("Stop meeting") {
            // TODO Phase 2g: send stop_meeting intent
        }
        .disabled(true)

        // Browse — wired in Phase 2h (depends on Phase 4 REST API)
        Button("Meetings…") {
            // TODO Phase 2h: open native meetings window (master/detail)
        }
        .disabled(true)

        Divider()

        // Identity — wired in Phase 3 (replaces the token-based Settings)
        Button("Sign in with Google…") {
            // TODO Phase 3: OAuth via browser → custom URL scheme handoff
        }
        .disabled(true)

        Button("Settings…") {
            openSettings()
        }

        Button(permissionsMenuLabel) {
            openWindow(id: "permissions")
            NSApp.activate(ignoringOtherApps: true)
        }

        Divider()

        // Debug: starts/stops a meeting end-to-end from the Mac
        // alone. Sequences capture + /audio + start_meeting in the
        // right order. Phase 2g replaces this with a proper compose
        // window (description input + Extract Tags + Start).
        Button(testMeetingMenuLabel) {
            Task { await model.toggleTestMeeting() }
        }
        .disabled(!model.isTestMeetingActive && !model.canStartTestMeeting)

        Divider()

        Button("Quit Meeting Companion") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }

    /// Reflects the test-meeting state: audio capture + streamer +
    /// the server-side meeting (kicked via start_meeting after the
    /// audio path is up).
    private var testMeetingMenuLabel: String {
        switch model.audioCapture.state {
        case .stopped:
            return "Start test meeting (debug)"
        case .error(let msg):
            return "Audio error · \(msg.prefix(40))"
        case .starting:
            return "Starting…"
        case .running:
            let captured = model.audioCapture.frameCount
            let streamed = model.audioStreamer.framesSent
            return "Stop test meeting (\(streamed)/\(captured) frames sent)"
        }
    }

    private func openSettings() {
        openWindow(id: "settings")
        NSApp.activate(ignoringOtherApps: true)
    }

    /// Label nudges the user when something's missing, without
    /// shouting. "Permissions…" stays neutral when everything's
    /// granted; gains a "•" prefix when not.
    private var permissionsMenuLabel: String {
        model.permissionMonitor.allGranted ? "Permissions…" : "• Permissions…"
    }
}
