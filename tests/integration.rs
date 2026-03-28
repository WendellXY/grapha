use assert_cmd::Command;
use predicates::prelude::*;

fn grapha() -> Command {
    Command::cargo_bin("grapha").unwrap()
}

#[test]
fn analyzes_single_file() {
    grapha()
        .arg("tests/fixtures/simple.rs")
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
        .arg("tests/fixtures/multi")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"run\""))
        .stdout(predicate::str::contains("\"name\": \"helper\""));
}

#[test]
fn filter_option_works() {
    grapha()
        .args(["tests/fixtures/simple.rs", "--filter", "fn"])
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
            "tests/fixtures/simple.rs",
            "-o",
            output.to_str().unwrap(),
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(&output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["version"], "0.1.0");
    assert!(parsed["nodes"].as_array().unwrap().len() > 0);
}

#[test]
fn empty_directory_produces_empty_graph() {
    let dir = tempfile::tempdir().unwrap();
    grapha()
        .arg(dir.path().to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("\"nodes\": []"));
}

#[test]
fn invalid_filter_shows_error() {
    grapha()
        .args(["tests/fixtures/simple.rs", "--filter", "bogus"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown node kind"));
}

#[test]
fn output_contains_version() {
    grapha()
        .arg("tests/fixtures/simple.rs")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"version\": \"0.1.0\""));
}
