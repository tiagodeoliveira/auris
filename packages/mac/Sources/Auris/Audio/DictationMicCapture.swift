// DictationMicCapture.swift
// Standalone mic capture used by the compose-panel STT button. Same
// AVAudioEngine pattern as `MicCapture`, but instead of writing
// Float32 samples to a shared `AudioRing` we convert to 16 kHz mono
// signed-16-bit-little-endian PCM bytes and hand them to a callback —
// the format the server's `/stt` endpoint expects on the wire.
//
// Why a separate class instead of generalizing MicCapture: dictation
// only happens before a meeting starts (no concurrent audio paths),
// the output format is different (Int16 vs Float32), and the lifetime
// is tied to a single SttSession. Sharing one class for both would
// mean either two output sinks or a per-frame conversion fork inside
// the meeting-audio path; cleaner to keep them apart.

@preconcurrency import AVFoundation
import Foundation
import OSLog

/// Not `@MainActor` — the AVAudio tap callback fires on a real-time
/// audio thread, so anything the tap touches has to be reachable from
/// outside MainActor. `start()` / `stop()` are still safe to call from
/// MainActor (they're just nonisolated method calls). The conversion
/// path uses only thread-locals + the converter; the only public
/// shared mutable surface is `onPcm`, which is `@Sendable`.
final class DictationMicCapture: @unchecked Sendable {
    /// Fires on the AVAudio tap queue (background) with one chunk per
    /// tap callback (~20-30 ms of audio at 16 kHz). Bytes are signed
    /// 16-bit little-endian PCM, mono, 16 kHz — exactly what the
    /// server's `/stt` endpoint streams to its STT provider.
    var onPcm: (@Sendable (Data) -> Void)?

    private let outputFormat: AVAudioFormat
    private let engine = AVAudioEngine()
    private var converter: AVAudioConverter?

    private static let log = Logger(
        subsystem: "com.auris.mac", category: "DictationMicCapture")

    init() {
        // 16 kHz mono Int16 — same sample rate the meeting pipeline
        // uses. Interleaved is the "non-deinterleaved" representation
        // for mono so each frame is exactly 2 bytes.
        self.outputFormat = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: 16000,
            channels: 1,
            interleaved: true)!
    }

    /// Begin capture. Throws if the engine fails to start (typically
    /// mic permission not actually granted at the OS layer).
    func start() throws {
        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        guard let converter = AVAudioConverter(from: inputFormat, to: outputFormat) else {
            throw NSError(
                domain: "DictationMicCapture", code: -1,
                userInfo: [NSLocalizedDescriptionKey: "AVAudioConverter init failed"])
        }
        self.converter = converter

        // Tap buffer ~21 ms @ 48 kHz. Same cadence MicCapture uses.
        // Runs on a real-time audio queue — do all the work here
        // (conversion is cheap, ~150 µs at this buffer size) and let
        // the callback hop wherever it needs.
        input.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { [weak self] buffer, _ in
            self?.handle(buffer: buffer)
        }

        engine.prepare()
        try engine.start()
        Self.log.info("dictation engine started, isRunning=\(self.engine.isRunning, privacy: .public)")
    }

    func stop() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
    }

    private func handle(buffer pcm: AVAudioPCMBuffer) {
        guard let converter else { return }

        let outputCapacity = AVAudioFrameCount(
            Double(pcm.frameLength) * (16000.0 / pcm.format.sampleRate) + 16)
        guard
            let outputBuffer = AVAudioPCMBuffer(
                pcmFormat: outputFormat,
                frameCapacity: outputCapacity)
        else { return }

        // Same gotcha as MicCapture: reset before each conversion or
        // the second call returns endOfStream with 0 frames.
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

        if conversionError != nil { return }

        let frameLength = Int(outputBuffer.frameLength)
        guard frameLength > 0,
            let int16Channel = outputBuffer.int16ChannelData
        else { return }

        // Copy the Int16 frames into a Data blob. Each frame is 2
        // bytes; little-endian on Apple Silicon and Intel both, so
        // the raw memory layout matches the wire format directly.
        let byteCount = frameLength * MemoryLayout<Int16>.size
        let data = Data(bytes: int16Channel[0], count: byteCount)
        onPcm?(data)
    }
}
