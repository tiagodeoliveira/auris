// ScreenshotCapture.swift
// Single-frame screen capture via ScreenCaptureKit
// (`SCScreenshotManager`, macOS 14+). Returns the active display
// as PNG bytes — that's what the moments capture path uploads.
//
// We piggyback on the same Screen Recording permission the audio
// capture path already requested in the permissions onboarding, so
// no new TCC dance is needed.

import AppKit
import CoreGraphics
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

enum ScreenshotCaptureError: Error, LocalizedError {
    case noDisplay
    case captureFailed(String)
    case encodeFailed
    case permissionDenied

    var errorDescription: String? {
        switch self {
        case .noDisplay:
            return "ScreenCaptureKit reported no displays."
        case .captureFailed(let msg):
            return "Screenshot capture failed: \(msg)"
        case .encodeFailed:
            return "Could not encode the screenshot as PNG."
        case .permissionDenied:
            return "Screen Recording permission not granted."
        }
    }
}

enum ScreenshotCapture {
    /// Capture the primary display as a PNG. Returns the encoded
    /// bytes ready to ship over a multipart upload.
    static func capturePrimaryDisplay() async throws -> Data {
        guard CGPreflightScreenCaptureAccess() else {
            throw ScreenshotCaptureError.permissionDenied
        }

        let content: SCShareableContent
        do {
            content = try await SCShareableContent.current
        } catch {
            throw ScreenshotCaptureError.captureFailed(error.localizedDescription)
        }
        guard let display = content.displays.first else {
            throw ScreenshotCaptureError.noDisplay
        }

        // Exclude Auris's own windows from the capture
        // (the floating meeting overlay, the menu-bar dropdown,
        // settings, etc.) so the moment screenshot shows whatever
        // the user is actually working on underneath. PID match is
        // the bulletproof identifier — bundle id is nil under
        // `swift run`, and matching on the bundle would miss the
        // dev-loop case.
        let ownPID = ProcessInfo.processInfo.processIdentifier
        let ownBundleId = Bundle.main.bundleIdentifier
        let selfApps = content.applications.filter { app in
            app.processID == ownPID
                || (ownBundleId != nil && app.bundleIdentifier == ownBundleId)
        }

        let filter = SCContentFilter(
            display: display,
            excludingApplications: selfApps,
            exceptingWindows: []
        )

        let config = SCStreamConfiguration()
        // Native pixel dimensions so the screenshot matches what the
        // user sees. macOS's coordinate space here is in pixels, not
        // points — `display.width` already gives us pixel count.
        config.width = display.width
        config.height = display.height
        // BGRA8 is the most reliable format for SCKit single-frame
        // capture; subsequent CGImage conversion handles colour space.
        config.pixelFormat = kCVPixelFormatType_32BGRA
        config.showsCursor = false

        let cgImage: CGImage
        do {
            cgImage = try await SCScreenshotManager.captureImage(
                contentFilter: filter,
                configuration: config
            )
        } catch {
            throw ScreenshotCaptureError.captureFailed(error.localizedDescription)
        }

        return try encodeAsPNG(cgImage)
    }

    /// CGImage → PNG via ImageIO. Fails fast if the destination
    /// can't be created (extremely rare; would mean an OS bug).
    private static func encodeAsPNG(_ image: CGImage) throws -> Data {
        let data = NSMutableData()
        guard let dest = CGImageDestinationCreateWithData(
            data as CFMutableData,
            UTType.png.identifier as CFString,
            1,
            nil
        ) else {
            throw ScreenshotCaptureError.encodeFailed
        }
        CGImageDestinationAddImage(dest, image, nil)
        guard CGImageDestinationFinalize(dest) else {
            throw ScreenshotCaptureError.encodeFailed
        }
        return data as Data
    }
}
