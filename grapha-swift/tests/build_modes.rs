#[path = "../build_support.rs"]
mod build_support;

use build_support::{
    BRIDGE_INPUTS, BridgeBuildResult, BridgeMode, PostBuildDecision, PreBuildDecision,
    bridge_build_paths, parse_bridge_mode, post_build_decision, pre_build_decision,
};
use std::path::Path;

#[test]
fn parses_default_mode_as_auto() {
    assert_eq!(parse_bridge_mode(None).unwrap(), BridgeMode::Auto);
}

#[test]
fn parses_known_modes() {
    assert_eq!(parse_bridge_mode(Some("auto")).unwrap(), BridgeMode::Auto);
    assert_eq!(parse_bridge_mode(Some("off")).unwrap(), BridgeMode::Off);
    assert_eq!(
        parse_bridge_mode(Some("required")).unwrap(),
        BridgeMode::Required
    );
}

#[test]
fn rejects_unknown_modes() {
    assert!(parse_bridge_mode(Some("sometimes")).is_err());
}

#[test]
fn watches_all_swift_bridge_inputs() {
    assert_eq!(
        BRIDGE_INPUTS,
        [
            "swift-bridge/Sources/",
            "swift-bridge/Package.swift",
            "swift-bridge/Package.resolved",
        ]
    );
}

#[test]
fn bridge_build_uses_out_dir_scratch_space() {
    let (scratch_dir, lib_dir) = bridge_build_paths(Path::new("/tmp/grapha-out"));

    assert_eq!(scratch_dir, Path::new("/tmp/grapha-out/swift-bridge-build"));
    assert_eq!(
        lib_dir,
        Path::new("/tmp/grapha-out/swift-bridge-build/release")
    );
}

#[test]
fn off_mode_skips_bridge_build_without_invoking_swift() {
    assert_eq!(
        pre_build_decision(BridgeMode::Off, true),
        PreBuildDecision::Skip("off (skipping Swift bridge build)")
    );
}

#[test]
fn auto_and_required_diverge_when_package_manifest_is_missing() {
    assert_eq!(
        pre_build_decision(BridgeMode::Auto, false),
        PreBuildDecision::Skip("auto (missing swift-bridge/Package.swift, using fallback)")
    );
    assert_eq!(
        pre_build_decision(BridgeMode::Required, false),
        PreBuildDecision::Panic("Swift bridge mode required: missing `swift-bridge/Package.swift`")
    );
}

#[test]
fn auto_mode_falls_back_when_swift_build_fails() {
    assert_eq!(
        post_build_decision(BridgeMode::Auto, BridgeBuildResult::FailedStatus),
        PostBuildDecision::Disable("auto (swift build failed, using fallback)")
    );
}

#[test]
fn required_mode_fails_when_swift_build_fails() {
    assert_eq!(
        post_build_decision(BridgeMode::Required, BridgeBuildResult::FailedStatus),
        PostBuildDecision::Panic("Swift bridge mode required: swift build failed".to_string())
    );
}

#[test]
fn success_and_missing_dylib_decisions_are_mode_aware() {
    assert_eq!(
        post_build_decision(BridgeMode::Auto, BridgeBuildResult::Success),
        PostBuildDecision::EnableBridge
    );
    assert_eq!(
        post_build_decision(BridgeMode::Auto, BridgeBuildResult::MissingDylib),
        PostBuildDecision::Disable(
            "auto (swift build succeeded but dylib is missing, using fallback)"
        )
    );
    assert_eq!(
        post_build_decision(BridgeMode::Required, BridgeBuildResult::MissingDylib),
        PostBuildDecision::Panic(
            "Swift bridge mode required: build succeeded but `libGraphaSwiftBridge.dylib` is missing"
                .to_string()
        )
    );
}

#[test]
fn launch_failure_only_falls_back_in_auto_mode() {
    assert_eq!(
        post_build_decision(BridgeMode::Auto, BridgeBuildResult::LaunchFailed),
        PostBuildDecision::Disable("auto (failed to launch `swift build`, using fallback)")
    );
    assert_eq!(
        post_build_decision(BridgeMode::Required, BridgeBuildResult::LaunchFailed),
        PostBuildDecision::Panic(
            "Swift bridge mode required: failed to launch `swift build`".to_string()
        )
    );
}
