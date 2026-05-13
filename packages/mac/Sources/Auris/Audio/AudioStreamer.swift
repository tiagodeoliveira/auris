// AudioStreamer.swift
// Drains the AsyncStream<Data> produced by AudioCapture and ships
// each frame as a binary WS message on a separate connection to the
// server's `/audio` endpoint.
//
// Auto-reconnects on transport failure (mirrors WebSocketClient's
// backoff: 500ms base, 30s cap, ×2 doubling, reset on healthy send).
// The audio capture side is unaffected — frames keep arriving from
// the AsyncStream during reconnect; the bounded `.bufferingNewest`
// policy on the source caps the queued audio at ~1 s, so longer
// gaps silently drop oldest frames rather than exploding memory.
//
// Late-binding contract on the server: `RemoteAudioSource` accepts
// the `/audio` connection regardless of meeting state, and the
// pipeline rx survives WS reconnects. So we don't need to gate
// `start_meeting` on `.streaming` — the meeting just runs silent
// for the brief window before audio flows.

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
        /// Lost the connection mid-stream; backing off before retry.
        case reconnecting
        case error(String)
    }

    private(set) var state: State = .idle
    private(set) var framesSent: UInt64 = 0
    private(set) var bytesSent: UInt64 = 0
    private(set) var muted: Bool = false

    private var task: URLSessionWebSocketTask?
    private var pumpTask: Task<Void, Never>?

    /// Connection params, retained across reconnect attempts.
    private var serverURL: String?
    /// Async-fetched on each (re)connect. /audio sessions can outlive
    /// a JWT (~1h) so the streamer fetches fresh on every retry; the
    /// caller doesn't have to time refreshes externally.
    private var tokenProvider: (@MainActor () async throws -> String)?

    /// True while transport-level drops should trigger backoff.
    /// `start(...)` enables; `stop()` disables.
    private var shouldAutoReconnect = false

    /// Current backoff for the next reconnect attempt. Doubles on
    /// each consecutive failure (capped at `backoffMax`); resets to
    /// `backoffBase` on the first successful send after a reconnect.
    private var nextBackoff: TimeInterval = backoffBase
    private static let backoffBase: TimeInterval = 0.5
    private static let backoffMax: TimeInterval = 30.0

    /// Consecutive failures since the last `.streaming` state. Caps
    /// the reconnect loop so a misconfigured / dead server doesn't
    /// spin us forever — most commonly when the server is in
    /// `local` audio mode and rejects every `/audio` connection
    /// with a Policy close. After this many tries we give up and
    /// surface `.error` so the UI can react.
    private var consecutiveFailures: Int = 0
    private static let maxConsecutiveFailures: Int = 10

    private static let log = Logger(
        subsystem: "com.auris.mac", category: "AudioStreamer")

    /// Begin streaming. Opens a WS to <serverURL>/audio?token=<token>
    /// and pulls from `frames` until the source ends or `stop()` is
    /// called. Transport-level drops trigger an internal reconnect
    /// loop with exponential backoff.
    func start(
        serverURL: String,
        tokenProvider: @escaping @MainActor () async throws -> String,
        frames: AsyncStream<Data>
    ) {
        stop()

        self.serverURL = serverURL
        self.tokenProvider = tokenProvider
        self.shouldAutoReconnect = true
        self.nextBackoff = Self.backoffBase
        self.consecutiveFailures = 0

        framesSent = 0
        bytesSent = 0
        muted = false

        pumpTask = Task { [weak self] in
            print("[AudioStreamer] pump task spawned")
            await self?.runPump(frames: frames)
            print("[AudioStreamer] pump task exited")
        }
    }

    /// Tear down the WS connection and stop pulling from the stream.
    /// Idempotent; safe to call multiple times or before start().
    /// Also opts out of the auto-reconnect loop.
    func stop() {
        shouldAutoReconnect = false
        pumpTask?.cancel()
        pumpTask = nil
        task?.cancel(with: .normalClosure, reason: nil)
        task = nil
        if state != .idle {
            state = .idle
        }
        muted = false
    }

    /// Locally gate audio egress without changing the meeting state on
    /// the server. Capture keeps running, but frames are dropped before
    /// they reach `/audio`.
    func setMuted(_ muted: Bool) {
        self.muted = muted
    }

    // MARK: - Pump

    /// Outer reconnect loop. Opens a WS, drains frames into it until
    /// the WS errors, then backs off and retries. Exits cleanly when
    /// the source stream ends (`iterator.next()` returns nil) or
    /// `stop()` is called.
    private func runPump(frames: AsyncStream<Data>) async {
        var iterator = frames.makeAsyncIterator()

        while !Task.isCancelled, shouldAutoReconnect {
            guard let task = await openSocket() else { return }

            // Drain frames until the WS errors (true → reconnect)
            // or the source ends / pump is cancelled (false → exit).
            let shouldRetry = await sendUntilFailure(iterator: &iterator, task: task)

            task.cancel(with: .normalClosure, reason: nil)
            if self.task === task { self.task = nil }

            if !shouldRetry { break }
            if Task.isCancelled || !shouldAutoReconnect { break }

            consecutiveFailures += 1
            if consecutiveFailures >= Self.maxConsecutiveFailures {
                let msg =
                    "Audio path failed \(consecutiveFailures) times. Server may be in `local` audio mode (set AURIS_AUDIO_SOURCE=remote) or unreachable."
                print("[AudioStreamer] pump: giving up — \(msg)")
                Self.log.error("\(msg, privacy: .public)")
                shouldAutoReconnect = false
                state = .error(msg)
                break
            }

            let delay = nextBackoff
            nextBackoff = min(nextBackoff * 2, Self.backoffMax)
            state = .reconnecting
            print(
                "[AudioStreamer] pump: WS dropped (\(consecutiveFailures)/\(Self.maxConsecutiveFailures)) — reconnecting in \(delay)s"
            )
            Self.log.warning(
                "audio WS dropped (\(self.consecutiveFailures, privacy: .public)/\(Self.maxConsecutiveFailures, privacy: .public)); reconnecting in \(delay, privacy: .public)s"
            )
            try? await Task.sleep(for: .seconds(delay))
        }

        if case .error = state {
            // Already surfaced as .error above — keep it, don't
            // overwrite with .idle on the way out.
        } else if state != .idle {
            state = .idle
        }
        Self.log.info(
            "audio pump exited (frames=\(self.framesSent, privacy: .public), bytes=\(self.bytesSent, privacy: .public))"
        )
    }

    /// Inner loop: pull frames from the iterator and send them over
    /// `task`. Returns `true` when the WS errors mid-stream (caller
    /// should reconnect), `false` when the source stream ends or the
    /// task is cancelled (caller should exit cleanly).
    private func sendUntilFailure(
        iterator: inout AsyncStream<Data>.Iterator,
        task: URLSessionWebSocketTask
    ) async -> Bool {
        while !Task.isCancelled {
            guard let frame = await iterator.next() else {
                print("[AudioStreamer] pump: source stream ended")
                shouldAutoReconnect = false  // source-driven exit, not transport
                return false
            }
            if muted { continue }

            do {
                try await task.send(.data(frame))
                if state != .streaming {
                    state = .streaming
                    // First successful send after a (re)connect:
                    // the path is healthy, reset both the backoff
                    // and the consecutive-failure cap so a future
                    // drop starts fresh.
                    nextBackoff = Self.backoffBase
                    consecutiveFailures = 0
                    if framesSent == 0 {
                        print("[AudioStreamer] pump: FIRST frame sent over WS")
                    } else {
                        print("[AudioStreamer] pump: send healthy after reconnect")
                    }
                }
                framesSent &+= 1
                bytesSent &+= UInt64(frame.count)
                if framesSent % 500 == 0 {
                    print("[AudioStreamer] pump: \(framesSent) frames sent (\(bytesSent) bytes)")
                }
            } catch {
                if Task.isCancelled { return false }
                print("[AudioStreamer] pump: send error: \(error.localizedDescription)")
                Self.log.warning(
                    "audio frame send failed: \(error.localizedDescription, privacy: .public)")
                return true
            }
        }
        return false
    }

    /// Open a fresh WS for `/audio` using the stored params. Sets
    /// state to `.connecting`. Returns nil + flips state to `.error`
    /// if the URL is invalid (terminal — won't be retried).
    private func openSocket() async -> URLSessionWebSocketTask? {
        guard let serverURL, let provider = tokenProvider else {
            return nil
        }
        let token: String
        do {
            token = try await provider()
        } catch {
            state = .error("Token fetch failed: \(error.localizedDescription)")
            return nil
        }
        guard let url = Self.buildAudioURL(serverURL: serverURL, token: token) else {
            state = .error("Invalid server URL.")
            shouldAutoReconnect = false
            return nil
        }
        state = .connecting
        let task = URLSession.shared.webSocketTask(with: url)
        self.task = task
        task.resume()
        print("[AudioStreamer] WS opening to \(url.absoluteString)")
        return task
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
