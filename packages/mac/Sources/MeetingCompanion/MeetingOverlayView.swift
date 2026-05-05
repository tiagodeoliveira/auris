// MeetingOverlayView.swift
// Floating status-bar overlay shown while a meeting is active.
// Layout: narrow left column (mode / level / controls) + wide
// right column (scrollable transcript). The window reads as a
// HUD rather than a regular window — borderless rounded
// translucent rect, floating above other apps, joining all
// Spaces. Resizable: drag any edge; width gets you more
// transcript per line, height gets you more rows.

import AppKit
import SwiftUI

struct MeetingOverlayView: View {
    @Bindable var model: AppModel
    @Environment(\.dismissWindow) private var dismissWindow
    @Environment(\.openWindow) private var openWindow

    @State private var mode: OverlayMode
    @State private var description: String = ""
    @State private var addingMetadata = false
    @FocusState private var descriptionFocused: Bool

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
        .background {
            // Translucent HUD look. Flat dark fill (rather than
            // a system material) gives us a precise opacity knob.
            // Tune the 0.84 here if it's still wrong.
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.black.opacity(0.84))
        }
        .background(WindowAccessor { window in
            window.level = .floating
            window.collectionBehavior.insert(.canJoinAllSpaces)
            window.isOpaque = false
            window.backgroundColor = .clear
            window.hasShadow = true
            window.isMovableByWindowBackground = true
            // Strip the title bar chrome so the rounded
            // translucent rect IS the entire visible window.
            // Keeps `.titled` in styleMask (some
            // window-management features depend on it) but hides
            // the visual chrome via fullSizeContentView +
            // transparent titlebar + hidden standard buttons.
            window.titlebarAppearsTransparent = true
            window.titleVisibility = .hidden
            window.styleMask.insert(.fullSizeContentView)
            window.standardWindowButton(.closeButton)?.isHidden = true
            window.standardWindowButton(.miniaturizeButton)?.isHidden = true
            window.standardWindowButton(.zoomButton)?.isHidden = true
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
        }
    }

    private var composePanel: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: "record.circle")
                    .font(.system(size: 15))
                    .foregroundStyle(.red)

                Text("Start meeting")
                    .font(.headline)

                Spacer()

                Button {
                    dismissWindow(id: "meeting-overlay")
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 16))
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Cancel")
            }

            ZStack(alignment: .topLeading) {
                if description.isEmpty {
                    Text("What's this meeting about? (optional)")
                        .foregroundStyle(.tertiary)
                        .padding(.top, 8)
                        .padding(.leading, 7)
                        .allowsHitTesting(false)
                }

                TextEditor(text: $description)
                    .font(.body)
                    .scrollContentBackground(.hidden)
                    .focused($descriptionFocused)
                    .frame(minHeight: 84)
            }
            .padding(4)
            .background(Color.white.opacity(0.08))
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(Color.white.opacity(0.16))
            )

            MetadataChipEditor(
                metadata: model.metadata,
                addingMetadata: $addingMetadata,
                setMetadata: { key, value in
                    Task { await model.setMetadata(key: key, value: value) }
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
                .help("Extract editable tags from the description")

                Spacer()

                if !model.canStartMeeting {
                    Text(notReadyHint)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Button("Start") {
                    submitDescription()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canStartMeeting || model.extractingMetadata)
            }
        }
        .foregroundStyle(.primary)
    }

    private var startingPanel: some View {
        HStack(spacing: 14) {
            MicActivityIcon(peak: combinedPeak, isLive: true)
                .frame(width: 38, height: 48)

            VStack(alignment: .leading, spacing: 4) {
                Text("Starting meeting")
                    .font(.headline)
                Text("Opening audio stream…")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            ProgressView()
                .controlSize(.small)
        }
    }

    private var livePanel: some View {
        HStack(alignment: .top, spacing: 10) {
            statusColumn
                .frame(width: 54)

            Divider()

            modeColumn
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    /// Left column: three stacked rows of status + controls.
    /// Distributed evenly so each lines up roughly with one row
    /// of transcript text on the right.
    private var statusColumn: some View {
        VStack(alignment: .center, spacing: 10) {
            HStack(spacing: 4) {
                Image(systemName: model.audioToBackendPaused ? "pause.circle.fill" : "record.circle.fill")
                    .foregroundStyle(model.audioToBackendPaused ? .yellow : .red)
                    .font(.system(size: 10))
                Text(model.audioToBackendPaused ? "Muted" : "Live")
                    .font(.caption2)
                    .fontWeight(.semibold)
            }
            .frame(maxWidth: .infinity)

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

            HStack(spacing: 6) {
                Button {
                    Task { await model.stopMeeting() }
                } label: {
                    Image(systemName: "stop.circle.fill")
                        .font(.system(size: 18))
                        .foregroundStyle(.red)
                }
                .buttonStyle(.plain)
                .help("Stop meeting")

                Menu {
                    Button("Settings…") {
                        openWindow(id: "settings")
                        NSApp.activate(ignoringOtherApps: true)
                    }
                } label: {
                    Image(systemName: "ellipsis.circle")
                        .font(.system(size: 16))
                        .foregroundStyle(.secondary)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .frame(width: 18)
            }
            .frame(maxWidth: .infinity)
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
                        .tracking(0.4)
                        .foregroundStyle(isActive ? .primary : .secondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background {
                            if isActive {
                                RoundedRectangle(cornerRadius: 5)
                                    .fill(Color.white.opacity(0.14))
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
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(items) { item in
                            Text(item.text)
                                .foregroundStyle(.primary)
                        }
                        if !interim.isEmpty {
                            Text(interim)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Color.clear.frame(height: 1).id("itemsEnd")
                }
                .font(.body)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .textSelection(.enabled)
            }
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
        mode = .starting
        descriptionFocused = false
        Task {
            await model.startMeeting(description: payload)
            if model.isMeetingActive {
                mode = .live
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
}

private enum OverlayMode: Equatable {
    case compose
    case starting
    case live

    var minWidth: CGFloat {
        switch self {
        case .compose: 460
        case .starting: 360
        case .live: 520
        }
    }

    var idealWidth: CGFloat {
        switch self {
        case .compose: 560
        case .starting: 420
        case .live: 820
        }
    }

    var minHeight: CGFloat {
        switch self {
        case .compose: 275
        case .starting: 80
        case .live: 110
        }
    }

    var idealHeight: CGFloat {
        switch self {
        case .compose: 315
        case .starting: 92
        case .live: 140
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
                    .padding(.vertical, 7)
                    .background(Color.white.opacity(0.08))
                    .clipShape(Capsule())
                    .overlay(Capsule().strokeBorder(Color.white.opacity(0.16)))
                    .help("Add metadata")
                }
            }
            .padding(.vertical, 2)
        }
        .scrollIndicators(.hidden)
        .frame(height: 42)
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
                .foregroundStyle(.secondary)

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
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Cancel")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 7)
        .fixedSize(horizontal: true, vertical: false)
        .background(Color.white.opacity(0.10))
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(Color.white.opacity(0.20)))
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
                .foregroundStyle(.secondary)
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
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .help("Remove \(keyName)")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 7)
        .fixedSize(horizontal: true, vertical: false)
        .background(Color.white.opacity(0.08))
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(Color.white.opacity(0.16)))
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
                    .fill(Color.black.opacity(0.28))
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
        if peak > 0.05 { return .green }
        return Color.gray.opacity(0.5)
    }

    private var outlineColor: Color {
        peak > 0.05 ? .green : Color.secondary
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

    func updateNSView(_ nsView: NSView, context: Context) {}
}
