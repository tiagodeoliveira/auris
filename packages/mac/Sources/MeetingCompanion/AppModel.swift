// AppModel.swift
// Single observable owner of app-wide state. Holds the user's
// settings + the live WebSocket connection + the current registered
// device identity. Views read derived state; menu actions call
// methods here.

import AppKit
import Foundation
import Observation

/// Upload state for a single staged chat attachment.
enum ChatAttachmentUploadState: Equatable {
    case uploading
    case uploaded(id: String)
    case failed(message: String)
}

/// A screenshot the user has staged for the next chat send. Held in
/// `AppModel.pendingChatAttachments` between capture and send.
struct ChatAttachmentDraft: Identifiable, Equatable {
    /// Local-only id so SwiftUI can key the chip strip. Distinct from
    /// the server-assigned attachment id (which lives in `state`
    /// once upload completes).
    let id: UUID

    /// Source PNG for the thumbnail render. Retained for the chip's
    /// lifetime in the strip.
    let image: NSImage

    /// Raw PNG bytes, kept until upload succeeds. Cleared after
    /// `.uploaded` to free memory; the server has the only copy now.
    var pngBytes: Data

    /// Upload lifecycle.
    var state: ChatAttachmentUploadState

    // NSImage doesn't conform to Equatable, so compare only the
    // fields SwiftUI's diffing cares about. The image and pngBytes
    // are immutable from SwiftUI's perspective for a given id.
    static func == (lhs: ChatAttachmentDraft, rhs: ChatAttachmentDraft) -> Bool {
        lhs.id == rhs.id && lhs.state == rhs.state
    }
}

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
    /// Auth0 native client. Owns refresh + access tokens; surfaces
    /// `getAccessToken()` to anything that needs to talk to the
    /// server. Always present; signed-out state means there's no
    /// refresh token on disk and `isSignedIn == false`.
    var auth0: Auth0Client

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

    /// Library artifact IDs the user picked during compose. Drained
    /// when MeetingStateChanged fires with state=active and a
    /// non-nil `meetingId` — at that point the meeting exists
    /// server-side and we POST `/meetings/:id/artifacts` once per
    /// id. Cleared on idle so a future compose starts fresh.
    private(set) var pendingArtifactAttachments: [String] = []

    /// Artifact IDs we've successfully attached to the current
    /// active meeting. Mid-meeting picker reads this to pre-check
    /// already-attached rows; updated as attaches succeed and on
    /// the active transition (compose-time attaches roll in here).
    /// Cleared on idle.
    private(set) var currentMeetingAttachedArtifactIds: Set<String> = []

    /// Past-meeting IDs the user picked during compose. Drained when
    /// MeetingStateChanged fires with state=active and a non-nil
    /// `meetingId` — mirrors `pendingArtifactAttachments`. Linking
    /// a past meeting is optional (zero or more); the server is
    /// idempotent on attach so re-firing a stale queue is safe.
    private(set) var pendingAttachedMeetings: [String] = []

    /// Past-meeting IDs we've successfully attached to the current
    /// active meeting (mirrors `currentMeetingAttachedArtifactIds`).
    /// Mid-meeting picker reads this to pre-check already-attached
    /// rows. Cleared on idle.
    private(set) var currentMeetingAttachedMeetingIds: Set<String> = []

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

    /// Screenshots staged for the next chat send. Capture appends a
    /// draft in `.uploading`; the upload Task mutates it to `.uploaded`
    /// or `.failed`. Cap of 4 — UI button disables when reached.
    /// Cleared on meeting-state → idle (orphan rows on the server are
    /// cascade-deleted with the meeting).
    private(set) var pendingChatAttachments: [ChatAttachmentDraft] = []
    let chatAttachmentLimit = 4

    /// Transient toast-like message for chat-attachment / chat-send
    /// status (mirrors `momentStatus`). Cleared by
    /// `scheduleChatStatusClear()` after ~2s.
    private(set) var chatStatus: String? = nil

    /// Which tab the Settings window should show on next open.
    /// "Settings…" leaves it where the user last was; "Meetings…"
    /// flips it to `.meetings` before opening the window. Bound by
    /// `SettingsView`'s TabView so the tab swap is reactive.
    var selectedSettingsTab: SettingsTab = .account

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
        self.auth0 = Auth0Client()
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
        case .disconnected: auth0.isSignedIn ? "circle" : "circle.dashed"
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
            auth0.isSignedIn ? "Not connected" : "Not signed in"
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

    /// True when the user can press "Connect" — i.e., signed in,
    /// server URL set, and the WS is currently dropped.
    var canConnect: Bool {
        !settings.serverURL.isEmpty && auth0.isSignedIn
            && webSocket.state == .disconnected
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

    /// Build a `MeetingsAPI` against the configured server URL with a
    /// freshly-fetched access token. Throws if the user isn't signed
    /// in or the URL doesn't parse.
    func makeMeetingsAPI() async throws -> MeetingsAPI {
        let token = try await auth0.getAccessToken()
        guard let api = MeetingsAPI.fromWSURL(settings.serverURL, token: token) else {
            throw NSError(
                domain: "AppModel", code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Invalid server URL"])
        }
        return api
    }

    /// Sibling of `makeMeetingsAPI` for the artifact subsystem. Same
    /// auth + URL plumbing; separate type so the two clients can
    /// evolve independently.
    func makeArtifactsAPI() async throws -> ArtifactsAPI {
        let token = try await auth0.getAccessToken()
        guard let api = ArtifactsAPI.fromWSURL(settings.serverURL, token: token) else {
            throw NSError(
                domain: "AppModel", code: -1,
                userInfo: [NSLocalizedDescriptionKey: "Invalid server URL"])
        }
        return api
    }

    /// Stage artifact ids to attach once the next start_meeting
    /// transitions the server to `active`. The compose UI calls
    /// this; `handle(event:)` consumes on the active transition.
    func setPendingArtifactAttachments(_ ids: [String]) {
        pendingArtifactAttachments = ids
    }

    /// Attach pending artifacts to a freshly-active meeting. Best
    /// effort — log on failure, don't fail the meeting itself.
    /// Per-artifact attach errors are independent so one rejection
    /// (e.g., status not yet `done`) doesn't drop the others.
    private func attachArtifacts(ids: [String], toMeeting meetingId: String) async {
        let api: ArtifactsAPI
        do {
            api = try await makeArtifactsAPI()
        } catch {
            print("[AppModel] attachArtifacts: makeArtifactsAPI failed: \(error)")
            return
        }
        for aid in ids {
            do {
                try await api.attach(meetingId: meetingId, artifactId: aid)
                currentMeetingAttachedArtifactIds.insert(aid)
                print("[AppModel] attached artifact \(aid) to meeting \(meetingId)")
            } catch {
                print("[AppModel] attach \(aid) failed: \(error)")
            }
        }
    }

    /// Mid-meeting attach. Caller (overlay's `…` menu picker) hands
    /// us a set of artifact ids; we attach each to whatever meeting
    /// is currently active. Idempotent server-side, so re-attaching
    /// the same artifact is a no-op.
    func attachArtifactsToCurrentMeeting(ids: [String]) async {
        guard let mid = currentMeetingId else {
            print("[AppModel] attachArtifactsToCurrentMeeting: no active meeting")
            return
        }
        await attachArtifacts(ids: ids, toMeeting: mid)
    }

    /// Stage past-meeting ids to attach once the next start_meeting
    /// transitions the server to `active`. Mirrors
    /// `setPendingArtifactAttachments`. Compose UI calls this; the
    /// active-state handler drains the queue.
    func setPendingAttachedMeetings(_ ids: [String]) {
        pendingAttachedMeetings = ids
    }

    /// Best-effort meeting-attach loop. Per-meeting errors are logged
    /// independently so one rejection doesn't drop the rest.
    private func attachMeetings(ids: [String], toMeeting parentId: String) async {
        let api: MeetingsAPI
        do {
            api = try await makeMeetingsAPI()
        } catch {
            print("[AppModel] attachMeetings: makeMeetingsAPI failed: \(error)")
            return
        }
        for mid in ids {
            do {
                try await api.attach(parentId: parentId, attachedId: mid)
                currentMeetingAttachedMeetingIds.insert(mid)
                print("[AppModel] attached meeting \(mid) to meeting \(parentId)")
            } catch {
                print("[AppModel] attach meeting \(mid) failed: \(error)")
            }
        }
    }

    /// Mid-meeting attach for past meetings — sibling of
    /// `attachArtifactsToCurrentMeeting`. Idempotent server-side.
    func attachMeetingsToCurrentMeeting(ids: [String]) async {
        guard let mid = currentMeetingId else {
            print("[AppModel] attachMeetingsToCurrentMeeting: no active meeting")
            return
        }
        await attachMeetings(ids: ids, toMeeting: mid)
    }

    /// Open a WS connection using the current settings + a token
    /// fetched from the Auth0 client. `WebSocketClient` calls back
    /// into the provider on every (re)connect, so an expired access
    /// token is silently refreshed before the new socket dials in.
    func connect() {
        print("[AppModel] connect() — serverURL=\(settings.serverURL) signedIn=\(auth0.isSignedIn)")
        webSocket.connect(
            serverURL: settings.serverURL,
            tokenProvider: { [auth0] in
                try await auth0.getAccessToken()
            }
        )
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

    /// Close the floating meeting-overlay window via AppKit. Looks
    /// up the window by its SwiftUI scene title ("Meeting"). Used
    /// when the server signals meeting end so the overlay doesn't
    /// linger in its live state — SwiftUI's `dismissWindow(id:)`
    /// has been observed to silently no-op for our menu-bar
    /// accessory app, so we go through AppKit directly.
    private func closeOverlayWindow() {
        for win in NSApp.windows where win.title == "Meeting" {
            win.close()
        }
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

    /// Mark a moment in the active meeting. Sends the WS intent and
    /// trusts the server to insert the row + delegate the screenshot
    /// back to us via `capture_moment_screenshot` (handled by
    /// `captureAndUploadMomentScreenshot`). Optimistic UI: the
    /// `momentStatus` flips to "Moment saved" right away — if the
    /// intent never lands (e.g. WS is mid-reconnect) the row simply
    /// won't appear in the meeting history.
    func markMoment(note: String? = nil) async {
        guard currentMeetingId != nil else {
            momentStatus = "No active meeting"
            scheduleMomentStatusClear()
            return
        }
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
        do {
            try await webSocket.send(intent: MarkMomentIntent(t: t, note: note))
            momentStatus = "Moment saved"
        } catch {
            print("[AppModel] markMoment send failed: \(error.localizedDescription)")
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
        let api: MeetingsAPI
        do {
            api = try await makeMeetingsAPI()
        } catch {
            print("[AppModel] capture_moment_screenshot: API setup failed: \(error.localizedDescription)")
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

    // MARK: - Chat attachments (staged screenshots)

    /// Capture the primary display, append a chip in `.uploading`,
    /// fire the upload Task. Gated on active/paused meeting + cap.
    /// The UI is responsible for not calling this from idle, but we
    /// double-check defensively here.
    func captureChatAttachment() async {
        guard webSocket.state == .connected else {
            chatStatus = "Not connected"
            scheduleChatStatusClear()
            return
        }
        guard let meetingId = currentMeetingId else {
            chatStatus = "No active meeting"
            scheduleChatStatusClear()
            return
        }
        guard pendingChatAttachments.count < chatAttachmentLimit else {
            chatStatus = "Maximum \(chatAttachmentLimit) screenshots per message"
            scheduleChatStatusClear()
            return
        }

        let png: Data
        do {
            png = try await ScreenshotCapture.capturePrimaryDisplay()
        } catch {
            chatStatus = "Capture failed: \(error.localizedDescription)"
            scheduleChatStatusClear()
            return
        }

        let draft = ChatAttachmentDraft(
            id: UUID(),
            image: NSImage(data: png) ?? NSImage(),
            pngBytes: png,
            state: .uploading
        )
        pendingChatAttachments.append(draft)

        Task { [weak self] in
            await self?.uploadChatAttachment(draftId: draft.id, meetingId: meetingId, png: png)
        }
    }

    /// Remove a chip from the local queue. The server-side row (if
    /// upload already landed) is left as an orphan and cleaned up
    /// when the parent meeting is deleted.
    func removeChatAttachment(draftId: UUID) {
        pendingChatAttachments.removeAll { $0.id == draftId }
    }

    /// Background upload for a staged chat attachment. Mutates the
    /// matching draft to `.uploaded` (with the server id) or
    /// `.failed` (with a user-readable message). Run-to-completion;
    /// removal of the chip during upload makes the final mutate a
    /// no-op.
    private func uploadChatAttachment(draftId: UUID, meetingId: String, png: Data) async {
        let api: MeetingsAPI
        do {
            api = try await makeMeetingsAPI()
        } catch {
            updateChatDraft(draftId: draftId) { d in
                d.state = .failed(message: "API setup failed: \(error.localizedDescription)")
            }
            return
        }
        do {
            let id = try await api.uploadChatAttachment(meetingId: meetingId, png: png)
            updateChatDraft(draftId: draftId) { d in
                d.state = .uploaded(id: id)
                d.pngBytes = Data()        // drop bytes; server has them
            }
        } catch {
            updateChatDraft(draftId: draftId) { d in
                d.state = .failed(message: error.localizedDescription)
            }
        }
    }

    /// Locate a draft by id, mutate via the caller's closure, and
    /// write the (copy) back so SwiftUI's `@Observable` macro sees
    /// the change as an array assignment. In-place struct mutation
    /// would also work, but the replace-by-index pattern matches the
    /// rest of the file (see `mergeItems`, `itemUpdated`).
    private func updateChatDraft(draftId: UUID, mutate: (inout ChatAttachmentDraft) -> Void) {
        guard let idx = pendingChatAttachments.firstIndex(where: { $0.id == draftId }) else {
            return
        }
        var d = pendingChatAttachments[idx]
        mutate(&d)
        pendingChatAttachments[idx] = d
    }

    /// Whether `sendChat` is currently allowed to dispatch. Blocked
    /// while any attachment is mid-upload — sending early would ship
    /// a chat with fewer images than the user staged.
    var canSendChatNow: Bool {
        !pendingChatAttachments.contains {
            if case .uploading = $0.state { return true }
            return false
        }
    }

    /// Clear `chatStatus` after a short delay so the toast-like
    /// label fades cleanly. Mirrors `scheduleMomentStatusClear`.
    private func scheduleChatStatusClear() {
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(2))
            self?.chatStatus = nil
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
                // The meeting stopped — could be us (this Mac sent
                // StopMeeting), the PWA (other client), or a server
                // restart. In every case we need to tear down local
                // audio capture, clear per-meeting transient state,
                // AND close the overlay window directly via AppKit.
                //
                // We tried using SwiftUI's `dismissWindow(id:)` from
                // the overlay's .onChange — it silently no-ops on
                // .accessory-policy menu-bar apps once the window's
                // borderless/titled style is mucked with. Closing
                // through AppKit's NSWindow.close() always works.
                print("[AppModel] meeting_state=idle — closing overlay")
                localMeetingTeardown()
                pendingArtifactAttachments = []
                currentMeetingAttachedArtifactIds = []
                pendingAttachedMeetings = []
                currentMeetingAttachedMeetingIds = []
                pendingChatAttachments = []
                closeOverlayWindow()
            }
            if state == "active", let mid = meetingId, !pendingArtifactAttachments.isEmpty {
                let ids = pendingArtifactAttachments
                pendingArtifactAttachments = []
                Task { await attachArtifacts(ids: ids, toMeeting: mid) }
            }
            if state == "active", let mid = meetingId, !pendingAttachedMeetings.isEmpty {
                let ids = pendingAttachedMeetings
                pendingAttachedMeetings = []
                Task { await attachMeetings(ids: ids, toMeeting: mid) }
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
        case .captureMomentScreenshot(let meetingId, let momentId, _):
            // Server-side per-connection routing means we only
            // receive this event when the server picked us; no
            // client-side filter required.
            Task { await captureAndUploadMomentScreenshot(meetingId: meetingId, momentId: momentId) }
        case .metadataChanged(let next):
            metadata = next
            extractingMetadata = false
        case .transcriptInterim(let text):
            transcriptInterim = text
        case .modeChanged(let mode, let tag, let items):
            currentMode = mode
            displayTag = tag
            itemsByMode[mode] = items
        case .itemsUpdate(let mode, let items):
            mergeItems(items, into: mode)
        case .itemUpdated(let mode, let item):
            // One-row in-place update. Replace the matching item by
            // id. Used by the expand_item flow to land the agent's
            // expansion into the item's `detail` field. If the id
            // isn't in the current list (rare — meeting end race),
            // drop silently.
            if var current = itemsByMode[mode],
                let idx = current.firstIndex(where: { $0.id == item.id })
            {
                current[idx] = item
                itemsByMode[mode] = current
            }
        case .artifactsChanged(let ids):
            // Server-authoritative attached set. Replaces whatever
            // we had locally — the user may have attached or
            // detached on the OTHER client (PWA), and the overlay's
            // attach picker pre-checks rows against this.
            currentMeetingAttachedArtifactIds = Set(ids)
        case .attachedMeetingsChanged(let ids):
            currentMeetingAttachedMeetingIds = Set(ids)
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

    /// Ask the agent to expand on a specific item. Server validates
    /// that the item id exists, kicks the agent through the same
    /// channel chat uses; the agent's text reply lands as the
    /// item's `detail` via `Event::ItemUpdated`.
    func expandItem(_ itemId: String) async {
        guard webSocket.state == .connected else { return }
        do {
            try await webSocket.send(intent: ExpandItemIntent(item_id: itemId))
        } catch {
            print("[AppModel] expand_item send failed: \(error)")
        }
    }

    /// Send a user chat message to the agent. Server validates that
    /// a meeting is active/paused; the agent's reply lands as new
    /// chat-mode items via the standard `ItemsUpdate` event.
    ///
    /// Optimistic echo: APPENDS the user's question + a "Thinking…"
    /// placeholder to the existing chat thread (chat is now an
    /// Append-strategy mode — prior turns stay visible). The
    /// server's `ItemsUpdate` is decoded by `mergeItems`, which
    /// strips items whose id starts with `chat-*-pending-` before
    /// appending the real Q+A pair so we don't end up with the
    /// pending bubbles lingering alongside the real ones.
    func sendChat(_ text: String) async {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let hasText = !trimmed.isEmpty
        let hasAttachments = !pendingChatAttachments.isEmpty
        // Allow image-only sends (empty text + at least one staged
        // screenshot). The agent's vision prompt handles the
        // attachment-only case server-side.
        guard hasText || hasAttachments else { return }
        guard webSocket.state == .connected else { return }

        // Block while any chip is mid-upload — shipping early would
        // drop images the user thinks they attached.
        if !canSendChatNow {
            chatStatus = "Waiting for screenshots to upload…"
            scheduleChatStatusClear()
            return
        }

        // Drain BEFORE the optimistic echo and WS send. Failed chips
        // are silently dropped from the dispatch but surfaced via a
        // status toast so the user knows their images didn't ride.
        let uploadedIds: [String] = pendingChatAttachments.compactMap { d in
            if case .uploaded(let id) = d.state { return id } else { return nil }
        }
        let failedCount = pendingChatAttachments.count - uploadedIds.count
        pendingChatAttachments = []

        if failedCount > 0 {
            chatStatus = "Skipped \(failedCount) failed screenshot\(failedCount == 1 ? "" : "s")"
            scheduleChatStatusClear()
        }

        // User bubble text. Empty-text + images shows a representative
        // label in the optimistic echo; the server's items_update
        // replaces it with whatever the real items reflect.
        let bubbleText = hasText ? trimmed : "[Image attached]"

        let userBubble = Item(
            id: "chat-q-pending-\(UUID().uuidString)",
            text: bubbleText,
            detail: nil,
            t: 0,
            meta: ItemMeta(
                speaker: nil, role: "user",
                importance: nil, owner: nil, due: nil,
                kind: nil, context: nil
            )
        )
        let pendingBubble = Item(
            id: "chat-a-pending-\(UUID().uuidString)",
            text: "Thinking…",
            detail: nil,
            t: 0,
            meta: ItemMeta(
                speaker: nil, role: "assistant-pending",
                importance: nil, owner: nil, due: nil,
                kind: nil, context: nil
            )
        )
        var current = itemsByMode["chat"] ?? []
        current.append(userBubble)
        current.append(pendingBubble)
        itemsByMode["chat"] = current
        do {
            try await webSocket.send(intent: ChatIntent(text: trimmed, attachmentIds: uploadedIds))
        } catch {
            print("[AppModel] chat send failed: \(error)")
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
            // Chat-mode special: drop optimistic-echo placeholders
            // (id prefix `chat-q-pending-` / `chat-a-pending-`)
            // before appending the server's real Q+A pair so the
            // pending bubbles don't linger after each send.
            if mode == "chat" {
                current.removeAll { $0.id.hasPrefix("chat-q-pending-") || $0.id.hasPrefix("chat-a-pending-") }
            }
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
            tokenProvider: { [auth0] in
                try await auth0.getAccessToken()
            },
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
