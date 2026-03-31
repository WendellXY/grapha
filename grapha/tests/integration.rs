use assert_cmd::Command;
use predicates::prelude::*;

fn grapha() -> Command {
    Command::cargo_bin("grapha").unwrap()
}

#[test]
fn analyzes_single_file() {
    grapha()
        .args(["analyze", "tests/fixtures/simple.rs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"struct\""))
        .stdout(predicate::str::contains("\"name\": \"Config\""))
        .stdout(predicate::str::contains("\"kind\": \"function\""))
        .stdout(predicate::str::contains("\"name\": \"default_config\""));
}

#[test]
fn analyzes_directory() {
    grapha()
        .args(["analyze", "tests/fixtures/multi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"run\""))
        .stdout(predicate::str::contains("\"name\": \"helper\""));
}

#[test]
fn filter_option_works() {
    grapha()
        .args(["analyze", "tests/fixtures/simple.rs", "--filter", "fn"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"function\""))
        .stdout(predicate::str::contains("\"kind\": \"struct\"").not());
}

#[test]
fn output_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.json");

    grapha()
        .args([
            "analyze",
            "tests/fixtures/simple.rs",
            "-o",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(&output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["version"], "0.1.0");
    assert!(!parsed["nodes"].as_array().unwrap().is_empty());
}

#[test]
fn empty_directory_produces_empty_graph() {
    let dir = tempfile::tempdir().unwrap();
    grapha()
        .args(["analyze", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"nodes\": []"));
}

#[test]
fn analyzes_swift_file() {
    grapha()
        .args(["analyze", "tests/fixtures/simple.swift"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"struct\""))
        .stdout(predicate::str::contains("\"name\": \"Config\""))
        .stdout(predicate::str::contains("\"kind\": \"function\""));
}

#[test]
fn invalid_filter_shows_error() {
    grapha()
        .args(["analyze", "tests/fixtures/simple.rs", "--filter", "bogus"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown node kind"));
}

#[test]
fn compact_flag_produces_grouped_output() {
    grapha()
        .args(["analyze", "tests/fixtures/simple.rs", "--compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"files\""))
        .stdout(predicate::str::contains("\"symbols\""))
        .stdout(predicate::str::contains("\"span\""));
}

#[test]
fn compact_flag_preserves_swiftui_hierarchy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ContentView.swift"),
        r#"
        import SwiftUI

        struct Row: View {
            let title: String
            var body: some View { Text(title) }
        }

        struct ContentView: View {
            var body: some View {
                VStack {
                    Text("Hello")
                    Row(title: "World")
                }
            }
        }
        "#,
    )
    .unwrap();

    grapha()
        .args(["analyze", dir.path().to_str().unwrap(), "--compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"view\""))
        .stdout(predicate::str::contains("\"name\": \"body\""))
        .stdout(predicate::str::contains("\"members\": ["))
        .stdout(predicate::str::contains("\"VStack\""))
        .stdout(predicate::str::contains("\"Text\""))
        .stdout(predicate::str::contains("\"Row\""))
        .stdout(predicate::str::contains("\"type_refs\": ["));
}

#[test]
fn output_contains_version() {
    grapha()
        .args(["analyze", "tests/fixtures/simple.rs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"version\": \"0.1.0\""));
}

#[test]
fn index_creates_sqlite_db() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");

    grapha()
        .args([
            "index",
            "tests/fixtures/simple.rs",
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("indexed"));

    assert!(store_dir.join("grapha.db").exists());
}

#[test]
fn index_json_format() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");

    grapha()
        .args([
            "index",
            "tests/fixtures/simple.rs",
            "--format",
            "json",
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(store_dir.join("graph.json").exists());
}

#[test]
fn repeated_index_uses_incremental_store_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    let source_path = dir.path().join("main.rs");
    std::fs::write(
        &source_path,
        "pub fn alpha() {}\npub fn beta() { alpha(); }\n",
    )
    .unwrap();

    grapha()
        .args([
            "index",
            dir.path().to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("full_rebuild"));

    std::fs::write(
        &source_path,
        "pub fn gamma() {}\npub fn beta() { gamma(); }\n",
    )
    .unwrap();

    grapha()
        .args([
            "index",
            dir.path().to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("incremental"))
        .stderr(predicate::str::contains("docs +1 ~0 -1"));

    grapha()
        .args(["search", "gamma", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"gamma\""))
        .stdout(predicate::str::contains("\"name\": \"alpha\"").not());
}

#[test]
fn context_command_returns_symbol_info() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");

    // First, index
    grapha()
        .args([
            "index",
            "tests/fixtures/simple.rs",
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Then query context
    grapha()
        .args([
            "context",
            "default_config",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"default_config\""));
}

#[test]
fn changes_command_runs_on_clean_repo() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");

    // Initialize a git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Configure git user for commits
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Create a Rust file and commit it
    std::fs::write(dir.path().join("main.rs"), "pub fn hello() {}").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Index it
    grapha()
        .args([
            "index",
            dir.path().to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Run changes — should succeed with no changes
    grapha()
        .args(["changes", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"changed_count\": 0"));
}

#[test]
fn search_command_finds_symbols() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");

    grapha()
        .args([
            "index",
            "tests/fixtures/simple.rs",
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    grapha()
        .args(["search", "Config", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Config"));
}
