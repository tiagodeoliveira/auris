// AudioStreamer.swift
// Drains the AsyncStream<Data> produced by AudioCapture and ships
// each frame as a binary WS message on a separate connection to the
// server's `/audio` endpoint (the RemoteAudioSource intake from
// Phase 1b).
//
// One connection per streamer instance. Lifetime tied to the
// streamer; cancel disconnects.

import Foundation
import OSLog
import Observation

@MainActor
@Observable
final class AudioStreamer {
    enum State: Equatable {
        case idle
        case connecting
        case streaming
        case error(String)
    }

    private(set) var state: State = .idle
    private(set) var framesSent: UInt64 = 0
    private(set) var bytesSent: UInt64 = 0

    private var task: URLSessionWebSocketTask?
    private var pumpTask: Task<Void, Never>?

    private static let log = Logger(
        subsystem: "com.meeting-companion.mac", category: "AudioStreamer")

    /// Begin streaming. Opens a WS to <serverURL>/audio?token=<token>
    /// and pulls from `frames` until the stream finishes or stop() is
    /// called.
    func start(serverURL: String, token: String, frames: AsyncStream<Data>) {
        stop()

        guard let url = Self.buildAudioURL(serverURL: serverURL, token: token) else {
            state = .error("Invalid server URL.")
            return
        }

        state = .connecting
        let task = URLSession.shared.webSocketTask(with: url)
        self.task = task
        task.resume()

        framesSent = 0
        bytesSent = 0

        pumpTask = Task { [weak self] in
            await self?.pump(frames: frames, task: task)
        }
    }

    /// Tear down the WS connection and stop pulling from the stream.
    /// Idempotent; safe to call multiple times or before start().
    func stop() {
        pumpTask?.cancel()
        pumpTask = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        if state != .idle {
            state = .idle
        }
    }

    private func pump(
        frames: AsyncStream<Data>,
        task: URLSessionWebSocketTask
    ) async {
        // The WS handshake is in flight while we start consuming
        // frames. URLSession buffers send() calls until open, so the
        // first few frames may queue briefly — that's fine; our
        // backpressure is at the audio source (bounded ring buffer).
        for await frame in frames {
            if Task.isCancelled { break }
            do {
                try await task.send(.data(frame))
                if state != .streaming {
                    state = .streaming
                }
                framesSent &+= 1
                bytesSent &+= UInt64(frame.count)
            } catch {
                if Task.isCancelled { return }
                Self.log.warning(
                    "audio frame send failed: \(error.localizedDescription, privacy: .public)")
                state = .error(error.localizedDescription)
                return
            }
        }
        // Source stream finished; close cleanly.
        Self.log.info(
            "audio source stream ended (frames=\(self.framesSent, privacy: .public), bytes=\(self.bytesSent, privacy: .public))"
        )
        if state != .idle {
            state = .idle
        }
    }

    private static func buildAudioURL(serverURL: String, token: String) -> URL? {
        guard var components = URLComponents(string: serverURL) else { return nil }
        guard let scheme = components.scheme?.lowercased(),
            scheme == "ws" || scheme == "wss"
        else { return nil }
        components.path = "/audio"
        components.queryItems = [URLQueryItem(name: "token", value: token)]
        return components.url
    }
}
