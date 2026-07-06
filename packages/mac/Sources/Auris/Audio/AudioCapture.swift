// AudioCapture.swift
// macOS audio capture via ScreenCaptureKit. Mirrors the Rust
// pipeline in packages/server/src/audio/capture.rs:
//
//   ┌────────────────┐      ┌──────────────┐      ┌──────────────┐
//   │ SCStream       │      │ AVAudio      │      │ AudioRing    │
//   │ (system audio) │─────▶│ Converter    │─────▶│ (system)     │─┐
//   │ 48k stereo F32 │      │ 48k st F32 → │      │              │ │
//   └────────────────┘      │ 16k mono F32 │      └──────────────┘ │
//                           └──────────────┘                       │
//   ┌────────────────┐      ┌──────────────┐      ┌──────────────┐ │
//   │ SCStream       │      │ AVAudio      │      │ AudioRing    │ │
//   │ (microphone)   │─────▶│ Converter    │─────▶│ (mic)        │─┤
//   │ 48k stereo F32 │      │ 48k st F32 → │      │              │ │
//   └────────────────┘      │ 16k mono F32 │      └──────────────┘ │
//                           └──────────────┘                       │
//                                                                  │
//                ┌─────────────────────────────────────────────────┘
//                ▼
//   ┌─────────────────────────┐
//   │ Mixer task (50 fps tick)│      AsyncStream<Data>
//   │ • drain 320 samples each│ ────▶ frames of 640 bytes
//   │ • sum + clamp           │       (16 kHz mono S16LE, 20 ms)
//   │ • Float32 → S16LE bytes │
//   └─────────────────────────┘
//
// Phase 2e produces frames into the AsyncStream; Phase 2f₂ wires the
// AudioStreamer that ships them to the server's /audio endpoint.

@preconcurrency import AVFoundation
import Foundation
import OSLog
import Observation
import ScreenCaptureKit

@MainActor
@Observable
final class AudioCapture {
    enum State: Equatable {
        case stopped
        case starting
        case running
        case error(String)
    }

    private(set) var state: State = .stopped

    /// Frames emitted into the output stream so far. Surfaced in the
    /// menu bar as a smoke-test signal.
    private(set) var frameCount: UInt64 = 0

    /// Output stream of mixed PCM frames. Set when start() succeeds;
    /// finished and cleared when stop() runs. Each `Data` is exactly
    /// `AudioFormat.mixerFrameBytes` bytes (640) of 16 kHz mono S16LE.
    private(set) var output: AsyncStream<Data>?

    private var stream: SCStream?
    private var systemHandler: SCAudioHandler?
    private var micCapture: MicCapture?
    private let systemRing = AudioRing(capacity: AudioConstants.ringCapacity)
    private let micRing = AudioRing(capacity: AudioConstants.ringCapacity)
    private var continuation: AsyncStream<Data>.Continuation?
    private var mixerTask: Task<Void, Never>?

    /// Per-source peak amplitude (0..1) smoothed with exponential
    /// decay. Drives the meeting overlay's VAD bars and is the
    /// quickest way to tell silent-input from real speech when
    /// transcripts go missing.
    private(set) var currentSysPeak: Float = 0
    private(set) var currentMicPeak: Float = 0

    private static let log = Logger(
        subsystem: "com.auris.mac", category: "AudioCapture")

    /// Last AudioCapture error message (or nil). Surfaced in the menu
    /// bar dropdown so failures are visible without opening Console.
    private(set) var lastErrorDetail: String?

    /// Begin capture. Idempotent: returns immediately if already
    /// running. Throws if SCKit setup fails (e.g., no display, missing
    /// permissions).
    func start() async throws {
        guard state == .stopped else { return }
        state = .starting
        lastErrorDetail = nil

        // Verify permissions BEFORE we go near SCKit. Failures here
        // give clearer diagnostics than SCKit's opaque init errors.
        let micStatus = AVCaptureDevice.authorizationStatus(for: .audio)
        let screenGranted = CGPreflightScreenCaptureAccess()
        Self.logBoth(
            "AudioCapture.start: mic=\(micStatus.rawValue) (\(Self.describe(micStatus))) screenRec=\(screenGranted)"
        )
        if !screenGranted {
            let msg = "Screen Recording permission not granted (required for SCKit)."
            lastErrorDetail = msg
            state = .error(msg)
            throw AudioCaptureError.permissionDenied(msg)
        }
        if micStatus != .authorized {
            let msg = "Microphone permission not granted (status=\(Self.describe(micStatus)))."
            lastErrorDetail = msg
            state = .error(msg)
            throw AudioCaptureError.permissionDenied(msg)
        }

        do {
            Self.logBoth("AudioCapture: fetching shareable content…")
            let content = try await SCShareableContent.current
            Self.logBoth(
                "AudioCapture: got content (\(content.displays.count) displays, \(content.applications.count) apps)"
            )
            guard let display = content.displays.first else {
                throw AudioCaptureError.noDisplay
            }
            let filter = SCContentFilter(
                display: display,
                excludingApplications: [],
                exceptingWindows: [])
            let config = SCStreamConfiguration()
            // SCStream requires a video config even when we only want
            // audio. 2x2 is the minimum that doesn't get rejected.
            config.width = 2
            config.height = 2
            config.capturesAudio = true
            // System audio only — microphone goes through
            // AVAudioEngine (`MicCapture`). SCKit's
            // `captureMicrophone` flag was observed to deliver
            // silent / no buffers on macOS 15+ even with
            // permissions correctly granted.
            config.sampleRate = 48000
            config.channelCount = 2

            let stream = SCStream(filter: filter, configuration: config, delegate: nil)
            Self.logBoth("AudioCapture: SCStream created")

            let systemHandler = SCAudioHandler(ring: systemRing, sourceLabel: "system")
            try stream.addStreamOutput(
                systemHandler, type: .audio,
                sampleHandlerQueue: DispatchQueue(label: "audio.system", qos: .userInteractive))
            Self.logBoth("AudioCapture: stream output added (system audio)")

            let micCapture = MicCapture(ring: micRing)
            try micCapture.start()
            Self.logBoth("AudioCapture: mic capture (AVAudioEngine) started")

            let (asyncStream, continuation) = AsyncStream<Data>.makeStream(
                bufferingPolicy: .bufferingNewest(50))  // ~1 s of frames if the consumer lags
            self.output = asyncStream
            self.continuation = continuation

            Self.logBoth("AudioCapture: calling startCapture()…")
            try await stream.startCapture()
            Self.logBoth("AudioCapture: startCapture() returned successfully")

            self.stream = stream
            self.systemHandler = systemHandler
            self.micCapture = micCapture

            mixerTask = Task { [weak self] in
                await self?.runMixerLoop()
            }

            state = .running
            Self.logBoth("AudioCapture: state=running, mixer task spawned")
        } catch {
            let msg = error.localizedDescription
            Self.logBoth("AudioCapture FAILED: \(msg)")
            lastErrorDetail = msg
            state = .error(msg)
            try? await teardown()
            throw error
        }
    }

    /// Log to both `OSLog` (Console.app) and stdout (terminal where
    /// `swift run` was invoked). Helpful while diagnosing — OSLog can
    /// be filtered out of Console by default subsystem rules.
    nonisolated private static func logBoth(_ message: String) {
        log.info("\(message, privacy: .public)")
        print("[AudioCapture] \(message)")
    }

    nonisolated private static func describe(_ status: AVAuthorizationStatus) -> String {
        switch status {
        case .notDetermined: "notDetermined"
        case .restricted: "restricted"
        case .denied: "denied"
        case .authorized: "authorized"
        @unknown default: "unknown"
        }
    }

    /// Stop capture and finish the output stream.
    func stop() {
        guard state != .stopped else { return }
        Task { try? await teardown() }
    }

    private func teardown() async throws {
        mixerTask?.cancel()
        mixerTask = nil
        if let stream = stream {
            try? await stream.stopCapture()
        }
        stream = nil
        systemHandler = nil
        micCapture?.stop()
        micCapture = nil
        continuation?.finish()
        continuation = nil
        output = nil
        state = .stopped
        Self.log.info("audio capture stopped (frames=\(self.frameCount, privacy: .public))")
    }

    /// 50 fps mixer: drain a 20 ms frame from each source, sum
    /// sample-wise (clamped to [-1, 1]), pack as S16LE, emit. Always
    /// emits — silence is a valid frame; the consumer downstream (STT
    /// / RemoteAudioSource) sees a steady cadence regardless of
    /// whether anyone is talking.
    private func runMixerLoop() async {
        print("[AudioCapture] mixer loop started")
        let interval: UInt64 = 20_000_000  // 20 ms in nanoseconds
        var nextDeadline = DispatchTime.now().uptimeNanoseconds + interval
        var ticksSinceFirstSample = 0
        var firstSampleSeen = false
        var totalTicks: UInt64 = 0

        while !Task.isCancelled {
            // Sleep until the next 20 ms tick. Drift-correcting: we
            // recompute the deadline rather than sleeping a fixed
            // duration each loop.
            let now = DispatchTime.now().uptimeNanoseconds
            if nextDeadline > now {
                try? await Task.sleep(nanoseconds: nextDeadline - now)
            }
            nextDeadline += interval

            let systemSamples = systemRing.drain(count: AudioFormat.mixerFrameSamples)
            let micSamples = micRing.drain(count: AudioFormat.mixerFrameSamples)

            totalTicks &+= 1
            if !firstSampleSeen, !systemSamples.isEmpty || !micSamples.isEmpty {
                firstSampleSeen = true
                Self.log.info(
                    "mixer: first non-empty drain (system=\(systemSamples.count, privacy: .public), mic=\(micSamples.count, privacy: .public))"
                )
                print(
                    "[AudioCapture] mixer FIRST non-empty drain (system=\(systemSamples.count), mic=\(micSamples.count))"
                )
            }

            // Mix sample-wise. Treat short/empty buffers as silence
            // (zero-fill) so the output cadence stays steady even if
            // one source briefly stalls. Track per-source peak so we
            // can tell at a glance whether anyone is actually
            // producing signal — Soniox emits no transcripts for
            // silence, so this is the first thing to check when
            // "frames flow but no text".
            var mixed = [Float](repeating: 0, count: AudioFormat.mixerFrameSamples)
            var sysPeak: Float = 0
            var micPeak: Float = 0
            for i in 0..<AudioFormat.mixerFrameSamples {
                let s = i < systemSamples.count ? systemSamples[i] : 0
                let m = i < micSamples.count ? micSamples[i] : 0
                if abs(s) > sysPeak { sysPeak = abs(s) }
                if abs(m) > micPeak { micPeak = abs(m) }
                mixed[i] = max(-1.0, min(1.0, s + m))
            }
            // Smooth peak with exponential decay. ~0.85^50 ≈ 3e-4
            // so a peak falls below visibility within ~1 s of
            // silence — fast enough that the VAD bar tracks
            // speech, slow enough that 20 ms ticks don't flicker.
            // Snap below the noise floor (~-80 dB) to exactly
            // zero, otherwise long silences let the value drift
            // into denormal float territory and the dB readout
            // shows nonsense like "-888 dB".
            let decay: Float = 0.85
            let floor: Float = 1e-4
            currentSysPeak = max(sysPeak, currentSysPeak * decay)
            currentMicPeak = max(micPeak, currentMicPeak * decay)
            if currentSysPeak < floor { currentSysPeak = 0 }
            if currentMicPeak < floor { currentMicPeak = 0 }

            ticksSinceFirstSample += 1
            // Heartbeat every ~10 s — confirms the mixer is alive
            // and shows current per-source peaks. Frequent
            // diagnostic spam moved out of here once the path
            // stabilised; if mic/system go silent unexpectedly,
            // grep this line for the dB readout.
            if ticksSinceFirstSample % 500 == 0 {
                let sysDb: Float = currentSysPeak > 0 ? 20 * log10f(currentSysPeak) : -Float.infinity
                let micDb: Float = currentMicPeak > 0 ? 20 * log10f(currentMicPeak) : -Float.infinity
                let sysDbStr = sysDb.isFinite ? String(format: "%.1f dB", sysDb) : "-∞ dB"
                let micDbStr = micDb.isFinite ? String(format: "%.1f dB", micDb) : "-∞ dB"
                print(
                    "[AudioCapture] mixer ~\(totalTicks / 50)s: emitted=\(self.frameCount) sys=\(sysDbStr) mic=\(micDbStr)"
                )
            }

            let frame = AudioFormat.packS16LE(mixed)
            await MainActor.run {
                self.frameCount &+= 1
                _ = self.continuation?.yield(frame)
            }
        }
    }
}

// MARK: - SCStreamOutput delegate

/// One handler per source (system audio, microphone). Receives
/// `CMSampleBuffer`s on a background queue, converts to 16 kHz mono
/// Float32, appends to its `AudioRing`. Sendable because it's shared
/// between SCKit's queue and the AudioCapture's main-actor context.
final class SCAudioHandler: NSObject, SCStreamOutput, @unchecked Sendable {
    private let ring: AudioRing
    private let sourceLabel: String
    private var converter: AVAudioConverter?
    private var inputFormat: AVAudioFormat?
    private var sampleBuffersSeen: UInt64 = 0
    private var samplesAppended: UInt64 = 0
    /// 16 kHz mono Float32 — the rings store this format.
    private let outputFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 16000,
        channels: 1,
        interleaved: true)!

    private static let log = Logger(
        subsystem: "com.auris.mac", category: "SCAudioHandler")

    init(ring: AudioRing, sourceLabel: String) {
        self.ring = ring
        self.sourceLabel = sourceLabel
    }

    nonisolated func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of type: SCStreamOutputType
    ) {
        guard sampleBuffer.isValid else { return }
        sampleBuffersSeen &+= 1
        if sampleBuffersSeen == 1 {
            Self.log.info("[\(self.sourceLabel, privacy: .public)] first sample buffer received")
            print("[AudioCapture] [\(sourceLabel)] FIRST sample buffer received")
        }

        guard let pcm = pcmBuffer(from: sampleBuffer) else {
            if sampleBuffersSeen <= 5 {
                Self.log.warning(
                    "[\(self.sourceLabel, privacy: .public)] pcmBuffer build failed (#\(self.sampleBuffersSeen, privacy: .public))"
                )
            }
            return
        }

        // Lazy-initialize the converter on the first sample buffer
        // (we need the input ASBD that SCKit actually delivers).
        if converter == nil {
            inputFormat = pcm.format
            converter = AVAudioConverter(from: pcm.format, to: outputFormat)
            Self.log.info(
                "[\(self.sourceLabel, privacy: .public)] converter init: \(pcm.format.description, privacy: .public) → 16k mono"
            )
            print("[AudioCapture] [\(sourceLabel)] converter init: \(pcm.format.description) → 16k mono")
        }
        guard let converter else { return }

        // Allocate output buffer sized for resampled output. SCKit
        // delivers ~1024 samples per callback at 48 kHz; converter
        // produces ~341 at 16 kHz. Round up generously.
        let outputCapacity = AVAudioFrameCount(
            Double(pcm.frameLength) * (16000.0 / pcm.format.sampleRate) + 16)
        guard let outputBuffer = AVAudioPCMBuffer(
            pcmFormat: outputFormat,
            frameCapacity: outputCapacity
        ) else {
            return
        }

        // See MicCapture.swift for the explanation: signalling
        // `.endOfStream` puts the converter into a drained state,
        // so we have to `reset()` between calls or every buffer
        // after the first returns 0 output frames.
        converter.reset()

        var consumed = false
        var conversionError: NSError?
        converter.convert(
            to: outputBuffer,
            error: &conversionError,
            withInputFrom: { _, status in
                if consumed {
                    status.pointee = .endOfStream
                    return nil
                }
                consumed = true
                status.pointee = .haveData
                return pcm
            })

        if let error = conversionError {
            Self.log.error(
                "[\(self.sourceLabel, privacy: .public)] convert error: \(error.localizedDescription, privacy: .public)"
            )
            print("[AudioCapture] [\(sourceLabel)] convert error: \(error.localizedDescription)")
            return
        }

        let frameLength = Int(outputBuffer.frameLength)
        guard frameLength > 0,
            let channelData = outputBuffer.floatChannelData
        else {
            return
        }
        let buffer = UnsafeBufferPointer(start: channelData[0], count: frameLength)
        ring.append(ArraySlice(Array(buffer)))
        samplesAppended &+= UInt64(frameLength)
    }

    /// Build an `AVAudioPCMBuffer` from a SCKit `CMSampleBuffer`.
    /// Handles both interleaved and planar layouts (planar = one
    /// `AudioBuffer` per channel).
    private func pcmBuffer(from sampleBuffer: CMSampleBuffer) -> AVAudioPCMBuffer? {
        guard let formatDesc = sampleBuffer.formatDescription,
            let asbd = formatDesc.audioStreamBasicDescription
        else {
            return nil
        }
        // Build AVAudioFormat WITHIN the closure — `withUnsafePointer`
        // only guarantees the pointer is valid for the closure body.
        let format = withUnsafePointer(to: asbd) { ptr -> AVAudioFormat? in
            AVAudioFormat(streamDescription: ptr)
        }
        guard let format else { return nil }
        let frameCount = AVAudioFrameCount(sampleBuffer.numSamples)
        guard frameCount > 0,
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount)
        else {
            return nil
        }
        buffer.frameLength = frameCount

        // Copy each AudioBuffer (one per channel for planar layouts,
        // a single buffer holding all channels for interleaved). Use
        // `withAudioBufferList` so the ABL's lifetime is managed.
        do {
            try sampleBuffer.withAudioBufferList { abl, _ in
                let buffers = UnsafeMutableAudioBufferListPointer(abl.unsafeMutablePointer)
                for i in 0..<buffers.count {
                    guard let src = buffers[i].mData,
                        let dst = buffer.floatChannelData?[i]
                    else { continue }
                    memcpy(dst, src, Int(buffers[i].mDataByteSize))
                }
            }
        } catch {
            return nil
        }
        return buffer
    }
}

// MARK: - Constants

enum AudioConstants {
    /// Per-source ring capacity in Float32 samples. 1 s at 16 kHz.
    /// In steady state rings hold 0–1 frames; this caps the worst-case
    /// memory if one source briefly stalls.
    static let ringCapacity = 16_000
}

// MARK: - Errors

enum AudioCaptureError: Error, LocalizedError {
    case noDisplay
    case permissionDenied(String)

    var errorDescription: String? {
        switch self {
        case .noDisplay:
            "ScreenCaptureKit reported no displays. (Is Screen Recording permission granted?)"
        case .permissionDenied(let detail):
            detail
        }
    }
}
