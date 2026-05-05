// swift-tools-version:5.9

import PackageDescription

let package = Package(
    name: "MeetingCompanion",
    platforms: [
        // macOS 15 is required for SCStream's microphone-capture
        // path (`config.captureMicrophone = true` and the
        // `.microphone` SCStreamOutputType). The Rust server gates
        // the same feature behind cargo's `macos_15_0` flag.
        // Using the string form (`.macOS("15.0")`) instead of `.v15`
        // because the latter requires swift-tools-version 6.0+.
        .macOS("15.0")
    ],
    products: [
        .executable(name: "MeetingCompanion", targets: ["MeetingCompanion"])
    ],
    dependencies: [
        // Will be added in subsequent Phase 2 sub-phases:
        // - Starscream or URLSession.WebSocketTask for WS client
        // - Sparkle for autoupdates (Phase 6+)
    ],
    targets: [
        .executableTarget(
            name: "MeetingCompanion",
            path: "Sources/MeetingCompanion"
        )
    ]
)
