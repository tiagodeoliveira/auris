// AppSettings.swift
// User-configurable settings persisted in UserDefaults. The Mac app's
// only persistent state today; OAuth tokens land here in Phase 3.

import Foundation
import Observation

/// Light vs dark visual theme for the meeting overlay. User-selectable
/// via Settings → Overlay; persisted in UserDefaults.
enum OverlayTheme: String, CaseIterable, Identifiable {
    case light
    case dark
    var id: String { rawValue }
    var displayName: String {
        switch self {
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }
}

@Observable
final class AppSettings {
    /// WebSocket server URL. Hardcoded for now — future builds will
    /// substitute this at compile time (e.g., dev vs prod targets each
    /// shipping their own constant baked into the binary). The user
    /// shouldn't be configuring this in-app.
    static let serverURLDefault = "ws://localhost:7331"

    /// Read-only convenience so call sites stay readable
    /// (`settings.serverURL`) and we can swap to a per-build value
    /// without touching every caller.
    var serverURL: String { Self.serverURLDefault }

    /// Visual theme for the meeting overlay. Drives the panel/text
    /// palette via `Color(light:dark:)`-adaptive tokens in `MCTheme`,
    /// gated through `.preferredColorScheme(...)` on the overlay root.
    var overlayTheme: OverlayTheme {
        didSet {
            UserDefaults.standard.set(overlayTheme.rawValue, forKey: Self.overlayThemeKey)
        }
    }

    /// Configurable translucency for the overlay window's panel fill
    /// and any inner bubble/card backgrounds. Range [0.4, 1.0]. Lower
    /// values let the desktop bleed through; 1.0 is fully opaque.
    var overlayOpacity: Double {
        didSet {
            UserDefaults.standard.set(overlayOpacity, forKey: Self.overlayOpacityKey)
        }
    }

    private static let overlayThemeKey = "overlayTheme"
    private static let overlayOpacityKey = "overlayOpacity"
    private static let overlayOpacityDefault: Double = 0.78

    init() {
        let storedTheme = UserDefaults.standard.string(forKey: Self.overlayThemeKey).flatMap(OverlayTheme.init(rawValue:))
        self.overlayTheme = storedTheme ?? .light
        let storedOpacity = UserDefaults.standard.object(forKey: Self.overlayOpacityKey) as? Double
        self.overlayOpacity = storedOpacity.map { min(max($0, 0.4), 1.0) } ?? Self.overlayOpacityDefault
    }
}
