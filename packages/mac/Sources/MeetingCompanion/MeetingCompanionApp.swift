// MeetingCompanionApp.swift
// Entry point. SwiftUI App + menu-bar-accessory activation policy.
// See packages/mac/README.md for the Phase 2 sub-phase plan.

import SwiftUI

@main
struct MeetingCompanionApp: App {
    @State private var model = AppModel()

    init() {
        // Make this a proper menu-bar accessory app: no Dock icon, no
        // entry in the app switcher, lifetime tied to the menu bar item.
        // Equivalent to setting LSUIElement=true in a bundle Info.plist,
        // but also works for the non-bundled `swift run` development path.
        NSApp.setActivationPolicy(.accessory)
    }

    var body: some Scene {
        MenuBarExtra("Meeting Companion", systemImage: model.statusSystemImageName) {
            MenuBarContent(model: model)
        }
    }
}
