// AppModel.swift
// Single observable owner of app-wide state. Holds the user's
// settings + the live WebSocket connection + the current registered
// device identity. Views read derived state; menu actions call
// methods here.

import AppKit
import Foundation
import Observation

@MainActor
@Observable
final class AppModel {
    // `var` (not `let`) so SwiftUI `@Bindable` can project bindings
    // through to nested observable state (`$model.settings.serverURL`
    // in SettingsView, etc.). We never actually reassign these.
    var settings: AppSettings
    var webSocket: WebSocketClient
    var permissionMonitor: PermissionMonitor
    var audioCapture: AudioCapture
    var audioStreamer: AudioStreamer

    /// This Mac's identity in the server's device registry. Set when
    /// the server replies with `device_registered`; cleared on
    /// disconnect. The full device list (including this one) is
    /// `availableDevices`.
    private(set) var ownDevice: Device?

    /// Snapshot of all registered devices, including this one. Updated
    /// from `snapshot.devices` and per-event `devices_changed`.
    private(set) var availableDevices: [Device] = []

    /// Latest in-flight transcript preview from the server. Replaced
    /// wholesale on each `transcript_interim` event; the meeting
    /// overlay shows this as a dim trailing line below the
    /// transcript-mode items.
    private(set) var transcriptInterim: String = ""

    /// Modes the server has declared available for this meeting.
    /// Static across a session today (transcript / highlights /
    /// actions / open_questions); seeded from `snapshot`.
    private(set) var availableModes: [ModeOption] = []

    /// The currently selected mode. Defaults to "transcript" until a
    /// snapshot arrives. `setMode` updates this optimistically before
    /// sending the intent — the server's `mode_changed` echo confirms
    /// it (or corrects to a different mode on validation failure).
    private(set) var currentMode: String = "transcript"

    /// Optional display-tag emitted with `mode_changed`; carries
    /// per-mode metadata the server wants surfaced (currently unused
    /// in the Mac UI, but decoded so contract drift surfaces here).
    private(set) var displayTag: String? = nil

    /// Server-assigned id of the currently-active meeting. `nil`
    /// when no meeting is active. Updated from `snapshot` and
    /// `meeting_state_changed`. Used today to link to history
    /// (`GET /meetings/<id>`) and as the basis for reconcile-on-
    /// reconnect logic — a different id after a snapshot means the
    /// server's view diverged from ours and we should resync.
    private(set) var currentMeetingId: String? = nil

    /// Wall-clock time the meeting started locally. Used to compute
    /// the `t` ms-offset when the user marks a moment. `nil` when no
    /// meeting is active or when we connected mid-meeting and the
    /// snapshot only told us *that* the meeting was active, not
    /// when it started — moments captured in that window approximate
    /// `t` from the meeting's `started_at` (server-side wall clock)
    /// once we fetch the meeting detail.
    private(set) var meetingStartedAt: Date? = nil

    /// Transient one-shot status line for moment capture. Visible
    /// in the overlay for ~2s after a capture, then cleared. Lets
    /// us confirm the capture without spawning a custom toast view.
    private(set) var momentStatus: String? = nil

    /// Which tab the Settings window should show on next open.
    /// "Settings…" leaves it where the user last was; "Meetings…"
    /// flips it to `.meetings` before opening the window. Bound by
    /// `SettingsView`'s TabView so the tab swap is reactive.
    var selectedSettingsTab: SettingsTab = .server

    /// Whether the floating overlay window is currently shown. Driven
    /// by `MeetingOverlayView`'s onAppear/onDisappear; consumed by
    /// the menu bar to flip the toggle label between Show/Hide. Lets
    /// the user surface the overlay even for PWA-initiated meetings,
    /// which don't auto-open it on this Mac.
    var isOverlayVisible: Bool = false

    /// Items per mode, populated lazily as the server pushes them.
    /// `transcript` mode is fed by the server's transcript
    /// summarizer (one item per committed STT utterance); other
    /// modes by their respective summarizer pipelines.
    private(set) var itemsByMode: [String: [Item]] = [:]

    /// Meeting metadata chips returned by `extract_metadata` and
    /// edited locally via `set_metadata`. StartMeeting intentionally
    /// omits metadata so the server preserves this reviewed state.
    private(set) var metadata: [String: String] = [:]

    /// True while the server is extracting metadata from the current
    /// pre-meeting description.
    private(set) var extractingMetadata: Bool = false

    /// Local audio egress gate. When true, capture keeps running but
    /// `AudioStreamer` drops frames before sending them to the backend.
    private(set) var audioToBackendPaused: Bool = false

    /// The capabilities this Mac advertises. Frozen at app start; will
    /// reflect granted permissions once 2d (permissions onboarding) lands.
    private let advertisedCapabilities: [Capability] = [
        .audioCapture,
        .screenCapture,
        .controlSurface,
        .systemAudio,
    ]

    init() {
        self.settings = AppSettings()
        self.webSocket = WebSocketClient()
        self.permissionMonitor = PermissionMonitor()
        self.audioCapture = AudioCapture()
        self.audioStreamer = AudioStreamer()
        self.webSocket.onMessage = { [weak self] event in
            self?.handle(event: event)
        }
        // Send register_device on every successful (re)connect.
        // The server treats each WS as a fresh device session, so
        // the same registration flow that runs on initial connect
        // must rerun after a transport drop. WebSocketClient fires
        // `onConnected` for both cases.
        self.webSocket.onConnected = { [weak self] in
            self?.sendRegisterDevice()
        }
        // Re-check permissions whenever the app comes back to the
        // foreground (the user may have toggled state in System
        // Settings). Tied to the model's lifetime via [weak self].
        Task { [weak self] in
            for await _ in NotificationCenter.default.notifications(
                named: NSApplication.didBecomeActiveNotification)
            {
                self?.permissionMonitor.refresh()
            }
        }
        // Auto-connect on launch when we already have credentials.
        // Phase 3 will replace the token with an OAuth-derived
        // identity, but the same isConfigured gate applies — the
        // app should never silently sit disconnected just because
        // the user didn't open the menu yet.
        if canConnect {
            connect()
        }
    }

    // MARK: - Derived state for views

    /// SF Symbol name shown as the menu bar icon. Reflects the
    /// connection state at a glance.
    var statusSystemImageName: String {
        switch webSocket.state {
        case .disconnected: settings.isConfigured ? "circle" : "circle.dashed"
        case .connecting: "circle.dotted"
        case .reconnecting: "arrow.clockwise.circle"
        case .connected: "circle.fill"
        case .error: "exclamationmark.circle.fill"
        }
    }

    /// Human-readable status line for the dropdown header.
    var statusLine: String {
        switch webSocket.state {
        case .disconnected:
            settings.isConfigured ? "Not connected" : "Not signed in"
        case .connecting: "Connecting…"
        case .reconnecting: "Reconnecting…"
        case .connected:
            if let d = ownDevice {
                "Connected · registered as \(d.hostname)"
            } else {
                "Connected · registering…"
            }
        case .error(let message): "Error: \(message)"
        }
    }

    /// True when the user can press "Connect" — i.e., settings exist
    /// and we're not already connecting/connected/reconnecting.
    var canConnect: Bool {
        settings.isConfigured && webSocket.state == .disconnected
    }

    /// True when the user can press "Disconnect". `.reconnecting`
    /// counts — letting the user cancel the backoff loop is a real
    /// affordance ("stop trying").
    var canDisconnect: Bool {
        switch webSocket.state {
        case .connecting, .connected, .reconnecting: true
        default: false
        }
    }

    // MARK: - Intent

    /// Open a WS connection using the current settings.
    /// `WebSocketClient` will dial in, retry with backoff on drops,
    /// and call `onConnected` once the handshake succeeds — that's
    /// where `register_device` actually goes out.
    func connect() {
        webSocket.connect(serverURL: settings.serverURL, token: settings.token)
    }

    /// Tear down the current WS connection. Server marks our device
    /// offline as a side effect of the close. Also opts out of the
    /// auto-reconnect loop until the user calls `connect()` again.
    func disconnect() {
        webSocket.disconnect()
        ownDevice = nil
        availableDevices = []
    }

    /// Build and send the `register_device` intent. Fired once per
    /// successful (re)connect by `webSocket.onConnected`.
    private func sendRegisterDevice() {
        let intent = RegisterDeviceIntent(
            hostname: Self.hostname(),
            capabilities: advertisedCapabilities
        )
        Task { [weak webSocket] in
            try? await webSocket?.send(intent: intent)
        }
    }

    /// True when a meeting is in progress or being started locally.
    var isMeetingActive: Bool {
        switch audioCapture.state {
        case .running, .starting: true
        default: false
        }
    }

    /// True when starting a meeting is meaningful — connected to
    /// the server, permissions in hand, no capture currently running.
    var canStartMeeting: Bool {
        webSocket.state == .connected
            && permissionMonitor.allGranted
            && audioCapture.state == .stopped
    }

    var canToggleBackendAudio: Bool {
        audioCapture.state == .running && audioStreamer.state == .streaming
    }

    /// Start a meeting end-to-end from the Mac. The sequence is
    /// order-sensitive:
    ///
    ///   1. Start audio capture (creates the AsyncStream).
    ///   2. Open the /audio WS streamer; first frame installs the
    ///      receiver into the server's RemoteAudioSource slot.
    ///   3. Wait for the streamer to confirm streaming state.
    ///   4. Send start_meeting on the control WS — server takes the
    ///      receiver out of the slot at this point.
    ///
    /// Reversing 3↔4 leaves the meeting in a "no audio source bound"
    /// state — the server's NotConnected error path. Phase 2g-2 will
    /// add metadata (extracted tags) to the intent.
    func startMeeting(description: String? = nil) async {
        guard canStartMeeting else { return }

        clearTranscript()
        guard await startAudioStream() else { return }
        // Stamp the local start so `markMoment` can compute `t`
        // without round-tripping the server. Cleared on teardown.
        meetingStartedAt = Date()

        do {
            try await webSocket.send(
                intent: StartMeetingIntent(
                    description: description,
                    audioSourceDeviceId: ownDevice?.id
                )
            )
            print("[AppModel] start_meeting sent (description=\(description ?? "nil"), source=\(ownDevice?.id ?? "nil"))")
        } catch {
            print("[AppModel] start_meeting send failed: \(error)")
            audioStreamer.stop()
            audioCapture.stop()
            meetingStartedAt = nil
        }
    }

    func stopMeeting() async {
        // Send stop_meeting first so the server tears down its
        // pipeline cleanly before we cut the audio source.
        do {
            try await webSocket.send(intent: StopMeetingIntent())
            print("[AppModel] stop_meeting sent")
        } catch {
            print("[AppModel] stop_meeting send failed: \(error)")
        }
        localMeetingTeardown()
    }

    /// Stop the local capture + streamer and reset meeting-scoped
    /// state, *without* sending `stop_meeting`. Used when the
    /// server has already torn down (server restart → snapshot
    /// arrives with `meeting_state: idle`); sending stop_meeting in
    /// that path would be a no-op at best, an error at worst.
    private func localMeetingTeardown() {
        audioStreamer.stop()
        audioCapture.stop()
        metadata = [:]
        extractingMetadata = false
        audioToBackendPaused = false
        meetingStartedAt = nil
        momentStatus = nil
    }

    /// Mark a moment in the active meeting. Captures a screenshot
    /// of the primary display, computes `t` ms-since-meeting-start,
    /// and POSTs both to `/meetings/:id/moments`. The server-side
    /// async worker fills in the LLM summary a few seconds later.
    /// Updates `momentStatus` so the overlay can flash a confirmation.
    func markMoment(note: String? = nil) async {
        guard let meetingId = currentMeetingId else {
            momentStatus = "No active meeting"
            scheduleMomentStatusClear()
            return
        }
        guard let api = MeetingsAPI.fromWSURL(settings.serverURL, token: settings.token) else {
            momentStatus = "Invalid server URL"
            scheduleMomentStatusClear()
            return
        }

        momentStatus = "Capturing…"

        // Compute t. If we don't have a local start (e.g. recovered
        // mid-meeting from a server reboot), best-effort with 0 — the
        // moment still lands; the timeline ordering will be slightly
        // off until the user starts a fresh meeting.
        let t: Int64
        if let start = meetingStartedAt {
            t = Int64(max(0, Date().timeIntervalSince(start) * 1000))
        } else {
            t = 0
        }

        // Screenshot capture is best-effort — if it fails we still
        // try to post the moment (without an image). The user gets
        // a status hint either way.
        var screenshot: Data? = nil
        do {
            screenshot = try await ScreenshotCapture.capturePrimaryDisplay()
        } catch {
            print("[AppModel] screenshot capture failed: \(error.localizedDescription)")
            momentStatus = "No screenshot · sending"
        }

        do {
            _ = try await api.createMoment(
                meetingId: meetingId,
                t: t,
                note: note,
                screenshotPNG: screenshot
            )
            momentStatus = screenshot == nil ? "Moment saved (no screenshot)" : "Moment saved"
        } catch {
            print("[AppModel] createMoment failed: \(error.localizedDescription)")
            momentStatus = "Save failed: \(error.localizedDescription)"
        }
        scheduleMomentStatusClear()
    }

    /// Reactive variant of `markMoment` for moments born on the WS
    /// path (PWA's mark_moment intent). The server has already
    /// inserted the row and decided we're the right device to grab a
    /// screenshot — we just capture and upload. Any failure logs and
    /// drops the screenshot; the moment row + summary are unaffected.
    private func captureAndUploadMomentScreenshot(meetingId: String, momentId: String) async {
        guard let api = MeetingsAPI.fromWSURL(settings.serverURL, token: settings.token) else {
            print("[AppModel] capture_moment_screenshot: invalid server URL")
            return
        }
        let png: Data
        do {
            png = try await ScreenshotCapture.capturePrimaryDisplay()
        } catch {
            print("[AppModel] capture_moment_screenshot: capture failed: \(error.localizedDescription)")
            return
        }
        do {
            try await api.uploadMomentScreenshot(meetingId: meetingId, momentId: momentId, png: png)
            print("[AppModel] capture_moment_screenshot: uploaded for moment=\(momentId)")
        } catch {
            print("[AppModel] capture_moment_screenshot: upload failed: \(error.localizedDescription)")
        }
    }

    /// Clear `momentStatus` after a short delay so the toast-like
    /// label fades cleanly. Always schedules; cancelling the timer
    /// on rapid successive captures isn't worth the bookkeeping.
    private func scheduleMomentStatusClear() {
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            self?.momentStatus = nil
        }
    }

    /// React to the server's audio-source binding. The server emits
    /// `audio_source_device_changed` whenever a meeting starts/stops
    /// or the binding shifts; clients react by starting or stopping
    /// their own `/audio` stream depending on whether *they* are the
    /// chosen source. PWA-initiated meetings targeting this Mac flow
    /// through this path — the Mac doesn't initiate, the server tells
    /// it to start.
    private func reconcileAudioSource(boundDeviceId: String?) async {
        let isUs = boundDeviceId != nil && boundDeviceId == ownDevice?.id
        if isUs {
            if audioCapture.state == .stopped {
                print("[AppModel] audio_source bound to us — starting capture")
                clearTranscript()
                _ = await startAudioStream()
                // Approximation: stamp local start now. PWA-initiated
                // meetings don't carry the wall-clock start over this
                // event, so moments captured early will have a `t`
                // offset that's a couple seconds off. Acceptable.
                meetingStartedAt = Date()
            }
        } else {
            if audioCapture.state != .stopped {
                print("[AppModel] audio_source no longer us — stopping capture")
                audioStreamer.stop()
                audioCapture.stop()
                meetingStartedAt = nil
            }
        }
    }

    func toggleBackendAudio() {
        guard canToggleBackendAudio else { return }
        audioToBackendPaused.toggle()
        audioStreamer.setMuted(audioToBackendPaused)
        print("[AppModel] backend audio \(audioToBackendPaused ? "paused" : "resumed")")
    }

    func extractMetadata(description: String) async {
        let trimmed = description.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, webSocket.state == .connected, !extractingMetadata else { return }

        extractingMetadata = true
        do {
            try await webSocket.send(intent: ExtractMetadataIntent(description: trimmed))
            print("[AppModel] extract_metadata sent")
        } catch {
            extractingMetadata = false
            print("[AppModel] extract_metadata send failed: \(error)")
        }
    }

    func setMetadata(key: String, value: String?) async {
        let k = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !k.isEmpty, webSocket.state == .connected else { return }
        let v = value?.trimmingCharacters(in: .whitespacesAndNewlines)

        do {
            try await webSocket.send(intent: SetMetadataIntent(
                key: k,
                value: v?.isEmpty == true ? nil : v
            ))
        } catch {
            print("[AppModel] set_metadata send failed: \(error)")
        }
    }

    // MARK: - Event handling

    /// Apply a decoded server event to local state. Called from
    /// `WebSocketClient.onMessage` (set up in init).
    private func handle(event: TypedServerEvent) {
        switch event {
        case .snapshot(let payload):
            availableDevices = payload.devices
            metadata = payload.metadata
            availableModes = payload.availableModes
            currentMode = payload.mode
            displayTag = payload.displayTag
            currentMeetingId = payload.meetingId
            // Snapshot only carries the *current* mode's items; the
            // others stay empty until the user clicks them (server
            // replies with `mode_changed` carrying that mode's list).
            itemsByMode = [payload.mode: payload.items]
            // Server vs. local state divergence: typically a server
            // restart (no persistence yet — meeting state is just
            // gone). Tear down our locally-running meeting so the
            // overlay flips back to compose; the user can hit Start
            // again on a fresh server.
            if payload.meetingState == "idle", isMeetingActive {
                print("[AppModel] snapshot meeting_state=idle while local meeting active — tearing down")
                localMeetingTeardown()
            }
        case .meetingStateChanged(let state, let meetingId):
            currentMeetingId = meetingId
            if state == "idle" {
                metadata = [:]
                extractingMetadata = false
                audioToBackendPaused = false
            }
        case .deviceRegistered(let device):
            ownDevice = device
            // Keep availableDevices in sync in case the broadcast
            // hasn't landed yet.
            if !availableDevices.contains(where: { $0.id == device.id }) {
                availableDevices.append(device)
            }
        case .devicesChanged(let devices):
            availableDevices = devices
            // If our own device was removed (e.g., server-side
            // unregister), clear the local mirror.
            if let ours = ownDevice, !devices.contains(where: { $0.id == ours.id }) {
                ownDevice = nil
            }
        case .audioSourceDeviceChanged(let boundDeviceId):
            // The server is telling us which device should be feeding
            // /audio. If it's us, ensure capture+streamer are running
            // (handles PWA-initiated meetings that targeted this Mac).
            // If it's someone else (or None), tear down our local
            // capture so we're not double-streaming. Idempotent —
            // audioCapture.start() is a no-op when already running.
            Task { await reconcileAudioSource(boundDeviceId: boundDeviceId) }
        case .captureMomentScreenshot(let target, let meetingId, let momentId, _):
            // Targeted broadcast: only the addressed device acts. The
            // server already filters by capability+online before
            // emitting, so reaching here means we're it.
            guard ownDevice?.id == target else { break }
            Task { await captureAndUploadMomentScreenshot(meetingId: meetingId, momentId: momentId) }
        case .metadataChanged(let next):
            metadata = next
            extractingMetadata = false
        case .transcriptInterim(let text):
            transcriptInterim = text
        case .transcriptCommitted:
            // Same content arrives as a transcript-mode `Item` via
            // `items_update` (server-side transcript summarizer);
            // the overlay reads from `itemsByMode["transcript"]`.
            // Variant kept decoded so future consumers can attach.
            break
        case .modeChanged(let mode, let tag, let items):
            currentMode = mode
            displayTag = tag
            itemsByMode[mode] = items
        case .itemsUpdate(let mode, let items):
            mergeItems(items, into: mode)
        case .error(let code, let message):
            if extractingMetadata { extractingMetadata = false }
            print("[AppModel] server error \(code): \(message)")
        case .unknown:
            // Unknown events fall through silently; we'll add cases
            // as we light up more flows.
            break
        }
    }

    /// Clear the live transcript on meeting boundaries — keeps the
    /// overlay from carrying state across meetings.
    func clearTranscript() {
        transcriptInterim = ""
        itemsByMode = [:]
        currentMode = "transcript"
        displayTag = nil
    }

    /// Send `set_mode` to the server. Optimistically updates
    /// `currentMode` first so the overlay snaps immediately;
    /// `mode_changed` echoes back with the items list.
    func setMode(_ mode: String) async {
        guard mode != currentMode else { return }
        guard webSocket.state == .connected else { return }
        currentMode = mode
        do {
            try await webSocket.send(intent: SetModeIntent(mode: mode))
        } catch {
            print("[AppModel] set_mode send failed: \(error)")
        }
    }

    /// Merge an `items_update` payload into the buffer for `mode`,
    /// honoring the mode's declared `UpdateStrategy`. Falls back to
    /// `append` if the mode isn't in `availableModes` (defensive —
    /// shouldn't happen but the server's broadcast would race).
    private func mergeItems(_ incoming: [Item], into mode: String) {
        let strategy = availableModes.first(where: { $0.id == mode })?.updateStrategy ?? .append
        switch strategy {
        case .replace:
            itemsByMode[mode] = incoming
        case .append:
            var current = itemsByMode[mode] ?? []
            current.append(contentsOf: incoming)
            // Bound long meetings — same 500-line ceiling that the
            // old `transcriptHistory` had. Replace strategy modes
            // are server-capped at 10 so they don't need this.
            if current.count > 500 {
                current.removeFirst(current.count - 500)
            }
            itemsByMode[mode] = current
        }
    }

    private func startAudioStream() async -> Bool {
        do {
            try await audioCapture.start()
        } catch {
            return false  // surfaced via audioCapture.state
        }
        guard let frames = audioCapture.output else { return false }

        audioToBackendPaused = false
        audioStreamer.start(
            serverURL: settings.serverURL,
            token: settings.token,
            frames: frames)

        // No need to wait for the /audio WS to open before sending
        // start_meeting. The server's `RemoteAudioSource` is now
        // late-binding: meeting can start with no audio client and
        // pick one up later, and a mid-meeting `/audio` reconnect
        // reuses the same downstream rx. The streamer's own
        // backoff loop handles transport drops in the background.
        return true
    }

    // MARK: - Helpers

    /// Best-effort hostname for device registration. Falls back to a
    /// stable but human-readable label if Host info is unavailable.
    private static func hostname() -> String {
        if let host = Host.current().localizedName, !host.isEmpty {
            return host
        }
        if let name = ProcessInfo.processInfo.hostName.split(separator: ".").first {
            return String(name)
        }
        return "Mac"
    }
}
