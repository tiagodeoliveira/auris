// MeetingCompanionApp.swift
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
struct MeetingCompanionApp: App {
    @NSApplicationDelegateAdaptor private var appDelegate: AppDelegate
    @State private var model = AppModel()

    var body: some Scene {
        MenuBarExtra("Meeting Companion", systemImage: model.statusSystemImageName) {
            MenuBarContent(model: model)
        }

        // Settings window — summoned from the menu via openWindow(id:).
        // Single instance; opening when already open just brings it
        // forward.
        Window("Settings", id: "settings") {
            SettingsView(model: model)
        }
        .windowResizability(.contentSize)

        Window("Permissions", id: "permissions") {
            PermissionsView(model: model)
        }
        .windowResizability(.contentSize)

        // Meeting compose — opened by the menu's "Start meeting…"
        // entry; collects an optional description and submits to
        // start_meeting. Dismisses itself on submit/cancel.
        Window("Start meeting", id: "meeting-compose") {
            MeetingComposeView(model: model)
        }
        .windowResizability(.contentSize)

        // Meeting overlay — small floating window shown while a
        // meeting is active. Opened by the compose view's submit
        // (or by the menu's "Start meeting…" path indirectly);
        // dismisses itself when the meeting ends.
        Window("Meeting", id: "meeting-overlay") {
            MeetingOverlayView(model: model)
        }
        .windowResizability(.contentSize)
        .windowStyle(.hiddenTitleBar)
    }
}
