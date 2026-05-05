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

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            statusColumn
                .frame(width: 100)

            Divider()

            transcriptColumn
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(12)
        .frame(
            minWidth: 520, idealWidth: 820, maxWidth: .infinity,
            minHeight: 110, idealHeight: 140, maxHeight: .infinity)
        .background {
            // Translucent HUD look. Flat dark fill (rather than
            // a system material) gives us a precise opacity knob.
            // Tune the 0.72 here if it's still wrong.
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.black.opacity(0.72))
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
            if !active { dismissWindow(id: "meeting-overlay") }
        }
    }

    /// Left column: three stacked rows of status + controls.
    /// Distributed evenly so each lines up roughly with one row
    /// of transcript text on the right.
    private var statusColumn: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: "record.circle.fill")
                    .foregroundStyle(.red)
                    .font(.system(size: 11))
                Text("Live")
                    .font(.caption)
                    .fontWeight(.semibold)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)

            MicLevelMeter(peak: combinedPeak)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)

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
                .frame(width: 22)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
    }

    /// Right column: scrollable transcript. Each committed
    /// utterance from `transcriptHistory` is its own paragraph;
    /// the rolling `transcriptInterim` follows in a dimmer style
    /// so the user can tell what's settled vs. still-forming.
    /// Auto-pins to the bottom on each update so the latest words
    /// stay visible without manual scrolling.
    private var transcriptColumn: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    if model.transcriptHistory.isEmpty, model.transcriptInterim.isEmpty {
                        Text("(listening…)")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(Array(model.transcriptHistory.enumerated()), id: \.offset) { _, line in
                            Text(line)
                                .foregroundStyle(.primary)
                        }
                        if !model.transcriptInterim.isEmpty {
                            Text(model.transcriptInterim)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Color.clear.frame(height: 1).id("transcriptEnd")
                }
                .font(.body)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .textSelection(.enabled)
            }
            .onChange(of: model.transcriptInterim) { _, _ in
                withAnimation(.linear(duration: 0.1)) {
                    proxy.scrollTo("transcriptEnd", anchor: .bottom)
                }
            }
            .onChange(of: model.transcriptHistory.count) { _, _ in
                withAnimation(.linear(duration: 0.1)) {
                    proxy.scrollTo("transcriptEnd", anchor: .bottom)
                }
            }
        }
    }

    /// Combined per-source peak. Drives the mic-level meter; the
    /// per-source split was a debugging affordance.
    private var combinedPeak: Float {
        max(model.audioCapture.currentSysPeak, model.audioCapture.currentMicPeak)
    }
}

/// Mic icon + 5 vertical bars that grow with the audio peak. The
/// outer bars have a smaller scale factor so the meter has an
/// EQ-silhouette shape (small / medium / tall / medium / small)
/// at full volume — visually distinct from a flat row, which
/// would just read as "five identical bars".
private struct MicLevelMeter: View {
    let peak: Float

    private static let scales: [CGFloat] = [0.45, 0.75, 1.0, 0.75, 0.45]

    var body: some View {
        HStack(spacing: 5) {
            Image(systemName: "mic.fill")
                .font(.system(size: 13))
                .foregroundStyle(peak > 0.05 ? Color.green : Color.secondary)
                .frame(width: 14)

            HStack(alignment: .center, spacing: 2) {
                ForEach(0..<MicLevelMeter.scales.count, id: \.self) { i in
                    bar(at: i)
                }
            }
            .frame(height: 22)
        }
        .animation(.linear(duration: 0.06), value: peak)
    }

    private func bar(at index: Int) -> some View {
        let baseHeight: CGFloat = 3
        let maxBarHeight: CGFloat = 20
        let scale = MicLevelMeter.scales[index]
        let p = CGFloat(min(1, peak))
        let h = baseHeight + (maxBarHeight - baseHeight) * p * scale
        return RoundedRectangle(cornerRadius: 1.5)
            .fill(barColor)
            .frame(width: 3, height: h)
    }

    private var barColor: Color {
        if peak > 0.5 { return .red }
        if peak > 0.05 { return .green }
        return Color.gray.opacity(0.5)
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
