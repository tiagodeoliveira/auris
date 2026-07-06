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
    /// Local-dev fallback when no `AurisServerURL` is
    /// embedded in the bundle's Info.plist. Used by `swift run`
    /// where there's no Info.plist; CI builds always have the key
    /// populated by envsubst from the `AURIS_SERVER_URL`
    /// repo variable (see .github/workflows/mac-bundle.yml).
    static let serverURLDefault = "ws://localhost:7331"

    /// WebSocket server URL. Resolution order:
    ///   1. `AURIS_SERVER_URL` env var — lets `just mac-run`
    ///      force a localhost target regardless of what's bundled.
    ///   2. The bundled Info.plist value — used by the signed CI build
    ///      where envsubst baked the production URL in.
    ///   3. Hardcoded `ws://localhost:7331` so an unbundled `swift run`
    ///      with no env config still finds a local server.
    var serverURL: String {
        if let env = ProcessInfo.processInfo.environment["AURIS_SERVER_URL"],
           !env.isEmpty
        {
            return env
        }
        if let bundled = Bundle.main.object(forInfoDictionaryKey: "AurisServerURL") as? String,
           !bundled.isEmpty
        {
            return bundled
        }
        return Self.serverURLDefault
    }

    /// Visual theme for the meeting overlay. Drives the panel/text
    /// palette via `Color(light:dark:)`-adaptive tokens in `AurisTheme`,
    /// gated through `.preferredColorScheme(...)` on the overlay root.
    var overlayTheme: OverlayTheme {
        didSet {
            UserDefaults.standard.set(overlayTheme.rawValue, forKey: Self.overlayThemeKey)
        }
    }

    /// Configurable translucency for the overlay window's panel fill
    /// and any inner bubble/card backgrounds. Range [0.01, 1.0]. The
    /// floor is intentionally near-zero so the user can dial the
    /// overlay down to a barely-visible heads-up — text strokes
    /// remain at full opacity above the panel fill, so 1% panel is
    /// still readable. 1.0 is fully opaque.
    var overlayOpacity: Double {
        didSet {
            UserDefaults.standard.set(overlayOpacity, forKey: Self.overlayOpacityKey)
        }
    }

    /// Whether the overlay should appear automatically when a meeting
    /// starts on a different surface (mobile / PWA). Set to false
    /// when the user manually dismisses the overlay during an active
    /// meeting; that gesture reads as "stop popping at me," and the
    /// preference survives across launches. Local-start (Mac itself)
    /// always shows the overlay regardless of this flag — see
    /// cross-surface-coordination.md Rule 5.
    var overlayAutoShow: Bool {
        didSet {
            UserDefaults.standard.set(overlayAutoShow, forKey: Self.overlayAutoShowKey)
        }
    }

    private static let overlayThemeKey = "overlayTheme"
    private static let overlayOpacityKey = "overlayOpacity"
    private static let overlayOpacityDefault: Double = 0.78
    private static let overlayAutoShowKey = "overlay.autoShow"

    init() {
        let storedTheme = UserDefaults.standard.string(forKey: Self.overlayThemeKey).flatMap(OverlayTheme.init(rawValue:))
        self.overlayTheme = storedTheme ?? .light
        let storedOpacity = UserDefaults.standard.object(forKey: Self.overlayOpacityKey) as? Double
        self.overlayOpacity = storedOpacity.map { min(max($0, 0.01), 1.0) } ?? Self.overlayOpacityDefault
        // UserDefaults' `bool(forKey:)` returns false for missing keys
        // — wrong default here. Read as `object(forKey:)` and treat
        // `nil` as "never set" so the default is true.
        if let stored = UserDefaults.standard.object(forKey: Self.overlayAutoShowKey) as? Bool {
            self.overlayAutoShow = stored
        } else {
            self.overlayAutoShow = true
        }
    }
}
