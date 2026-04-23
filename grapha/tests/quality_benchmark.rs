//! Repeatable graph-quality harness for Grapha.
//!
//! Run locally with:
//! `cargo test -p grapha --test quality_benchmark -- --ignored --nocapture`
//!
//! The fixture is synthetic and Rust-only, so it does not depend on any Swift
//! sample project. The test measures:
//! - impact traversal behavior
//! - architecture violation detection
//! - compact-output size versus the regular analyze output
//! - elapsed time for each CLI path as a lightweight latency signal

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::Value;

fn grapha() -> Command {
    Command::cargo_bin("grapha").unwrap()
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("quality")
}

fn copy_fixture_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();

    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_fixture_tree(&src_path, &dst_path);
        } else {
            fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

fn prepare_project() -> tempfile::TempDir {
    let tempdir = tempfile::tempdir().unwrap();
    copy_fixture_tree(&fixture_root(), tempdir.path());
    tempdir
}

fn run_cli(args: &[&str]) -> (Duration, String) {
    let started = Instant::now();
    let output = grapha()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    (
        started.elapsed(),
        String::from_utf8(output).expect("CLI output should be valid UTF-8"),
    )
}

fn run_index(project_root: &Path) -> Duration {
    let store_dir = project_root.join(".grapha");
    let started = Instant::now();
    grapha()
        .args([
            "index",
            project_root.to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();
    started.elapsed()
}

fn json_output(args: &[&str]) -> (Duration, Value) {
    let (elapsed, stdout) = run_cli(args);
    let value = serde_json::from_str(&stdout).expect("CLI output should be JSON");
    (elapsed, value)
}

#[test]
#[ignore]
fn graph_quality_benchmark() {
    let project = prepare_project();
    let project_root = project.path();
    let index_elapsed = run_index(project_root);

    let (impact_elapsed, impact_json) = json_output(&[
        "symbol",
        "impact",
        "fetch",
        "-p",
        project_root.to_str().unwrap(),
    ]);
    let depth_1 = impact_json["depth_1"]
        .as_array()
        .expect("impact depth_1 should be an array");
    let depth_2 = impact_json["depth_2"]
        .as_array()
        .expect("impact depth_2 should be an array");
    let depth_3_plus = impact_json["depth_3_plus"]
        .as_array()
        .expect("impact depth_3_plus should be an array");

    assert_eq!(impact_json["total_affected"].as_u64(), Some(3));
    assert_eq!(depth_1.len(), 1);
    assert_eq!(depth_1[0]["name"], "load");
    assert_eq!(depth_2.len(), 1);
    assert_eq!(depth_2[0]["name"], "run");
    assert_eq!(depth_3_plus.len(), 1);
    assert_eq!(depth_3_plus[0]["name"], "main");

    let (arch_elapsed, arch_json) =
        json_output(&["repo", "arch", "-p", project_root.to_str().unwrap()]);
    assert_eq!(arch_json["configured"].as_bool(), Some(true));
    assert_eq!(arch_json["total_violations"].as_u64(), Some(1));
    let violations = arch_json["violations"]
        .as_array()
        .expect("architecture violations should be an array");
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0]["source_layer"].as_str(), Some("infra"));
    assert_eq!(violations[0]["target_layer"].as_str(), Some("ui"));
    assert_eq!(violations[0]["source"]["name"].as_str(), Some("load"));
    assert_eq!(
        violations[0]["target"]["name"].as_str(),
        Some("render_banner")
    );

    let (analyze_elapsed, analyze_stdout) = run_cli(&["analyze", project_root.to_str().unwrap()]);
    let (compact_elapsed, compact_stdout) =
        run_cli(&["analyze", project_root.to_str().unwrap(), "--compact"]);

    let analyze_bytes = analyze_stdout.len();
    let compact_bytes = compact_stdout.len();
    assert!(
        compact_bytes < analyze_bytes,
        "compact output should be smaller than regular analyze output: compact={compact_bytes}, regular={analyze_bytes}"
    );

    let metrics = serde_json::json!({
        "index_ms": index_elapsed.as_secs_f64() * 1000.0,
        "impact_ms": impact_elapsed.as_secs_f64() * 1000.0,
        "arch_ms": arch_elapsed.as_secs_f64() * 1000.0,
        "analyze_ms": analyze_elapsed.as_secs_f64() * 1000.0,
        "compact_ms": compact_elapsed.as_secs_f64() * 1000.0,
        "analyze_bytes": analyze_bytes,
        "compact_bytes": compact_bytes,
        "compact_ratio": compact_bytes as f64 / analyze_bytes as f64,
        "impact_total_affected": impact_json["total_affected"],
        "architecture_violations": arch_json["total_violations"],
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&metrics).expect("metrics should serialize")
    );
}
