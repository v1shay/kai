// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "NotchPrototype",
    platforms: [.macOS(.v13)],
    targets: [.executableTarget(name: "NotchPrototype", exclude: ["Resources"], linkerSettings: [.unsafeFlags([
        "-Xlinker", "-sectcreate", "-Xlinker", "__TEXT", "-Xlinker", "__info_plist",
        "-Xlinker", "Support/NotchPrototype-Info.plist"
    ])]), .testTarget(name: "KaiFeatureTests", dependencies: ["NotchPrototype"], path: "Tests/KaiFeatureTests")]
)
