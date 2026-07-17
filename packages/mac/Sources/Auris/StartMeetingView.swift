// StartMeetingView.swift
// Standalone start-meeting popup. Hosts the meeting compose form in a
// normal titled window (like Settings), excluded from screen sharing so
// the description and attached-meeting titles never leak into a call the
// user is sharing while they start capture. The floating overlay
// (MeetingOverlayView) owns everything from "Start" onward.

import AppKit
import SwiftUI

struct StartMeetingView: View {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow

    @State private var description: String = ""
    @State private var addingMetadata = false
    @FocusState private var descriptionFocused: Bool
    @State private var dictation: DictationController? = nil
    @State private var selectedArtifacts: [Artifact] = []
    @State private var showArtifactPicker = false
    @State private var selectedMeetings: [MeetingSummary] = []
    @State private var showMeetingPicker = false

    /// Set by the failed-start recovery path before it reopens this
    /// window, so the reopen's onAppear preserves the user's input
    /// instead of wiping it. Consumed (cleared) on that appear.
    @State private var preserveComposeOnAppear = false

    var body: some View {
        composeForm
            .padding(16)
            .frame(minWidth: 520, idealWidth: 620, maxWidth: .infinity,
                   minHeight: 260, maxHeight: .infinity)
            .background(AurisTheme.panel.opacity(model.settings.overlayOpacity))
            .foregroundStyle(AurisTheme.text)
            .preferredColorScheme(model.settings.overlayTheme == .dark ? .dark : .light)
            .background(WindowAccessor { window in
                // Only the screen-share exclusion; everything else stays
                // standard-window default (titled, resizable, traffic
                // lights, normal level). Unlike the overlay we do NOT set
                // .floating / canJoinAllSpaces / borderless.
                window.sharingType = .none
            })
            .onAppear {
                // Singleton Window: SwiftUI never tears this view down, so
                // @State would carry the previous compose into the next
                // open. Reset on (re)appear for a clean form each time —
                // EXCEPT when the failed-start recovery path reopened us to
                // hand the user's input back (preserveComposeOnAppear).
                if preserveComposeOnAppear {
                    preserveComposeOnAppear = false
                } else {
                    resetComposeState()
                }
                descriptionFocused = true
            }
            .onChange(of: model.hasActiveMeeting) { _, active in
                // A meeting started (here or remotely) — compose is done.
                if active { closeSelf() }
            }
    }

    private var composeForm: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Image(systemName: "record.circle")
                    .font(.system(size: 15))
                    .foregroundStyle(AurisTheme.danger)

                Text("Start meeting")
                    .font(.system(size: 17, weight: .semibold))

                Spacer()

                Button {
                    closeSelf()
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 16))
                        .foregroundStyle(AurisTheme.muted)
                }
                .buttonStyle(.plain)
                .help("Cancel")
            }

            ZStack(alignment: .topLeading) {
                if description.isEmpty {
                    Text("What's this meeting about? (optional)")
                        .foregroundStyle(AurisTheme.subtle)
                        .padding(.top, 10)
                        .padding(.leading, 10)
                        .allowsHitTesting(false)
                }

                TextEditor(text: $description)
                    .font(.system(size: 15))
                    .foregroundStyle(AurisTheme.text)
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
                                    Circle().fill(AurisTheme.panel.opacity(model.settings.overlayOpacity))
                                )
                                .overlay(
                                    Circle().strokeBorder(
                                        dictation?.isLocked == true
                                            ? AurisTheme.danger : AurisTheme.border)
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
            .background(AurisTheme.input.opacity(model.settings.overlayOpacity))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(AurisTheme.border)
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

            MeetingChipStrip(
                attached: selectedMeetings,
                onPick: { showMeetingPicker = true },
                onRemove: { id in
                    selectedMeetings.removeAll { $0.id == id }
                }
            )

            // Assist sensitivity row — three-segment picker. The
            // value is staged into AppModel locally and rides on the
            // next `start_meeting` intent. Tightly-styled to match
            // the metadata chip strip above; appears in every
            // compose surface (no `if` gate — sensitivity is always
            // a meaningful starting choice).
            HStack(spacing: 8) {
                Text("Assist")
                    .font(.caption)
                    .foregroundStyle(AurisTheme.muted)
                Picker("", selection: Binding(
                    get: { model.assistSensitivity },
                    set: { value in
                        Task { await model.setAssistSensitivity(value) }
                    }
                )) {
                    ForEach(AssistSensitivity.allCases, id: \.self) { v in
                        Text(v.displayName).tag(v)
                    }
                }
                .labelsHidden()
                .pickerStyle(.segmented)
            }

            HStack(spacing: 10) {
                Spacer()

                if !model.canStartMeeting {
                    Text(notReadyHint)
                        .font(.caption)
                        .foregroundStyle(AurisTheme.muted)
                }

                Button("Start") {
                    start()
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canStartMeeting)
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
        .sheet(isPresented: $showMeetingPicker) {
            MeetingPickerSheet(
                model: model,
                alreadySelectedIds: Set(selectedMeetings.map { $0.id }),
                excludeMeetingId: model.currentMeetingId,
                onConfirm: { picked in
                    selectedMeetings = picked
                }
            )
        }
    }

    private func start() {
        let trimmed = description.trimmingCharacters(in: .whitespacesAndNewlines)
        let payload: String? = trimmed.isEmpty ? nil : trimmed
        model.setPendingArtifactAttachments(selectedArtifacts.map { $0.id })
        model.setPendingAttachedMeetings(selectedMeetings.map { $0.id })
        descriptionFocused = false
        model.showOverlayWindow()   // overlay paints .starting immediately
        closeSelf()
        Task {
            await model.startMeeting(description: payload)
            if !model.hasActiveMeeting {
                // Start didn't take AND no meeting is running anywhere
                // (hasActiveMeeting, not isMeetingActive: a meeting that
                // just started on another client owns the overlay — don't
                // close its live HUD). Dismiss our orphaned starting
                // spinner and bring compose back with the user's input
                // intact (preserveComposeOnAppear stops onAppear wiping it).
                model.closeOverlayWindow()
                preserveComposeOnAppear = true
                openWindow(id: "start-meeting")
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }

    private func closeSelf() {
        for win in NSApp.windows where win.title == "Start Meeting" {
            win.close()
        }
    }

    private func resetComposeState() {
        description = ""
        selectedArtifacts = []
        selectedMeetings = []
        addingMetadata = false
        if dictation?.isLocked == true { dictation?.toggle() }
    }

    private var notReadyHint: String {
        if model.webSocket.state != .connected { return "Not connected" }
        if !model.permissionMonitor.allGranted { return "Permissions not granted" }
        if model.audioCapture.state != .stopped { return "Audio capture busy" }
        if model.currentMeetingId != nil { return "Meeting already running elsewhere" }
        return ""
    }

    private func ensureDictation() -> DictationController {
        if let d = dictation { return d }
        let d = DictationController(
            serverURL: model.settings.serverURL,
            tokenProvider: { [model] in try await model.auth0.getAccessToken() }
        )
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
        case .listening: AurisTheme.danger
        case .starting, .stopping: AurisTheme.amber
        case .error: AurisTheme.danger
        default: AurisTheme.muted
        }
    }
}
