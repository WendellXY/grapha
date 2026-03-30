use assert_cmd::Command;
use predicates::prelude::*;

fn grapha() -> Command {
    Command::cargo_bin("grapha").unwrap()
}

#[test]
fn analyze_outputs_dataflow_fields() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.rs"),
        "fn main() { save_data(); }\nfn save_data() {}\n",
    )
    .unwrap();

    grapha()
        .args(["analyze", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("nodes"))
        .stdout(predicate::str::contains("edges"));
}

#[test]
fn index_and_entries_works() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(
        dir.path().join("main.rs"),
        "fn main() { helper(); }\nfn helper() {}\n",
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
        .success();

    grapha()
        .args(["entries", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("entries"));
}

#[test]
fn trace_command_works() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(
        dir.path().join("main.rs"),
        "fn main() { helper(); }\nfn helper() {}\n",
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
        .success();

    grapha()
        .args(["trace", "main", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("entry"));
}

#[test]
fn reverse_command_works() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(
        dir.path().join("main.rs"),
        "fn main() { helper(); }\nfn helper() {}\n",
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
        .success();

    grapha()
        .args(["reverse", "helper", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("symbol"));
}
