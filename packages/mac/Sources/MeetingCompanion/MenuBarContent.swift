// MenuBarContent.swift
// The dropdown shown when the menu bar icon is clicked. Most actions
// are stubbed out (disabled) at the scaffold stage and lit up by
// subsequent Phase 2 sub-phases. See packages/mac/README.md.

import SwiftUI

struct MenuBarContent: View {
    @Bindable var model: AppModel

    var body: some View {
        // Header
        Text("Meeting Companion")
            .font(.headline)
        Text(model.statusLine)
            .foregroundStyle(.secondary)

        Divider()

        // Meeting lifecycle — wired in Phase 2c (server connection)
        Button("Start meeting…") {
            // TODO Phase 2c: open compose window, then send start_meeting
        }
        .disabled(true)

        Button("Stop meeting") {
            // TODO Phase 2c: send stop_meeting intent
        }
        .disabled(true)

        // Browse — wired in Phase 2g
        Button("Meetings…") {
            // TODO Phase 2g: open native meetings window (master/detail)
        }
        .disabled(true)

        Divider()

        // Identity — wired in Phase 3
        Button("Sign in with Google…") {
            // TODO Phase 3: OAuth via browser → custom URL scheme handoff
        }

        // Configuration — wired in Phase 2b (basic) / Phase 6 (full)
        Button("Settings…") {
            // TODO Phase 2b: open Settings window (Account, General, Permissions)
        }
        .disabled(true)

        Button("Permissions…") {
            // TODO Phase 2d: walk user through Microphone + Screen Recording grants
        }
        .disabled(true)

        Divider()

        Button("Quit Meeting Companion") {
            NSApplication.shared.terminate(nil)
        }
        .keyboardShortcut("q")
    }
}
