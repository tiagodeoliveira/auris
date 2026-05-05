// WebSocketClient.swift
// Thin wrapper over URLSessionWebSocketTask. Owns one connection at a
// time; exposes its state as observable so SwiftUI can render it.
//
// Auto-reconnects on transport failure with exponential backoff.
// Mirrors the server-side soniox reconnect policy (500ms base, 30s
// cap, ×2 doubling, reset on healthy receive) so client/server logs
// share the same reconnect cadence. An explicit `disconnect()` opts
// out of the loop — only network/server-side drops trigger it.

import Foundation
import Observation
import OSLog

@MainActor
@Observable
final class WebSocketClient {
    enum State: Equatable {
        case disconnected
        case connecting
        case connected
        /// Lost the connection; backing off before the next attempt.
        case reconnecting
        case error(String)
    }

    enum SendError: Error {
        case notConnected
        case encodeFailed(any Error)
    }

    /// Last message received, surfaced raw for the UI to peek at while
    /// the protocol decoder is built out in later sub-phases.
    private(set) var state: State = .disconnected
    private(set) var lastMessagePreview: String?
    private(set) var messagesReceived: Int = 0

    /// Invoked on every successfully decoded server event. AppModel
    /// subscribes to act on `device_registered`, `devices_changed`,
    /// etc. Set this once at construction; the closure is held weakly
    /// by callers.
    var onMessage: ((TypedServerEvent) -> Void)?

    /// Invoked every time the WS transitions into `.connected` —
    /// including reconnects. AppModel uses this to (re-)send
    /// `register_device`, which is the only intent the server needs
    /// to recognise this Mac on a fresh connection.
    var onConnected: (() -> Void)?

    private var task: URLSessionWebSocketTask?
    private var receiveLoop: Task<Void, Never>?
    private var reconnectTask: Task<Void, Never>?

    /// Connection parameters, retained so the reconnect loop can
    /// re-dial without the caller re-supplying them.
    private var serverURL: String?
    private var token: String?

    /// True while a transport-level drop should trigger backoff +
    /// reconnect. Set by `connect(...)`, cleared by `disconnect()`.
    private var shouldAutoReconnect = false

    /// Current backoff for the next reconnect attempt. Doubles on
    /// each consecutive failure (capped at `backoffMax`); resets to
    /// `backoffBase` on a healthy receive.
    private var nextBackoff: TimeInterval = backoffBase
    private static let backoffBase: TimeInterval = 0.5
    private static let backoffMax: TimeInterval = 30.0

    private static let log = Logger(
        subsystem: "com.meeting-companion.mac", category: "WebSocketClient")

    /// Open a WS connection to the given server URL with the given
    /// auth token. Enables auto-reconnect for the lifetime of this
    /// session; further drops dial back in with backoff until
    /// `disconnect()` is called.
    func connect(serverURL: String, token: String) {
        teardownConnection()

        self.serverURL = serverURL
        self.token = token
        shouldAutoReconnect = true
        nextBackoff = Self.backoffBase
        openSocket()
    }

    /// Tear down any active connection AND opt out of auto-reconnect.
    /// User-driven; never called from the reconnect loop.
    func disconnect() {
        shouldAutoReconnect = false
        teardownConnection()
        if state != .disconnected {
            state = .disconnected
        }
    }

    /// Tear down sockets/tasks without changing the auto-reconnect
    /// disposition. Used both by user-driven `disconnect()` and as a
    /// preamble inside `connect()` to ensure no stale tasks remain.
    private func teardownConnection() {
        reconnectTask?.cancel()
        reconnectTask = nil
        receiveLoop?.cancel()
        receiveLoop = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
    }

    /// Open a fresh `URLSessionWebSocketTask` using the stored
    /// `serverURL` + `token`. Called both from `connect()` and from
    /// the backoff loop.
    private func openSocket() {
        guard let serverURL, let token else { return }
        guard let url = Self.buildURL(serverURL: serverURL, token: token) else {
            state = .error("Invalid server URL.")
            shouldAutoReconnect = false  // bad config — no point retrying
            return
        }

        state = .connecting
        let task = URLSession.shared.webSocketTask(with: url)
        self.task = task
        task.resume()

        receiveLoop = Task { [weak self] in
            await self?.runReceiveLoop(task: task)
        }
    }

    /// Send an `Encodable` intent as a JSON text frame. Safe to call
    /// before the WS handshake completes — URLSession buffers the
    /// frame until the connection is open.
    func send<T: Encodable>(intent: T) async throws {
        guard let task else { throw SendError.notConnected }
        let data: Data
        do {
            data = try JSONEncoder().encode(intent)
        } catch {
            throw SendError.encodeFailed(error)
        }
        guard let text = String(data: data, encoding: .utf8) else {
            throw SendError.encodeFailed(
                NSError(domain: "WebSocketClient", code: -1, userInfo: nil))
        }
        try await task.send(.string(text))
    }

    // MARK: - Internals

    /// Build `ws[s]://host:port/?token=<token>` from the user-entered
    /// server URL. Validates that it's a parseable URL with a ws scheme.
    private static func buildURL(serverURL: String, token: String) -> URL? {
        guard var components = URLComponents(string: serverURL) else { return nil }
        guard let scheme = components.scheme?.lowercased(),
            scheme == "ws" || scheme == "wss"
        else { return nil }

        // Path must be present; "/" is the PWA-protocol root, "/audio"
        // is for the RemoteAudioSource (used by future audio streamer).
        if components.path.isEmpty {
            components.path = "/"
        }
        components.queryItems = [URLQueryItem(name: "token", value: token)]
        return components.url
    }

    /// Loop `receive()` calls until the task fails or is cancelled.
    /// First successful receive flips state to `.connected` (and
    /// fires `onConnected` so AppModel re-registers). Errors trigger
    /// the backoff loop when auto-reconnect is enabled.
    private func runReceiveLoop(task: URLSessionWebSocketTask) async {
        while !Task.isCancelled {
            do {
                let message = try await task.receive()
                if state != .connected {
                    state = .connected
                    nextBackoff = Self.backoffBase  // healthy traffic resets the cooldown
                    onConnected?()
                }
                handleMessage(message)
            } catch {
                if Task.isCancelled { return }
                if shouldAutoReconnect {
                    Self.log.warning(
                        "WS dropped (\(error.localizedDescription, privacy: .public)); reconnecting in \(self.nextBackoff, privacy: .public)s"
                    )
                    scheduleReconnect()
                } else {
                    state = .error(error.localizedDescription)
                }
                return
            }
        }
    }

    /// Wait `nextBackoff` seconds then attempt to reopen the socket.
    /// Doubles the backoff for the next failure (capped). Cancelled
    /// by `disconnect()` or by a fresh `connect()`.
    private func scheduleReconnect() {
        let delay = nextBackoff
        nextBackoff = min(nextBackoff * 2, Self.backoffMax)
        state = .reconnecting

        reconnectTask?.cancel()
        reconnectTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(delay))
            guard let self else { return }
            guard !Task.isCancelled, self.shouldAutoReconnect else { return }
            self.openSocket()
        }
    }

    /// Track count + preview, then decode and forward to onMessage.
    /// Decode failures log but don't break the connection.
    private func handleMessage(_ message: URLSessionWebSocketTask.Message) {
        messagesReceived += 1
        switch message {
        case .string(let text):
            lastMessagePreview = String(text.prefix(80))
            do {
                if let event = try decodeServerEvent(from: text) {
                    onMessage?(event)
                }
            } catch {
                Self.log.warning("decode failed: \(error.localizedDescription, privacy: .public)")
            }
        case .data(let data):
            lastMessagePreview = "<binary \(data.count) bytes>"
        @unknown default:
            lastMessagePreview = "<unknown frame type>"
        }
    }
}
