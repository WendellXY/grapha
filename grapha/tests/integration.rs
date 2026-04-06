use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn grapha() -> Command {
    Command::cargo_bin("grapha").unwrap()
}

fn strip_ansi(input: &str) -> String {
    let mut stripped = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        stripped.push(ch);
    }
    stripped
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

fn write_localizable_fixture(path: &std::path::Path, key: &str, value: &str, comment: &str) {
    std::fs::write(
        path,
        format!(
            r#"{{
  "sourceLanguage" : "en",
  "strings" : {{
    "{key}" : {{
      "comment" : "{comment}",
      "localizations" : {{
        "en" : {{
          "stringUnit" : {{
            "state" : "translated",
            "value" : "{value}"
          }}
        }}
      }}
    }}
  }},
  "version" : "1.0"
}}"#
        ),
    )
    .unwrap();
}

fn write_strings_fixture(path: &std::path::Path, key: &str, value: &str) {
    std::fs::write(path, format!(r#""{key}" = "{value}";"#)).unwrap();
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
    assert!(store_dir.join("localization.json").exists());
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
    assert!(store_dir.join("localization.json").exists());
}

#[test]
fn symbol_search_includes_id_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(
        dir.path().join("main.rs"),
        r#"
        fn helper() {}
        fn run() { helper(); }
        "#,
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

    let output = grapha()
        .args([
            "symbol",
            "search",
            "helper",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&output).unwrap();
    let first = parsed.as_array().unwrap().first().unwrap();
    assert_eq!(first["name"], "helper");
    assert!(
        first.get("id").is_some(),
        "default search output should include id"
    );
}

#[test]
fn flow_entries_tree_respects_file_field_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(
        dir.path().join("main.rs"),
        r#"
        fn helper() {}
        fn main() { helper(); }
        "#,
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
        .args([
            "flow",
            "entries",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
            "--fields",
            "none",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("main [function]"))
        .stdout(predicate::str::contains("(main.rs)").not());
}

#[test]
fn flow_origin_help_mentions_full_field_alias() {
    grapha()
        .args(["flow", "origin", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"full\"/\"all\"/\"none\""));
}

#[test]
fn index_skips_invalid_xcstrings_catalogs() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");

    std::fs::write(
        dir.path().join("ContentView.swift"),
        r#"
        import SwiftUI

        struct ContentView: View {
            var body: some View { Text("Hello") }
        }
        "#,
    )
    .unwrap();
    write_localizable_fixture(
        &dir.path().join("Localizable.xcstrings"),
        "hello",
        "Hello",
        "Greeting",
    );
    std::fs::write(
        dir.path().join("Broken.xcstrings"),
        r#"{
  "sourceLanguage" : "en",
  "strings" : {
    "broken" : {},
  },
  "version" : "1.0"
}"#,
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
        .stderr(predicate::str::contains(
            "skipped invalid localization catalog Broken.xcstrings",
        ));

    assert!(store_dir.join("localization.json").exists());
}

#[test]
fn localize_and_usages_commands_resolve_swiftui_xcstrings() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");

    std::fs::write(
        dir.path().join("ContentView.swift"),
        r#"
        import SwiftUI

        struct ContentView: View {
            var body: some View {
                VStack {
                    Text(.accountForgetPassword)
                }
            }
        }
        "#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("Strings.generated.swift"),
        r#"
        import Foundation

        public enum L10n {
            public static var accountForgetPassword: String {
                L10n.tr("Localizable", "account_forget_password", fallback: "Forgot Password")
            }

            private static func tr(_ table: String, _ key: String, fallback: String) -> String {
                fallback
            }
        }
        "#,
    )
    .unwrap();

    write_localizable_fixture(
        &dir.path().join("Localizable.xcstrings"),
        "account_forget_password",
        "Forgot Password",
        "Shown on the login screen",
    );

    grapha()
        .args([
            "index",
            dir.path().to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(store_dir.join("localization.json").exists());
    std::fs::remove_file(dir.path().join("Localizable.xcstrings")).unwrap();

    let localize_output = grapha()
        .args(["l10n", "symbol", "body", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let localize: Value = serde_json::from_slice(&localize_output).unwrap();
    let matches = localize["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0]["record"]["key"].as_str(),
        Some("account_forget_password")
    );
    assert_eq!(
        matches[0]["record"]["catalog_file"].as_str(),
        Some("Localizable.xcstrings")
    );
    assert_eq!(
        matches[0]["record"]["source_value"].as_str(),
        Some("Forgot Password")
    );
    assert_eq!(
        matches[0]["reference"]["wrapper_name"].as_str(),
        Some("accountForgetPassword")
    );

    let usages_output = grapha()
        .args([
            "l10n",
            "usages",
            "account_forget_password",
            "--table",
            "Localizable",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let usages: Value = serde_json::from_slice(&usages_output).unwrap();
    let usage_records = usages["records"].as_array().unwrap();
    assert_eq!(usage_records.len(), 1);
    let usage_sites = usage_records[0]["usages"].as_array().unwrap();
    assert_eq!(usage_sites.len(), 1);
    assert_eq!(usage_sites[0]["owner"]["name"].as_str(), Some("body"));
    assert_eq!(usage_sites[0]["view"]["name"].as_str(), Some("Text"));
    assert_eq!(
        usage_sites[0]["reference"]["wrapper_name"].as_str(),
        Some("accountForgetPassword")
    );
}

#[test]
fn localize_and_usages_commands_resolve_swiftui_strings_with_l10n_resource() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    let resources_dir = dir.path().join("Resources/en.lproj");
    std::fs::create_dir_all(&resources_dir).unwrap();

    std::fs::write(
        dir.path().join("ContentView.swift"),
        r#"
        import SwiftUI

        struct ContentView: View {
            var body: some View {
                VStack {
                    Text(i18n: .accountForgetPassword)
                }
            }
        }
        "#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("L10nResource.swift"),
        r#"
        import SwiftUI

        public struct L10nResource {
            public let key: String
            public let table: String
            public let fallback: String

            public init(_ key: String, table: String, fallback: String) {
                self.key = key
                self.table = table
                self.fallback = fallback
            }

            public var translation: String {
                fallback
            }
        }

        extension Text {
            public init(i18n resource: L10nResource) {
                self.init(resource.translation)
            }
        }
        "#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("Strings.generated.swift"),
        r#"
        import Foundation

        extension L10nResource {
            public static let accountForgetPassword = L10nResource(
                "account_forget_password",
                table: "Localizable",
                fallback: "Forgot Password"
            )
        }
        "#,
    )
    .unwrap();

    write_strings_fixture(
        &resources_dir.join("Localizable.strings"),
        "account_forget_password",
        "Forgot Password",
    );

    grapha()
        .args([
            "index",
            dir.path().to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    let localize_output = grapha()
        .args(["l10n", "symbol", "body", "-p", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let localize: Value = serde_json::from_slice(&localize_output).unwrap();
    let matches = localize["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0]["record"]["key"].as_str(),
        Some("account_forget_password")
    );
    assert_eq!(
        matches[0]["record"]["catalog_file"].as_str(),
        Some("Resources/en.lproj/Localizable.strings")
    );
    assert_eq!(
        matches[0]["reference"]["wrapper_base"].as_str(),
        Some("L10nResource")
    );

    let usages_output = grapha()
        .args([
            "l10n",
            "usages",
            "account_forget_password",
            "--table",
            "Localizable",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let usages: Value = serde_json::from_slice(&usages_output).unwrap();
    let usage_records = usages["records"].as_array().unwrap();
    assert_eq!(usage_records.len(), 1);
    assert_eq!(
        usage_records[0]["record"]["catalog_dir"].as_str(),
        Some("Resources")
    );
    assert_eq!(
        usage_records[0]["usages"][0]["reference"]["wrapper_base"].as_str(),
        Some("L10nResource")
    );
}

#[test]
fn localize_and_usages_prefer_nearest_duplicate_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    let auth_sources = dir.path().join("Packages/Auth/Sources/Auth");
    let profile_sources = dir.path().join("Packages/Profile/Sources/Profile");
    std::fs::create_dir_all(&auth_sources).unwrap();
    std::fs::create_dir_all(&profile_sources).unwrap();

    std::fs::write(
        auth_sources.join("AuthView.swift"),
        r#"
        import SwiftUI

        struct AuthView: View {
            var body: some View {
                VStack {
                    Text(.sharedTitle)
                }
            }
        }
        "#,
    )
    .unwrap();

    std::fs::write(
        auth_sources.join("Strings.generated.swift"),
        r#"
        import Foundation

        public enum L10n {
            public static var sharedTitle: String {
                L10n.tr("Localizable", "shared_title", fallback: "Shared")
            }

            private static func tr(_ table: String, _ key: String, fallback: String) -> String {
                fallback
            }
        }
        "#,
    )
    .unwrap();

    write_localizable_fixture(
        &dir.path().join("Packages/Auth/Localizable.xcstrings"),
        "shared_title",
        "Auth Shared",
        "Auth catalog",
    );
    write_localizable_fixture(
        &dir.path().join("Packages/Profile/Localizable.xcstrings"),
        "shared_title",
        "Profile Shared",
        "Profile catalog",
    );

    grapha()
        .args([
            "index",
            dir.path().to_str().unwrap(),
            "--store-dir",
            store_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    let localize_output = grapha()
        .args([
            "l10n",
            "symbol",
            "AuthView",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let localize: Value = serde_json::from_slice(&localize_output).unwrap();
    let matches = localize["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0]["record"]["catalog_file"].as_str(),
        Some("Packages/Auth/Localizable.xcstrings")
    );
    assert_eq!(
        matches[0]["record"]["source_value"].as_str(),
        Some("Auth Shared")
    );

    let usages_output = grapha()
        .args([
            "l10n",
            "usages",
            "shared_title",
            "--table",
            "Localizable",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let usages: Value = serde_json::from_slice(&usages_output).unwrap();
    let usage_records = usages["records"].as_array().unwrap();
    assert_eq!(usage_records.len(), 2);

    let auth_record = usage_records
        .iter()
        .find(|record| {
            record["record"]["catalog_file"].as_str() == Some("Packages/Auth/Localizable.xcstrings")
        })
        .expect("auth catalog should be present");
    assert_eq!(auth_record["usages"].as_array().unwrap().len(), 1);
    assert_eq!(
        auth_record["usages"][0]["owner"]["file"].as_str(),
        Some("Packages/Auth/Sources/Auth/AuthView.swift")
    );

    let profile_record = usage_records
        .iter()
        .find(|record| {
            record["record"]["catalog_file"].as_str()
                == Some("Packages/Profile/Localizable.xcstrings")
        })
        .expect("profile catalog should be present");
    assert!(
        profile_record["usages"].as_array().unwrap().is_empty(),
        "farther duplicate catalog should not claim the AuthView usage"
    );
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
        .stderr(predicate::str::contains("incremental"));

    grapha()
        .args([
            "symbol",
            "search",
            "gamma",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"gamma\""))
        .stdout(predicate::str::contains("\"name\": \"alpha\"").not());
}

#[test]
fn dataflow_command_outputs_json_and_tree() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(
        dir.path().join("main.rs"),
        "pub fn handler() { persist(); }\nfn persist() {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("grapha.toml"),
        r#"
[[classifiers]]
pattern = "persist"
terminal = "persistence"
direction = "read_write"
operation = "UPSERT"
"#,
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
        .args([
            "flow",
            "graph",
            "handler",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"effect\""))
        .stdout(predicate::str::contains("\"kind\": \"read\""))
        .stdout(predicate::str::contains("\"kind\": \"write\""));

    grapha()
        .args([
            "flow",
            "graph",
            "handler",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("summary: symbols="))
        .stdout(predicate::str::contains("[effect:persistence]"))
        .stdout(predicate::str::contains("read ->"));
}

#[test]
fn tree_output_respects_color_modes() {
    let dir = tempfile::tempdir().unwrap();
    let store_dir = dir.path().join(".grapha");
    std::fs::write(
        dir.path().join("main.rs"),
        "pub fn handler() { persist(); }\nfn persist() {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("grapha.toml"),
        r#"
[[classifiers]]
pattern = "persist"
terminal = "persistence"
direction = "read_write"
operation = "UPSERT"
"#,
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

    let plain = grapha()
        .args([
            "flow",
            "graph",
            "handler",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
            "--color",
            "never",
        ])
        .output()
        .unwrap();
    assert!(plain.status.success());
    let plain_stdout = String::from_utf8(plain.stdout).unwrap();
    assert!(!plain_stdout.contains("\x1b["));

    let colored = grapha()
        .args([
            "flow",
            "graph",
            "handler",
            "-p",
            dir.path().to_str().unwrap(),
            "--format",
            "tree",
            "--color",
            "always",
        ])
        .output()
        .unwrap();
    assert!(colored.status.success());
    let colored_stdout = String::from_utf8(colored.stdout).unwrap();
    assert!(colored_stdout.contains("\x1b["));
    assert_eq!(strip_ansi(&colored_stdout), plain_stdout);
}

#[test]
fn json_output_ignores_color_mode() {
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

    let output = grapha()
        .args([
            "symbol",
            "context",
            "default_config",
            "-p",
            dir.path().to_str().unwrap(),
            "--color",
            "always",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("\x1b["));
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["symbol"]["name"], "default_config");
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
            "symbol",
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
fn search_fields_projection_works() {
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

    let output = grapha()
        .args([
            "symbol",
            "search",
            "main",
            "-p",
            dir.path().to_str().unwrap(),
            "--fields",
            "id,signature,role",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).unwrap();
    let first = &parsed[0];
    assert_eq!(first["name"], "main");
    assert_eq!(first["kind"], "function");
    assert_eq!(first["id"], "main.rs::main");
    assert_eq!(first["signature"], "fn main()");
    assert_eq!(first["role"], "entry_point");
    assert!(first.get("file").is_none());
}

#[test]
fn search_context_projection_keeps_relationships() {
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

    let output = grapha()
        .args([
            "symbol",
            "search",
            "main",
            "-p",
            dir.path().to_str().unwrap(),
            "--context",
            "--fields",
            "snippet",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Value = serde_json::from_slice(&output).unwrap();
    let first = &parsed[0];
    assert_eq!(first["name"], "main");
    assert!(first["snippet"].as_str().unwrap().contains("helper"));
    assert_eq!(first["calls"][0], "main.rs::helper");
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
        .args(["repo", "changes", "-p", dir.path().to_str().unwrap()])
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
        .args([
            "symbol",
            "search",
            "Config",
            "-p",
            dir.path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Config"));
}
