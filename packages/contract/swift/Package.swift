// swift-tools-version:5.9
//
// Wire protocol types for the Meeting Companion Mac client. Source
// of truth lives in `packages/contract/proto/meeting_companion/v1/*.proto`;
// generated `.pb.swift` files are committed under
// `Sources/MeetingCompanionContract/Generated/` so consumers don't
// need protoc / buf at build time.
//
// Regenerate via `just contract-gen` from the workspace root.

import PackageDescription

let package = Package(
    name: "MeetingCompanionContract",
    // Match the Mac client's deployment target. String form (vs
    // `.v15`) is required when swift-tools-version is 5.9 — the
    // `.v15` enum case landed later.
    platforms: [.macOS("15.0")],
    products: [
        .library(
            name: "MeetingCompanionContract",
            targets: ["MeetingCompanionContract"]
        )
    ],
    dependencies: [
        .package(
            url: "https://github.com/apple/swift-protobuf.git",
            from: "1.27.0"
        )
    ],
    targets: [
        .target(
            name: "MeetingCompanionContract",
            dependencies: [
                .product(name: "SwiftProtobuf", package: "swift-protobuf")
            ],
            path: "Sources/MeetingCompanionContract"
        )
    ]
)
