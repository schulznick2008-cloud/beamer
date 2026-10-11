// swift-tools-version:5.10
import PackageDescription

// libbeamer_transport.a wird von build.sh vorher gebaut (cargo build --release in ../transport).
let transportLib = "../transport/target/release"

let package = Package(
    name: "BeamerTeleport",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "BeamerTeleport", targets: ["BeamerTeleport"]),
    ],
    targets: [
        .target(name: "CBeamerTransport"),
        .target(
            name: "TeleportCore",
            dependencies: ["CBeamerTransport"],
            linkerSettings: [
                .unsafeFlags(["-L\(transportLib)", "-lbeamer_transport", "-liconv"]),
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("VideoToolbox"),
                .linkedFramework("CoreMedia"),
                .linkedFramework("CoreVideo"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("AppKit"),
            ]
        ),
        .executableTarget(name: "BeamerTeleport", dependencies: ["TeleportCore"]),
        .testTarget(name: "TeleportCoreTests", dependencies: ["TeleportCore"]),
    ],
    swiftLanguageVersions: [.v5]
)
