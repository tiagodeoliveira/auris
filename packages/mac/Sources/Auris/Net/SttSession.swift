// SttSession.swift
// One-shot dictation client. Connects to the server's `/stt`
// WebSocket; the server holds the upstream provider's API key and
// can swap providers without a Mac client redeploy.
//
// Wire shape mirrors `packages/server/src/stt_ws.rs`:
//   client → server : binary PCM frames (16 kHz mono S16LE)
//   server → client : tagged JSON
//     - {type:"ready"}
//     - {type:"interim",text}
//     - {type:"final",text,t_start_ms,t_end_ms}
//     - {type:"error",code,message}
//
// Lifecycle: `start()` opens the WS and resolves once the server
// sends `ready` (or surfaces an auth/connect error). `feed(_:)` ships
// PCM frames; `stop()` flushes and closes. The same instance can be
// reused across separate dictations — `stop()` resets internal state.

import Foundation

@MainActor
final class SttSession {
    enum State: Equatable {
        case idle
        case connecting
        case ready
        case error(String)
        case closed
    }

    /// Updates as the session progresses. SwiftUI views can observe
    /// transitively via `interim` / `finalText`; the state itself is
    /// useful for showing a transient "listening…" affordance.
    private(set) var state: State = .idle

    /// Latest interim preview text. Updated on every `interim` frame
    /// from the server; cleared whenever a `final` lands.
    private(set) var interim: String = ""

    /// Accumulated finalized utterances, joined with single spaces.
    /// Cleared by `stop()`. The view binds this directly to the
    /// `description` text field while a session is active.
    private(set) var finalText: String = ""

    /// Fires whenever `finalText` or `interim` changes — the view's
    /// observer hooks here to update the textarea binding without
    /// owning all the server-message routing.
    var onUpdate: ((_ finalText: String, _ interim: String) -> Void)?
    var onError: ((String) -> Void)?
    var onClosed: (() -> Void)?

    private var task: URLSessionWebSocketTask?
    private var receiveLoop: Task<Void, Never>?
    private var pending: [Data] = []

    /// Open the WS to `<serverURL>/stt`. Resolves immediately; the
    /// server's `ready` frame transitions state to `.ready`.
    func start(
        serverURL: String,
        tokenProvider: @MainActor () async throws -> String,
    ) async {
        // Tear any stale state down — calling start twice in a row
        // shouldn't leak the previous socket.
        await stop()

        state = .connecting
        let token: String
        do {
            token = try await tokenProvider()
        } catch {
            let msg = "Auth failed: \(error.localizedDescription)"
            state = .error(msg)
            onError?(msg)
            return
        }

        guard let url = Self.buildURL(serverURL: serverURL, token: token) else {
            state = .error("Invalid server URL")
            onError?("Invalid server URL")
            return
        }

        let task = URLSession.shared.webSocketTask(with: url)
        self.task = task
        task.resume()
        self.receiveLoop = Task { [weak self] in
            await self?.runReceiveLoop(task: task)
        }
    }

    /// Send a 16 kHz mono S16LE PCM chunk. Call as often as the mic
    /// produces audio. Frames received before `start()` resolves are
    /// dropped (the server has nothing to attach them to).
    func feed(_ pcm: Data) {
        guard let task else {
            // Buffer if connecting — the server will start the
            // upstream session as soon as the WS is open.
            if case .connecting = state {
                pending.append(pcm)
            }
            return
        }
        if state == .ready {
            // Drain any pending frames first so order is preserved.
            if !pending.isEmpty {
                let backlog = pending
                pending.removeAll()
                for frame in backlog {
                    task.send(.data(frame)) { _ in }
                }
            }
            task.send(.data(pcm)) { _ in }
        } else {
            pending.append(pcm)
        }
    }

    /// Tell the server we're done and close. Idempotent.
    func stop() async {
        receiveLoop?.cancel()
        receiveLoop = nil
        if let task {
            // "stop" hint — lets the server flush the in-flight
            // buffer before the WS close races it. Best-effort; if
            // the send fails the WS close path still triggers a
            // server-side cancel.
            task.send(.string(#"{"type":"stop"}"#)) { _ in }
            task.cancel(with: .normalClosure, reason: nil)
        }
        task = nil
        pending.removeAll()
        if state != .closed {
            state = .closed
            onClosed?()
        }
        finalText = ""
        interim = ""
    }

    /// Reset the in-memory transcript so a new dictation starts
    /// clean. Doesn't touch the WS. Useful when the same session
    /// instance is reused across multiple compose-panel dictations.
    func resetTranscript() {
        finalText = ""
        interim = ""
        onUpdate?(finalText, interim)
    }

    // MARK: - Internals

    private func runReceiveLoop(task: URLSessionWebSocketTask) async {
        while !Task.isCancelled {
            do {
                let message = try await task.receive()
                handle(message: message)
            } catch {
                if Task.isCancelled { return }
                let msg = error.localizedDescription
                state = .error(msg)
                onError?(msg)
                return
            }
        }
    }

    private func handle(message: URLSessionWebSocketTask.Message) {
        switch message {
        case .string(let text):
            handle(text: text)
        case .data:
            // Server doesn't currently send binary frames; ignore.
            break
        @unknown default:
            break
        }
    }

    private func handle(text: String) {
        guard let data = text.data(using: .utf8),
            let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let type = obj["type"] as? String
        else {
            return
        }
        switch type {
        case "ready":
            state = .ready
        case "interim":
            interim = (obj["text"] as? String) ?? ""
            onUpdate?(finalText, interim)
        case "final":
            let utterance = ((obj["text"] as? String) ?? "").trimmingCharacters(
                in: .whitespacesAndNewlines)
            if !utterance.isEmpty {
                finalText = finalText.isEmpty ? utterance : "\(finalText) \(utterance)"
            }
            interim = ""
            onUpdate?(finalText, interim)
        case "error":
            let msg = (obj["message"] as? String) ?? (obj["code"] as? String) ?? "STT error"
            state = .error(msg)
            onError?(msg)
        default:
            break
        }
    }

    /// Build `<server>/stt?token=<jwt>`. Same shape as
    /// `WebSocketClient.buildURL` but routed at `/stt` instead of `/`.
    private static func buildURL(serverURL: String, token: String) -> URL? {
        guard var components = URLComponents(string: serverURL) else { return nil }
        guard let scheme = components.scheme?.lowercased(),
            scheme == "ws" || scheme == "wss"
        else { return nil }
        components.path = "/stt"
        components.queryItems = [URLQueryItem(name: "token", value: token)]
        return components.url
    }
}
