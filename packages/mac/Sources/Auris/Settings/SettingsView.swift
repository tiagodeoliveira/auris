// SettingsView.swift
// Settings window. Tabbed: "Server" (the existing creds form) +
// "Meetings" (browse persisted meetings via the REST API). Phase 3
// will replace the token field with "Sign in with Google".
//
// The window itself is registered in `AurisApp.swift`
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
    case quickAsks
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

            QuickAsksTab(model: model)
                .tabItem { Label("Quick Asks", systemImage: "text.bubble") }
                .tag(SettingsTab.quickAsks)

            PermissionsView(model: model)
                .tabItem { Label("Permissions", systemImage: "lock.shield") }
                .tag(SettingsTab.permissions)
        }
        .frame(minWidth: 720, minHeight: 460)
        .navigationTitle("Settings")
        // No .preferredColorScheme — inherit the system appearance.
        // The SettingsTheme tokens below are AppKit-semantic colors
        // so the 3-tier elevation hierarchy (sidebar < background <
        // card) reads correctly in both light and dark mode.
        .tint(SettingsTheme.blue)
        .background(SettingsTheme.background)
        // Not maximizeable: grey out the green button so the window can't be
        // zoomed or taken full-screen; it stays resizable via its edges.
        .background(WindowAccessor { window in
            window.collectionBehavior.insert(.fullScreenNone)
            window.standardWindowButton(.zoomButton)?.isEnabled = false
        })
    }
}

// MARK: - Auris brand mark

/// Auris brand mark — two nested ear arcs opening left + a coral
/// focal-point dot. Stroke colour follows `.primary` so it adapts
/// to light/dark mode automatically when rendered inline in a View
/// hierarchy.
///
/// For menu bar use, see `AurisMark.menuBarTemplateImage` — the
/// system's MenuBarExtra can't extract a usable alpha mask from a
/// SwiftUI View directly, so we pre-rasterize there.
struct AurisMark: View {
    /// Outer-arc diameter in points. The mark fits in a
    /// `size × size` square.
    var size: CGFloat = 22

    var body: some View {
        ZStack {
            // Outer arc — left half of a circle (opens right).
            // Matches the master SVG (`assets/branding/auris-master.svg`)
            // and the PWA mark. Circle paths start at 3 o'clock and
            // trace clockwise; trim(0, 0.5) → bottom half, rotated +90°
            // → left half (the `(` shape, mouth facing right).
            arc(radius: size * 0.42)
            // Inner arc — same shape, smaller radius.
            arc(radius: size * 0.22)
            // Coral focal dot, sitting inside the opening (right of centre).
            Circle()
                .fill(SettingsTheme.blue)
                .frame(width: size * 0.14, height: size * 0.14)
                .offset(x: size * 0.18)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    @ViewBuilder
    private func arc(radius: CGFloat) -> some View {
        Circle()
            .trim(from: 0, to: 0.5)
            .rotation(.degrees(90))
            .stroke(
                Color.primary,
                style: StrokeStyle(lineWidth: size * 0.13, lineCap: .round)
            )
            .frame(width: radius * 2, height: radius * 2)
    }

    /// Pre-rasterized template NSImage of the mark, sized for the
    /// macOS menu bar (18×18pt @ Retina scale). MenuBarExtra needs
    /// a real NSImage with a usable alpha mask — feeding it a
    /// SwiftUI View directly produces a solid silhouette / nothing
    /// depending on appearance state.
    ///
    /// `isTemplate = true` + `Image(nsImage:).renderingMode(.template)`
    /// lets macOS apply its standard menu-bar tinting (white on dark
    /// menu bar, black on light, dim when inactive). The arc colour
    /// reads correctly in every appearance, at the cost of losing
    /// the coral dot's brand colour at this tiny size — a tradeoff
    /// where consistency wins.
    @MainActor
    static let menuBarTemplateImage: NSImage = {
        // Renders the inline AurisMark verbatim via ImageRenderer —
        // no compensating transform. Both this NSImage and the
        // inline SwiftUI render flow from the same `AurisMark.body`,
        // so any future geometry change propagates to both sites
        // predictably.
        //
        // (Earlier revisions of this code applied .scaleEffect(x:-1)
        // here, on the theory that ImageRenderer's NSImage output
        // was horizontally flipped relative to the SwiftUI render.
        // That was a misdiagnosis: the scaleEffect was the entire
        // mirror, not a compensation. After the AurisMark body was
        // corrected to match the brand master orientation, removing
        // it kept the menu bar consistent with the inline mark.)
        let renderer = ImageRenderer(content: AurisMark(size: 18))
        renderer.scale = NSScreen.main?.backingScaleFactor ?? 2
        let image = renderer.nsImage ?? NSImage(size: NSSize(width: 18, height: 18))
        image.isTemplate = true
        return image
    }()
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

    /// User-facing marketing version. For CI-built bundles (nightly +
    /// tagged) this is `0.1.0-<sha7>` or `0.4.2`-style; for the
    /// `swift run` development path Info.plist isn't populated so we
    /// degrade to "dev" rather than show a bare "?".
    private var appVersion: String {
        let info = Bundle.main.infoDictionary
        return (info?["CFBundleShortVersionString"] as? String) ?? "dev"
    }

    /// Map an Auth0 `sub` to a human-readable identity provider label.
    /// `sub` is documented as `<connection>|<user-id>`; the connection
    /// prefix names the social/database provider. Unknown providers
    /// fall back to a capitalized version of the prefix so we don't
    /// claim more knowledge than we have.
    private func providerLabel(forSub sub: String) -> String {
        let prefix = sub.split(separator: "|", maxSplits: 1).first.map(String.init) ?? sub
        switch prefix {
        case "auth0": return "Username/password"
        case "google-oauth2": return "Google"
        case "apple": return "Apple"
        case "github": return "GitHub"
        case "facebook": return "Facebook"
        case "linkedin": return "LinkedIn"
        case "windowslive": return "Microsoft"
        case "twitter": return "Twitter/X"
        case "email": return "Email link"
        case "sms": return "SMS"
        default: return prefix.capitalized
        }
    }

    /// Return the user-id portion of an Auth0 `sub` (the part after
    /// `|`). Falls back to the full sub if no separator is present —
    /// shouldn't happen with real Auth0 subs but defends against
    /// unexpected token shapes.
    private func userIdTail(forSub sub: String) -> String {
        let parts = sub.split(separator: "|", maxSplits: 1)
        return parts.count == 2 ? String(parts[1]) : sub
    }

    var body: some View {
        SettingsTabShell(
            title: "Account",
            description: "Signed-in identity and Mac overlay appearance. Auris \(appVersion)."
        ) {
            Form {
                Section {
                if let id = model.auth0.identity {
                    VStack(alignment: .leading, spacing: 8) {
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
                        // Sign-in method + user ID. Auth0's `sub` claim
                        // is shaped `<provider>|<id>` (e.g.,
                        // "google-oauth2|123456789"); split to surface
                        // each piece on its own row so users can
                        // recognize which identity they're signed in
                        // with at a glance.
                        Divider()
                        HStack(spacing: 8) {
                            Text("Sign-in method")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Text(providerLabel(forSub: id.sub))
                                .font(.caption)
                                .foregroundStyle(.primary)
                        }
                        HStack(spacing: 8) {
                            Text("User ID")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Text(userIdTail(forSub: id.sub))
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.primary)
                                .textSelection(.enabled)
                                .lineLimit(1)
                                .truncationMode(.middle)
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

                Toggle(
                    "Show overlay when a meeting starts elsewhere",
                    isOn: $model.settings.overlayAutoShow
                )
            } header: {
                Text("Overlay")
            } footer: {
                Text(
                    "Theme switches the overlay between light and dark palettes; opacity drives the panel and chat-bubble translucency together so contents nest into the same level of see-through. The auto-show toggle controls whether the overlay appears on this Mac when a meeting starts on your phone or in the web app — closing the overlay during a meeting turns it off; flip it back on here."
                )
                .font(.footnote)
                .foregroundStyle(.secondary)
            }
            }
            .formStyle(.grouped)
            .scrollContentBackground(.hidden)
        }
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
    // Bounded poll that re-fetches the detail after a wrap-up retry so
    // the banner flips running → success/failed (and the new actions /
    // open questions appear) without the user hitting reload. Cancelled
    // when the selection changes or a new retry starts.
    @State private var wrapUpPoll: Task<Void, Never>?

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
            // A different meeting (or none) — stop polling the old one.
            wrapUpPoll?.cancel()
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
        // Match the detail pane's background so the two halves of the
        // split read as one continuous surface. Previously this used
        // `.underPageBackgroundColor` (a system "recessed" tone), which
        // worked when the detail pane was an elevated card — but the
        // detail pane is `Color.clear` here, so the sidebar ended up
        // looking recessed against an even-lighter content surface,
        // i.e. visually below the content instead of beside it. The
        // NavigationSplitView divider provides the master/detail
        // separation; we don't need a tone difference too.
        .background(SettingsTheme.background)
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
            MeetingDetailView(
                detail: detail,
                model: model,
                onRetryWrapUp: { await retryWrapUp() }
            )
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

    /// POST /meetings/:id/retry-wrap-up for the selected meeting. On
    /// success the server returns the refreshed detail (now `running`),
    /// which we splice in so the banner flips immediately; then we poll
    /// until the extractor finishes (or fails) so the new actions /
    /// open questions appear without a manual reload. Returns nil on
    /// success or an error message for the banner to surface.
    private func retryWrapUp() async -> String? {
        guard let id = selectedId else { return nil }
        guard let api = await makeAPI() else {
            return "Server URL is invalid; check the Account tab."
        }
        wrapUpPoll?.cancel()
        do {
            let refreshed = try await api.retryWrapUp(id: id)
            if selectedId == id { detail = refreshed }
            startWrapUpPoll(id: id)
            return nil
        } catch {
            return (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// Re-fetch the detail every few seconds while the wrap-up status
    /// is `running`, up to a bound so a wedged worker can't poll
    /// forever (the user can always hit reload). Stops as soon as the
    /// status leaves `running` or the user navigates away.
    private func startWrapUpPoll(id: String) {
        wrapUpPoll = Task { @MainActor in
            for _ in 0..<20 {
                try? await Task.sleep(for: .seconds(3))
                if Task.isCancelled || selectedId != id { return }
                guard let api = await makeAPI(),
                      let refreshed = try? await api.detail(id: id)
                else { continue }
                if selectedId != id { return }
                detail = refreshed
                if refreshed.wrapUpStatus != "running" { return }
            }
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
    /// Re-run the wrap-up extractor for this meeting. Returns nil on
    /// success, or an error message to surface under the banner. The
    /// parent (`MeetingsTab`) owns the actual POST + detail refresh +
    /// polling, since it holds the mutable `detail`.
    var onRetryWrapUp: () async -> String?

    @State private var descriptionExpanded = false
    @State private var pdfState: PdfDownloadState = .idle
    // Rename: `titleOverride` holds the optimistic name (detail is a
    // `let`, so we shadow its title locally); reverted on a failed
    // PATCH. The list row updates on the next reload.
    @State private var showRename = false
    @State private var titleDraft = ""
    @State private var titleOverride: String?
    @State private var renameError: String?
    // Wrap-up retry: disable the button + show a spinner while the
    // POST is in flight; surface a transient error beside it.
    @State private var retryingWrapUp = false
    @State private var wrapUpError: String?
    // Header "Regenerate" action confirmation (it overwrites the
    // existing wrap-up).
    @State private var showRegenerateConfirm = false

    private enum PdfDownloadState: Equatable {
        case idle, working, failed
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(
                        titleOverride
                            ?? pickMeetingTitle(
                                description: detail.description, metadata: detail.metadata)
                    )
                    .font(.title2)
                    .fontWeight(.semibold)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    renameButton
                    if detail.endedAt != nil {
                        regenerateWrapUpButton
                    }
                    downloadPdfButton
                }
                .alert("Rename meeting", isPresented: $showRename) {
                    TextField("Title", text: $titleDraft)
                    Button("Rename") { Task { await rename() } }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text("Give this meeting a name.")
                }
                .confirmationDialog(
                    "Regenerate wrap-up?",
                    isPresented: $showRegenerateConfirm,
                    titleVisibility: .visible
                ) {
                    Button("Regenerate") {
                        Task {
                            retryingWrapUp = true
                            wrapUpError = await onRetryWrapUp()
                            retryingWrapUp = false
                        }
                    }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text(
                        "Re-runs the summary, highlights, actions, and open questions from the transcript, replacing the current ones. This can take a little while."
                    )
                }

                timingRow

                // Wrap-up extractor status banner. Renders when the
                // server reports the post-meeting extractor either
                // failed (LLM error, quota, network blip — actions /
                // open questions for this meeting may be incomplete)
                // or is still running (refresh to see results). The
                // `success` and `nil` (legacy meeting) states render
                // nothing.
                if let status = detail.wrapUpStatus,
                   status == "failed" || status == "running"
                {
                    wrapUpBanner(status: status)
                }

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

                let itemsByMode = detail.itemsByMode ?? [:]

                // Summary leads the readout — it's the narrative
                // takeaway, so it renders before moments and the other
                // structured sections. No per-item timestamp marker:
                // the final summary is a single prose blob, not a
                // timeline of beats.
                if let summary = itemsByMode["summary"], !summary.isEmpty {
                    summaryBlock(summary)
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
        .alert(
            "Rename failed",
            isPresented: Binding(
                get: { renameError != nil },
                set: { if !$0 { renameError = nil } }
            )
        ) {
            Button("OK", role: .cancel) { renameError = nil }
        } message: {
            Text(renameError ?? "")
        }
    }

    /// Pencil button that seeds the rename field with the current
    /// title (blank for the "Untitled meeting" placeholder so the user
    /// types into an empty field) and presents the rename alert.
    /// Regenerate the post-meeting wrap-up (summary + highlights +
    /// actions + open questions) for any finished meeting, not just a
    /// failed one. Confirms first since it overwrites. While the
    /// request is in flight the label shows a spinner; the resulting
    /// `running` status then drives the banner + the parent's poll.
    private var regenerateWrapUpButton: some View {
        Button {
            showRegenerateConfirm = true
        } label: {
            HStack(spacing: 4) {
                if retryingWrapUp {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: "arrow.clockwise").imageScale(.small)
                }
                Text(retryingWrapUp ? "regenerating…" : "Wrap-up")
                    .font(.system(.caption, design: .monospaced))
            }
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(retryingWrapUp)
        .help("Regenerate this meeting's summary, highlights, actions, and open questions")
    }

    private var renameButton: some View {
        Button {
            let current =
                titleOverride
                ?? pickMeetingTitle(description: detail.description, metadata: detail.metadata)
            titleDraft = current == "Untitled meeting" ? "" : current
            showRename = true
        } label: {
            Image(systemName: "pencil").imageScale(.small)
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .help("Rename this meeting")
    }

    /// Optimistically apply the new title, then PATCH it. Revert +
    /// surface an error alert if the server rejects it.
    @MainActor
    private func rename() async {
        let next = titleDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !next.isEmpty else { return }
        let previous = titleOverride
        titleOverride = next
        do {
            let api = try await model.makeMeetingsAPI()
            try await api.rename(id: detail.id, title: next)
        } catch {
            titleOverride = previous
            renameError =
                (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// "↓ PDF" button + NSSavePanel handoff. We use AppKit's save
    /// panel directly because SwiftUI's `.fileExporter` requires a
    /// `FileDocument` shape (it'd serialize to bytes synchronously
    /// off the main actor), and the PDF render lives on the server
    /// behind an authenticated GET — easier to fetch the bytes
    /// imperatively and write them at the user-chosen path.
    private var downloadPdfButton: some View {
        Button {
            Task { await downloadPdf() }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: pdfState == .working ? "arrow.down.circle" : "arrow.down.circle")
                    .imageScale(.small)
                Text(pdfState == .working ? "generating…" : pdfState == .failed ? "failed" : "PDF")
                    .font(.system(.caption, design: .monospaced))
            }
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(pdfState == .working)
    }

    @MainActor
    private func downloadPdf() async {
        pdfState = .working
        do {
            let api = try await model.makeMeetingsAPI()
            let data = try await api.exportPdf(meetingId: detail.id)
            // NSSavePanel.runModal blocks until the user picks. We
            // wrap in a MainActor await so callers stay async-clean;
            // the panel itself is synchronous AppKit.
            let panel = NSSavePanel()
            panel.allowedContentTypes = [.pdf]
            panel.nameFieldStringValue = "meeting-\(detail.id).pdf"
            panel.canCreateDirectories = true
            let resp = panel.runModal()
            guard resp == .OK, let url = panel.url else {
                pdfState = .idle
                return
            }
            try data.write(to: url)
            pdfState = .idle
        } catch {
            print("[MeetingDetailView] pdf download failed:", error)
            pdfState = .failed
            // Auto-revert the "failed" label after a couple of
            // seconds so the user can retry without a manual reset.
            try? await Task.sleep(nanoseconds: 2_500_000_000)
            pdfState = .idle
        }
    }

    /// The final meeting summary, rendered as plain prose. Unlike the
    /// structured modes, summary items carry no useful timestamp (the
    /// narrative spans the whole meeting), so we skip the `[mm:ss] ▸`
    /// marker that `DetailItemRow` adds and just print the text.
    @ViewBuilder
    private func summaryBlock(_ items: [Item]) -> some View {
        Text("Summary").font(.headline)
        VStack(alignment: .leading, spacing: 8) {
            ForEach(items) { item in
                Text(item.text)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
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
        ("assist", "Assist"),
        ("highlights", "Highlights"),
        ("actions", "Actions"),
        ("open_questions", "Open questions"),
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

    /// Banner shown when the post-meeting wrap-up extractor either
    /// failed or is still running. Two colour variants matching the
    /// PWA banner palette: red for failed (action items / open
    /// questions may be incomplete), yellow for running (refresh
    /// to see results when ready).
    @ViewBuilder
    private func wrapUpBanner(status: String) -> some View {
        let isFailed = status == "failed"
        let label = isFailed
            ? "Wrap-up extraction failed — actions + open questions for this meeting may be incomplete."
            : "Wrap-up extraction still running — refresh to see actions + open questions when ready."
        let bg = isFailed
            ? Color.red.opacity(0.12)
            : Color.orange.opacity(0.12)
        let fg = isFailed ? Color.red : Color.orange
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: isFailed ? "exclamationmark.triangle.fill" : "clock.fill")
                    .foregroundStyle(fg)
                Text(label)
                    .font(.callout)
                    .foregroundStyle(.primary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            // Retry is only offered for `failed`. `running` is already
            // in flight — the parent polls it to completion.
            if isFailed {
                HStack(spacing: 8) {
                    Button {
                        Task {
                            retryingWrapUp = true
                            wrapUpError = await onRetryWrapUp()
                            retryingWrapUp = false
                        }
                    } label: {
                        HStack(spacing: 4) {
                            if retryingWrapUp {
                                ProgressView().controlSize(.small)
                            }
                            Text(retryingWrapUp ? "Re-running…" : "Try again")
                        }
                    }
                    .disabled(retryingWrapUp)
                    if let wrapUpError {
                        Text(wrapUpError)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                }
            }
        }
        .padding(10)
        .background(bg)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .strokeBorder(fg.opacity(0.4), lineWidth: 1)
        )
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
        case "assist":
            return meta.assistType.flatMap { s in
                s.isEmpty ? nil : s.uppercased()
            } ?? ""
        default:
            return ""
        }
    }

    /// Emoji prefix for assist-mode items — mirrors the live
    /// overlay's `assistTypeGlyph` in `MeetingOverlayView.swift` so
    /// the finalized detail view's at-a-glance type chip feels
    /// familiar. Empty for non-assist modes.
    private var assistTypeGlyph: String {
        guard mode == "assist", let raw = item.meta?.assistType else { return "" }
        switch raw {
        case "definition": return "📖  "
        case "question": return "❓  "
        case "memory": return "🧠  "
        case "coach": return "💡  "
        default: return ""
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
                Text(assistTypeGlyph + item.text)
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
    /// Screenshots this message rode (persisted in meta by the server).
    private var attachmentCount: Int { item.meta?.attachmentIds?.count ?? 0 }

    var body: some View {
        HStack {
            if isUser { Spacer(minLength: 32) }
            VStack(alignment: .trailing, spacing: 4) {
                Text(item.text)
                if attachmentCount > 0 {
                    HStack(spacing: 3) {
                        Image(systemName: "photo")
                        if attachmentCount > 1 { Text("\(attachmentCount)") }
                    }
                    .font(.system(size: 10, weight: .semibold))
                    .opacity(0.85)
                }
            }
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
        SettingsTabShell(
            title: "Artifacts",
            description: "Files attached to your meetings. Documents and images give the meeting agents context they wouldn't otherwise pick up from audio alone.",
            action: {
                HStack(spacing: 8) {
                    if uploading {
                        ProgressView().controlSize(.small)
                    }
                    Button {
                        Task { await pickAndUpload() }
                    } label: {
                        Label("Upload…", systemImage: "arrow.up.doc")
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(SettingsTheme.blue)
                    .disabled(uploading)
                    Button {
                        Task { await reloadList() }
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .help("Reload")
                    .disabled(listLoading)
                }
            }
        ) {
            list
        }
        .task { await reloadList() }
        .onDisappear { refreshTimer?.invalidate() }
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
                        ArtifactRow(
                            artifact: a,
                            onDelete: { Task { await deleteArtifact(id: a.id) } },
                            onRetry: { Task { await retrySummary(id: a.id) } }
                        )
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

    /// POST /artifacts/:id/retry-summary. On success the server flips
    /// `summary_status` back to `pending` and re-queues the worker;
    /// we splice the updated row into the local list and kick the
    /// pending-poll so the row flips to `done` (or `failed` again)
    /// without needing the reload button.
    private func retrySummary(id: String) async {
        guard let api = await makeAPI() else { return }
        do {
            let updated = try await api.retrySummary(id: id)
            if let i = artifacts.firstIndex(where: { $0.id == id }) {
                artifacts[i] = updated
            }
            scheduleAutoRefresh()
        } catch {
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
    let onRetry: () -> Void
    @State private var confirmDelete = false
    @State private var isExpanded = false
    /// Disables the retry button while the request is in flight so a
    /// double-tap doesn't fire two POSTs. The server's `failed → pending`
    /// state guard would reject the second anyway, but disabling
    /// prevents the user from seeing a spurious 400.
    @State private var retrying = false

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
                        HStack(spacing: 6) {
                            Text("Summary failed — server logs may have more.")
                                .font(.caption)
                                .foregroundStyle(.red)
                            Button {
                                retrying = true
                                onRetry()
                            } label: {
                                Label("Retry", systemImage: "arrow.clockwise")
                                    .labelStyle(.titleAndIcon)
                                    .font(.caption)
                            }
                            .buttonStyle(.borderless)
                            .controlSize(.small)
                            .disabled(retrying)
                            .help("Re-run the summary worker for this artifact")
                        }
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

/// Settings-window color tokens.
///
/// `background` and `sidebar` are intentionally NOT explicit Colors —
/// instead, callers should `.background(Color.clear)` (or omit the
/// modifier) so SwiftUI inherits the window's chrome, which Cocoa
/// keeps in sync with the system appearance automatically.
///
/// The "raised" surfaces (cards) use SwiftUI materials, which are
/// guaranteed-dynamic and also give a subtle blur — closer to native
/// macOS feel than a flat hex color.
enum SettingsTheme {
    /// Use `.background(SettingsTheme.background)` to clear any
    /// inherited tint and fall through to the window bg. Effectively
    /// a no-op surface; relies on the NSWindow content view.
    static let background = Color.clear
    /// Subtle recessed pane. `quaternary` reads as "behind" the
    /// default surface and adapts to both modes.
    static let sidebar = Color(nsColor: .underPageBackgroundColor)
    /// Raised cards use a material rather than a Color so they
    /// adapt + get the standard macOS blur. Apply with
    /// `.background(SettingsTheme.cardMaterial)`.
    static let cardMaterial: Material = .regularMaterial
    /// Legacy alias kept so existing `.background(SettingsTheme.card)`
    /// call sites compile — same visual as `cardMaterial` would
    /// produce; defined as the closest equivalent flat color via
    /// a non-dynamic-bridging path. New call sites should prefer
    /// `cardMaterial`.
    static let card = Color.clear
    /// Hairline divider between cards / between rows. SwiftUI's
    /// secondary content already follows appearance so this is the
    /// safest mapping.
    static let border = Color.secondary.opacity(0.3)
    /// Brand accent. Kept as an explicit hex (not `Color.accentColor`)
    /// so the sign-in button + slider tint always read as Auris coral
    /// regardless of the user's system accent setting.
    ///
    /// Name `.blue` is legacy — kept so existing `SettingsTheme.blue`
    /// call sites don't have to be touched. The value is coral.
    static let blue = Color(hex: 0xD97757)
}

private extension Color {
    init(hex: UInt32) {
        let r = Double((hex >> 16) & 0xff) / 255.0
        let g = Double((hex >> 8) & 0xff) / 255.0
        let b = Double(hex & 0xff) / 255.0
        self.init(red: r, green: g, blue: b)
    }
}
