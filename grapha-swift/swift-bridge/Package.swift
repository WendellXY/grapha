// swift-tools-version: 6.3
import PackageDescription

let package = Package(
    name: "GraphaSwiftBridge",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "GraphaSwiftBridge", type: .dynamic, targets: ["GraphaSwiftBridge"]),
    ],
    dependencies: [
        .package(url: "https://github.com/WendellXY/CodableKit.git", from: "2.0.0"),
        .package(url: "https://github.com/swiftlang/swift-syntax.git", from: "603.0.0"),
    ],
    targets: [
        .systemLibrary(
            name: "CIndexStore",
            path: "Sources/CIndexStore"
        ),
        .target(
            name: "GraphaSwiftBridge",
            dependencies: [
                "CIndexStore",
                .product(name: "CodableKit", package: "CodableKit"),
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftParser", package: "swift-syntax"),
            ],
            linkerSettings: [
                .unsafeFlags([
                    "-L/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib",
                    "-Xlinker", "-rpath",
                    "-Xlinker", "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib",
                ]),
            ]
        ),
        .testTarget(
            name: "GraphaSwiftBridgeTests",
            dependencies: ["GraphaSwiftBridge"]
        ),
    ]
)
