// PermissionsView.swift
// Embedded as a tab inside the Settings window. Shows current state
// of Microphone + Screen Recording grants and offers a one-click
// action to request each.

import SwiftUI

struct PermissionsView: View {
    @Bindable var model: AppModel

    var body: some View {
        Form {
            Section {
                permissionRow(
                    title: "Microphone",
                    status: model.permissionMonitor.microphone,
                    explanation:
                        "Required to capture your voice during a meeting.",
                    action: {
                        Task { await model.permissionMonitor.requestMicrophone() }
                    },
                    settingsAction: {
                        model.permissionMonitor.openMicrophoneSettings()
                    }
                )
            } header: {
                Text("Permissions")
            }

            Section {
                permissionRow(
                    title: "Screen Recording",
                    status: model.permissionMonitor.screenRecording,
                    explanation:
                        "Required to capture the other person's voice via system audio. macOS gates system-audio capture behind Screen Recording, even when we don't actually record the screen.",
                    action: {
                        model.permissionMonitor.requestScreenRecording()
                    },
                    settingsAction: {
                        model.permissionMonitor.openScreenRecordingSettings()
                    }
                )
            }
        }
        .formStyle(.grouped)
        .padding()
        .onReceive(
            NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)
        ) { _ in
            model.permissionMonitor.refresh()
        }
    }

    @ViewBuilder
    private func permissionRow(
        title: String,
        status: PermissionMonitor.Status,
        explanation: String,
        action: @escaping () -> Void,
        settingsAction: @escaping () -> Void
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: statusIcon(for: status))
                    .foregroundStyle(statusColor(for: status))
                Text(title)
                    .font(.headline)
                Spacer()
                Text(statusText(for: status))
                    .foregroundStyle(.secondary)
                    .font(.subheadline)
            }
            Text(explanation)
                .font(.footnote)
                .foregroundStyle(.secondary)
            HStack {
                if status != .granted {
                    Button("Request…", action: action)
                }
                Button("Open System Settings", action: settingsAction)
                    .buttonStyle(.link)
            }
        }
        .padding(.vertical, 4)
    }

    private func statusIcon(for status: PermissionMonitor.Status) -> String {
        switch status {
        case .granted: "checkmark.circle.fill"
        case .denied: "xmark.circle.fill"
        case .notDetermined: "questionmark.circle"
        }
    }

    private func statusColor(for status: PermissionMonitor.Status) -> Color {
        switch status {
        case .granted: .green
        case .denied: .red
        case .notDetermined: .secondary
        }
    }

    private func statusText(for status: PermissionMonitor.Status) -> String {
        switch status {
        case .granted: "Granted"
        case .denied: "Denied"
        case .notDetermined: "Not granted"
        }
    }
}
