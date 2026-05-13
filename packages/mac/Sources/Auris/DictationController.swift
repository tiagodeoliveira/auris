// DictationController.swift
// Glue between the compose-panel mic button, the local mic capture,
// and the server-side `/stt` WebSocket. Owned by MeetingOverlayView
// while the compose panel is on screen; one instance per overlay
// presentation.
//
// State machine:
//   .idle       — no dictation in flight.
//   .starting   — mic + STT WS opening; brief, usually 100-300 ms.
//   .listening  — receiving audio + emitting interim/final text.
//   .stopping   — closing the WS, draining final flush.
//
// The controller exposes one bindable `text` property: the view binds
// the description TextEditor to it. On dictation start the controller
// captures whatever's already in the field as `prefix`, then keeps
// `text = prefix + (finalText + interim)` updated as the server
// streams transcripts. The user keeps anything they typed before
// hitting the mic.

import Foundation
import Observation

@MainActor
@Observable
final class DictationController {
    enum State: Equatable {
        case idle
        case starting
        case listening
        case stopping
        case error(String)
    }

    /// Effective description text. View binds the TextEditor to this.
    /// While dictating, mutating it is a no-op so the live STT stream
    /// stays in control of the field; outside dictation it's a normal
    /// `String` and the view writes back through the binding.
    var text: String = ""

    private(set) var state: State = .idle

    private let mic = DictationMicCapture()
    private let session = SttSession()

    /// Server URL + token provider supplied by AppModel — same pair
    /// the WebSocketClient uses, so the same Auth0 silent renew path
    /// applies on every dictation start.
    private let serverURL: () -> String
    private let tokenProvider: @MainActor () async throws -> String

    /// Snapshot of `text` taken when a dictation starts; everything
    /// the STT session emits is appended to this. Stays stable across
    /// the dictation; cleared when the controller transitions back
    /// to `.idle`.
    private var prefix: String = ""

    /// Set when dictation is active so the view's `text` binding can
    /// short-circuit user keystrokes (we don't want a fast typer to
    /// race with incoming server frames). Plain Bool — no @Bindable
    /// wrapping — because the view never reads it directly; it's
    /// gated through `text`'s setter.
    var isLocked: Bool { state == .starting || state == .listening }

    init(
        serverURL: @autoclosure @escaping () -> String,
        tokenProvider: @MainActor @escaping () async throws -> String,
    ) {
        self.serverURL = serverURL
        self.tokenProvider = tokenProvider
        wireSessionCallbacks()
        wireMicCallback()
    }

    private func wireSessionCallbacks() {
        session.onUpdate = { [weak self] finalText, interim in
            guard let self else { return }
            // Prefix may end without trailing space; pad it so the
            // dictated text doesn't collide with the user's last
            // character. Empty prefix -> no leading space.
            let joiner = (self.prefix.isEmpty || self.prefix.hasSuffix(" ")) ? "" : " "
            let body = finalText.isEmpty
                ? interim
                : (interim.isEmpty ? finalText : "\(finalText) \(interim)")
            self.text = "\(self.prefix)\(joiner)\(body)"
        }
        session.onError = { [weak self] err in
            guard let self else { return }
            self.state = .error(err)
            self.mic.stop()
        }
        session.onClosed = { [weak self] in
            guard let self else { return }
            // Closed without an error -> we asked for it. Drop back
            // to idle so the mic button shows the start state again.
            if case .stopping = self.state {
                self.state = .idle
            }
        }
    }

    private func wireMicCallback() {
        mic.onPcm = { [weak self] data in
            // The mic tap fires on a background actor; hop back to
            // MainActor so we can touch the SttSession (also Main).
            // PCM frames are tiny (~1.3 KB at 20 ms / 16 kHz / Int16)
            // so the hop's overhead is negligible.
            Task { @MainActor [weak self] in
                self?.session.feed(data)
            }
        }
    }

    /// Toggle: start dictation if idle, stop if listening. The button
    /// in the compose panel calls this directly so the lifecycle
    /// stays inside the controller.
    func toggle() {
        switch state {
        case .idle, .error:
            start()
        case .listening, .starting:
            Task { await stop() }
        case .stopping:
            break
        }
    }

    private func start() {
        prefix = text
        state = .starting
        do {
            try mic.start()
        } catch {
            state = .error("Mic start failed: \(error.localizedDescription)")
            return
        }
        Task { [weak self] in
            guard let self else { return }
            await self.session.start(
                serverURL: self.serverURL(),
                tokenProvider: self.tokenProvider
            )
            // Server didn't get past the WS handshake — surface the
            // error and tear down the mic so we don't keep capturing
            // PCM nobody's listening to.
            if case .error(let msg) = self.session.state {
                self.state = .error(msg)
                self.mic.stop()
                return
            }
            self.state = .listening
        }
    }

    func stop() async {
        guard state != .idle && state != .stopping else { return }
        state = .stopping
        mic.stop()
        await session.stop()
        prefix = ""
        // .stopping → .idle transitions in `session.onClosed`. If the
        // close fires before our state read above (race), force it.
        if state == .stopping {
            state = .idle
        }
    }
}
