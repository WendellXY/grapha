use assert_cmd::Command;
use predicates::prelude::*;

fn grapha() -> Command {
    Command::cargo_bin("grapha").unwrap()
}

fn index_temp_project(source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(dir.path().join("main.rs"), source).unwrap();

    grapha()
        .args([
            "index",
            dir.path().to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    dir
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
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args(["trace", "main", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("entry"));
}

#[test]
fn reverse_command_works() {
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args(["reverse", "helper", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("symbol"));
}

#[test]
fn impact_command_defaults_to_json() {
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args(["impact", "helper", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"source\""))
        .stdout(predicate::str::contains("\"total_affected\": 1"));
}

#[test]
fn context_tree_format_works() {
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args([
            "context",
            "helper",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("helper [function] (main.rs)"))
        .stdout(predicate::str::contains("callers (1)"))
        .stdout(predicate::str::contains("main [function] (main.rs)"))
        .stdout(predicate::str::contains("└──"));
}

#[test]
fn entries_tree_format_works() {
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args([
            "entries",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("entry points (1)"))
        .stdout(predicate::str::contains("main [function] (main.rs)"))
        .stdout(predicate::str::contains("└──"));
}

#[test]
fn trace_tree_format_works() {
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args([
            "trace",
            "main",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("main [function] (main.rs)"))
        .stdout(predicate::str::contains(
            "summary: flows=0, reads=0, writes=0, async_crossings=0",
        ))
        .stdout(predicate::str::contains("flows (0)"));
}

#[test]
fn reverse_tree_format_works() {
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args([
            "reverse",
            "helper",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("helper [function] (main.rs)"))
        .stdout(predicate::str::contains("affected entries (1)"))
        .stdout(predicate::str::contains(
            "main [entry] [function] (main.rs)",
        ))
        .stdout(predicate::str::contains("└──"))
        .stdout(predicate::str::contains("\"symbol\"").not());
}

#[test]
fn impact_tree_format_works() {
    let dir = index_temp_project("fn main() { helper(); }\nfn helper() {}\n");

    grapha()
        .args([
            "impact",
            "helper",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("helper [function] (main.rs)"))
        .stdout(predicate::str::contains(
            "summary: depth_1=1, depth_2=0, depth_3_plus=0, total=1",
        ))
        .stdout(predicate::str::contains("dependents (1)"))
        .stdout(predicate::str::contains("main [function] (main.rs)"))
        .stdout(predicate::str::contains("└──"))
        .stdout(predicate::str::contains("\"source\"").not());
}
