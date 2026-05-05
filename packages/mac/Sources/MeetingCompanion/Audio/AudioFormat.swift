// AudioFormat.swift
// Tiny utilities for converting between Float32 audio samples and the
// S16LE byte format Soniox expects. All functions are pure and
// thread-safe (no shared state).

import Foundation

enum AudioFormat {
    /// Frames-per-mixer-tick at 16 kHz mono. 320 samples = 20 ms,
    /// matches the cadence Soniox prefers and the size SCKit
    /// typically delivers per source per frame.
    static let mixerFrameSamples = 320

    /// Output frame size in bytes (320 samples × 2 bytes/sample S16LE).
    static let mixerFrameBytes = mixerFrameSamples * 2

    /// Soft-clamp Float32 in [-1, 1] and convert to Int16.
    /// Values outside the range get pinned to ±32767.
    @inlinable
    static func floatToInt16(_ sample: Float) -> Int16 {
        let clamped = max(-1.0, min(1.0, sample))
        // Use 32767 (not 32768) to keep symmetric range; the
        // half-LSB asymmetry doesn't affect STT.
        return Int16(clamped * 32767.0)
    }

    /// Pack `[Float]` mono samples (assumed clamped) as little-endian
    /// signed-16 bytes. Output length is `samples.count * 2`.
    static func packS16LE(_ samples: [Float]) -> Data {
        var data = Data(count: samples.count * 2)
        data.withUnsafeMutableBytes { rawBuffer in
            let bytes = rawBuffer.bindMemory(to: UInt8.self)
            for (i, sample) in samples.enumerated() {
                let v = floatToInt16(sample)
                let bits = UInt16(bitPattern: v)
                // Little-endian: low byte first, high byte second.
                bytes[i * 2] = UInt8(bits & 0xFF)
                bytes[i * 2 + 1] = UInt8((bits >> 8) & 0xFF)
            }
        }
        return data
    }
}
