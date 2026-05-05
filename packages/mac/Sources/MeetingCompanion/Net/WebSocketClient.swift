// WebSocketClient.swift
// Thin wrapper over URLSessionWebSocketTask. Owns one connection at a
// time; exposes its state as observable so SwiftUI can render it.
// No reconnection logic yet (will be added when needed); explicit
// connect/disconnect calls drive transitions.

import Foundation
import Observation

@MainActor
@Observable
final class WebSocketClient {
    enum State: Equatable {
        case disconnected
        case connecting
        case connected
        case error(String)
    }

    /// Last message received, surfaced raw for the UI to peek at while
    /// the protocol decoder is built out in later sub-phases.
    private(set) var state: State = .disconnected
    private(set) var lastMessagePreview: String?
    private(set) var messagesReceived: Int = 0

    private var task: URLSessionWebSocketTask?
    private var receiveLoop: Task<Void, Never>?

    /// Open a WS connection to the given server URL with the given
    /// auth token. Replaces any existing connection.
    func connect(serverURL: String, token: String) {
        disconnect()

        guard let url = Self.buildURL(serverURL: serverURL, token: token) else {
            state = .error("Invalid server URL.")
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

    /// Tear down any active connection.
    func disconnect() {
        receiveLoop?.cancel()
        receiveLoop = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        if state != .disconnected {
            state = .disconnected
        }
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
    /// First successful receive flips state to `.connected`. Errors
    /// transition to `.error(...)` and exit the loop.
    private func runReceiveLoop(task: URLSessionWebSocketTask) async {
        while !Task.isCancelled {
            do {
                let message = try await task.receive()
                if state != .connected {
                    state = .connected
                }
                handleMessage(message)
            } catch {
                if Task.isCancelled { return }
                state = .error(error.localizedDescription)
                return
            }
        }
    }

    /// Phase 2c: just count + preview. Phase 2g+ will decode this into
    /// the typed contract and dispatch.
    private func handleMessage(_ message: URLSessionWebSocketTask.Message) {
        messagesReceived += 1
        switch message {
        case .string(let text):
            lastMessagePreview = String(text.prefix(80))
        case .data(let data):
            lastMessagePreview = "<binary \(data.count) bytes>"
        @unknown default:
            lastMessagePreview = "<unknown frame type>"
        }
    }
}
