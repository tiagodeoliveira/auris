// MeetingOverlayView.swift
// Floating always-on-top meeting overlay.
// Shared visual direction with the PWA: light glass panel, blue primary
// action, amber moment action, red destructive action, and high-contrast
// transcript text. The window keeps a stable wide footprint across compose,
// starting, and live states so the meeting UI doesn't jump when capture starts.

import AppKit
import SwiftUI

struct MeetingOverlayView: View {
    @Bindable var model: AppModel
    @Environment(\.dismissWindow) private var dismissWindow

    @State private var mode: OverlayMode
    @State private var description: String = ""
    @State private var addingMetadata = false
    @FocusState private var descriptionFocused: Bool

    /// Owns the compose-panel mic button's lifecycle. Created lazily
    /// on first use so we don't spin up a mic / WS for users who
    /// never click it.
    @State private var dictation: DictationController? = nil

    /// IDs of items the user has expanded to reveal `detail`.
    /// Persists across tab switches so re-selecting a tab restores
    /// what was expanded; cleared when a new meeting begins (the
    /// IDs change anyway, so the set goes stale naturally).
    @State private var expandedItemIds: Set<String> = []

    /// Library artifacts selected for attach during this compose
    /// session. Flushed into `AppModel.pendingArtifactAttachments`
    /// on Start; the WS event handler picks them up when the
    /// meeting transitions to active.
    @State private var selectedArtifacts: [Artifact] = []
    @State private var showArtifactPicker = false

    /// Separate sheet for the live-meeting attach flow (overlay
    /// `…` menu). Keeps onConfirm semantics distinct from the
    /// compose-time picker which stages into selectedArtifacts.
    @State private var showLiveArtifactPicker = false

    init(model: AppModel) {
        self.model = model
        _mode = State(initialValue: model.isMeetingActive ? .live : .compose)
    }

    var body: some View {
        Group {
            switch mode {
            case .compose:
                composePanel
            case .starting:
                startingPanel
            case .live:
                livePanel
            }
        }
        .padding(12)
        .frame(
            minWidth: mode.minWidth, idealWidth: mode.idealWidth, maxWidth: .infinity,
            minHeight: mode.minHeight, idealHeight: mode.idealHeight, maxHeight: .infinity)
        .fixedSize(horizontal: false, vertical: true)
        .background {
            RoundedRectangle(cornerRadius: 12)
                .fill(MCTheme.panel.opacity(0.94))
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .strokeBorder(MCTheme.border, lineWidth: 1)
                )
                .shadow(color: Color.black.opacity(0.18), radius: 22, y: 10)
        }
        .foregroundStyle(MCTheme.text)
        .ignoresSafeArea()
        .background(WindowAccessor { window in
            window.level = .floating
            window.collectionBehavior.insert(.canJoinAllSpaces)
            window.isOpaque = false
            window.backgroundColor = .clear
            window.hasShadow = true
            window.isMovableByWindowBackground = true
            // Borderless matters here: hiddenTitleBar still reserves
            // titlebar real estate on some macOS window configurations,
            // which lets desktop wallpaper show through as a blue strip.
            window.styleMask = [.borderless, .resizable, .fullSizeContentView]
            window.standardWindowButton(.closeButton)?.isHidden = true
            window.standardWindowButton(.miniaturizeButton)?.isHidden = true
            window.standardWindowButton(.zoomButton)?.isHidden = true
            window.minSize = NSSize(width: mode.minWidth, height: mode.minHeight)
        })
        .onChange(of: model.isMeetingActive) { _, active in
            if active {
                mode = .live
            } else if mode == .live || mode == .starting {
                addingMetadata = false
                dismissWindow(id: "meeting-overlay")
            }
        }
        .onAppear {
            mode = model.isMeetingActive ? .live : .compose
            addingMetadata = false
            descriptionFocused = mode == .compose
            model.isOverlayVisible = true
        }
        .onDisappear {
            model.isOverlayVisible = false
        }
    }

    private var composePanel: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "record.circle")
                    .font(.system(size: 15))
                    .foregroundStyle(MCTheme.danger)

                Text("Start meeting")
                    .font(.system(size: 17, weight: .semibold))

                Spacer()

                Button {
                    dismissWindow(id: "meeting-overlay")
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 16))
                        .foregroundStyle(MCTheme.muted)
                }
                .buttonStyle(.plain)
                .help("Cancel")
            }

            ZStack(alignment: .topLeading) {
                if description.isEmpty {
                    Text("What's this meeting about? (optional)")
                        .foregroundStyle(MCTheme.subtle)
                        .padding(.top, 10)
                        .padding(.leading, 10)
                        .allowsHitTesting(false)
                }

                TextEditor(text: $description)
                    .font(.system(size: 15))
                    .foregroundStyle(MCTheme.text)
                    .scrollContentBackground(.hidden)
                    .focused($descriptionFocused)
                    .frame(minHeight: 64)
                    // While dictation is active the controller owns
                    // the field — disable native editing so the user
                    // doesn't race with incoming server frames.
                    .disabled(dictation?.isLocked == true)

                // Mic button — bottom-right corner of the input,
                // small enough to not crowd the placeholder text.
                VStack {
                    Spacer()
                    HStack {
                        Spacer()
                        Button {
                            ensureDictation().toggle()
                        } label: {
                            Image(systemName: micIcon)
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(micTint)
                                .frame(width: 26, height: 26)
                                .background(
                                    Circle().fill(MCTheme.panel.opacity(0.85))
                                )
                                .overlay(
                                    Circle().strokeBorder(
                                        dictation?.isLocked == true
                                            ? MCTheme.danger : MCTheme.border)
                                )
                        }
                        .buttonStyle(.plain)
                        .help(dictation?.isLocked == true
                            ? "Stop dictation" : "Dictate description")
                    }
                }
                .padding(6)
            }
            .padding(6)
            .background(MCTheme.input)
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(MCTheme.border)
            )
            // Keep the description field in sync with the controller
            // while dictating — the controller writes to its own
            // `text` property; this mirrors back to the @State the
            // TextEditor binds to.
            .onChange(of: dictation?.text ?? "") { _, newValue in
                if dictation?.isLocked == true {
                    description = newValue
                }
            }

            MetadataChipEditor(
                metadata: model.metadata,
                addingMetadata: $addingMetadata,
                setMetadata: { key, value in
                    Task { await model.setMetadata(key: key, value: value) }
                }
            )

            ArtifactChipStrip(
                attached: selectedArtifacts,
                onPick: { showArtifactPicker = true },
                onRemove: { id in
                    selectedArtifacts.removeAll { $0.id == id }
                }
            )

            HStack(spacing: 10) {
                Button {
                    Task { await model.extractMetadata(description: description) }
                } label: {
                    Label(
                        model.extractingMetadata ? "Extracting…" : "Extract tags",
                        systemImage: model.extractingMetadata ? "hourglass" : "tag"
                    )
                }
                .disabled(description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    || model.extractingMetadata)
                .buttonStyle(SecondaryPillButtonStyle())
                .help("Extract editable tags from the description")

                Spacer()

                if !model.canStartMeeting {
                    Text(notReadyHint)
                        .font(.caption)
                        .foregroundStyle(MCTheme.muted)
                }

                Button("Start") {
                    submitDescription()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canStartMeeting || model.extractingMetadata)
                .buttonStyle(PrimaryPillButtonStyle())
            }
        }
        .sheet(isPresented: $showArtifactPicker) {
            ArtifactPickerSheet(
                model: model,
                alreadySelectedIds: Set(selectedArtifacts.map { $0.id }),
                onConfirm: { picked in
                    // Replace, not append: the picker shows current
                    // selection state (checked rows for already-
                    // attached items), so the result is the new
                    // canonical set.
                    selectedArtifacts = picked
                }
            )
        }
    }

    private var startingPanel: some View {
        HStack(spacing: 16) {
            MicActivityIcon(peak: combinedPeak, isLive: true)
                .frame(width: 38, height: 48)

            VStack(alignment: .leading, spacing: 4) {
                Text("Starting meeting")
                    .font(.system(size: 17, weight: .semibold))
                Text("Opening audio stream…")
                    .font(.caption)
                    .foregroundStyle(MCTheme.muted)
            }

            Spacer()

            ProgressView()
                .controlSize(.small)
        }
    }

    private var livePanel: some View {
        ZStack(alignment: .topTrailing) {
            HStack(alignment: .top, spacing: 14) {
                statusColumn
                    .frame(width: 92)

                Rectangle()
                    .fill(MCTheme.border)
                    .frame(width: 1)

                modeColumn
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }

            VStack(alignment: .trailing, spacing: 8) {
                actionCluster

                if let status = model.momentStatus, !status.isEmpty {
                    Text(status)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(MCTheme.amberText)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 7)
                        .background(MCTheme.amberSoft)
                        .clipShape(Capsule())
                        .overlay(Capsule().strokeBorder(MCTheme.amberBorder))
                        .transition(.opacity.combined(with: .move(edge: .top)))
                }
            }
            .padding(.top, -2)
        }
    }

    /// Left column: status only. Actions live in the top-right cluster
    /// so the mic reads as meeting state rather than another command.
    private var statusColumn: some View {
        VStack(alignment: .center, spacing: 0) {
            HStack(spacing: 4) {
                Image(systemName: model.audioToBackendPaused ? "pause.circle.fill" : "record.circle.fill")
                    .foregroundStyle(model.audioToBackendPaused ? MCTheme.amber : MCTheme.danger)
                    .font(.system(size: 10))
                Text(model.audioToBackendPaused ? "Muted" : "Live")
                    .font(.caption2)
                    .fontWeight(.semibold)
                    .foregroundStyle(MCTheme.text)
            }
            .frame(maxWidth: .infinity)

            Spacer(minLength: 0)

            Button {
                model.toggleBackendAudio()
            } label: {
                MicActivityIcon(
                    peak: model.audioToBackendPaused ? 0 : combinedPeak,
                    isLive: model.isMeetingActive && !model.audioToBackendPaused
                )
                .frame(width: 34, height: 42)
            }
            .buttonStyle(.plain)
            .disabled(!model.canToggleBackendAudio)
            .frame(maxWidth: .infinity)
            .help(model.audioToBackendPaused ? "Resume backend audio" : "Pause backend audio")

            Spacer(minLength: 0)
        }
        .frame(maxHeight: .infinity)
    }

    private var actionCluster: some View {
        HStack(spacing: 7) {
            Button {
                Task { await model.markMoment() }
            } label: {
                Image(systemName: "bookmark.circle.fill")
                    .font(.system(size: 20))
            }
            .buttonStyle(IconCircleButtonStyle(tint: MCTheme.amber))
            .keyboardShortcut("m", modifiers: [.command, .shift])
            .disabled(!model.isMeetingActive)
            .help("Mark moment (⇧⌘M)")

            Menu {
                Button {
                    showLiveArtifactPicker = true
                } label: {
                    Label("Attach artifact…", systemImage: "doc.text")
                }
                .disabled(!model.auth0.isSignedIn)
            } label: {
                Image(systemName: "ellipsis.circle.fill")
                    .font(.system(size: 20))
                    .foregroundStyle(MCTheme.muted)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("More actions")

            Button {
                Task { await model.stopMeeting() }
            } label: {
                Image(systemName: "stop.circle.fill")
                    .font(.system(size: 20))
            }
            .buttonStyle(IconCircleButtonStyle(tint: MCTheme.danger))
            .help("Stop meeting")

        }
        .padding(4)
        .background(MCTheme.panel.opacity(0.82))
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(MCTheme.border))
        .sheet(isPresented: $showLiveArtifactPicker) {
            ArtifactPickerSheet(
                model: model,
                // Mid-meeting picker doesn't pre-check anything.
                // The user picks what to add; attach is idempotent
                // server-side so re-attaching is a no-op.
                alreadySelectedIds: [],
                onConfirm: { picked in
                    let ids = picked.map { $0.id }
                    Task { await model.attachArtifactsToCurrentMeeting(ids: ids) }
                }
            )
        }
    }

    /// Right column: mode-tabs row over a scrollable items list.
    /// Mirrors `packages/pwa/src/ui/mode-tabs.ts` — same short
    /// uppercase labels, same active-state semantics. The items
    /// area shows `itemsByMode[currentMode]` plus, in transcript
    /// mode only, the dim trailing `transcriptInterim` line.
    private var modeColumn: some View {
        VStack(alignment: .leading, spacing: 6) {
            modeTabs
            itemsList
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var modeTabs: some View {
        HStack(spacing: 4) {
            ForEach(model.availableModes) { mode in
                let isActive = mode.id == model.currentMode
                Button {
                    Task { await model.setMode(mode.id) }
                } label: {
                    Text(Self.shortLabel(for: mode))
                        .font(.system(size: 10, weight: isActive ? .bold : .semibold))
                        .tracking(0.6)
                        .foregroundStyle(isActive ? MCTheme.blue : MCTheme.muted)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background {
                            if isActive {
                                Capsule().fill(MCTheme.blueSoft)
                            }
                        }
                }
                .buttonStyle(.plain)
            }
            Spacer(minLength: 0)
        }
    }

    private var itemsList: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    let items = model.itemsByMode[model.currentMode] ?? []
                    let interim = model.currentMode == "transcript" ? model.transcriptInterim : ""

                    if items.isEmpty, interim.isEmpty {
                        Text(emptyHint)
                            .foregroundStyle(MCTheme.muted)
                    } else {
                        ForEach(items) { item in
                            ItemRow(
                                item: item,
                                isExpanded: expandedItemIds.contains(item.id),
                                onToggle: { toggleExpanded(item.id) }
                            )
                        }
                        if !interim.isEmpty {
                            Text(interim)
                                .foregroundStyle(MCTheme.muted)
                                .italic()
                        }
                    }
                    Color.clear.frame(height: 1).id("itemsEnd")
                }
                .font(.system(size: 16, weight: .regular))
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .textSelection(.enabled)
                .padding(.trailing, 48)
            }
            .scrollIndicators(.hidden)
            .onChange(of: model.transcriptInterim) { _, _ in
                guard model.currentMode == "transcript" else { return }
                withAnimation(.linear(duration: 0.1)) {
                    proxy.scrollTo("itemsEnd", anchor: .bottom)
                }
            }
            .onChange(of: itemsCountForCurrentMode) { _, _ in
                withAnimation(.linear(duration: 0.1)) {
                    proxy.scrollTo("itemsEnd", anchor: .bottom)
                }
            }
            .onChange(of: model.currentMode) { _, _ in
                // Tab switch — jump straight to the bottom of the
                // newly-selected list without animating; smoother
                // than scrolling through unrelated content.
                proxy.scrollTo("itemsEnd", anchor: .bottom)
            }
        }
    }

    /// Toggle an item's expanded state. Called from `ItemRow` via
    /// the chevron tap.
    private func toggleExpanded(_ id: String) {
        if expandedItemIds.contains(id) {
            expandedItemIds.remove(id)
        } else {
            expandedItemIds.insert(id)
        }
    }

    /// Short uppercase tab labels. Falls back to the server-provided
    /// label when we haven't seen the mode id before.
    private static func shortLabel(for mode: ModeOption) -> String {
        switch mode.id {
        case "transcript": return "TRANSCRIPT"
        case "highlights": return "HIGHLIGHTS"
        case "actions": return "ACTIONS"
        case "open_questions": return "QUESTIONS"
        default: return mode.label.uppercased()
        }
    }

    /// Tracked separately so `.onChange` can fire on the active
    /// mode's item count without referencing `itemsByMode` directly
    /// (dictionary equality on every render is wasteful).
    private var itemsCountForCurrentMode: Int {
        model.itemsByMode[model.currentMode]?.count ?? 0
    }

    private var emptyHint: String {
        switch model.currentMode {
        case "transcript": return "(listening…)"
        case "highlights": return "(no highlights yet)"
        case "actions": return "(no action items yet)"
        case "open_questions": return "(no open questions yet)"
        default: return "(no items yet)"
        }
    }

    /// Combined per-source peak. Drives the mic-level meter; the
    /// per-source split was a debugging affordance.
    private var combinedPeak: Float {
        max(model.audioCapture.currentSysPeak, model.audioCapture.currentMicPeak)
    }

    private func submitDescription() {
        let trimmed = description.trimmingCharacters(in: .whitespacesAndNewlines)
        let payload: String? = trimmed.isEmpty ? nil : trimmed
        // Hand the picked artifact ids to AppModel; the WS event
        // handler will fire `attach` for each once the server
        // transitions the meeting to active. Stage *before* sending
        // start_meeting so the active event handler always sees them.
        model.setPendingArtifactAttachments(selectedArtifacts.map { $0.id })
        mode = .starting
        descriptionFocused = false
        Task {
            await model.startMeeting(description: payload)
            if model.isMeetingActive {
                mode = .live
                selectedArtifacts = []
            } else {
                mode = .compose
                descriptionFocused = true
            }
        }
    }

    /// One-line nudge when Start is disabled.
    private var notReadyHint: String {
        if model.webSocket.state != .connected { return "Not connected" }
        if !model.permissionMonitor.allGranted { return "Permissions not granted" }
        if model.audioCapture.state != .stopped { return "Audio capture busy" }
        return ""
    }

    /// Lazily build the controller on first mic-button click. Keeps
    /// the WS / mic capture out of the picture for users who never
    /// dictate, and lets us capture the AppModel reference here
    /// without needing it at view-init time.
    private func ensureDictation() -> DictationController {
        if let d = dictation { return d }
        let d = DictationController(
            serverURL: model.settings.serverURL,
            tokenProvider: { [model] in try await model.auth0.getAccessToken() }
        )
        // Adopt whatever's currently in the description field as the
        // dictation prefix so we append rather than wipe.
        d.text = description
        dictation = d
        return d
    }

    private var micIcon: String {
        switch dictation?.state {
        case .listening, .starting: "mic.fill"
        default: "mic"
        }
    }

    private var micTint: Color {
        switch dictation?.state {
        case .listening: MCTheme.danger
        case .starting, .stopping: MCTheme.amber
        case .error: MCTheme.danger
        default: MCTheme.muted
        }
    }
}

private enum OverlayMode: Equatable {
    case compose
    case starting
    case live

    var minWidth: CGFloat {
        switch self {
        case .compose: 900
        case .starting: 520
        case .live: 900
        }
    }

    var idealWidth: CGFloat {
        switch self {
        case .compose: 1120
        case .starting: 620
        case .live: 1180
        }
    }

    var minHeight: CGFloat {
        switch self {
        case .compose: 245
        case .starting: 96
        case .live: 152
        }
    }

    var idealHeight: CGFloat {
        switch self {
        case .compose: 275
        case .starting: 112
        case .live: 174
        }
    }
}

private enum MCTheme {
    static let panel = Color(hex: 0xF7FAFE)
    static let panelElevated = Color(hex: 0xFFFFFF)
    static let input = Color(hex: 0xEEF4FA)
    static let border = Color(hex: 0xD5DEE9)
    static let text = Color(hex: 0x17212E)
    static let muted = Color(hex: 0x647386)
    static let subtle = Color(hex: 0x96A3B4)
    static let blue = Color(hex: 0x2563EB)
    static let blueSoft = Color(hex: 0xDBEAFE)
    static let amber = Color(hex: 0xF2B705)
    static let amberText = Color(hex: 0x765A00)
    static let amberSoft = Color(hex: 0xFFF5C7)
    static let amberBorder = Color(hex: 0xF5D45F)
    static let danger = Color(hex: 0xE5484D)
    static let dangerSoft = Color(hex: 0xFFE2E3)
    static let success = Color(hex: 0x2EA043)
}

private extension Color {
    init(hex: UInt32) {
        let r = Double((hex >> 16) & 0xff) / 255.0
        let g = Double((hex >> 8) & 0xff) / 255.0
        let b = Double(hex & 0xff) / 255.0
        self.init(red: r, green: g, blue: b)
    }
}

private struct PrimaryPillButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 18)
            .padding(.vertical, 8)
            .background(MCTheme.blue.opacity(configuration.isPressed ? 0.82 : 1))
            .clipShape(Capsule())
    }
}

private struct SecondaryPillButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(MCTheme.text)
            .padding(.horizontal, 14)
            .padding(.vertical, 7)
            .background(configuration.isPressed ? MCTheme.input.opacity(0.7) : MCTheme.panelElevated)
            .clipShape(Capsule())
            .overlay(Capsule().strokeBorder(MCTheme.border))
    }
}

private struct IconCircleButtonStyle: ButtonStyle {
    let tint: Color

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(tint)
            .frame(width: 28, height: 28)
            .background(configuration.isPressed ? tint.opacity(0.18) : tint.opacity(0.10))
            .clipShape(Circle())
            .overlay(Circle().strokeBorder(tint.opacity(0.28)))
    }
}

/// One line of a mode's items list. Decorates the raw `text` with
/// a speaker chip (when `meta.speaker` is present) and a disclosure
/// chevron (when `detail` is present), expanding inline to show
/// `detail` when the chevron is clicked.
///
/// Text remains selectable via the surrounding `.textSelection`
/// modifier; only the chevron handles the toggle gesture, so it
/// doesn't fight with copy-paste.
private struct ItemRow: View {
    let item: Item
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let speaker = item.meta?.speaker, !speaker.isEmpty {
                    Text(speaker)
                        .font(.system(size: 10, weight: .semibold))
                        .tracking(0.3)
                        .foregroundStyle(MCTheme.muted)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background {
                            RoundedRectangle(cornerRadius: 3)
                                .fill(MCTheme.input)
                        }
                }

                Text(item.text)
                    .foregroundStyle(MCTheme.text)
                    .frame(maxWidth: .infinity, alignment: .leading)

                if item.detail != nil {
                    Button(action: onToggle) {
                        Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(MCTheme.muted)
                            .frame(width: 14, height: 14)
                    }
                    .buttonStyle(.plain)
                    .help(isExpanded ? "Hide detail" : "Show detail")
                }
            }

            if isExpanded, let detail = item.detail, !detail.isEmpty {
                Text(detail)
                    .font(.callout)
                    .foregroundStyle(MCTheme.muted)
                    .padding(.leading, 8)
                    .padding(.top, 1)
            }
        }
    }
}

private struct MetadataChipEditor: View {
    let metadata: [String: String]
    @Binding var addingMetadata: Bool
    let setMetadata: (String, String?) -> Void

    @State private var newKey = ""
    @State private var newValue = ""
    @FocusState private var addFocus: AddFocus?

    private enum AddFocus {
        case key
        case value
    }

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(metadata.keys.sorted(), id: \.self) { key in
                    MetadataChip(
                        keyName: key,
                        value: metadata[key] ?? "",
                        setMetadata: setMetadata
                    )
                }

                if addingMetadata {
                    addChip
                } else {
                    Button {
                        addingMetadata = true
                        DispatchQueue.main.async { addFocus = .key }
                    } label: {
                        Label("Add", systemImage: "plus")
                            .labelStyle(.titleAndIcon)
                            .lineLimit(1)
                    }
                    .buttonStyle(.plain)
                    .font(.caption)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 5)
                    .background(MCTheme.panelElevated)
                    .clipShape(Capsule())
                    .overlay(Capsule().strokeBorder(MCTheme.border))
                    .help("Add metadata")
                }
            }
            .padding(.vertical, 2)
        }
        .scrollIndicators(.hidden)
        .frame(height: 36)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var addChip: some View {
        HStack(spacing: 5) {
            TextField("key", text: $newKey)
                .textFieldStyle(.plain)
                .font(.caption)
                .frame(width: min(130, max(44, CGFloat(newKey.count * 7 + 24))))
                .focused($addFocus, equals: .key)
                .lineLimit(1)
                .onSubmit {
                    if newKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        cancelAdd()
                    } else {
                        addFocus = .value
                    }
                }

            Text("=")
                .font(.caption)
                .foregroundStyle(MCTheme.muted)

            TextField("value", text: $newValue)
                .textFieldStyle(.plain)
                .font(.caption)
                .frame(width: min(220, max(64, CGFloat(newValue.count * 7 + 34))))
                .focused($addFocus, equals: .value)
                .lineLimit(1)
                .onSubmit { commitAdd() }

            Button {
                commitAdd()
            } label: {
                Image(systemName: "checkmark.circle.fill")
                    .font(.system(size: 13))
            }
            .buttonStyle(.plain)
            .help("Save metadata")

            Button {
                cancelAdd()
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 13))
                    .foregroundStyle(MCTheme.muted)
            }
            .buttonStyle(.plain)
            .help("Cancel")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .fixedSize(horizontal: true, vertical: false)
        .background(MCTheme.panelElevated)
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(MCTheme.blue.opacity(0.35)))
    }

    private func commitAdd() {
        let key = newKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else {
            cancelAdd()
            return
        }
        setMetadata(key, newValue)
        newKey = ""
        newValue = ""
        addingMetadata = false
        addFocus = nil
    }

    private func cancelAdd() {
        newKey = ""
        newValue = ""
        addingMetadata = false
        addFocus = nil
    }
}

private struct MetadataChip: View {
    let keyName: String
    let value: String
    let setMetadata: (String, String?) -> Void

    @State private var draftValue: String
    @FocusState private var focused: Bool

    init(keyName: String, value: String, setMetadata: @escaping (String, String?) -> Void) {
        self.keyName = keyName
        self.value = value
        self.setMetadata = setMetadata
        _draftValue = State(initialValue: value)
    }

    var body: some View {
        HStack(spacing: 5) {
            Text(keyName)
                .font(.caption)
                .fontWeight(.semibold)
                .foregroundStyle(MCTheme.muted)
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)

            TextField("value", text: $draftValue)
                .textFieldStyle(.plain)
                .font(.caption)
                .frame(width: min(240, max(48, CGFloat(draftValue.count * 7 + 24))))
                .focused($focused)
                .lineLimit(1)
                .onSubmit { commit() }
                .onChange(of: focused) { _, isFocused in
                    if !isFocused { commit() }
                }
                .onChange(of: value) { _, next in
                    if !focused { draftValue = next }
                }

            Button {
                setMetadata(keyName, nil)
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 13))
                    .foregroundStyle(MCTheme.muted)
            }
            .buttonStyle(.plain)
            .help("Remove \(keyName)")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .fixedSize(horizontal: true, vertical: false)
        .background(MCTheme.panelElevated)
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(MCTheme.border))
    }

    private func commit() {
        let next = draftValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if next != value {
            setMetadata(keyName, next.isEmpty ? nil : next)
        }
    }
}

/// Compact mic glyph where activity fills the capsule itself instead
/// of using a separate EQ bar row.
private struct MicActivityIcon: View {
    let peak: Float
    let isLive: Bool

    var body: some View {
        GeometryReader { proxy in
            let w = proxy.size.width
            let h = proxy.size.height
            let p = CGFloat(min(1, max(0, peak)))
            let capsuleWidth = w * 0.48
            let capsuleHeight = h * 0.58
            let capsuleY = h * 0.04
            let fillHeight = max(capsuleHeight * 0.16, capsuleHeight * p)

            ZStack {
                RoundedRectangle(cornerRadius: capsuleWidth / 2)
                    .fill(MCTheme.input)
                    .frame(width: capsuleWidth, height: capsuleHeight)
                    .position(x: w / 2, y: capsuleY + capsuleHeight / 2)

                RoundedRectangle(cornerRadius: capsuleWidth / 2)
                    .fill(fillColor)
                    .frame(width: capsuleWidth - 6, height: fillHeight)
                    .position(
                        x: w / 2,
                        y: capsuleY + capsuleHeight - fillHeight / 2 - 3)
                    .opacity(isLive ? 1 : 0.35)

                RoundedRectangle(cornerRadius: capsuleWidth / 2)
                    .strokeBorder(outlineColor, lineWidth: 2.4)
                    .frame(width: capsuleWidth, height: capsuleHeight)
                    .position(x: w / 2, y: capsuleY + capsuleHeight / 2)

                MicYoke()
                    .stroke(outlineColor, style: StrokeStyle(lineWidth: 2.8, lineCap: .round))
                    .frame(width: w * 0.72, height: h * 0.34)
                    .position(x: w / 2, y: h * 0.51)

                Capsule()
                    .fill(outlineColor)
                    .frame(width: 3, height: h * 0.22)
                    .position(x: w / 2, y: h * 0.84)
            }
        }
        .animation(.linear(duration: 0.08), value: peak)
    }

    private var fillColor: Color {
        if peak > 0.5 { return .red }
        if peak > 0.05 { return MCTheme.success }
        return MCTheme.subtle
    }

    private var outlineColor: Color {
        peak > 0.05 ? MCTheme.success : MCTheme.muted
    }
}

private struct MicYoke: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: rect.minX, y: rect.minY + rect.height * 0.12))
        path.addCurve(
            to: CGPoint(x: rect.midX, y: rect.maxY),
            control1: CGPoint(x: rect.minX, y: rect.maxY * 0.72),
            control2: CGPoint(x: rect.midX * 0.62, y: rect.maxY)
        )
        path.addCurve(
            to: CGPoint(x: rect.maxX, y: rect.minY + rect.height * 0.12),
            control1: CGPoint(x: rect.midX * 1.38, y: rect.maxY),
            control2: CGPoint(x: rect.maxX, y: rect.maxY * 0.72)
        )
        return path
    }
}

/// Lets us reach the underlying `NSWindow` so we can configure
/// behaviors SwiftUI doesn't expose on `Window` scenes (window
/// level, space behavior). The accessor itself is invisible.
private struct WindowAccessor: NSViewRepresentable {
    let configure: (NSWindow) -> Void

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async {
            if let window = view.window {
                configure(window)
            }
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async {
            if let window = nsView.window {
                configure(window)
            }
        }
    }
}

// MARK: - Artifact compose UI

/// Horizontal strip of currently-attached artifact chips with a
/// trailing "+ Artifact" button. Lives between the metadata chip
/// editor and the compose-panel buttons so attachments read as a
/// peer concept to project / title metadata.
private struct ArtifactChipStrip: View {
    let attached: [Artifact]
    let onPick: () -> Void
    let onRemove: (String) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(attached) { a in
                    ArtifactChip(artifact: a, onRemove: { onRemove(a.id) })
                }
                Button(action: onPick) {
                    HStack(spacing: 4) {
                        Image(systemName: "plus")
                            .font(.system(size: 10, weight: .semibold))
                        Text(attached.isEmpty ? "Attach artifact" : "Add")
                            .font(.system(size: 11, weight: .medium))
                    }
                    .foregroundStyle(MCTheme.blue)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background {
                        RoundedRectangle(cornerRadius: 6)
                            .strokeBorder(MCTheme.blue.opacity(0.4), style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
                    }
                }
                .buttonStyle(.plain)
            }
        }
        .frame(height: 28)
    }
}

private struct ArtifactChip: View {
    let artifact: Artifact
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: "doc.text")
                .font(.system(size: 9))
                .foregroundStyle(MCTheme.muted)
            Text(artifact.name)
                .font(.system(size: 11, weight: .medium))
                .lineLimit(1)
                .foregroundStyle(MCTheme.text)
            Button(action: onRemove) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(MCTheme.muted)
            }
            .buttonStyle(.plain)
            .help("Remove")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background {
            RoundedRectangle(cornerRadius: 6)
                .fill(MCTheme.input)
        }
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .strokeBorder(MCTheme.border)
        )
    }
}

/// Modal sheet showing the user's library with a multi-select
/// checkbox column. Only `done` artifacts are selectable; pending
/// rows show a spinner and are unselectable; failed rows show in
/// red. Confirming returns the picked set to the caller.
private struct ArtifactPickerSheet: View {
    @Bindable var model: AppModel
    let alreadySelectedIds: Set<String>
    let onConfirm: ([Artifact]) -> Void
    @Environment(\.dismiss) private var dismiss

    @State private var library: [Artifact] = []
    @State private var selectedIds: Set<String> = []
    @State private var loading = true
    @State private var loadError: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Attach artifacts")
                    .font(.headline)
                Spacer()
                Text("\(selectedIds.count) selected")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            Divider()

            content

            Divider()
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Attach") {
                    let picked = library.filter { selectedIds.contains($0.id) }
                    onConfirm(picked)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
        }
        .frame(width: 480, height: 420)
        .task { await load() }
        .onAppear { selectedIds = alreadySelectedIds }
    }

    @ViewBuilder
    private var content: some View {
        if loading && library.isEmpty {
            VStack { Spacer(); ProgressView(); Spacer() }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let err = loadError {
            VStack(alignment: .leading, spacing: 4) {
                Text("Couldn't load artifacts").font(.headline)
                Text(err).font(.caption).foregroundStyle(.secondary)
                Button("Retry") { Task { await load() } }
            }
            .padding(20)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else if library.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: "doc.text").font(.system(size: 28)).foregroundStyle(.secondary)
                Text("No artifacts yet").foregroundStyle(.secondary)
                Text("Upload one from Settings → Artifacts.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(library) { a in
                        ArtifactPickerRow(
                            artifact: a,
                            isSelected: selectedIds.contains(a.id),
                            onToggle: {
                                if selectedIds.contains(a.id) {
                                    selectedIds.remove(a.id)
                                } else if a.summaryStatus == "done" {
                                    selectedIds.insert(a.id)
                                }
                            }
                        )
                    }
                }
                .padding(8)
            }
        }
    }

    private func load() async {
        loading = true
        defer { loading = false }
        let api: ArtifactsAPI
        do {
            api = try await model.makeArtifactsAPI()
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            return
        }
        do {
            library = try await api.list()
            loadError = nil
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }
}

private struct ArtifactPickerRow: View {
    let artifact: Artifact
    let isSelected: Bool
    let onToggle: () -> Void

    private var isSelectable: Bool { artifact.summaryStatus == "done" }

    var body: some View {
        Button(action: onToggle) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: isSelected ? "checkmark.square.fill" : "square")
                    .font(.system(size: 14))
                    .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                    .opacity(isSelectable ? 1.0 : 0.4)
                VStack(alignment: .leading, spacing: 2) {
                    Text(artifact.name)
                        .font(.body)
                        .fontWeight(.medium)
                        .lineLimit(1)
                        .foregroundStyle(isSelectable ? Color.primary : Color.secondary)
                    if let s = artifact.shortSummary, !s.isEmpty {
                        Text(s)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                    if artifact.summaryStatus == "pending" {
                        Text("Generating summary…")
                            .font(.caption2).italic()
                            .foregroundStyle(.secondary)
                    } else if artifact.summaryStatus == "failed" {
                        Text("Summary failed")
                            .font(.caption2)
                            .foregroundStyle(.red)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(8)
            .background(isSelected ? Color.accentColor.opacity(0.08) : Color.clear)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!isSelectable && !isSelected)
    }
}
