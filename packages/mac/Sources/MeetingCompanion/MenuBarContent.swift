// MenuBarContent.swift
// The dropdown shown when the menu bar icon is clicked.
//
// Layout follows the macOS convention: status header → primary
// meeting actions → connection toggle → configuration → quit. Items
// use `Label(text, systemImage:)` so the system renders them with
// inline SF Symbols, matching native Apple menus.

import SwiftUI

struct MenuBarContent: View {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        statusHeader

        Divider()

        // Meeting actions — the primary reason to open this menu.
        if !model.isMeetingActive {
            Button {
                openWindow(id: "meeting-overlay")
                NSApp.activate(ignoringOtherApps: true)
            } label: {
                Label("Start meeting…", systemImage: "record.circle")
            }
            .disabled(!model.canStartMeeting)
        } else {
            Button {
                if model.isOverlayVisible {
                    dismissWindow(id: "meeting-overlay")
                } else {
                    openWindow(id: "meeting-overlay")
                    NSApp.activate(ignoringOtherApps: true)
                }
            } label: {
                Label(
                    model.isOverlayVisible ? "Hide overlay" : "Show overlay",
                    systemImage: model.isOverlayVisible ? "eye.slash" : "eye"
                )
            }

            Button(role: .destructive) {
                Task { await model.stopMeeting() }
            } label: {
                Label("Stop meeting", systemImage: "stop.circle.fill")
            }
        }

        Button {
            model.selectedSettingsTab = .meetings
            openWindow(id: "settings")
            NSApp.activate(ignoringOtherApps: true)
        } label: {
            Label("Meetings…", systemImage: "list.bullet.rectangle")
        }
        .disabled(!model.auth0.isSignedIn)

        Button {
            model.selectedSettingsTab = .artifacts
            openWindow(id: "settings")
            NSApp.activate(ignoringOtherApps: true)
        } label: {
            Label("Artifacts…", systemImage: "doc.text")
        }
        .disabled(!model.auth0.isSignedIn)

        Divider()

        // Connection toggle. Only one of these is shown at a time, so
        // the menu height stays stable across state transitions.
        if model.canConnect {
            Button {
                model.connect()
            } label: {
                Label("Connect", systemImage: "bolt.horizontal")
            }
        }
        if model.canDisconnect {
            Button {
                model.disconnect()
            } label: {
                Label("Disconnect", systemImage: "bolt.horizontal.fill")
            }
        }

        Divider()

        Button {
            openSettings()
        } label: {
            Label("Settings…", systemImage: "gearshape")
        }

        Button {
            model.selectedSettingsTab = .permissions
            openWindow(id: "settings")
            NSApp.activate(ignoringOtherApps: true)
        } label: {
            Label(
                "Permissions",
                systemImage: model.permissionMonitor.allGranted
                    ? "lock.shield"
                    : "exclamationmark.shield"
            )
        }

        Divider()

        Button(role: .destructive) {
            NSApplication.shared.terminate(nil)
        } label: {
            Label("Quit Meeting Companion", systemImage: "power")
        }
        .keyboardShortcut("q")
    }

    /// Compact status header. The dot is concatenated into the same
    /// `Text` as the title (rather than placed in an HStack alongside)
    /// because macOS menu items render each child view as its own
    /// row — an HStack(Image + Text) gets flattened into two stacked
    /// rows. `Text + Text` concatenation stays inline.
    @ViewBuilder
    private var statusHeader: some View {
        Text(Image(systemName: statusDotIcon))
            .foregroundStyle(statusDotColor)
            + Text("  \(headerTitle)")
            .font(.headline)
        if let subtitle = headerSubtitle {
            Text(subtitle)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func openSettings() {
        openWindow(id: "settings")
        NSApp.activate(ignoringOtherApps: true)
    }

    // MARK: - Header derivations

    private var statusDotIcon: String {
        switch model.webSocket.state {
        case .connected: "circle.fill"
        case .connecting, .reconnecting: "circle.dotted"
        case .error: "exclamationmark.circle.fill"
        case .disconnected: "circle"
        }
    }

    private var statusDotColor: Color {
        switch model.webSocket.state {
        case .connected: .green
        case .connecting, .reconnecting: .yellow
        case .error: .red
        case .disconnected: .secondary
        }
    }

    private var headerTitle: String {
        switch model.webSocket.state {
        case .connected:
            model.isMeetingActive ? "In a meeting" : "Connected"
        case .connecting: "Connecting…"
        case .reconnecting: "Reconnecting…"
        case .disconnected:
            model.auth0.isSignedIn ? "Not connected" : "Not signed in"
        case .error: "Connection error"
        }
    }

    /// Second line is the hostname when connected (so the user can
    /// confirm which Mac they're on), the error message when failing,
    /// and a "Sign in via Settings" hint when unconfigured.
    private var headerSubtitle: String? {
        switch model.webSocket.state {
        case .connected:
            model.ownDevice?.hostname
        case .error(let message):
            message
        case .disconnected where !model.auth0.isSignedIn:
            "Open Settings to sign in"
        default:
            nil
        }
    }
}
