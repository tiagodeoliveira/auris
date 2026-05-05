// swift-tools-version:5.9

import PackageDescription

let package = Package(
    name: "MeetingCompanion",
    platforms: [
        .macOS(.v14)
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
