// MeetingOverlayView.swift
// Floating always-on-top meeting overlay.
// Shared visual direction with the PWA: light glass panel, blue primary
// action, amber moment action, red destructive action, and high-contrast
// transcript text. The window keeps a stable wide footprint across starting
// and live states so the meeting UI doesn't jump when capture starts.

import AppKit
import SwiftUI

struct MeetingOverlayView: View {
    @Bindable var model: AppModel

    @State private var mode: OverlayMode

    /// IDs of items the user has expanded to reveal `detail`.
    /// Persists across tab switches so re-selecting a tab restores
    /// what was expanded; cleared when a new meeting begins (the
    /// IDs change anyway, so the set goes stale naturally).
    @State private var expandedItemIds: Set<String> = []
    /// User explicitly collapsed these — overrides the auto-expand
    /// path that would otherwise show the panel for any item that
    /// has a `detail` value. Lets cross-client expand_item still
    /// auto-open the row on this Mac (when detail flows in from
    /// the PWA's expand) without overriding a local user collapse.
    @State private var manuallyCollapsedIds: Set<String> = []

    /// Live-meeting attach flow (overlay `…` menu). Distinct from the
    /// compose-time picker, which now lives in StartMeetingView.
    @State private var showLiveArtifactPicker = false

    /// Mid-meeting attach flow (overlay live cluster). Distinct from
    /// the compose-time picker.
    @State private var showLiveMeetingPicker = false

    /// Chat input state. `chatDraft` mirrors the text-field; `chatBusy`
    /// disables submit while the agent's reply is in flight (cleared
    /// when chat-mode items grow, indicating the round-trip landed).
    @State private var chatDraft: String = ""
    @State private var chatBusy: Bool = false

    /// Programmatic focus for the chat input. `.accessory` activation
    /// policy means the overlay window doesn't auto-become key on
    /// click; without setting `isChatFocused = true` (and activating
    /// the app) when the chat tab opens, keystrokes have nowhere to
    /// land and the field appears dead.
    @FocusState private var isChatFocused: Bool

    init(model: AppModel) {
        self.model = model
        // `hasActiveMeeting` so the overlay opens in .live whenever a
        // meeting is happening for this user — even if it was started
        // on the phone or PWA. The Mac is a control surface, not just
        // a capture device.
        _mode = State(initialValue: model.hasActiveMeeting ? .live : .starting)
    }

    /// Close / hide the overlay window. SwiftUI's `dismissWindow(id:)`
    /// has been observed to silently no-op in our menu-bar accessory
    /// app (same finding as `AppModel.closeOverlayWindow`), so we go
    /// through AppKit. For the singleton `Window(id:)` pattern this
    /// is "hide" semantically — the window is preserved and a future
    /// `openWindow(id: "meeting-overlay")` re-shows it without
    /// rebuilding state.
    private func closeOverlay() {
        for win in NSApp.windows where win.title == "Meeting" {
            win.close()
        }
    }

    var body: some View {
        Group {
            switch mode {
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
        // .fixedSize(horizontal: false, vertical: true) was here —
        // it locked the window's vertical extent to the content's
        // ideal height, preventing the user from resizing taller.
        // Removing it lets the window grow on the Y axis; the
        // surrounding ScrollViews (transcript, items list, chat
        // bubbles) absorb the extra space gracefully.
        .background {
            RoundedRectangle(cornerRadius: 12)
                .fill(AurisTheme.panel.opacity(model.settings.overlayOpacity))
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .strokeBorder(AurisTheme.border, lineWidth: 1)
                )
                .shadow(color: Color.black.opacity(0.18), radius: 22, y: 10)
        }
        .foregroundStyle(AurisTheme.text)
        .ignoresSafeArea()
        .preferredColorScheme(model.settings.overlayTheme == .dark ? .dark : .light)
        .environment(\.overlayOpacity, model.settings.overlayOpacity)
        .background(WindowAccessor { window in
            window.level = .floating
            window.collectionBehavior.insert(.canJoinAllSpaces)
            window.isOpaque = false
            window.backgroundColor = .clear
            window.hasShadow = true
            window.isMovableByWindowBackground = true
            // Hide the overlay from screen-sharing tools (Zoom,
            // Meet, Teams, macOS native screen capture). The
            // moment-screenshot path goes through ScreenCaptureKit
            // with a self-exclusion filter, but third-party
            // screen-share readers go through `CGWindowList…`
            // APIs that honor `sharingType` instead. Setting this
            // to `.none` means the window simply isn't visible to
            // any external recorder.
            window.sharingType = .none
            // Keep `.titled` in the style mask — borderless windows
            // return `canBecomeKey == false` by default, which means
            // SwiftUI TextField inside the overlay silently refuses
            // keystrokes (e.g., the chat input). Hide the titlebar
            // cosmetically via `titleVisibility` + transparent
            // titlebar instead. The blue-wallpaper-strip issue
            // mentioned previously was solved by `isOpaque = false`
            // + transparent titlebar — the title area no longer
            // shows desktop through it.
            window.styleMask = [.titled, .resizable, .fullSizeContentView]
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.standardWindowButton(.closeButton)?.isHidden = true
            window.standardWindowButton(.miniaturizeButton)?.isHidden = true
            window.standardWindowButton(.zoomButton)?.isHidden = true
            window.minSize = NSSize(width: mode.minWidth, height: mode.minHeight)
        })
        // Watch the broad `hasActiveMeeting` signal (= currentMeetingId-
        // derived) so the overlay reacts to meetings started on any
        // surface — phone, PWA, this Mac. Drives both directions:
        //   - active becomes true  → switch to .live
        //   - active becomes false → dismiss if we were showing live/starting
        //
        // `currentMeetingId` flips the instant we receive
        // `meeting_state_changed → idle` from the server, well before
        // `audioCapture.stop()` finishes its async teardown. Watching
        // the id-derived flag (rather than the narrower `isMeetingActive`)
        // makes the overlay react server-promptly on remote stops — a
        // PWA-side stop no longer leaves the Mac overlay stuck on the
        // live view for several seconds.
        .onChange(of: model.hasActiveMeeting) { _, active in
            if active {
                mode = .live
            } else {
                // Meeting ended (or a local start failed). Clear live-scoped
                // drafts so the next meeting's HUD starts clean, then dismiss.
                resetLiveState()
                closeOverlay()
            }
        }
        .onChange(of: model.isLocallyStartingMeeting) { _, starting in
            guard !model.hasActiveMeeting else { return }  // .live wins
            if starting {
                mode = .starting
            } else {
                // Start attempt ended without going active — nothing to show.
                closeOverlay()
            }
        }
        .onAppear {
            mode = model.hasActiveMeeting ? .live : .starting
            model.isOverlayVisible = true
        }
        .onDisappear {
            model.isOverlayVisible = false
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
                    .foregroundStyle(AurisTheme.muted)
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
                    .fill(AurisTheme.border)
                    .frame(width: 1)

                modeColumn
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }

            VStack(alignment: .trailing, spacing: 8) {
                HStack(spacing: 6) {
                    // Hide-overlay button — closes the window without
                    // stopping the meeting. The Window(id:) instance
                    // is preserved; re-opening from the menu shows it
                    // again with state intact.
                    //
                    // This is the user's "stop popping at me" gesture
                    // (Rule 5). Flip `overlayAutoShow` off so future
                    // remote-initiated meetings don't bring the overlay
                    // back — they can re-enable it from Settings or by
                    // starting a meeting locally (always-shows).
                    Button {
                        model.settings.overlayAutoShow = false
                        closeOverlay()
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 16))
                            .foregroundStyle(AurisTheme.muted)
                    }
                    .buttonStyle(.plain)
                    .help("Hide overlay (meeting keeps running)")

                    actionCluster
                }

                if let status = model.momentStatus, !status.isEmpty {
                    Text(status)
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(AurisTheme.amberText)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 7)
                        .background(AurisTheme.amberSoft)
                        .clipShape(Capsule())
                        .overlay(Capsule().strokeBorder(AurisTheme.amberBorder))
                        .transition(.opacity.combined(with: .move(edge: .top)))
                }
            }
            .padding(.top, -2)
        }
    }

    /// Left column: status only. Actions live in the top-right cluster
    /// so the mic reads as meeting state rather than another command.
    ///
    /// Three states (in priority order):
    ///   - Remote — meeting is active but this Mac is not capturing
    ///     audio (phone or PWA is the source). Local mute is
    ///     irrelevant; show "Remote" with an antenna glyph.
    ///   - Muted — this Mac IS capturing locally, but the backend
    ///     audio is paused. Amber pause icon.
    ///   - Live — capturing locally, streaming to backend. Red dot.
    private var statusColumn: some View {
        let isRemote = model.hasActiveMeeting && !model.isMeetingActive
        let isMuted = !isRemote && model.audioToBackendPaused

        let icon: String
        let iconColor: Color
        let label: String
        if isRemote {
            icon = "antenna.radiowaves.left.and.right"
            iconColor = AurisTheme.text.opacity(0.65)
            label = "Remote"
        } else if isMuted {
            icon = "pause.circle.fill"
            iconColor = AurisTheme.amber
            label = "Muted"
        } else {
            icon = "record.circle.fill"
            iconColor = AurisTheme.danger
            label = "Live"
        }

        return VStack(alignment: .center, spacing: 0) {
            HStack(spacing: 4) {
                Image(systemName: icon)
                    .foregroundStyle(iconColor)
                    .font(.system(size: 10))
                Text(label)
                    .font(.caption2)
                    .fontWeight(.semibold)
                    .foregroundStyle(AurisTheme.text)
            }
            .frame(maxWidth: .infinity)

            Spacer(minLength: 0)

            Button {
                model.toggleBackendAudio()
            } label: {
                MicActivityIcon(
                    // No local capture during a remote meeting →
                    // suppress the peak meter (would otherwise show
                    // whatever stale value the smoother is carrying).
                    peak: (isRemote || model.audioToBackendPaused) ? 0 : combinedPeak,
                    isLive: model.isMeetingActive && !model.audioToBackendPaused
                )
                .frame(width: 34, height: 42)
            }
            .buttonStyle(.plain)
            .disabled(!model.canToggleBackendAudio)
            .frame(maxWidth: .infinity)
            // Tooltip mirrors the three-state label so screen reader
            // and hover both see consistent wording.
            .help(
                isRemote
                    ? "Audio is captured on another device for this meeting"
                    : isMuted
                        ? "Resume backend audio"
                        : "Pause backend audio"
            )

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
            .buttonStyle(IconCircleButtonStyle(tint: AurisTheme.amber))
            .keyboardShortcut("m", modifiers: [.command, .shift])
            .disabled(!model.hasActiveMeeting)
            .help("Mark moment (⇧⌘M)")

            Button {
                showLiveArtifactPicker = true
            } label: {
                Image(systemName: "paperclip.circle.fill")
                    .font(.system(size: 20))
            }
            .buttonStyle(IconCircleButtonStyle(tint: AurisTheme.blue))
            .disabled(!model.auth0.isSignedIn || !model.hasActiveMeeting)
            .help("Attach artifact")

            Button {
                showLiveMeetingPicker = true
            } label: {
                Image(systemName: "link.circle.fill")
                    .font(.system(size: 20))
            }
            .buttonStyle(IconCircleButtonStyle(tint: AurisTheme.blue))
            .disabled(!model.auth0.isSignedIn || !model.hasActiveMeeting)
            .help("Attach past meeting")

            // Mid-meeting sensitivity flip. SwiftUI `Menu` over a
            // `Picker` gives us a single icon-circle button consistent
            // with the rest of the cluster, while the picker form
            // renders selected-state checkmarks for free. The picker's
            // binding routes through `setAssistSensitivity`, which
            // sends the `set_assist_sensitivity` intent + mirrors the
            // local AppModel value optimistically.
            Menu {
                Picker(
                    "Assist sensitivity",
                    selection: Binding(
                        get: { model.assistSensitivity },
                        set: { value in
                            Task { await model.setAssistSensitivity(value) }
                        }
                    )
                ) {
                    ForEach(AssistSensitivity.allCases, id: \.self) { v in
                        Text(v.displayName).tag(v)
                    }
                }
            } label: {
                Image(systemName: "slider.horizontal.3")
                    .font(.system(size: 14, weight: .semibold))
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .buttonStyle(IconCircleButtonStyle(tint: AurisTheme.blue))
            .disabled(!model.hasActiveMeeting)
            .help("Assist sensitivity (\(model.assistSensitivity.displayName))")

            Button {
                Task { await model.stopMeeting() }
            } label: {
                Image(systemName: "stop.circle.fill")
                    .font(.system(size: 20))
            }
            .buttonStyle(IconCircleButtonStyle(tint: AurisTheme.danger))
            .help("Stop meeting")

        }
        .padding(4)
        .background(AurisTheme.panel.opacity(model.settings.overlayOpacity))
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(AurisTheme.border))
        .sheet(isPresented: $showLiveArtifactPicker) {
            ArtifactPickerSheet(
                model: model,
                // Pre-check what's already attached to the running
                // meeting so the picker reflects current state.
                // Server-side attach is idempotent, so user picks
                // are translated into the *delta* (newly checked).
                alreadySelectedIds: model.currentMeetingAttachedArtifactIds,
                onConfirm: { picked in
                    let ids = picked.map { $0.id }
                    Task { await model.attachArtifactsToCurrentMeeting(ids: ids) }
                }
            )
        }
        .sheet(isPresented: $showLiveMeetingPicker) {
            MeetingPickerSheet(
                model: model,
                alreadySelectedIds: model.currentMeetingAttachedMeetingIds,
                excludeMeetingId: model.currentMeetingId,
                onConfirm: { picked in
                    let ids = picked.map { $0.id }
                    Task { await model.attachMeetingsToCurrentMeeting(ids: ids) }
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
            if model.currentMode == "chat" {
                chatPanel
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                itemsList
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    /// Chat tab's bubble list + input row. Q+A pairs from chat-mode
    /// items render as right-aligned (user) / left-aligned (assistant)
    /// bubbles. Replace strategy means a new exchange clobbers the
    /// previous pair.
    private var chatPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            chatBubbles
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            chatInputRow
        }
        .onAppear { focusChatInput() }
        .onChange(of: model.currentMode) { _, newMode in
            if newMode == "chat" { focusChatInput() }
        }
    }

    /// Bring the menu-bar app to the foreground so the overlay window
    /// becomes a key window, then move SwiftUI focus into the chat
    /// input. Without the activate-the-app step, `.accessory` apps
    /// don't accept keystrokes even when a TextField has logical
    /// focus. Run on a tiny delay so the focus assignment happens
    /// after SwiftUI finishes rendering the chat panel.
    private func focusChatInput() {
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
            isChatFocused = true
        }
    }

    private var chatBubbles: some View {
        // Bottom-pinned chat scroll via the native `defaultScrollAnchor`:
        // newest message pinned to the bottom, starts there, user can
        // still scroll up.
        //
        // Plain `VStack`, NOT `LazyVStack`. `defaultScrollAnchor(.bottom)`
        // re-pins the scroll offset whenever content height changes; a
        // `LazyVStack` only *estimates* off-screen row heights, so as the
        // thread grows (or text streams) the reported height keeps
        // shifting and the anchor jumps the view on every re-pin — visible
        // as the scrollbar oscillating while parked at the bottom, settling
        // only once the user scrolls up and disengages the anchor. A plain
        // VStack measures every row exactly, giving the anchor a stable
        // height to pin against. The lazy variant's only benefit here —
        // skipping markdown re-parse for off-screen bubbles — is already
        // covered: `Item` is Equatable and the ForEach is keyed by id, so
        // SwiftUI skips unchanged rows regardless. Chat threads are short,
        // width-capped bubbles; eager layout is cheap. (The 500-item
        // transcript/items list keeps LazyVStack — different scale.)
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                let items = model.itemsByMode["chat"] ?? []
                if items.isEmpty {
                    Text("Ask the agent anything.")
                        .foregroundStyle(AurisTheme.muted)
                        .padding(.top, 8)
                } else {
                    ForEach(items) { item in
                        ChatBubbleRow(item: item)
                            .id(item.id)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .padding(.trailing, 8)
        }
        .scrollIndicators(.hidden)
        .defaultScrollAnchor(.bottom)
    }

    private var chatInputRow: some View {
        VStack(spacing: 4) {
            ChatAttachmentStrip(model: model)
            // Quick-ask chip row — saved prompts from the user's
            // library. Tap dispatches the snippet's full text as a
            // chat message, same as if it had been typed.
            if !model.quickAsks.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(model.quickAsks, id: \.id) { ask in
                            Button {
                                let prompt = (ask.detail ?? "").trimmingCharacters(
                                    in: .whitespacesAndNewlines
                                )
                                guard !prompt.isEmpty else { return }
                                Task { await model.sendChat(prompt) }
                            } label: {
                                Text(ask.text)
                                    .font(.system(size: 11, weight: .medium))
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 4)
                            }
                            .buttonStyle(.plain)
                            .foregroundStyle(AurisTheme.blue)
                            .overlay(
                                RoundedRectangle(cornerRadius: 12)
                                    .strokeBorder(AurisTheme.blue, lineWidth: 1)
                            )
                            .disabled(chatBusy)
                            .help((ask.detail ?? "").prefix(200).description)
                        }
                    }
                    .padding(.horizontal, 2)
                }
            }

            HStack(spacing: 8) {
                TextField("Ask the agent…", text: $chatDraft)
                    .textFieldStyle(.plain)
                    .focused($isChatFocused)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(AurisTheme.input.opacity(model.settings.overlayOpacity))
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .strokeBorder(isChatFocused ? AurisTheme.blue : AurisTheme.border)
                    )
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                    .disabled(chatBusy)
                    .onSubmit { submitChat() }

                Button {
                    Task { await model.captureChatAttachment() }
                } label: {
                    Image(systemName: "camera.fill")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundStyle(canCaptureChatAttachment ? AurisTheme.blue : AurisTheme.muted)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                }
                .buttonStyle(.plain)
                .disabled(!canCaptureChatAttachment)
                .help(captureButtonTooltip)

                Button {
                    submitChat()
                } label: {
                    Text("Send")
                        .font(.system(size: 12, weight: .semibold))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                }
                .buttonStyle(.borderedProminent)
                .tint(AurisTheme.blue)
                .disabled(chatBusy || chatDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            .onChange(of: hasPendingChatBubble) { _, pending in
                // While a `meta.role == "assistant-pending"` bubble sits
                // in chat-mode items, we're waiting on the agent. The
                // server's ItemsUpdate replaces it with a real assistant
                // bubble (role == "assistant") → flip back to enabled.
                chatBusy = pending
            }
            // Safety net for a WS reconnect mid-stream: the snapshot
            // may carry `meta.streaming == true` items whose deltas
            // were already consumed during the disconnect window, so
            // `hasPendingChatBubble` stays true forever and the input
            // is permanently locked. Auto-release after 60s of being
            // stuck. `.task(id: chatBusy)` cancels the prior task on
            // every flip, so a normal sub-60s round-trip never trips.
            .task(id: chatBusy) {
                guard chatBusy else { return }
                try? await Task.sleep(nanoseconds: 60 * 1_000_000_000)
                if chatBusy {
                    chatBusy = false
                }
            }
        }
    }

    /// Camera button is only useful when the server agrees we're in
    /// a meeting (mirrors the gate inside `captureChatAttachment`) and
    /// we haven't hit the per-message attachment cap.
    private var canCaptureChatAttachment: Bool {
        model.currentMeetingId != nil
            && model.pendingChatAttachments.count < model.chatAttachmentLimit
    }

    private var captureButtonTooltip: String {
        if model.currentMeetingId == nil {
            return "Start a meeting to attach screenshots"
        }
        if model.pendingChatAttachments.count >= model.chatAttachmentLimit {
            return "Maximum \(model.chatAttachmentLimit) screenshots per message"
        }
        return "Capture screen"
    }

    /// True while the chat input should stay locked. Covers two
    /// phases: the optimistic-echo placeholder (`meta.role ==
    /// "assistant-pending"`) AND the active streaming phase
    /// (`meta.streaming == true`, set by the server's agent fire
    /// for the assistant bubble during deltas; flipped to false
    /// on the terminal ItemUpdated). Unlocks the moment the
    /// terminal event lands.
    private var hasPendingChatBubble: Bool {
        (model.itemsByMode["chat"] ?? []).contains(where: {
            $0.meta?.role == "assistant-pending"
                || $0.meta?.streaming == true
        })
    }

    private func submitChat() {
        let text = chatDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !chatBusy else { return }
        chatBusy = true
        chatDraft = ""
        Task { await model.sendChat(text) }
    }

    private var modeTabs: some View {
        // Quick asks is a glasses-only mode — on the Mac overlay the
        // same prompts surface as a chip row above the chat input
        // (see chatInputRow). Filter it out of the tab picker so it
        // doesn't double up.
        let visibleModes = model.availableModes.filter { $0.id != "quick_asks" }
        return HStack(spacing: 4) {
            ForEach(visibleModes) { mode in
                let isActive = mode.id == model.currentMode
                Button {
                    model.setMode(mode.id)
                } label: {
                    Text(Self.shortLabel(for: mode))
                        .font(.system(size: 10, weight: isActive ? .bold : .semibold))
                        .tracking(0.6)
                        .foregroundStyle(isActive ? AurisTheme.blue : AurisTheme.muted)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background {
                            if isActive {
                                Capsule().fill(AurisTheme.blueSoft)
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
                // `LazyVStack` (not plain VStack) so the 500-item
                // transcript cap doesn't force SwiftUI to evaluate
                // every `ItemRow` body on each state update — only
                // the rows in the viewport pay rendering cost. The
                // interim transcript line lives in its own child
                // view (`TranscriptInterimLine`) so its ~200ms write
                // cadence from Soniox doesn't co-invalidate the
                // items ForEach — without that isolation, each
                // interim chunk would re-evaluate the entire list.
                LazyVStack(alignment: .leading, spacing: 4) {
                    let items = model.itemsByMode[model.currentMode] ?? []
                    let isTranscript = model.currentMode == "transcript"

                    if items.isEmpty {
                        Text(emptyHint)
                            .foregroundStyle(AurisTheme.muted)
                    } else {
                        ForEach(items) { item in
                            ItemRow(
                                item: item,
                                mode: model.currentMode,
                                isExpanded: isEffectivelyExpanded(item),
                                onToggle: { toggleExpanded(item.id) }
                            )
                        }
                    }
                    if isTranscript {
                        TranscriptInterimLine(model: model)
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

    /// True when an item's expanded panel should be shown. Three
    /// inputs decide:
    ///   1. user explicitly collapsed → false (override auto-expand)
    ///   2. user explicitly expanded → true
    ///   3. detail present → true (auto-open, including when the
    ///      detail flowed in from another client's expand_item)
    private func isEffectivelyExpanded(_ item: Item) -> Bool {
        if manuallyCollapsedIds.contains(item.id) { return false }
        if expandedItemIds.contains(item.id) { return true }
        if let d = item.detail, !d.isEmpty { return true }
        return false
    }

    /// Toggle an item's expanded state. Called from `ItemRow` via
    /// the chevron tap. On *expand* (not collapse) for an item
    /// whose `detail` is still nil, fire the `expand_item` intent
    /// so the agent computes the expansion. The reply lands via
    /// `Event::ItemUpdated` and re-renders the row.
    private func toggleExpanded(_ id: String) {
        let item = (model.itemsByMode[model.currentMode] ?? [])
            .first(where: { $0.id == id })
        let currentlyExpanded = item.map { isEffectivelyExpanded($0) } ?? false
        if currentlyExpanded {
            // Collapse: explicit-mark so a later detail update
            // doesn't auto-reopen the panel.
            expandedItemIds.remove(id)
            manuallyCollapsedIds.insert(id)
        } else {
            expandedItemIds.insert(id)
            manuallyCollapsedIds.remove(id)
            let needsFetch = item.map { $0.detail == nil || $0.detail?.isEmpty == true } ?? false
            if needsFetch {
                Task { await model.expandItem(id) }
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
        case "summary": return "SUMMARY"
        case "chat": return "CHAT"
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

    /// Clear live-scoped view @State at the meeting-end boundary so no
    /// draft bleeds into the next meeting's HUD. (Compose-scoped state
    /// now lives in StartMeetingView, which resets itself on appear.)
    private func resetLiveState() {
        chatDraft = ""
        chatBusy = false
        expandedItemIds = []
        manuallyCollapsedIds = []
    }
}

private enum OverlayMode: Equatable {
    case starting
    case live

    var minWidth: CGFloat { switch self { case .starting: 520; case .live: 900 } }
    var idealWidth: CGFloat { switch self { case .starting: 620; case .live: 1180 } }
    var minHeight: CGFloat { switch self { case .starting: 96; case .live: 152 } }
    var idealHeight: CGFloat { switch self { case .starting: 112; case .live: 174 } }
}

/// Color tokens for the meeting overlay. Each token resolves to a
/// different RGB pair depending on the active appearance — the
/// overlay sets `.preferredColorScheme(...)` from `AppSettings.overlayTheme`,
/// which in turn drives NSColor's dynamicProvider to pick the right
/// variant on first paint and on every theme switch thereafter.
enum AurisTheme {
    static let panel = Color(light: 0xF7FAFE, dark: 0x1B2230)
    static let panelElevated = Color(light: 0xFFFFFF, dark: 0x232B3A)
    static let input = Color(light: 0xEEF4FA, dark: 0x1F2735)
    static let border = Color(light: 0xD5DEE9, dark: 0x39414F)
    static let text = Color(light: 0x17212E, dark: 0xE6EBF2)
    static let muted = Color(light: 0x647386, dark: 0x9AA7B8)
    static let subtle = Color(light: 0x96A3B4, dark: 0x6B7889)
    static let blue = Color(light: 0x2563EB, dark: 0x3B82F6)
    static let blueSoft = Color(light: 0xDBEAFE, dark: 0x1E3A8A)
    static let amber = Color(light: 0xF2B705, dark: 0xFFC533)
    static let amberText = Color(light: 0x765A00, dark: 0xFFD970)
    static let amberSoft = Color(light: 0xFFF5C7, dark: 0x3A2A00)
    static let amberBorder = Color(light: 0xF5D45F, dark: 0x6B5300)
    static let danger = Color(light: 0xE5484D, dark: 0xFF6369)
    static let dangerSoft = Color(light: 0xFFE2E3, dark: 0x4A1216)
    static let success = Color(light: 0x2EA043, dark: 0x4CC665)
}

private extension Color {
    init(hex: UInt32) {
        let r = Double((hex >> 16) & 0xff) / 255.0
        let g = Double((hex >> 8) & 0xff) / 255.0
        let b = Double(hex & 0xff) / 255.0
        self.init(red: r, green: g, blue: b)
    }

    /// Light/dark adaptive Color backed by an NSColor with a dynamic
    /// provider — resolves to the right variant per the resolved
    /// appearance (which `.preferredColorScheme(...)` sets at the
    /// overlay root). RGB hex shorthand at both ends keeps the
    /// palette table in `AurisTheme` readable.
    init(light: UInt32, dark: UInt32) {
        let lightColor = NSColor(srgbHex: light)
        let darkColor = NSColor(srgbHex: dark)
        let dynamic = NSColor(name: nil) { appearance in
            let match = appearance.bestMatch(from: [.aqua, .darkAqua, .vibrantLight, .vibrantDark])
            switch match {
            case .darkAqua, .vibrantDark: return darkColor
            default: return lightColor
            }
        }
        self.init(nsColor: dynamic)
    }
}

private extension NSColor {
    convenience init(srgbHex hex: UInt32) {
        let r = CGFloat((hex >> 16) & 0xff) / 255.0
        let g = CGFloat((hex >> 8) & 0xff) / 255.0
        let b = CGFloat(hex & 0xff) / 255.0
        self.init(srgbRed: r, green: g, blue: b, alpha: 1)
    }
}

/// Per-overlay opacity value, propagated from
/// `AppSettings.overlayOpacity` via the overlay root and read by any
/// child that paints an opaque panel-style background. Default is the
/// pre-configurable baseline (0.78) so views constructed outside the
/// overlay tree still get a sensible value.
private struct OverlayOpacityKey: EnvironmentKey {
    static let defaultValue: Double = 0.78
}

extension EnvironmentValues {
    fileprivate var overlayOpacity: Double {
        get { self[OverlayOpacityKey.self] }
        set { self[OverlayOpacityKey.self] = newValue }
    }
}

struct PrimaryPillButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(.white)
            .padding(.horizontal, 18)
            .padding(.vertical, 8)
            .background(AurisTheme.blue.opacity(configuration.isPressed ? 0.82 : 1))
            .clipShape(Capsule())
    }
}

private struct SecondaryPillButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(AurisTheme.text)
            .padding(.horizontal, 14)
            .padding(.vertical, 7)
            .background(configuration.isPressed ? AurisTheme.input.opacity(0.7) : AurisTheme.panelElevated)
            .clipShape(Capsule())
            .overlay(Capsule().strokeBorder(AurisTheme.border))
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

/// One line of a mode's items list. Mirrors the PWA's items-mirror
/// row structure:
///
///   `[mm:ss]  ▸  item text`
///   `         META · CHIP · WORDS`
///
/// Dim, italicized trailing line under the transcript items that
/// shows Soniox's in-flight interim text. Extracted into its own
/// view so the ~200ms write cadence of `model.transcriptInterim`
/// only re-evaluates THIS small body — not the parent items list
/// (whose ForEach is the actual CPU cost). Empty state renders
/// nothing (no spacer, no blank Text) so it doesn't displace
/// layout when no one's talking.
private struct TranscriptInterimLine: View {
    let model: AppModel

    var body: some View {
        let interim = model.transcriptInterim
        if !interim.isEmpty {
            Text(interim)
                .foregroundStyle(AurisTheme.muted)
                .italic()
        }
    }
}

/// Timestamp pill on the left in mono dim, blue triangle bullet,
/// then the body text, then a per-mode meta chip beneath it
/// (KIND · CONTEXT for questions, OWNER · DUE for actions,
/// IMPORTANCE for highlights, SPEAKER for transcript). Disclosure
/// chevron appears only when `detail` is present.
private struct ItemRow: View {
    let item: Item
    let mode: String
    let isExpanded: Bool
    let onToggle: () -> Void

    private var timestampLabel: String {
        let total = max(0, Int(item.t / 1000))
        let mm = total / 60
        let ss = total % 60
        return String(format: "[%02d:%02d]", mm, ss)
    }

    /// Emoji prefix for assist-mode items, distinguishing the four
    /// sub-types at a glance:
    ///   📖 definition / ❓ question / 🧠 memory / 💡 coach
    /// Empty string for non-assist modes (no-op prefix).
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

    /// Per-mode meta chip text. Returns empty string when there's
    /// nothing meaningful to render so the row collapses to one line.
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
        case "transcript":
            return meta.speaker.flatMap { s in s.isEmpty ? nil : "SPEAKER · \(s)" } ?? ""
        default:
            return ""
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(timestampLabel)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(AurisTheme.muted)

                Text("▸")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(AurisTheme.blue)

                Text(assistTypeGlyph + item.text)
                    .foregroundStyle(AurisTheme.text)
                    .frame(maxWidth: .infinity, alignment: .leading)

                Button(action: onToggle) {
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(AurisTheme.muted)
                        .frame(width: 14, height: 14)
                }
                .buttonStyle(.plain)
                .help(isExpanded ? "Hide detail" : "Show detail")
            }

            if !metaText.isEmpty {
                // Indent under the body text — line up with the
                // triangle's right edge so the meta visually attaches
                // to the bullet text rather than the timestamp.
                Text(metaText)
                    .font(.system(size: 10, weight: .semibold, design: .monospaced))
                    .tracking(0.4)
                    .foregroundStyle(AurisTheme.muted)
                    .padding(.leading, 70)
            }

            if isExpanded {
                if let detail = item.detail, !detail.isEmpty {
                    Text(detail)
                        .font(.callout)
                        .foregroundStyle(AurisTheme.muted)
                        .padding(.leading, 70)
                        .padding(.top, 1)
                } else {
                    // Round-trip in flight (or hadn't been requested
                    // yet). The toggle handler kicks off the
                    // expand_item intent the first time the chevron
                    // is opened on an item without detail.
                    Text("Expanding…")
                        .font(.callout)
                        .italic()
                        .foregroundStyle(AurisTheme.muted)
                        .padding(.leading, 70)
                        .padding(.top, 1)
                }
            }
        }
    }
}

/// Chat-mode bubble. `meta.role == "user"` right-aligns with the
/// brand-blue fill; anything else (assistant or omitted) left-aligns
/// with the panel surface. `meta.role == "assistant-pending"` is
/// the optimistic placeholder rendered while the agent's reply is
/// in flight — the bubble's content is a `TypingDots` indicator
/// that animates three staggered dots until the real reply lands.
private struct ChatBubbleRow: View {
    let item: Item
    /// Mirrors the overlay's panel opacity so the bubble nests into
    /// the window's translucency rather than punching through with
    /// its own opaque fill. Applies to both user (blue) and
    /// assistant (panel) variants — the user's "this is mine"
    /// emphasis comes from the blue accent, not from being more
    /// opaque than the surrounding chrome.
    @Environment(\.overlayOpacity) private var overlayOpacity

    private var role: String {
        item.meta?.role ?? ""
    }
    private var isUser: Bool { role == "user" }
    private var isAssistant: Bool { role == "assistant" }
    /// Render typing dots either for the server's optimistic
    /// placeholder (role == "assistant-pending") OR for the brief
    /// window where the role has flipped to "assistant" + streaming
    /// is true but no text has accumulated yet (usually ≤500ms
    /// before the first delta).
    private var isPending: Bool {
        role == "assistant-pending"
            || (isAssistant && item.meta?.streaming == true && item.text.isEmpty)
    }

    /// Agent answers are markdown — render bold/italic/code-spans/
    /// links/line-breaks via AttributedString. Conservative inline-only
    /// parsing (`.inlineOnlyPreservingWhitespace`) intentionally
    /// ignores headers/lists/blockquotes so the rendering matches the
    /// PWA's strict-allowlist approach. User bubbles and the pending
    /// placeholder stay plain text.
    private var rendered: Text {
        if isAssistant {
            if let attr = try? AttributedString(
                markdown: item.text,
                options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
            ) {
                return Text(attr)
            }
        }
        return Text(item.text)
    }

    /// How many screenshots this (user) message carried. 0 for
    /// assistant bubbles and plain text messages.
    private var attachmentCount: Int {
        item.meta?.attachmentIds?.count ?? 0
    }

    /// Small "this message had image(s)" cue: a photo glyph, plus the
    /// count when more than one. We don't show the images themselves —
    /// just signal they rode along.
    @ViewBuilder private var attachmentIndicator: some View {
        HStack(spacing: 3) {
            Image(systemName: "photo")
            if attachmentCount > 1 { Text("\(attachmentCount)") }
        }
        .font(.system(size: 10, weight: .semibold))
        .opacity(0.85)
    }

    var body: some View {
        HStack {
            if isUser { Spacer(minLength: 32) }
            Group {
                if isPending {
                    // Three-dot staggered typing indicator stands in
                    // for the agent's reply while the LLM is composing.
                    // Lives inside the same bubble chrome the final
                    // assistant reply will use, so the transition
                    // pending → final feels like text materializing
                    // in place rather than a layout change.
                    TypingDots()
                } else if attachmentCount > 0 {
                    VStack(alignment: .trailing, spacing: 4) {
                        rendered
                        attachmentIndicator
                    }
                } else {
                    rendered
                }
            }
            .foregroundStyle(isUser ? Color.white : AurisTheme.text)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(
                (isUser ? AurisTheme.blue : AurisTheme.panel).opacity(overlayOpacity)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(isUser ? Color.clear : AurisTheme.border, lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: 12))
            // Cap the bubble width instead of `.frame(maxWidth: .infinity)`.
            // A flexible-infinity frame fighting the sibling `Spacer` for
            // the row's width forces SwiftUI to re-propose sizes and
            // re-walk each bubble's padding→background→overlay subtree,
            // which compounds into a pathological multi-second layout
            // pass over a chat thread (observed: main thread 100% in
            // nested `sizeThatFits` → beachball). The Spacer alone does
            // the left/right alignment; the cap keeps long messages from
            // spanning the whole panel.
            .frame(maxWidth: 460, alignment: isUser ? .trailing : .leading)
            if !isUser { Spacer(minLength: 32) }
        }
    }
}

/// Three-dot "agent is thinking" indicator. Each dot pulses with a
/// staggered offset so the eye reads it as activity rather than a
/// static glyph. Rendered inside the same bubble chrome as a real
/// assistant reply so when the server swaps the pending placeholder
/// for the final answer, only the content swaps — bubble border,
/// radius, alignment all stay put.
private struct TypingDots: View {
    @State private var phase: Double = 0

    var body: some View {
        HStack(spacing: 4) {
            ForEach(0 ..< 3, id: \.self) { i in
                Circle()
                    .fill(AurisTheme.text.opacity(0.55))
                    .frame(width: 6, height: 6)
                    .opacity(opacityFor(index: i))
                    .offset(y: yFor(index: i))
            }
        }
        .frame(minHeight: 14)
        .onAppear {
            withAnimation(.easeInOut(duration: 1.2).repeatForever(autoreverses: false)) {
                phase = 1
            }
        }
    }

    /// Stagger by index: each dot's pulse leads the next by ~0.15
    /// of the cycle. Modulo wraps so the sequence loops smoothly.
    private func opacityFor(index: Int) -> Double {
        let offset = Double(index) * 0.15
        let local = (phase - offset).truncatingRemainder(dividingBy: 1.0)
        let normalized = local < 0 ? local + 1.0 : local
        // Sharp peak at ~0.3 of the cycle, fade out by ~0.6.
        return normalized < 0.3 ? 0.3 + normalized * 2.33 : max(0.3, 1.0 - (normalized - 0.3) * 1.4)
    }

    private func yFor(index: Int) -> CGFloat {
        // Same phase math as opacity, mapped to a small vertical
        // bounce so the dots also "lift" as they brighten.
        let offset = Double(index) * 0.15
        let local = (phase - offset).truncatingRemainder(dividingBy: 1.0)
        let normalized = local < 0 ? local + 1.0 : local
        return normalized < 0.3 ? CGFloat(-2.0 * normalized * 3.33) : 0
    }
}

struct MetadataChipEditor: View {
    let metadata: [String: String]
    @Binding var addingMetadata: Bool
    let setMetadata: (String, String?) -> Void

    @Environment(\.overlayOpacity) private var overlayOpacity

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
                    .background(AurisTheme.panelElevated.opacity(overlayOpacity))
                    .clipShape(Capsule())
                    .overlay(Capsule().strokeBorder(AurisTheme.border))
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
                .foregroundStyle(AurisTheme.muted)

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
                    .foregroundStyle(AurisTheme.muted)
            }
            .buttonStyle(.plain)
            .help("Cancel")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .fixedSize(horizontal: true, vertical: false)
        .background(AurisTheme.panelElevated.opacity(overlayOpacity))
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(AurisTheme.blue.opacity(0.35)))
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

    @Environment(\.overlayOpacity) private var overlayOpacity

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
                .foregroundStyle(AurisTheme.muted)
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
                    .foregroundStyle(AurisTheme.muted)
            }
            .buttonStyle(.plain)
            .help("Remove \(keyName)")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .fixedSize(horizontal: true, vertical: false)
        .background(AurisTheme.panelElevated.opacity(overlayOpacity))
        .clipShape(Capsule())
        .overlay(Capsule().strokeBorder(AurisTheme.border))
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
                    .fill(AurisTheme.input)
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
        if peak > 0.05 { return AurisTheme.success }
        return AurisTheme.subtle
    }

    private var outlineColor: Color {
        peak > 0.05 ? AurisTheme.success : AurisTheme.muted
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
struct WindowAccessor: NSViewRepresentable {
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
struct ArtifactChipStrip: View {
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
                    .foregroundStyle(AurisTheme.blue)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background {
                        RoundedRectangle(cornerRadius: 6)
                            .strokeBorder(AurisTheme.blue.opacity(0.4), style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
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
                .foregroundStyle(AurisTheme.muted)
            Text(artifact.name)
                .font(.system(size: 11, weight: .medium))
                .lineLimit(1)
                .foregroundStyle(AurisTheme.text)
            Button(action: onRemove) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(AurisTheme.muted)
            }
            .buttonStyle(.plain)
            .help("Remove")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background {
            RoundedRectangle(cornerRadius: 6)
                .fill(AurisTheme.input)
        }
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .strokeBorder(AurisTheme.border)
        )
    }
}

/// Horizontal strip of staged chat screenshots above the chat input.
/// Empty drafts list → renders nothing (no reserved gap). Each chip
/// shows the captured thumbnail with an upload-state overlay and an
/// X close button that drops the draft.
private struct ChatAttachmentStrip: View {
    @Bindable var model: AppModel

    var body: some View {
        if !model.pendingChatAttachments.isEmpty {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(model.pendingChatAttachments) { draft in
                        ChatAttachmentChip(draft: draft) {
                            model.removeChatAttachment(draftId: draft.id)
                        }
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
            }
            .frame(height: 72)
        }
    }
}

/// 64×64 thumbnail chip for a single staged screenshot. Visual state
/// follows `ChatAttachmentUploadState`: spinner while uploading,
/// warning glyph + red border on failure, clean thumb when uploaded.
private struct ChatAttachmentChip: View {
    let draft: ChatAttachmentDraft
    let onRemove: () -> Void

    var body: some View {
        ZStack(alignment: .topTrailing) {
            Image(nsImage: draft.image)
                .resizable()
                .aspectRatio(contentMode: .fill)
                .frame(width: 64, height: 64)
                .clipShape(RoundedRectangle(cornerRadius: 6))
                .overlay(stateOverlay)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(borderColor, lineWidth: 1)
                )

            Button(action: onRemove) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.white, .black.opacity(0.65))
                    .font(.system(size: 14))
            }
            .buttonStyle(.plain)
            .offset(x: 4, y: -4)
        }
        .help(tooltip)
    }

    @ViewBuilder private var stateOverlay: some View {
        switch draft.state {
        case .uploading:
            ProgressView()
                .controlSize(.small)
                .padding(4)
                .background(.black.opacity(0.4), in: Circle())
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.yellow)
                .padding(4)
                .background(.black.opacity(0.4), in: Circle())
        case .uploaded:
            EmptyView()
        }
    }

    private var borderColor: Color {
        switch draft.state {
        case .uploading: return .secondary.opacity(0.5)
        case .failed:    return .red
        case .uploaded:  return .clear
        }
    }

    private var tooltip: String {
        switch draft.state {
        case .uploading:       return "Uploading screenshot…"
        case .failed(let msg): return "Upload failed: \(msg)"
        case .uploaded:        return "Screenshot ready"
        }
    }
}

/// Modal sheet showing the user's library with a multi-select
/// checkbox column. Only `done` artifacts are selectable; pending
/// rows show a spinner and are unselectable; failed rows show in
/// red. Confirming returns the picked set to the caller.
struct ArtifactPickerSheet: View {
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
        // The compose panel uses the app's default scheme; the
        // overlay window itself is rendered against an arbitrary
        // desktop. Force light so the picker is legible regardless
        // of system appearance — matches SettingsView.
        .preferredColorScheme(.light)
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

// MARK: - Past-meeting compose UI

/// Horizontal strip of past-meeting chips with a trailing "+
/// Attach meeting" button. Sibling of `ArtifactChipStrip` — same
/// visual treatment, distinct concept (carry-over context vs.
/// attached artifacts).
struct MeetingChipStrip: View {
    let attached: [MeetingSummary]
    let onPick: () -> Void
    let onRemove: (String) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(attached) { m in
                    MeetingChip(meeting: m, onRemove: { onRemove(m.id) })
                }
                Button(action: onPick) {
                    HStack(spacing: 4) {
                        Image(systemName: "plus")
                            .font(.system(size: 10, weight: .semibold))
                        Text(attached.isEmpty ? "Attach meeting" : "Add")
                            .font(.system(size: 11, weight: .medium))
                    }
                    .foregroundStyle(AurisTheme.blue)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background {
                        RoundedRectangle(cornerRadius: 6)
                            .strokeBorder(AurisTheme.blue.opacity(0.4), style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
                    }
                }
                .buttonStyle(.plain)
            }
        }
        .frame(height: 28)
    }
}

private struct MeetingChip: View {
    let meeting: MeetingSummary
    let onRemove: () -> Void

    private var label: String {
        let desc = (meeting.description ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !desc.isEmpty { return desc.count > 32 ? String(desc.prefix(29)) + "…" : desc }
        if let t = meeting.metadata["title"]?.trimmingCharacters(in: .whitespacesAndNewlines),
            !t.isEmpty
        {
            return t
        }
        return "Meeting"
    }

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: "calendar")
                .font(.system(size: 9))
                .foregroundStyle(AurisTheme.muted)
            Text(label)
                .font(.system(size: 11, weight: .medium))
                .lineLimit(1)
                .foregroundStyle(AurisTheme.text)
            Button(action: onRemove) {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(AurisTheme.muted)
            }
            .buttonStyle(.plain)
            .help("Remove")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background {
            RoundedRectangle(cornerRadius: 6)
                .fill(AurisTheme.input)
        }
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .strokeBorder(AurisTheme.border)
        )
    }
}

/// Modal sheet listing past meetings with a multi-select checkbox
/// column. Mirrors `ArtifactPickerSheet`. The picker filters out
/// the active meeting (a meeting can't attach to itself — server
/// enforces with a CHECK constraint).
struct MeetingPickerSheet: View {
    @Bindable var model: AppModel
    let alreadySelectedIds: Set<String>
    /// When non-nil, the active meeting is hidden from the list.
    let excludeMeetingId: String?
    let onConfirm: ([MeetingSummary]) -> Void
    @Environment(\.dismiss) private var dismiss

    @State private var library: [MeetingSummary] = []
    @State private var selectedIds: Set<String> = []
    @State private var loading = true
    @State private var loadError: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Attach past meetings")
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
        .preferredColorScheme(.light)
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
                Text("Couldn't load meetings").font(.headline)
                Text(err).font(.caption).foregroundStyle(.secondary)
                Button("Retry") { Task { await load() } }
            }
            .padding(20)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        } else if library.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: "calendar").font(.system(size: 28)).foregroundStyle(.secondary)
                Text("No past meetings yet").foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 4) {
                    ForEach(library) { m in
                        MeetingPickerRow(
                            meeting: m,
                            isSelected: selectedIds.contains(m.id),
                            onToggle: {
                                if selectedIds.contains(m.id) {
                                    selectedIds.remove(m.id)
                                } else {
                                    selectedIds.insert(m.id)
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
        let api: MeetingsAPI
        do {
            api = try await model.makeMeetingsAPI()
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            return
        }
        do {
            let all = try await api.list()
            library = excludeMeetingId.map { id in all.filter { $0.id != id } } ?? all
            loadError = nil
        } catch {
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }
}

private struct MeetingPickerRow: View {
    let meeting: MeetingSummary
    let isSelected: Bool
    let onToggle: () -> Void

    private var title: String {
        let desc = (meeting.description ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !desc.isEmpty { return desc }
        if let t = meeting.metadata["title"]?.trimmingCharacters(in: .whitespacesAndNewlines),
            !t.isEmpty
        {
            return t
        }
        return "Meeting"
    }

    private static let formatter: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .medium
        f.timeStyle = .short
        return f
    }()

    var body: some View {
        Button(action: onToggle) {
            HStack(alignment: .top, spacing: 10) {
                Image(systemName: isSelected ? "checkmark.square.fill" : "square")
                    .font(.system(size: 14))
                    .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.body)
                        .fontWeight(.medium)
                        .lineLimit(1)
                        .foregroundStyle(Color.primary)
                    Text(Self.formatter.string(from: meeting.startedAt))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(8)
            .background(isSelected ? Color.accentColor.opacity(0.08) : Color.clear)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
