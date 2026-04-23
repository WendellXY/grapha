use std::path::{Path, PathBuf};

pub const BRIDGE_MODE_ENV: &str = "GRAPHA_SWIFT_BRIDGE_MODE";
pub const BRIDGE_INPUTS: &[&str] = &[
    "swift-bridge/Sources/",
    "swift-bridge/Package.swift",
    "swift-bridge/Package.resolved",
];

pub fn bridge_build_paths(out_dir: &Path) -> (PathBuf, PathBuf) {
    let profile_dir = staged_bridge_dir(out_dir);
    let profile = profile_dir
        .file_name()
        .expect("profile output directory should have a final path component");
    let scratch_root = profile_dir
        .parent()
        .expect("profile output directory should live under target");
    let scratch_dir = scratch_root.join("swift-bridge-build").join(profile);
    let lib_dir = scratch_dir.join("release");
    (scratch_dir, lib_dir)
}

pub fn staged_bridge_dir(out_dir: &Path) -> PathBuf {
    out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR should live under target/<profile>/build/<pkg>/out")
        .to_path_buf()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMode {
    Auto,
    Off,
    Required,
}

impl BridgeMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Required => "required",
        }
    }
}

pub fn parse_bridge_mode(raw: Option<&str>) -> Result<BridgeMode, String> {
    match raw.unwrap_or(BridgeMode::Auto.as_str()) {
        "auto" => Ok(BridgeMode::Auto),
        "off" => Ok(BridgeMode::Off),
        "required" => Ok(BridgeMode::Required),
        other => Err(format!(
            "unsupported {BRIDGE_MODE_ENV}: {other} (expected auto, off, or required)"
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreBuildDecision {
    Skip(&'static str),
    Build,
    Panic(&'static str),
}

pub fn pre_build_decision(mode: BridgeMode, package_manifest_exists: bool) -> PreBuildDecision {
    match (mode, package_manifest_exists) {
        (BridgeMode::Off, _) => PreBuildDecision::Skip("off (skipping Swift bridge build)"),
        (BridgeMode::Auto, false) => {
            PreBuildDecision::Skip("auto (missing swift-bridge/Package.swift, using fallback)")
        }
        (BridgeMode::Required, false) => PreBuildDecision::Panic(
            "Swift bridge mode required: missing `swift-bridge/Package.swift`",
        ),
        (_, true) => PreBuildDecision::Build,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeBuildResult {
    Success,
    MissingDylib,
    FailedStatus,
    LaunchFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostBuildDecision {
    EnableBridge,
    Disable(&'static str),
    Panic(String),
}

pub fn post_build_decision(mode: BridgeMode, result: BridgeBuildResult) -> PostBuildDecision {
    match (mode, result) {
        (_, BridgeBuildResult::Success) => PostBuildDecision::EnableBridge,
        (BridgeMode::Auto, BridgeBuildResult::MissingDylib) => {
            PostBuildDecision::Disable("auto (swift build succeeded but dylib is missing, using fallback)")
        }
        (BridgeMode::Auto, BridgeBuildResult::FailedStatus) => {
            PostBuildDecision::Disable("auto (swift build failed, using fallback)")
        }
        (BridgeMode::Auto, BridgeBuildResult::LaunchFailed) => {
            PostBuildDecision::Disable("auto (failed to launch `swift build`, using fallback)")
        }
        (BridgeMode::Required, BridgeBuildResult::MissingDylib) => PostBuildDecision::Panic(
            "Swift bridge mode required: build succeeded but `libGraphaSwiftBridge.dylib` is missing"
                .to_string(),
        ),
        (BridgeMode::Required, BridgeBuildResult::FailedStatus) => {
            PostBuildDecision::Panic("Swift bridge mode required: swift build failed".to_string())
        }
        (BridgeMode::Required, BridgeBuildResult::LaunchFailed) => PostBuildDecision::Panic(
            "Swift bridge mode required: failed to launch `swift build`".to_string(),
        ),
        (BridgeMode::Off, _) => unreachable!("off mode does not run swift build"),
    }
}
