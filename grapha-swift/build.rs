mod build_support;

use std::path::Path;
use std::process::Command;

use build_support::{
    BRIDGE_INPUTS, BRIDGE_MODE_ENV, BridgeBuildResult, BridgeMode, PostBuildDecision,
    PreBuildDecision, parse_bridge_mode, post_build_decision, pre_build_decision,
};

const DISABLE_SWIFT_SANDBOX_ENV: &str = "GRAPHA_SWIFT_BUILD_DISABLE_SANDBOX";

fn main() {
    println!("cargo::rustc-check-cfg=cfg(no_swift_bridge)");
    println!("cargo:rerun-if-env-changed={BRIDGE_MODE_ENV}");
    println!("cargo:rerun-if-env-changed={DISABLE_SWIFT_SANDBOX_ENV}");

    let mode = parse_bridge_mode(std::env::var(BRIDGE_MODE_ENV).ok().as_deref())
        .unwrap_or_else(|err| panic!("{err}"));

    for path in BRIDGE_INPUTS {
        println!("cargo:rerun-if-changed={path}");
    }

    if mode == BridgeMode::Off {
        disable_bridge("off (skipping Swift bridge build)");
        return;
    }

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("Cargo should always provide CARGO_MANIFEST_DIR to build scripts");
    let bridge_dir = Path::new(&manifest_dir).join("swift-bridge");
    let package_manifest = bridge_dir.join("Package.swift");
    match pre_build_decision(mode, package_manifest.exists()) {
        PreBuildDecision::Skip(message) => {
            disable_bridge(message);
            return;
        }
        PreBuildDecision::Build => {}
        PreBuildDecision::Panic(message) => panic!("{message}"),
    }

    let mut swift_build = Command::new("swift");
    swift_build.arg("build");

    if disable_swift_sandbox() {
        swift_build.arg("--disable-sandbox");
    }

    let status = swift_build
        .args(["-c", "release"])
        .current_dir(&bridge_dir)
        .status();

    let lib_path = bridge_dir.join(".build/release");
    let build_result = match status {
        Ok(s) if s.success() && lib_path.join("libGraphaSwiftBridge.dylib").exists() => {
            BridgeBuildResult::Success
        }
        Ok(s) if s.success() => BridgeBuildResult::MissingDylib,
        Ok(_) => BridgeBuildResult::FailedStatus,
        Err(_) => BridgeBuildResult::LaunchFailed,
    };

    match post_build_decision(mode, build_result) {
        PostBuildDecision::EnableBridge => {
            println!("cargo:warning=Swift bridge mode: {}", mode.as_str());
            println!("cargo:rustc-env=SWIFT_BRIDGE_PATH={}", lib_path.display());
        }
        PostBuildDecision::Disable(message) => {
            disable_bridge(message);
        }
        PostBuildDecision::Panic(message) => {
            panic!("{message}");
        }
    }
}

fn disable_bridge(message: &str) {
    println!("cargo:warning=Swift bridge mode: {message}");
    println!("cargo:rustc-cfg=no_swift_bridge");
}

fn disable_swift_sandbox() -> bool {
    matches!(
        std::env::var(DISABLE_SWIFT_SANDBOX_ENV)
            .ok()
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}
