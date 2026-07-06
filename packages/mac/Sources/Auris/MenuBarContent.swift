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
    @ObservedObject var updater: UpdaterController
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        statusHeader
            // Fallback for `AppModel.showOverlayWindow()` when no
            // matching NSWindow exists yet — e.g., on the very first
            // remote-initiated meeting after launch, before the user
            // has opened the overlay through any other path. The
            // notification round-trips through here to call
            // `openWindow(id:)` from a context that has the
            // environment action.
            .onReceive(
                NotificationCenter.default.publisher(for: .aurisRequestShowOverlay)
            ) { _ in
                openWindow(id: "meeting-overlay")
                NSApp.activate(ignoringOtherApps: true)
            }

        Divider()

        // Meeting actions — the primary reason to open this menu.
        // `hasActiveMeeting` (not `isMeetingActive`) is the right gate:
        // when a meeting is driven by another client (phone, PWA), the
        // Mac is a control surface — Show/Hide/Stop are still the
        // useful affordances, not a disabled Start button.
        if !model.hasActiveMeeting {
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

        // Single entry into the Auris window. The window remembers
        // the last-opened tab (Account / Meetings / Artifacts /
        // Quick Asks / Permissions) via `model.selectedSettingsTab`,
        // so deep-linking from the menu isn't needed — the user
        // lands wherever they left off. Icon flips to a warning
        // shield when any permission is missing so the menu still
        // surfaces that state without a dedicated row.
        Button {
            openSettings()
        } label: {
            Label(
                "Open Auris…",
                systemImage: model.permissionMonitor.allGranted
                    ? "gearshape"
                    : "exclamationmark.shield"
            )
        }

        Divider()

        Button {
            updater.checkForUpdates()
        } label: {
            Label("Check for updates…", systemImage: "arrow.triangle.2.circlepath")
        }
        .disabled(!updater.canCheckForUpdates)

        Button(role: .destructive) {
            NSApplication.shared.terminate(nil)
        } label: {
            Label("Quit Auris", systemImage: "power")
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
            model.hasActiveMeeting ? "In a meeting" : "Connected"
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
