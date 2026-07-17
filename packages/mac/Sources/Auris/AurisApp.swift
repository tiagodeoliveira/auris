// AurisApp.swift
// Entry point. SwiftUI App + menu-bar-accessory activation policy.
// See packages/mac/README.md for the Phase 2 sub-phase plan.

import AppKit
import SwiftUI

/// AppDelegate is the right place to call setActivationPolicy:
/// `NSApp` (the AppKit singleton) is guaranteed to be initialized by
/// the time `applicationDidFinishLaunching` fires. Calling
/// `NSApp.setActivationPolicy(.accessory)` from `App.init()` crashes
/// with an implicit-unwrap on `NSApp` because AppKit hasn't bootstrapped
/// when SwiftUI's App initializer runs.
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // Make this a proper menu-bar accessory app: no Dock icon, no
        // entry in the app switcher, lifetime tied to the menu bar item.
        // Equivalent to setting LSUIElement=true in a bundle Info.plist,
        // but also works for the non-bundled `swift run` development path.
        NSApp.setActivationPolicy(.accessory)
    }
}

@main
struct AurisApp: App {
    @NSApplicationDelegateAdaptor private var appDelegate: AppDelegate
    @State private var model = AppModel()

    /// Sparkle auto-update controller. Owned by the App struct for
    /// the process lifetime; constructing it kicks off the background
    /// check loop per the Info.plist's SUEnableAutomaticChecks +
    /// SUScheduledCheckInterval settings.
    @StateObject private var updaterController = UpdaterController()

    var body: some Scene {
        // Menu bar icon. When the server is reachable, show the
        // Auris ear-arcs mark (brand). When the connection is in any
        // other state (connecting / reconnecting / error / signed
        // out), fall back to the SF Symbol that telegraphs that
        // state so the user notices trouble at a glance instead of
        // seeing a perpetually-happy logo.
        MenuBarExtra {
            MenuBarContent(model: model, updater: updaterController)
        } label: {
            if case .connected = model.webSocket.state {
                // Image(nsImage:) of a pre-rasterized template image —
                // MenuBarExtra can't extract a usable alpha mask from
                // a SwiftUI View directly, so we feed it an NSImage
                // with isTemplate=true and let macOS handle tinting.
                Image(nsImage: AurisMark.menuBarTemplateImage)
                    .renderingMode(.template)
            } else {
                Image(systemName: model.statusSystemImageName)
            }
        }

        // Settings window — summoned from the menu via openWindow(id:).
        // Single instance; opening when already open just brings it
        // forward.
        // Settings now hosts both the server creds form and the
        // Meetings browser (master/detail). Content-min so the user
        // can drag the window taller when reading long transcripts.
        Window("Settings", id: "settings") {
            SettingsView(model: model)
        }
        .windowResizability(.contentMinSize)

        // Start-meeting popup — a normal titled window like Settings, but
        // screen-share-excluded (see StartMeetingView). Opened from the menu
        // via openWindow(id: "start-meeting"). Single instance.
        Window("Start Meeting", id: "start-meeting") {
            StartMeetingView(model: model)
        }
        .windowResizability(.contentMinSize)

        // Meeting overlay — the single floating meeting surface. It
        // opens in the starting state once a start is in flight, then
        // becomes the live transcript HUD. Compose lives in the
        // separate "start-meeting" window above.
        Window("Meeting", id: "meeting-overlay") {
            MeetingOverlayView(model: model)
        }
        // contentMinSize lets the user grow the window past the
        // content's intrinsic size on both axes; contentSize would
        // pin it. The overlay's view defines minWidth/minHeight per
        // overlay mode (compose / starting / live) — those still
        // apply as the floor.
        .windowResizability(.contentMinSize)
        .windowStyle(.hiddenTitleBar)
    }
}
