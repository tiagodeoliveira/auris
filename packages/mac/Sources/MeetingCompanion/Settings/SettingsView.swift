// SettingsView.swift
// Settings window. Phase 2c: server URL + token. Phase 3 replaces
// the token field with "Sign in with Google".

import SwiftUI

struct SettingsView: View {
    @Bindable var model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Form {
            Section {
                TextField("Server URL", text: $model.settings.serverURL)
                    .textFieldStyle(.roundedBorder)
                    .autocorrectionDisabled()
                SecureField("Token", text: $model.settings.token)
                    .textFieldStyle(.roundedBorder)
            } header: {
                Text("Server")
            } footer: {
                Text("For local dev: ws://localhost:7331 with token `dev`. Phase 3 replaces this with Sign in with Google.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .padding()
        .frame(minWidth: 480, minHeight: 240)
        .navigationTitle("Settings")
    }
}
