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
/// `account` was previously `server` (held the WS URL + shared
/// token); the URL is now build-time and the token is gone, so the
/// tab is purely about identity.
enum SettingsTab: String, Hashable, CaseIterable {
    case account
    case meetings
    case artifacts
    case permissions
}

struct SettingsView: View {
    @Bindable var model: AppModel

    var body: some View {
        TabView(selection: $model.selectedSettingsTab) {
            AccountTab(model: model)
                .tabItem { Label("Account", systemImage: "person.crop.circle") }
                .tag(SettingsTab.account)

            MeetingsTab(model: model)
                .tabItem { Label("Meetings", systemImage: "list.bullet.rectangle") }
                .tag(SettingsTab.meetings)

            ArtifactsTab(model: model)
                .tabItem { Label("Artifacts", systemImage: "doc.text") }
                .tag(SettingsTab.artifacts)

            PermissionsView(model: model)
                .tabItem { Label("Permissions", systemImage: "lock.shield") }
                .tag(SettingsTab.permissions)
        }
        .frame(minWidth: 720, minHeight: 460)
        .navigationTitle("Settings")
        .preferredColorScheme(.light)
        .tint(SettingsTheme.blue)
        .background(SettingsTheme.background)
    }
}

// MARK: - Account tab

/// Identity surface — Auth0 sign-in / sign-out. The button label is
/// just "Sign in" because Auth0's Universal Login lets the user pick
/// their identity provider (Google, email, password, etc.) once they
/// land on the hosted page; we shouldn't bake one provider into the
/// CTA when the tenant might enable several.
private struct AccountTab: View {
    @Bindable var model: AppModel
    @State private var signInError: String? = nil
    @State private var signingIn = false

    var body: some View {
        Form {
            Section {
                if let id = model.auth0.identity {
                    HStack(spacing: 12) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(id.name ?? id.email ?? id.sub)
                                .font(.body)
                            if let email = id.email, email != id.name {
                                Text(email)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        Spacer()
                        Button("Sign out") {
                            model.auth0.signOut()
                            model.disconnect()
                        }
                    }
                } else {
                    Button(signingIn ? "Signing in…" : "Sign in") {
                        signingIn = true
                        signInError = nil
                        Task {
                            do {
                                try await model.auth0.signIn()
                                if model.canConnect { model.connect() }
                            } catch {
                                signInError = error.localizedDescription
                            }
                            signingIn = false
                        }
                    }
                    .disabled(signingIn)
                    if let err = signInError {
                        Text(err).font(.caption).foregroundStyle(.red)
                    }
                }
            } header: {
                Text("Account")
            } footer: {
                Text(
                    "Sign in once; the Mac app stores a refresh token and reconnects silently across launches."
                )
                .font(.footnote)
                .foregroundStyle(.secondary)
            }

            Section {
                Picker("Theme", selection: $model.settings.overlayTheme) {
                    ForEach(OverlayTheme.allCases) { theme in
                        Text(theme.displayName).tag(theme)
                    }
                }
                .pickerStyle(.segmented)

                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("Opacity")
                        Spacer()
                        Text("\(Int(model.settings.overlayOpacity * 100))%")
                            .foregroundStyle(.secondary)
                            .font(.callout.monospacedDigit())
                    }
                    Slider(value: $model.settings.overlayOpacity, in: 0.01 ... 1.0, step: 0.01)
                }
            } header: {
                Text("Overlay")
            } footer: {
                Text(
                    "Theme switches the overlay between light and dark palettes; opacity drives the panel and chat-bubble translucency together so contents nest into the same level of see-through."
                )
                .font(.footnote)
                .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .background(SettingsTheme.background)
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
                    MeetingRow(meeting: m)
                        .tag(m.id)
                        // Three deletion paths: trackpad two-finger
                        // swipe (.swipeActions), right-click
                        // (.contextMenu), and ⌫ on the selected row
                        // (.onDeleteCommand). On macOS the swipe is
                        // hard to discover so the other two carry
                        // most of the weight.
                        .swipeActions(edge: .trailing) {
                            Button(role: .destructive) {
                                Task { await deleteMeeting(id: m.id) }
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                        }
                        .contextMenu {
                            Button(role: .destructive) {
                                Task { await deleteMeeting(id: m.id) }
                            } label: {
                                Label("Delete meeting", systemImage: "trash")
                            }
                        }
                }
            }
        }
        .listStyle(.inset)
        .scrollContentBackground(.hidden)
        .background(SettingsTheme.sidebar)
        .onDeleteCommand {
            if let id = selectedId {
                Task { await deleteMeeting(id: id) }
            }
        }
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
                .background(SettingsTheme.background)
        }
    }

    // Networking

    private func makeAPI() async -> MeetingsAPI? {
        try? await model.makeMeetingsAPI()
    }

    private func reloadList() async {
        guard let api = await makeAPI() else {
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
        guard let api = await makeAPI() else { return }
        detailLoading = true
        defer { detailLoading = false }
        do {
            detail = try await api.detail(id: id)
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Server delete + local list update. Optimistic on the local
    /// remove (drop the row up front for snappy UI), with a reload
    /// on failure to put it back if the server actually rejected.
    private func deleteMeeting(id: String) async {
        guard let api = await makeAPI() else { return }
        let removedIndex = meetings.firstIndex(where: { $0.id == id })
        let removedItem = removedIndex.map { meetings[$0] }
        if let i = removedIndex {
            meetings.remove(at: i)
        }
        if selectedId == id {
            selectedId = nil
            detail = nil
        }
        do {
            try await api.delete(id: id)
        } catch {
            // Revert on failure so the user sees the row come back.
            if let i = removedIndex, let item = removedItem {
                meetings.insert(item, at: i)
            }
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }
}

// One row in the master list. Single line of summary; the LLM-extracted
// metadata.title (or first line of description, truncated) as the
// headline, then time + duration as a caption.
private struct MeetingRow: View {
    let meeting: MeetingSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(pickMeetingTitle(description: meeting.description, metadata: meeting.metadata))
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

// Right-pane detail. Title + timing + description + metadata + moments + transcript.
private struct MeetingDetailView: View {
    let detail: MeetingDetail
    @Bindable var model: AppModel

    @State private var descriptionExpanded = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(pickMeetingTitle(description: detail.description, metadata: detail.metadata))
                    .font(.title2)
                    .fontWeight(.semibold)
                    .textSelection(.enabled)

                timingRow

                // Description (freeform context). Distinct from the
                // headline title and the structured metadata block —
                // collapsed by default behind a chevron disclosure
                // so a multi-page paste doesn't dominate the view.
                if let desc = detail.description?.trimmingCharacters(in: .whitespacesAndNewlines),
                   !desc.isEmpty,
                   desc != pickMeetingTitle(description: detail.description, metadata: detail.metadata)
                {
                    descriptionBlock(desc)
                }

                if !detail.metadata.isEmpty {
                    metadataBlock
                }

                if let artifacts = detail.artifacts, !artifacts.isEmpty {
                    Divider()
                    artifactsBlock(artifacts)
                }

                if let usage = detail.llmUsage, usage.calls > 0 {
                    Divider()
                    llmUsageBlock(usage)
                }

                if let moments = detail.moments, !moments.isEmpty {
                    Divider()
                    momentsBlock(moments)
                }

                let itemsByMode = detail.itemsByMode ?? [:]
                ForEach(MeetingDetailView.modeOrder, id: \.id) { mode in
                    if let items = itemsByMode[mode.id], !items.isEmpty {
                        Divider()
                        modeItemsBlock(label: mode.label, mode: mode.id, items: items)
                    }
                }

                Divider()

                transcriptBlock
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .textSelection(.enabled)
        }
        .scrollContentBackground(.hidden)
        .background(SettingsTheme.background)
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

    @ViewBuilder
    private func llmUsageBlock(_ usage: MeetingLlmUsage) -> some View {
        Text("LLM usage").font(.headline)
        // `inputTokens` and `cachedInputTokens` are disjoint buckets
        // in rig 0.36's Anthropic mapping — input = fresh billable,
        // cached = cache-read at 0.10× rate. Older builds subtracted
        // them assuming overlap, which produced "billable = 0" once
        // prompt caching kicked in. Show as separate counts.
        VStack(alignment: .leading, spacing: 4) {
            llmUsageRow("calls", String(usage.calls))
            llmUsageRow("input (billable)", formatTokens(usage.inputTokens))
            llmUsageRow("cached read (0.10×)", formatTokens(usage.cachedInputTokens))
            llmUsageRow("output tokens", formatTokens(usage.outputTokens))
            if let model = usage.modelId {
                llmUsageRow("model", model)
            }
            if let provider = usage.provider {
                llmUsageRow("provider", provider)
            }
        }
    }

    @ViewBuilder
    private func llmUsageRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(width: 110, alignment: .leading)
            Text(value).font(.callout)
        }
    }

    private func formatTokens(_ n: Int64) -> String {
        // Light-touch grouping so big numbers don't blur into a wall.
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.groupingSeparator = ","
        return formatter.string(from: NSNumber(value: n)) ?? String(n)
    }

    /// Fixed render order matching the live overlay's tab order so
    /// the meeting-detail view feels familiar. Modes with no items
    /// for this meeting are skipped entirely.
    fileprivate static let modeOrder: [(id: String, label: String)] = [
        ("highlights", "Highlights"),
        ("actions", "Actions"),
        ("open_questions", "Open questions"),
        ("summary", "Summary"),
        ("chat", "Chat"),
    ]

    @ViewBuilder
    private func modeItemsBlock(label: String, mode: String, items: [Item]) -> some View {
        Text(label).font(.headline)
        VStack(alignment: .leading, spacing: 6) {
            if mode == "chat" {
                ForEach(items) { item in
                    DetailChatBubbleRow(item: item)
                }
            } else {
                ForEach(items) { item in
                    DetailItemRow(item: item, mode: mode)
                }
            }
        }
    }

    /// Attached artifacts block. Compact rows: name + mime pill +
    /// short summary. Doesn't expose detach (past meetings should
    /// preserve their attachment history); cleaning up an artifact
    /// uses Settings → Artifacts → trash, which cascades.
    @ViewBuilder
    private func artifactsBlock(_ artifacts: [Artifact]) -> some View {
        Text("Attached artifacts").font(.headline)
        VStack(alignment: .leading, spacing: 8) {
            ForEach(artifacts) { a in
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: "doc.text")
                        .foregroundStyle(.secondary)
                        .padding(.top, 2)
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(a.name).font(.callout).fontWeight(.medium).lineLimit(1)
                            Text(a.mimeType)
                                .font(.system(size: 9, weight: .semibold))
                                .tracking(0.4)
                                .foregroundStyle(.secondary)
                                .padding(.horizontal, 5)
                                .padding(.vertical, 1)
                                .background {
                                    RoundedRectangle(cornerRadius: 3)
                                        .fill(Color.gray.opacity(0.12))
                                }
                        }
                        if let s = a.shortSummary, !s.isEmpty {
                            Text(s).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                        }
                    }
                    Spacer()
                }
                .padding(8)
                .background(SettingsTheme.card)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(SettingsTheme.border))
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

    /// Description block — chevron + heading + inline snippet collapsed,
    /// or the full prose in a max-height scroll box when expanded.
    /// Mirrors the PWA's `meetings-detail-description` pattern.
    @ViewBuilder
    private func descriptionBlock(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                descriptionExpanded.toggle()
            } label: {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(descriptionExpanded ? "▾" : "▸")
                        .foregroundStyle(.secondary)
                        .font(.caption)
                    Text("DESCRIPTION")
                        .font(.caption)
                        .fontWeight(.semibold)
                        .foregroundStyle(.secondary)
                    if !descriptionExpanded {
                        Text(snippet(of: text))
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if descriptionExpanded {
                ScrollView {
                    Text(text)
                        .font(.callout)
                        .foregroundStyle(.primary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10)
                }
                .frame(maxHeight: 280)
                .background(Color.secondary.opacity(0.08))
                .clipShape(RoundedRectangle(cornerRadius: 6))
            }
        }
    }

    private func snippet(of text: String) -> String {
        let flat = text.replacingOccurrences(of: "\n", with: " ")
            .trimmingCharacters(in: .whitespaces)
        if flat.count <= 70 { return flat }
        return String(flat.prefix(69)) + "…"
    }
}

/// Title shown at the top of meeting detail and as the row headline
/// in the master list. Order of preference:
///   1. `metadata["title"]` — extracted by the LLM, short and clean.
///   2. First non-empty line of the description, truncated to 80 chars.
///   3. "Untitled meeting" fallback.
///
/// Previously the description was used directly as the title, which
/// works fine for one-line descriptions but sprawls across the pane
/// when the user pastes a multi-page job posting as context.
func pickMeetingTitle(description: String?, metadata: [String: String]) -> String {
    if let t = metadata["title"]?.trimmingCharacters(in: .whitespaces), !t.isEmpty {
        return t
    }
    if let d = description?.trimmingCharacters(in: .whitespacesAndNewlines), !d.isEmpty {
        let firstLine = d.split(separator: "\n").first.map(String.init)?
            .trimmingCharacters(in: .whitespaces) ?? ""
        if firstLine.count <= 80 { return firstLine }
        return String(firstLine.prefix(79)) + "…"
    }
    return "Untitled meeting"
}

/// One row in a per-mode persisted-items block. Mirrors the live
/// overlay's ItemRow layout so the meeting-detail view feels
/// familiar: timestamp pill + blue triangle bullet + body text +
/// optional mode-specific meta chip beneath.
private struct DetailItemRow: View {
    let item: Item
    let mode: String

    private var timestampLabel: String {
        let total = max(0, Int(item.t / 1000))
        return String(format: "[%02d:%02d]", total / 60, total % 60)
    }

    private var metaText: String {
        guard let meta = item.meta else { return "" }
        switch mode {
        case "actions":
            let owner = meta.owner.flatMap { s in s.isEmpty ? nil : "OWNER · \(s)" } ?? ""
            let due = meta.due.flatMap { s in s.isEmpty ? nil : "DUE · \(s)" } ?? ""
            return [owner, due].filter { !$0.isEmpty }.joined(separator: " · ")
        case "highlights":
            return meta.importance.flatMap { s in
                s.isEmpty ? nil : "IMPORTANCE · \(s)"
            } ?? ""
        case "open_questions":
            let kind = meta.kind.flatMap { s in s.isEmpty ? nil : s.uppercased() } ?? ""
            let ctx = meta.context.flatMap { s in s.isEmpty ? nil : s } ?? ""
            switch (kind.isEmpty, ctx.isEmpty) {
            case (true, true): return ""
            case (false, true): return kind
            case (true, false): return ctx
            case (false, false): return "\(kind) · \(ctx)"
            }
        default:
            return ""
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(timestampLabel)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                Text("▸")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(.blue)
                Text(item.text)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if !metaText.isEmpty {
                Text(metaText)
                    .font(.system(size: 10, weight: .semibold, design: .monospaced))
                    .tracking(0.4)
                    .foregroundStyle(.secondary)
                    .padding(.leading, 70)
            }
            if let detail = item.detail, !detail.isEmpty {
                Text(detail)
                    .font(.callout)
                    .foregroundStyle(.primary)
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color.gray.opacity(0.08))
                    .overlay(
                        Rectangle()
                            .fill(Color.blue)
                            .frame(width: 2),
                        alignment: .leading
                    )
                    .padding(.leading, 70)
                    .padding(.top, 4)
            }
        }
    }
}

/// Chat bubble in the meeting-detail view. `meta.role == "user"`
/// right-aligns blue; anything else left-aligns surface. No pending
/// state here — by the time a meeting is in history, every chat
/// turn has already been answered (or stored as a placeholder
/// failure bubble).
private struct DetailChatBubbleRow: View {
    let item: Item

    private var isUser: Bool { item.meta?.role == "user" }

    var body: some View {
        HStack {
            if isUser { Spacer(minLength: 32) }
            Text(item.text)
                .foregroundStyle(isUser ? Color.white : Color.primary)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .background(isUser ? Color.blue : Color.gray.opacity(0.12))
                .overlay(
                    RoundedRectangle(cornerRadius: 11)
                        .strokeBorder(isUser ? Color.clear : Color.gray.opacity(0.3), lineWidth: 1)
                )
                .clipShape(RoundedRectangle(cornerRadius: 11))
                .frame(maxWidth: .infinity, alignment: isUser ? .trailing : .leading)
            if !isUser { Spacer(minLength: 32) }
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
                let url = screenshotURL(rel: rel)
            {
                Button {
                    expanded = true
                } label: {
                    AuthorizedImage(url: url, auth0: model.auth0)
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
                    ScreenshotLightbox(url: url, auth0: model.auth0)
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
        .background(SettingsTheme.card)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(SettingsTheme.border)
        )
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

    /// Build the absolute screenshot URL from the relative one
    /// returned by the server. Doesn't need an API instance — just
    /// the WS URL → REST origin transform.
    private func screenshotURL(rel: String) -> URL? {
        guard var c = URLComponents(string: model.settings.serverURL) else { return nil }
        switch c.scheme?.lowercased() {
        case "ws": c.scheme = "http"
        case "wss": c.scheme = "https"
        default: return nil
        }
        c.path = ""
        c.query = nil
        guard let base = c.url else { return nil }
        let trimmed = rel.hasPrefix("/") ? String(rel.dropFirst()) : rel
        return URL(string: trimmed, relativeTo: base)?.absoluteURL
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
    let auth0: Auth0Client
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
                AuthorizedImage(url: url, auth0: auth0, contentMode: .fit)
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
    let auth0: Auth0Client
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
        let token: String
        do {
            token = try await auth0.getAccessToken()
        } catch {
            failed = true
            return
        }
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

// MARK: - Artifacts tab

/// Personal library of uploaded documents/images. PLAN.md §3.7.
/// Upload via "Upload…" button (NSOpenPanel). Each row shows a
/// status badge (pending / done / failed) reflecting the async
/// summarizer worker's progress; the row is selectable for
/// attaching to a meeting only when status is `done`.
private struct ArtifactsTab: View {
    @Bindable var model: AppModel
    @State private var artifacts: [Artifact] = []
    @State private var loadError: String?
    @State private var listLoading = false
    @State private var uploading = false
    @State private var refreshTimer: Timer?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            toolbar
            Divider()
            list
        }
        .background(SettingsTheme.background)
        .task { await reloadList() }
        .onDisappear { refreshTimer?.invalidate() }
    }

    private var toolbar: some View {
        HStack(spacing: 8) {
            Button {
                Task { await pickAndUpload() }
            } label: {
                Label("Upload…", systemImage: "arrow.up.doc")
            }
            .disabled(uploading)
            if uploading {
                ProgressView().controlSize(.small)
            }
            Spacer()
            Button {
                Task { await reloadList() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .help("Reload")
            .disabled(listLoading)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    @ViewBuilder
    private var list: some View {
        if listLoading && artifacts.isEmpty {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let err = loadError, artifacts.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("Couldn't load artifacts").font(.headline)
                Text(err).font(.caption).foregroundStyle(.secondary)
                Button("Retry") { Task { await reloadList() } }.padding(.top, 4)
            }
            .padding(20)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else if artifacts.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: "doc.text").font(.system(size: 32)).foregroundStyle(.secondary)
                Text("No artifacts yet").foregroundStyle(.secondary)
                Text("Upload a document or image to give meeting agents context.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(artifacts) { a in
                        ArtifactRow(artifact: a) {
                            Task { await deleteArtifact(id: a.id) }
                        }
                    }
                }
                .padding(16)
            }
            .scrollContentBackground(.hidden)
        }
    }

    // MARK: networking

    private func makeAPI() async -> ArtifactsAPI? {
        try? await model.makeArtifactsAPI()
    }

    private func reloadList() async {
        guard let api = await makeAPI() else {
            loadError = "Server URL is invalid; check the Account tab."
            return
        }
        listLoading = true
        defer { listLoading = false }
        do {
            artifacts = try await api.list()
            loadError = nil
            // If any artifact is still pending, schedule a refresh
            // so the user sees the status flip without hitting the
            // reload button. The summary worker usually completes
            // within a few seconds for text/markdown.
            if artifacts.contains(where: { $0.summaryStatus == "pending" }) {
                scheduleAutoRefresh()
            } else {
                refreshTimer?.invalidate()
                refreshTimer = nil
            }
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func scheduleAutoRefresh() {
        refreshTimer?.invalidate()
        // Light polling — a meeting-prep upload doesn't need to be
        // sub-second, but a 2 s tick keeps the UI feeling alive
        // while the worker runs.
        refreshTimer = Timer.scheduledTimer(withTimeInterval: 2.0, repeats: false) { _ in
            Task { @MainActor in await reloadList() }
        }
    }

    private func pickAndUpload() async {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.message = "Pick a document or image to add to your artifact library."
        panel.prompt = "Upload"
        panel.allowedContentTypes = []  // accept anything; server filters by mime
        guard panel.runModal() == .OK, let url = panel.url else { return }
        await upload(url: url)
    }

    private func upload(url: URL) async {
        guard let api = await makeAPI() else {
            loadError = "Server URL is invalid; check the Account tab."
            return
        }
        let name = url.lastPathComponent
        let mime = mimeType(for: url)
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            loadError = "Couldn't read \(name): \(error.localizedDescription)"
            return
        }
        uploading = true
        defer { uploading = false }
        do {
            _ = try await api.upload(name: name, mimeType: mime, data: data)
            await reloadList()
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func deleteArtifact(id: String) async {
        guard let api = await makeAPI() else { return }
        let removedIndex = artifacts.firstIndex(where: { $0.id == id })
        let removed = removedIndex.map { artifacts[$0] }
        if let i = removedIndex { artifacts.remove(at: i) }
        do {
            try await api.delete(id: id)
        } catch {
            // Revert on failure.
            if let i = removedIndex, let r = removed { artifacts.insert(r, at: i) }
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Best-effort mime-type guess from the URL's extension. Covers
    /// the formats the server's whitelist accepts (text/markdown,
    /// text/plain, text/html, text/csv, application/json,
    /// application/pdf, image/png, image/jpeg). Falls back to
    /// `application/octet-stream` so the server's whitelist returns
    /// 400 with a clear message rather than us pre-rejecting on the
    /// client.
    private func mimeType(for url: URL) -> String {
        switch url.pathExtension.lowercased() {
        case "md", "markdown": return "text/markdown"
        case "txt": return "text/plain"
        case "html", "htm": return "text/html"
        case "csv": return "text/csv"
        case "json": return "application/json"
        case "pdf": return "application/pdf"
        case "png": return "image/png"
        case "jpg", "jpeg": return "image/jpeg"
        default: return "application/octet-stream"
        }
    }
}

/// One row in the artifacts list. Status badge + name + size +
/// short-summary preview (when populated). Disclosure chevron
/// expands the long summary inline. Trash button on the right;
/// right-click also offers Delete.
private struct ArtifactRow: View {
    let artifact: Artifact
    let onDelete: () -> Void
    @State private var confirmDelete = false
    @State private var isExpanded = false

    /// Long summary is shown only when the artifact is `done` and
    /// actually has content. Pending/failed artifacts have no
    /// long summary worth expanding to.
    private var canExpand: Bool {
        artifact.summaryStatus == "done"
            && artifact.longSummary.map { !$0.isEmpty } == true
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .top, spacing: 10) {
                statusBadge
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        Text(artifact.name)
                            .font(.body)
                            .fontWeight(.medium)
                            .lineLimit(1)
                        Text(artifact.mimeType)
                            .font(.system(size: 9, weight: .semibold))
                            .tracking(0.4)
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background {
                                RoundedRectangle(cornerRadius: 3)
                                    .fill(Color.gray.opacity(0.12))
                            }
                        Spacer(minLength: 4)
                        Text(humanSize(artifact.sizeBytes))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        if canExpand {
                            Button {
                                isExpanded.toggle()
                            } label: {
                                Image(
                                    systemName: isExpanded ? "chevron.down" : "chevron.right"
                                )
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(.secondary)
                                .frame(width: 14, height: 14)
                            }
                            .buttonStyle(.plain)
                            .help(isExpanded ? "Hide details" : "Show details")
                        }
                        Button {
                            confirmDelete = true
                        } label: {
                            Image(systemName: "trash")
                                .font(.system(size: 12))
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .help("Delete artifact")
                        .confirmationDialog(
                            "Delete “\(artifact.name)”?",
                            isPresented: $confirmDelete,
                            titleVisibility: .visible
                        ) {
                            Button("Delete", role: .destructive, action: onDelete)
                            Button("Cancel", role: .cancel) {}
                        } message: {
                            Text("Removes the file from your library and from any meetings it was attached to. This cannot be undone.")
                        }
                    }
                    if artifact.summaryStatus == "done", let s = artifact.shortSummary, !s.isEmpty {
                        Text(s)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .lineLimit(3)
                    }
                    if artifact.summaryStatus == "pending" {
                        Text("Generating summary…")
                            .font(.caption).italic()
                            .foregroundStyle(.secondary)
                    }
                    if artifact.summaryStatus == "failed" {
                        Text("Summary failed — server logs may have more.")
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
            }

            if isExpanded, let long = artifact.longSummary, !long.isEmpty {
                // Long summary panel. Indented past the status badge
                // column so the alignment matches the short summary
                // above. Selectable so users can copy useful chunks
                // (named entities, decisions, etc.) into other apps.
                Text(long)
                    .font(.callout)
                    .foregroundStyle(.primary)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
                    .padding(.leading, 26)
                    .padding(.trailing, 4)
            }
        }
        .padding(10)
        .background(SettingsTheme.card)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .strokeBorder(SettingsTheme.border)
        )
        .contextMenu {
            Button(role: .destructive, action: onDelete) {
                Label("Delete artifact", systemImage: "trash")
            }
        }
    }

    @ViewBuilder
    private var statusBadge: some View {
        switch artifact.summaryStatus {
        case "done":
            Image(systemName: "checkmark.circle.fill")
                .foregroundStyle(.green)
        case "pending":
            ProgressView().controlSize(.small)
        case "failed":
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
        default:
            Image(systemName: "questionmark.circle")
                .foregroundStyle(.secondary)
        }
    }

    private func humanSize(_ bytes: Int64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: bytes)
    }
}

private enum SettingsTheme {
    static let background = Color(hex: 0xF7FAFE)
    static let sidebar = Color(hex: 0xEEF4FB)
    static let card = Color(hex: 0xFFFFFF)
    static let border = Color(hex: 0xD5DEE9)
    static let blue = Color(hex: 0x2563EB)
}

private extension Color {
    init(hex: UInt32) {
        let r = Double((hex >> 16) & 0xff) / 255.0
        let g = Double((hex >> 8) & 0xff) / 255.0
        let b = Double(hex & 0xff) / 255.0
        self.init(red: r, green: g, blue: b)
    }
}
