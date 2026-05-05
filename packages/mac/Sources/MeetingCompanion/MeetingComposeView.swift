// MeetingComposeView.swift
// Pre-meeting compose window. v1 collects only an optional
// description; Phase 2g-2 layers an Extract Tags step on top
// (calls the server with the description, displays returned
// tags as pills, lets the user edit before starting). The
// description is plumbed straight through to `start_meeting`.

import AppKit
import SwiftUI

struct MeetingComposeView: View {
    @Bindable var model: AppModel
    @Environment(\.dismissWindow) private var dismissWindow
    @Environment(\.openWindow) private var openWindow

    @State private var description: String = ""
    @FocusState private var descriptionFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Start a meeting")
                .font(.title3)
                .fontWeight(.semibold)

            VStack(alignment: .leading, spacing: 4) {
                Text("Description")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                ZStack(alignment: .topLeading) {
                    if description.isEmpty {
                        Text("What's this meeting about? (optional)")
                            .foregroundStyle(.tertiary)
                            .padding(.top, 8)
                            .padding(.leading, 5)
                            .allowsHitTesting(false)
                    }
                    TextEditor(text: $description)
                        .font(.body)
                        .scrollContentBackground(.hidden)
                        .focused($descriptionFocused)
                        .frame(minHeight: 110)
                }
                .padding(4)
                .background(Color(nsColor: .textBackgroundColor))
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .strokeBorder(Color.gray.opacity(0.3))
                )
            }

            HStack {
                if !model.canStartMeeting {
                    Text(notReadyHint)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button("Cancel") {
                    dismissWindow(id: "meeting-compose")
                }
                .keyboardShortcut(.cancelAction)

                Button("Start meeting") {
                    submit()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canStartMeeting)
            }
        }
        .padding(20)
        .frame(width: 440, height: 230)
        .onAppear { descriptionFocused = true }
    }

    private func submit() {
        let trimmed = description.trimmingCharacters(in: .whitespacesAndNewlines)
        let payload: String? = trimmed.isEmpty ? nil : trimmed
        // Open the overlay first so the meeting transition feels
        // continuous — overlay appears, compose dismisses, audio
        // capture starts under the overlay's VAD bars.
        openWindow(id: "meeting-overlay")
        NSApp.activate(ignoringOtherApps: true)
        Task { await model.startMeeting(description: payload) }
        dismissWindow(id: "meeting-compose")
    }

    /// One-line nudge when Start is disabled — saves the user a
    /// trip to the menu to figure out what's missing.
    private var notReadyHint: String {
        if model.webSocket.state != .connected { return "Not connected" }
        if !model.permissionMonitor.allGranted { return "Permissions not granted" }
        if model.audioCapture.state != .stopped { return "Audio capture busy" }
        return ""
    }
}
