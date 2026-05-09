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
        // Sparkle drives the OTA update flow. App polls SUFeedURL
        // (an appcast.xml attached to each GitHub Release), prompts
        // the user when a newer signed bundle is available, downloads
        // + verifies the EdDSA signature, and replaces the .app in
        // /Applications. CI signs the bundle on tag pushes — see
        // .github/workflows/mac-bundle.yml.
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.6.0")
    ],
    targets: [
        .executableTarget(
            name: "MeetingCompanion",
            dependencies: [
                .product(name: "Sparkle", package: "Sparkle")
            ],
            path: "Sources/MeetingCompanion"
        )
    ]
)
