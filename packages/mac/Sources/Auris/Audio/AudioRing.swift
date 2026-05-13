// AudioRing.swift
// Lock-protected ring buffer of Float32 audio samples. Used as the
// per-source buffer between an SCKit sample-handler queue (system
// audio or microphone) and the mixer task that drains them at 50 fps.
//
// "Ring" here is conceptual — implemented as an Array with a length
// cap that drops oldest samples when full. With both sources running
// at ~50 fps and the mixer also at 50 fps, steady state holds
// 0–1 frames in each ring; the cap protects against transient stalls.

import Foundation

/// Thread-safe via `NSLock`. Sendable-checked manually because we
/// share an instance across the SCKit handler queue and the mixer
/// task.
final class AudioRing: @unchecked Sendable {
    private let lock = NSLock()
    private var samples: [Float]
    private let capacity: Int

    init(capacity: Int) {
        self.capacity = capacity
        self.samples = []
        self.samples.reserveCapacity(capacity)
    }

    /// Append samples to the back of the ring. If the ring would
    /// exceed `capacity`, the oldest samples are dropped.
    func append(_ newSamples: ArraySlice<Float>) {
        lock.lock()
        defer { lock.unlock() }
        samples.append(contentsOf: newSamples)
        if samples.count > capacity {
            samples.removeFirst(samples.count - capacity)
        }
    }

    /// Drain up to `count` samples from the front. Returns fewer than
    /// `count` only if the ring has fewer available — callers should
    /// treat a short result as "this source had no data this tick"
    /// and zero-fill (silence) if mixing.
    func drain(count: Int) -> [Float] {
        lock.lock()
        defer { lock.unlock() }
        let take = min(count, samples.count)
        guard take > 0 else { return [] }
        let frame = Array(samples.prefix(take))
        samples.removeFirst(take)
        return frame
    }

    /// Current sample count. Diagnostics only — not consistent
    /// against concurrent appends/drains.
    var approximateCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return samples.count
    }
}
