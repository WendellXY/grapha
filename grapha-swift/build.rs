use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(no_swift_bridge)");

    let swift_version = Command::new("swift").arg("--version").output();
    if swift_version.is_err() {
        println!("cargo:warning=Swift toolchain not found — bridge disabled");
        println!("cargo:rustc-cfg=no_swift_bridge");
        return;
    }

    let bridge_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("swift-bridge");
    if !bridge_dir.join("Package.swift").exists() {
        println!("cargo:warning=swift-bridge/Package.swift not found — bridge disabled");
        println!("cargo:rustc-cfg=no_swift_bridge");
        return;
    }

    let status = Command::new("swift")
        .args(["build", "-c", "release"])
        .current_dir(&bridge_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            let lib_path = bridge_dir.join(".build/release");
            println!("cargo:rustc-env=SWIFT_BRIDGE_PATH={}", lib_path.display());
            println!("cargo:rerun-if-changed=swift-bridge/Sources/");
            println!("cargo:rerun-if-changed=swift-bridge/Package.swift");
        }
        Ok(s) => {
            println!(
                "cargo:warning=Swift bridge build failed (exit {s}), using tree-sitter fallback"
            );
            println!("cargo:rustc-cfg=no_swift_bridge");
        }
        Err(e) => {
            println!("cargo:warning=Swift bridge build error: {e}, using tree-sitter fallback");
            println!("cargo:rustc-cfg=no_swift_bridge");
        }
    }
}
