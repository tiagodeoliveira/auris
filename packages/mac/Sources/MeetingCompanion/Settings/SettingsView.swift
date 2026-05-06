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
            MeetingDetailView(detail: detail, model: model)
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

// Right-pane detail. Description + timing + metadata + moments + transcript.
private struct MeetingDetailView: View {
    let detail: MeetingDetail
    @Bindable var model: AppModel

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

                if let moments = detail.moments, !moments.isEmpty {
                    Divider()
                    momentsBlock(moments)
                }

                Divider()

                transcriptBlock
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .textSelection(.enabled)
        }
    }

    @ViewBuilder
    private func momentsBlock(_ moments: [Moment]) -> some View {
        Text("Moments").font(.headline)
        VStack(alignment: .leading, spacing: 10) {
            ForEach(moments) { moment in
                MomentCard(moment: moment, meetingId: detail.id, model: model)
            }
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

/// One row in the moments list. Layout: screenshot thumbnail (if
/// any) on the left, timestamp + note + summary stacked on the
/// right. Pending summaries render in italic secondary; failed
/// summaries render in red. Click the thumbnail to expand it in
/// a sheet for easier reading.
private struct MomentCard: View {
    let moment: Moment
    let meetingId: String
    @Bindable var model: AppModel
    @State private var expanded = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            if let rel = moment.screenshotURL,
                let api = MeetingsAPI.fromWSURL(model.settings.serverURL, token: model.settings.token),
                let url = api.screenshotURL(forRelativePath: rel)
            {
                Button {
                    expanded = true
                } label: {
                    AuthorizedImage(url: url, token: model.settings.token)
                        .frame(width: 120, height: 75)
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                        .overlay(
                            RoundedRectangle(cornerRadius: 6)
                                .strokeBorder(Color.gray.opacity(0.25))
                        )
                }
                .buttonStyle(.plain)
                .help("Click to enlarge")
                .sheet(isPresented: $expanded) {
                    ScreenshotLightbox(url: url, token: model.settings.token)
                }
            }
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Text(formatOffset(moment.t))
                        .font(.system(.caption, design: .monospaced).weight(.semibold))
                        .foregroundStyle(.secondary)
                    if moment.kind != "manual" {
                        Text(moment.kind.uppercased())
                            .font(.system(size: 9, weight: .semibold))
                            .tracking(0.5)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background {
                                RoundedRectangle(cornerRadius: 3)
                                    .fill(Color.gray.opacity(0.18))
                            }
                    }
                }
                if let note = moment.note, !note.isEmpty {
                    Text(note)
                        .font(.callout)
                        .foregroundStyle(.primary)
                }
                summaryView
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .padding(8)
        .background(Color.white.opacity(0.04))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    @ViewBuilder
    private var summaryView: some View {
        switch moment.summaryStatus {
        case "done":
            if let s = moment.summary, !s.isEmpty {
                Text(s).font(.body)
            } else {
                // Empty summary on done is unusual but possible; treat as info.
                Text("(empty summary)").font(.callout).foregroundStyle(.secondary)
            }
        case "pending":
            Text("Generating summary…")
                .font(.callout).italic()
                .foregroundStyle(.secondary)
        case "failed":
            Text(moment.summary ?? "Summary failed.")
                .font(.callout)
                .foregroundStyle(.red)
        default:
            Text(moment.summary ?? "")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
    }

    /// `12345` ms → `"00:12"` (m:ss for short meetings, h:mm:ss for
    /// long ones). Used so users can scan when each moment was hit.
    private func formatOffset(_ ms: Int64) -> String {
        let total = max(0, ms) / 1000
        let s = Int(total % 60)
        let m = Int((total / 60) % 60)
        let h = Int(total / 3600)
        if h > 0 {
            return String(format: "%d:%02d:%02d", h, m, s)
        }
        return String(format: "%d:%02d", m, s)
    }
}

/// Full-size screenshot viewer presented as a sheet. Click the
/// background or hit Esc to dismiss. The image is wrapped in a
/// scroll view so screenshots larger than the sheet are still
/// fully reachable; we don't add explicit pan/zoom for v1.
private struct ScreenshotLightbox: View {
    let url: URL
    let token: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Spacer()
                Button {
                    dismiss()
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 22))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
                .help("Close (Esc)")
                .padding(8)
            }

            ScrollView([.horizontal, .vertical]) {
                AuthorizedImage(url: url, token: token, contentMode: .fit)
                    .frame(minWidth: 700, minHeight: 440)
                    .padding(16)
            }
            .background(Color.black.opacity(0.4))
            // Click anywhere on the background to dismiss; the
            // `.fixedSize()` image above doesn't fill the scroll
            // view, so empty area receives the tap.
            .contentShape(Rectangle())
            .onTapGesture { dismiss() }
        }
        .frame(minWidth: 720, idealWidth: 1100, minHeight: 480, idealHeight: 700)
    }
}

/// Loads an image from a URL with a Bearer header and renders it.
/// Replaces `AsyncImage` for our auth'd screenshot fetch — the
/// stock loader can't add headers. Reloads when `url` changes;
/// state is per-row so navigating between meetings doesn't keep
/// stale bytes in memory.
private struct AuthorizedImage: View {
    let url: URL
    let token: String
    /// `.fill` for thumbnails (clip to frame), `.fit` for lightbox
    /// (preserve aspect within available space). Default `.fill`
    /// matches the original thumbnail behavior.
    var contentMode: ContentMode = .fill
    @State private var image: NSImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: contentMode)
            } else if failed {
                placeholder("photo.badge.exclamationmark", color: .secondary)
            } else {
                placeholder("photo", color: .secondary)
            }
        }
        .task(id: url) {
            await load()
        }
    }

    private func placeholder(_ system: String, color: Color) -> some View {
        ZStack {
            Color.gray.opacity(0.12)
            Image(systemName: system)
                .foregroundStyle(color)
                .font(.title2)
        }
    }

    private func load() async {
        var req = URLRequest(url: url)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        do {
            let (data, resp) = try await URLSession.shared.data(for: req)
            if let http = resp as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                failed = true
                return
            }
            if let img = NSImage(data: data) {
                image = img
            } else {
                failed = true
            }
        } catch {
            failed = true
        }
    }
}
