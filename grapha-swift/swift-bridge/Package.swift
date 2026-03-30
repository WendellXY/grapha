// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "GraphaSwiftBridge",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "GraphaSwiftBridge", type: .dynamic, targets: ["GraphaSwiftBridge"]),
    ],
    dependencies: [
        .package(url: "https://github.com/swiftlang/swift-syntax.git", from: "601.0.0"),
    ],
    targets: [
        .target(
            name: "GraphaSwiftBridge",
            dependencies: [
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftParser", package: "swift-syntax"),
            ]
        ),
    ]
)
