// PermissionMonitor.swift
// Tracks the macOS permissions the app needs to capture audio (mic +
// system audio via SCKit) and (future, Phase 5) screen content.
//
// Two distinct platform stories:
//
// - Microphone: AVCaptureDevice gives us a tri-state (notDetermined,
//   granted, denied) and a one-shot async request that produces the
//   familiar in-app prompt the first time, then settled forever.
//
// - Screen Recording: CGPreflight returns a bool only — there's no
//   public API to distinguish "denied" from "not yet asked". Worse,
//   the system never grants this with a single click; it always
//   bounces the user to System Settings → Privacy & Security →
//   Screen Recording. CGRequestScreenCaptureAccess() opens the
//   prompt; we re-check on app activation (user came back from
//   Settings) to learn whether they flipped the toggle.

import AVFoundation
import AppKit
import CoreGraphics
import Observation

@MainActor
@Observable
final class PermissionMonitor {
    enum Status: Equatable {
        case notDetermined
        case granted
        case denied
    }

    var microphone: Status = .notDetermined
    var screenRecording: Status = .notDetermined

    init() {
        refresh()
        // Re-read OS state every time the user comes back to the app.
        // The two scenarios this catches:
        //   1. User clicked "Open System Settings", flipped a toggle,
        //      switched back to the app — without this, the in-app
        //      UI keeps showing "Request…" until the next launch.
        //   2. User revoked a permission in Settings — we'd happily
        //      keep claiming `.granted` forever otherwise.
        // No observer cleanup: PermissionMonitor lives the process
        // lifetime; the OS releases notification observers at exit.
        NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.refresh()
            }
        }
    }

    /// True when both required permissions are granted. Audio capture
    /// (Phase 2e) requires both — system audio capture goes through
    /// SCKit, which is gated by Screen Recording, even though we only
    /// consume the audio sub-stream.
    var allGranted: Bool {
        microphone == .granted && screenRecording == .granted
    }

    /// Re-read system state. Call on app activation to pick up
    /// changes the user made in System Settings.
    func refresh() {
        microphone = Self.readMicrophone()
        screenRecording = Self.readScreenRecording()
    }

    /// Triggers the macOS in-app permission prompt the first time.
    /// On subsequent calls it's a no-op and just returns the current
    /// granted state. Updates `microphone` synchronously after.
    @discardableResult
    func requestMicrophone() async -> Bool {
        let granted = await AVCaptureDevice.requestAccess(for: .audio)
        microphone = granted ? .granted : .denied
        return granted
    }

    /// Triggers the macOS Screen Recording prompt. The prompt opens
    /// System Settings; the user must toggle our app on. We re-check
    /// shortly after (in case it was a fast click) and again on the
    /// next app activation.
    func requestScreenRecording() {
        _ = CGRequestScreenCaptureAccess()
        Task {
            try? await Task.sleep(for: .milliseconds(500))
            refresh()
        }
    }

    /// Open System Settings directly to the Screen Recording pane.
    /// Useful when the request prompt isn't shown (e.g., already
    /// denied, so the system silently does nothing).
    func openScreenRecordingSettings() {
        let url = URL(
            string:
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        )!
        NSWorkspace.shared.open(url)
    }

    /// Open System Settings directly to the Microphone pane.
    func openMicrophoneSettings() {
        let url = URL(
            string:
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        )!
        NSWorkspace.shared.open(url)
    }

    private static func readMicrophone() -> Status {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .notDetermined: .notDetermined
        case .authorized: .granted
        case .denied, .restricted: .denied
        @unknown default: .denied
        }
    }

    private static func readScreenRecording() -> Status {
        // No tri-state available; collapse "denied" and "not yet
        // asked" into `.notDetermined` and treat both the same in
        // the UI (action button is "Request…" in both cases).
        CGPreflightScreenCaptureAccess() ? .granted : .notDetermined
    }
}
