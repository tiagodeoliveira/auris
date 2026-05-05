// SettingsView.swift
// Settings window. Tabbed: "Server" (the existing creds form) +
// "Meetings" (browse persisted meetings via the REST API). Phase 3
// will replace the token field with "Sign in with Google".
//
// The window itself is registered in `MeetingCompanionApp.swift`
// with id `"settings"`. Both menu entries (`Settings…` and
// `Meetings…`) open the same window; the latter pre-selects the
// Meetings tab via `AppModel.selectedSettingsTab`.

import SwiftUI

/// Tabs available in the Settings window. `String` raw value just
/// for nice debug output; `SwiftUI.tag(...)` only requires Hashable.
enum SettingsTab: String, Hashable, CaseIterable {
    case server
    case meetings
}

struct SettingsView: View {
    @Bindable var model: AppModel

    var body: some View {
        TabView(selection: $model.selectedSettingsTab) {
            ServerTab(model: model)
                .tabItem { Label("Server", systemImage: "network") }
                .tag(SettingsTab.server)

            MeetingsTab(model: model)
                .tabItem { Label("Meetings", systemImage: "list.bullet.rectangle") }
                .tag(SettingsTab.meetings)
        }
        .frame(minWidth: 720, minHeight: 460)
        .navigationTitle("Settings")
    }
}

// MARK: - Server tab

/// Existing creds form, lifted into its own view.
private struct ServerTab: View {
    @Bindable var model: AppModel

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
                Text(
                    "For local dev: ws://localhost:7331 with token `dev`. The REST API for browsing meetings is derived from this URL (port + 1)."
                )
                .font(.footnote)
                .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .padding()
    }
}

// MARK: - Meetings tab

/// Master/detail browse of persisted meetings. Fetches `GET /meetings`
/// on appear (and via the toolbar reload button); selecting a row
/// fetches `GET /meetings/:id` for the inlined transcript.
private struct MeetingsTab: View {
    @Bindable var model: AppModel
    @State private var meetings: [MeetingSummary] = []
    @State private var detail: MeetingDetail?
    @State private var selectedId: String?
    @State private var loadError: String?
    @State private var listLoading = false
    @State private var detailLoading = false

    var body: some View {
        NavigationSplitView {
            list
                .frame(minWidth: 260)
                .navigationSplitViewColumnWidth(min: 240, ideal: 280)
                .toolbar {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            Task { await reloadList() }
                        } label: {
                            Image(systemName: "arrow.clockwise")
                        }
                        .help("Reload")
                        .disabled(listLoading)
                    }
                }
        } detail: {
            detailPane
        }
        .task { await reloadList() }
        .onChange(of: selectedId) { _, newId in
            guard let id = newId else {
                detail = nil
                return
            }
            Task { await loadDetail(id: id) }
        }
    }

    // List pane

    private var list: some View {
        List(selection: $selectedId) {
            if listLoading && meetings.isEmpty {
                ProgressView().frame(maxWidth: .infinity, alignment: .center)
            } else if let err = loadError, meetings.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Couldn't load meetings").font(.headline)
                    Text(err).font(.caption).foregroundStyle(.secondary)
                    Button("Retry") { Task { await reloadList() } }
                        .padding(.top, 4)
                }
                .padding(.vertical, 8)
            } else if meetings.isEmpty {
                Text("No meetings yet").foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 12)
            } else {
                ForEach(meetings) { m in
                    MeetingRow(meeting: m).tag(m.id)
                }
            }
        }
        .listStyle(.inset)
    }

    // Detail pane

    @ViewBuilder
    private var detailPane: some View {
        if detailLoading {
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let detail {
            MeetingDetailView(detail: detail)
        } else {
            Text(meetings.isEmpty ? "" : "Select a meeting")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // Networking

    private func makeAPI() -> MeetingsAPI? {
        MeetingsAPI.fromWSURL(model.settings.serverURL, token: model.settings.token)
    }

    private func reloadList() async {
        guard let api = makeAPI() else {
            loadError = "Server URL is invalid; check the Server tab."
            return
        }
        listLoading = true
        defer { listLoading = false }
        do {
            let result = try await api.list()
            meetings = result
            loadError = nil
            // If the previously-selected meeting is gone (e.g.
            // server wiped its data dir), drop the selection so
            // the detail pane doesn't keep showing stale content.
            if let sel = selectedId, !result.contains(where: { $0.id == sel }) {
                selectedId = nil
                detail = nil
            }
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func loadDetail(id: String) async {
        guard let api = makeAPI() else { return }
        detailLoading = true
        defer { detailLoading = false }
        do {
            detail = try await api.detail(id: id)
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }
}

// One row in the master list. Single line of summary; the description
// or a fallback as the headline, then time + duration as a caption.
private struct MeetingRow: View {
    let meeting: MeetingSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(meeting.description?.isEmpty == false ? meeting.description! : "Untitled meeting")
                .font(.body)
                .lineLimit(1)
            HStack(spacing: 6) {
                Text(meeting.startedAt, format: .dateTime.day().month().hour().minute())
                Text("·").foregroundStyle(.tertiary)
                Text(durationLabel).foregroundStyle(.secondary)
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }

    /// Human duration. "in progress" while ended_at is nil so the
    /// list naturally distinguishes the active meeting from past ones.
    private var durationLabel: String {
        guard let endedAt = meeting.endedAt else { return "in progress" }
        let seconds = Int(endedAt.timeIntervalSince(meeting.startedAt))
        if seconds < 60 { return "\(seconds)s" }
        let mins = seconds / 60
        let rem = seconds % 60
        if mins < 60 { return "\(mins)m \(rem)s" }
        let hours = mins / 60
        return "\(hours)h \(mins % 60)m"
    }
}

// Right-pane detail. Description + timing + metadata + transcript.
private struct MeetingDetailView: View {
    let detail: MeetingDetail

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(detail.description?.isEmpty == false ? detail.description! : "Untitled meeting")
                    .font(.title2)
                    .fontWeight(.semibold)
                    .textSelection(.enabled)

                timingRow

                if !detail.metadata.isEmpty {
                    metadataBlock
                }

                Divider()

                transcriptBlock
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .textSelection(.enabled)
        }
    }

    private var timingRow: some View {
        HStack(alignment: .top, spacing: 18) {
            VStack(alignment: .leading, spacing: 1) {
                Text("Started").font(.caption2).foregroundStyle(.secondary)
                Text(detail.startedAt, format: .dateTime.day().month().year().hour().minute())
                    .font(.callout)
            }
            if let endedAt = detail.endedAt {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Ended").font(.caption2).foregroundStyle(.secondary)
                    Text(endedAt, format: .dateTime.day().month().year().hour().minute())
                        .font(.callout)
                }
            } else {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Status").font(.caption2).foregroundStyle(.secondary)
                    Text("in progress").font(.callout).foregroundStyle(.orange)
                }
            }
        }
    }

    private var metadataBlock: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Metadata").font(.headline)
            ForEach(detail.metadata.keys.sorted(), id: \.self) { key in
                HStack(spacing: 8) {
                    Text(key)
                        .font(.system(.callout, design: .monospaced))
                        .foregroundStyle(.secondary)
                    Text(detail.metadata[key] ?? "")
                        .font(.callout)
                }
            }
        }
    }

    @ViewBuilder
    private var transcriptBlock: some View {
        Text("Transcript").font(.headline)
        if detail.transcript.isEmpty {
            Text("(no transcript captured)")
                .foregroundStyle(.secondary)
                .font(.callout)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(detail.transcript) { item in
                    TranscriptRow(item: item)
                }
            }
        }
    }
}

private struct TranscriptRow: View {
    let item: Item

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            if let speaker = item.meta?.speaker, !speaker.isEmpty {
                Text(speaker)
                    .font(.system(size: 10, weight: .semibold))
                    .tracking(0.3)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background {
                        RoundedRectangle(cornerRadius: 3)
                            .fill(Color.gray.opacity(0.15))
                    }
            }
            Text(item.text)
                .font(.body)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
