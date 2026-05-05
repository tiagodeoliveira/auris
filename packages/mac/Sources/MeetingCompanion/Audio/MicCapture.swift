// MicCapture.swift
// AVAudioEngine-based microphone capture. Replaces SCKit's
// `SCStreamConfiguration.captureMicrophone` path, which on macOS 15+
// has been observed to deliver silent (zero-filled) buffers — or no
// buffers at all — even when permissions are granted. AVAudioEngine
// is the long-stable mic API and behaves predictably.
//
// Lifecycle: `start()` installs a tap on `inputNode`, converts each
// buffer to 16 kHz mono Float32, and appends into the shared
// `AudioRing` that the mixer drains. `stop()` removes the tap and
// halts the engine. The ring itself is owned by `AudioCapture`.

@preconcurrency import AVFoundation
import Foundation
import OSLog

/// Owns a single AVAudioEngine + tap. The tap callback fires on a
/// background queue managed by AVAudioEngine; the ring is
/// thread-safe so direct access from that queue is fine.
final class MicCapture: @unchecked Sendable {
    private let ring: AudioRing
    private let outputFormat: AVAudioFormat
    private let engine = AVAudioEngine()
    private var converter: AVAudioConverter?
    private var samplesAppended: UInt64 = 0
    private var inputPeakWindow: Float = 0
    private var sampleBuffersSeen: UInt64 = 0

    private static let log = Logger(
        subsystem: "com.meeting-companion.mac", category: "MicCapture")

    init(ring: AudioRing) {
        self.ring = ring
        // 16 kHz mono Float32 — matches the mixer's input format
        // and the SCKit system-audio handler's output format.
        self.outputFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: 16000,
            channels: 1,
            interleaved: true)!
    }

    /// Begin capture. Installs a tap on the engine's input node.
    /// Throws if the engine fails to start (typically: mic
    /// permission not actually granted at the OS layer despite
    /// `AVCaptureDevice.authorizationStatus == .authorized`).
    func start() throws {
        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        print("[MicCapture] input node format: \(inputFormat.description)")
        Self.log.info("input node format: \(inputFormat.description, privacy: .public)")

        guard let converter = AVAudioConverter(from: inputFormat, to: outputFormat) else {
            throw NSError(
                domain: "MicCapture", code: -1,
                userInfo: [NSLocalizedDescriptionKey: "AVAudioConverter init failed"])
        }
        self.converter = converter

        // Tap buffer size 1024 frames @ ~48 kHz ≈ 21 ms — matches
        // SCKit's audio callback cadence so the two sources stay
        // in step at the mixer.
        input.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { [weak self] buffer, _ in
            self?.handle(buffer: buffer)
        }

        // `prepare()` pre-allocates the audio render path. Without
        // it, the engine has been observed to deliver only the
        // initial flush buffer and then stall — there's no
        // downstream graph keeping it active for tap-only setups.
        engine.prepare()
        try engine.start()
        print("[MicCapture] engine started, isRunning=\(engine.isRunning)")
    }

    func stop() {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        print("[MicCapture] engine stopped (samples appended: \(samplesAppended))")
    }

    private func handle(buffer pcm: AVAudioPCMBuffer) {
        sampleBuffersSeen &+= 1
        if sampleBuffersSeen == 1 {
            print("[MicCapture] FIRST tap buffer received (\(pcm.frameLength) frames)")
        }

        // Per-buffer peak — drives the periodic input_peak log
        // below, which is the simplest "is the mic actually
        // delivering signal?" indicator.
        if let channelData = pcm.floatChannelData {
            let frames = Int(pcm.frameLength)
            let channels = Int(pcm.format.channelCount)
            var bufferPeak: Float = 0
            for ch in 0..<channels {
                let p = channelData[ch]
                for i in 0..<frames {
                    let v = abs(p[i])
                    if v > bufferPeak { bufferPeak = v }
                }
            }
            if bufferPeak > inputPeakWindow { inputPeakWindow = bufferPeak }
        }

        guard let converter else { return }

        let outputCapacity = AVAudioFrameCount(
            Double(pcm.frameLength) * (16000.0 / pcm.format.sampleRate) + 16)
        guard let outputBuffer = AVAudioPCMBuffer(
            pcmFormat: outputFormat,
            frameCapacity: outputCapacity)
        else { return }

        // Reset the converter to a fresh state before each call.
        // Without this, signalling `.endOfStream` from the input
        // block on the first buffer puts the converter into a
        // sealed/drained state, and every subsequent call returns
        // `.endOfStream` with 0 output frames.
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
            print("[MicCapture] convert error: \(error.localizedDescription)")
            return
        }

        let frameLength = Int(outputBuffer.frameLength)
        guard frameLength > 0,
            let channelData = outputBuffer.floatChannelData
        else { return }

        let view = UnsafeBufferPointer(start: channelData[0], count: frameLength)
        ring.append(ArraySlice(Array(view)))
        samplesAppended &+= UInt64(frameLength)

        // Heartbeat every ~10 s of converted audio. Frequent
        // enough to confirm "still alive", quiet enough to leave
        // the terminal usable while a meeting runs.
        if samplesAppended % 160_000 < UInt64(frameLength) {
            let peak = inputPeakWindow
            inputPeakWindow = 0
            let dB: Float = peak > 0 ? 20 * log10f(peak) : -Float.infinity
            let dBStr = dB.isFinite ? String(format: "%.1f dB", dB) : "-∞ dB (silence)"
            print(
                "[MicCapture] ~\(samplesAppended / 16_000)s captured input_peak=\(dBStr)"
            )
        }
    }
}
