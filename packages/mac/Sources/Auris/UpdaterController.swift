// UpdaterController.swift
// Thin SwiftUI-friendly wrapper around `SPUStandardUpdaterController`.
// Sparkle drives the OTA flow — periodic background checks against
// `SUFeedURL` (Info.plist), download + EdDSA verification, install +
// relaunch. `SUEnableAutomaticChecks` + `SUScheduledCheckInterval`
// in the bundled Info.plist control the cadence; the user can
// always force a check via the menu-bar's "Check for updates…"
// item.

import Foundation
import Sparkle

@MainActor
final class UpdaterController: ObservableObject {
    let underlying: SPUStandardUpdaterController

    /// Mirrors `SPUUpdater.canCheckForUpdates`. We expose it as an
    /// `@Published` so the menu item can disable itself while a check
    /// is already in flight (avoiding the "two checks racing" UX where
    /// the user mashes the menu while waiting for the daily auto-check
    /// dialog).
    @Published var canCheckForUpdates: Bool = false

    init() {
        // `startingUpdater: true` kicks off the scheduled-check loop
        // immediately. The check itself is async — the first network
        // request happens after a short bootstrap delay so app launch
        // stays snappy.
        let controller = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )
        self.underlying = controller

        // Bridge SPUUpdater's KVO-published `canCheckForUpdates` into
        // our @Published property. Sparkle publishes via Combine; we
        // assign into `$canCheckForUpdates` so SwiftUI views observe
        // changes directly.
        controller.updater.publisher(for: \.canCheckForUpdates)
            .receive(on: RunLoop.main)
            .assign(to: &$canCheckForUpdates)
    }

    func checkForUpdates() {
        underlying.updater.checkForUpdates()
    }
}
