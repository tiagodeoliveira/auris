// MainThreadWatchdog.swift
//
// Captures main-thread hangs (the "incredibly slow → beachball →
// force-quit" symptom) that are otherwise impossible to root-cause
// after the fact. The app's static hot paths are all bounded, so the
// freeze needs evidence from the live failure rather than a guess.
//
// How it works: a background queue pings the main queue once a second
// to stamp a "last beat" time, and independently checks how stale that
// stamp is. When the main thread stops servicing the queue (a hang),
// the beat goes stale while the background checker keeps running — so
// it can log, from OFF the main thread, exactly when the hang started,
// how long it lasted, what the main thread was last doing (breadcrumb),
// and the process memory footprint at that moment.
//
// Read the captures with:
//   log show --predicate 'subsystem == "com.auris.mac" && category == "watchdog"' --last 1h
// or stream live with `log stream --predicate '…'`.
//
// Zero behavior change: it only observes + logs. The `note(_:)` calls
// from the main actor are a lock + string assignment (sub-microsecond),
// so they don't meaningfully add to the hot path they instrument.

import Darwin
import Foundation
import os

/// Observes main-thread responsiveness and emits OSLog faults when the
/// main thread hangs. `@unchecked Sendable` because shared state is
/// guarded by `lock`; the checker runs on a private background queue.
final class MainThreadWatchdog: @unchecked Sendable {
    private static let log = Logger(subsystem: "com.auris.mac", category: "watchdog")

    /// A main-thread stall at or above this is treated as a hang. 3 s
    /// is well past any legitimate frame/layout cost but short enough
    /// to bracket the onset of the kind of freeze that ends in a
    /// force-quit.
    private let hangThreshold: TimeInterval
    /// How often the checker runs and the main-queue beat is scheduled.
    private let tick: TimeInterval = 1.0
    /// Healthy-state memory/breadcrumb log cadence (every Nth tick), so
    /// growth over a long meeting is visible even without a hang.
    private let heartbeatEvery = 30

    private let queue = DispatchQueue(label: "com.auris.watchdog", qos: .utility)
    private let lock = NSLock()
    private var lastBeat = Date()
    private var breadcrumb = "—"
    private var inHang = false
    private var hangStartedAt: Date?
    private var checkCount = 0
    private var started = false

    init(hangThreshold: TimeInterval = 3.0) {
        self.hangThreshold = hangThreshold
    }

    /// Begin observing. Idempotent. Call once at app start.
    func start() {
        lock.lock()
        if started {
            lock.unlock()
            return
        }
        started = true
        lastBeat = Date()
        lock.unlock()
        Self.log.info("watchdog started (hang threshold \(self.hangThreshold, format: .fixed(precision: 1))s)")
        scheduleBeat()
        scheduleCheck()
    }

    /// Record what the main thread is currently doing. Called from the
    /// main actor at the top of the websocket event handler so a hang
    /// capture names the last-handled event. Cheap: lock + assign.
    func note(_ crumb: @autoclosure () -> String) {
        let c = crumb()
        lock.lock()
        breadcrumb = c
        lock.unlock()
    }

    // MARK: - Internals

    /// Main-queue liveness beat. Re-arms itself from the main thread, so
    /// while the main thread is hung no new beat is stamped and the gap
    /// the checker sees grows monotonically.
    private func scheduleBeat() {
        DispatchQueue.main.asyncAfter(deadline: .now() + tick) { [weak self] in
            guard let self else { return }
            self.lock.lock()
            self.lastBeat = Date()
            self.lock.unlock()
            self.scheduleBeat()
        }
    }

    /// Background checker. Runs independent of the main thread, so it
    /// keeps firing during a hang.
    private func scheduleCheck() {
        queue.asyncAfter(deadline: .now() + tick) { [weak self] in
            guard let self else { return }
            self.check()
            self.scheduleCheck()
        }
    }

    private func check() {
        lock.lock()
        let beat = lastBeat
        let crumb = breadcrumb
        lock.unlock()

        let gap = Date().timeIntervalSince(beat)
        let memMB = Self.memoryFootprintMB()

        if gap >= hangThreshold {
            if !inHang {
                inHang = true
                hangStartedAt = beat
                Self.log.fault(
                    "MAIN THREAD HANG: unresponsive ~\(gap, format: .fixed(precision: 1))s; last main-thread activity=\(crumb, privacy: .public); mem=\(memMB, format: .fixed(precision: 1))MB"
                )
            } else {
                // Still hung — periodic update so the log shows the hang
                // growing (and whether memory is climbing through it).
                Self.log.fault(
                    "MAIN THREAD HANG continues ~\(gap, format: .fixed(precision: 1))s; last=\(crumb, privacy: .public); mem=\(memMB, format: .fixed(precision: 1))MB"
                )
            }
            return
        }

        if inHang {
            inHang = false
            let total = hangStartedAt.map { Date().timeIntervalSince($0) } ?? gap
            hangStartedAt = nil
            Self.log.fault(
                "main thread recovered after ~\(total, format: .fixed(precision: 1))s hang; mem=\(memMB, format: .fixed(precision: 1))MB"
            )
        }

        checkCount += 1
        if checkCount % heartbeatEvery == 0 {
            Self.log.info(
                "heartbeat: mem=\(memMB, format: .fixed(precision: 1))MB; last main-thread activity=\(crumb, privacy: .public)"
            )
        }
    }

    /// Resident memory footprint in MB via `task_vm_info`. Thread-safe;
    /// callable from the background checker. Returns 0 on failure.
    private static func memoryFootprintMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(
            MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard kr == KERN_SUCCESS else { return 0 }
        return Double(info.phys_footprint) / 1024.0 / 1024.0
    }
}
