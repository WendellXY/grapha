# Grapha MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a CLI tool that parses Rust source files via tree-sitter, extracts symbols and relationships, and outputs a JSON graph optimized for LLM consumption.

**Architecture:** Language-pluggable extractor with trait-based abstraction. A `LanguageExtractor` trait defines the per-language interface; `RustExtractor` is the first implementation. The core graph model (`Node`, `Edge`, `Graph`) is language-agnostic. Files are discovered via the `ignore` crate, extracted independently, then merged into a single graph.

**Tech Stack:** Rust, clap (CLI), tree-sitter + tree-sitter-rust (parsing), serde/serde_json (serialization), ignore (file discovery), anyhow/thiserror (errors)

**Spec:** `docs/superpowers/specs/2026-03-28-grapha-mvp-design.md`

---

## File Map

| File | Responsibility |
|------|---------------|
| `Cargo.toml` | Project metadata and dependencies |
| `src/main.rs` | CLI entry point, clap args, orchestration |
| `src/graph.rs` | `Node`, `Edge`, `Graph`, `NodeKind`, `EdgeKind`, `Visibility`, `Span` types + serde |
| `src/error.rs` | `GraphaError` enum with thiserror |
| `src/extract.rs` | `LanguageExtractor` trait, `ExtractionResult`, extractor registry |
| `src/extract/rust.rs` | `RustExtractor` — tree-sitter-rust CST walking |
| `src/discover.rs` | File discovery using `ignore` crate |
| `src/merge.rs` | Merge per-file `ExtractionResult`s into a single `Graph`, resolve cross-file edges |
| `src/filter.rs` | `--filter` logic: prune nodes by kind, drop orphaned edges |
| `tests/fixtures/simple.rs` | Test fixture: simple Rust file |
| `tests/fixtures/multi/lib.rs` | Test fixture: multi-file project entry |
| `tests/fixtures/multi/utils.rs` | Test fixture: multi-file project utility module |
| `tests/integration.rs` | End-to-end CLI integration tests |

---

### Task 1: Project Scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

- [ ] **Step 1: Initialize Cargo project**

```bash
cd /Users/wendell/Developer/WeNext/grapha
cargo init --name grapha
```

- [ ] **Step 2: Set up Cargo.toml with all dependencies**

Replace the generated `Cargo.toml` with:

```toml
[package]
name = "grapha"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
ignore = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tree-sitter = "0.25"
tree-sitter-rust = "0.23"

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

- [ ] **Step 3: Write a minimal main.rs placeholder**

```rust
fn main() {
    println!("grapha v0.1.0");
}
```

- [ ] **Step 4: Verify it builds**

Run: `cargo build`
Expected: compiles successfully

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs
git commit -m "chore: scaffold project with dependencies"
```

---

### Task 2: Graph Data Model

**Files:**
- Create: `src/graph.rs`
- Modify: `src/main.rs` (add module declaration)

- [ ] **Step 1: Write tests for graph types and serialization**

Create `src/graph.rs` with the types and tests at the bottom:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Field,
    Variant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Calls,
    Uses,
    Implements,
    Contains,
    TypeRef,
    Inherits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Crate,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: [usize; 2],
    pub end: [usize; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub file: PathBuf,
    pub span: Span,
    pub visibility: Visibility,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Graph {
    pub version: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn new() -> Self {
        Self {
            version: "0.1.0".to_string(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&NodeKind::Function).unwrap();
        assert_eq!(json, "\"function\"");

        let json = serde_json::to_string(&NodeKind::Struct).unwrap();
        assert_eq!(json, "\"struct\"");
    }

    #[test]
    fn edge_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&EdgeKind::TypeRef).unwrap();
        assert_eq!(json, "\"type_ref\"");
    }

    #[test]
    fn visibility_serializes_as_snake_case() {
        let json = serde_json::to_string(&Visibility::Public).unwrap();
        assert_eq!(json, "\"public\"");
    }

    #[test]
    fn span_serializes_as_arrays() {
        let span = Span {
            start: [10, 0],
            end: [15, 1],
        };
        let json = serde_json::to_string(&span).unwrap();
        assert_eq!(json, r#"{"start":[10,0],"end":[15,1]}"#);
    }

    #[test]
    fn graph_serializes_with_version() {
        let graph = Graph::new();
        let json = serde_json::to_string_pretty(&graph).unwrap();
        assert!(json.contains("\"version\": \"0.1.0\""));
        assert!(json.contains("\"nodes\": []"));
        assert!(json.contains("\"edges\": []"));
    }

    #[test]
    fn full_graph_round_trips() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "src/main.rs::main".to_string(),
                kind: NodeKind::Function,
                name: "main".to_string(),
                file: PathBuf::from("src/main.rs"),
                span: Span {
                    start: [0, 0],
                    end: [3, 1],
                },
                visibility: Visibility::Private,
                metadata: HashMap::new(),
            }],
            edges: vec![],
        };
        let json = serde_json::to_string(&graph).unwrap();
        let deserialized: Graph = serde_json::from_str(&json).unwrap();
        assert_eq!(graph, deserialized);
    }
}
```

- [ ] **Step 2: Add module declaration to main.rs**

```rust
mod graph;

fn main() {
    println!("grapha v0.1.0");
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib graph`
Expected: all 6 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/graph.rs src/main.rs
git commit -m "feat: add graph data model with serialization"
```

---

### Task 3: Error Types

**Files:**
- Create: `src/error.rs`
- Modify: `src/main.rs` (add module declaration)

- [ ] **Step 1: Create error types**

Create `src/error.rs`:

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum GraphaError {
    #[error("failed to parse {path}: {reason}")]
    Parse { path: PathBuf, reason: String },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("unsupported language for file: {path}")]
    UnsupportedLanguage { path: PathBuf },
}
```

- [ ] **Step 2: Add module declaration to main.rs**

```rust
mod error;
mod graph;

fn main() {
    println!("grapha v0.1.0");
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: compiles successfully

- [ ] **Step 4: Commit**

```bash
git add src/error.rs src/main.rs
git commit -m "feat: add typed error types with thiserror"
```

---

### Task 4: Extractor Trait

**Files:**
- Create: `src/extract.rs`
- Modify: `src/main.rs` (add module declaration)

- [ ] **Step 1: Create the trait and ExtractionResult**

Create `src/extract.rs`:

```rust
use std::path::Path;

use crate::graph::{Edge, Node};

pub mod rust;

#[derive(Debug, Clone)]
pub struct ExtractionResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl ExtractionResult {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }
}

pub trait LanguageExtractor {
    fn language(&self) -> &str;
    fn file_extensions(&self) -> &[&str];
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult>;
}
```

- [ ] **Step 2: Create a placeholder for RustExtractor**

Create `src/extract/rust.rs`:

```rust
use std::path::Path;

use super::{ExtractionResult, LanguageExtractor};

pub struct RustExtractor;

impl LanguageExtractor for RustExtractor {
    fn language(&self) -> &str {
        "rust"
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn extract(&self, _source: &[u8], _file_path: &Path) -> anyhow::Result<ExtractionResult> {
        Ok(ExtractionResult::new())
    }
}
```

- [ ] **Step 3: Add module declaration to main.rs**

```rust
mod error;
mod extract;
mod graph;

fn main() {
    println!("grapha v0.1.0");
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: compiles successfully

- [ ] **Step 5: Commit**

```bash
git add src/extract.rs src/extract/rust.rs src/main.rs
git commit -m "feat: add LanguageExtractor trait and RustExtractor stub"
```

---

### Task 5: RustExtractor — Symbol Extraction

**Files:**
- Modify: `src/extract/rust.rs`

This task implements extraction of all symbol types: functions, structs, enums, traits, impls, modules, fields, and variants. Also extracts visibility and metadata (`async`, `unsafe`). Generates `Contains` edges for structural nesting.

- [ ] **Step 1: Write tests for symbol extraction**

Add to the bottom of `src/extract/rust.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeKind, NodeKind, Visibility};

    fn extract(source: &str) -> ExtractionResult {
        let extractor = RustExtractor;
        extractor
            .extract(source.as_bytes(), Path::new("test.rs"))
            .unwrap()
    }

    fn find_node<'a>(result: &'a ExtractionResult, name: &str) -> &'a crate::graph::Node {
        result
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("node '{}' not found", name))
    }

    fn has_edge(result: &ExtractionResult, source: &str, target: &str, kind: EdgeKind) -> bool {
        result
            .edges
            .iter()
            .any(|e| e.source == source && e.target == target && e.kind == kind)
    }

    #[test]
    fn extracts_function() {
        let result = extract("pub fn greet(name: &str) -> String { format!(\"hi {}\", name) }");
        let node = find_node(&result, "greet");
        assert_eq!(node.kind, NodeKind::Function);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_async_unsafe_metadata() {
        let result = extract("pub async fn fetch() {} unsafe fn danger() {}");
        let fetch = find_node(&result, "fetch");
        assert_eq!(fetch.metadata.get("async").map(|s| s.as_str()), Some("true"));
        let danger = find_node(&result, "danger");
        assert_eq!(danger.metadata.get("unsafe").map(|s| s.as_str()), Some("true"));
    }

    #[test]
    fn extracts_struct_with_fields() {
        let result = extract(
            r#"
            pub struct Config {
                pub debug: bool,
                name: String,
            }
            "#,
        );
        let config = find_node(&result, "Config");
        assert_eq!(config.kind, NodeKind::Struct);
        assert_eq!(config.visibility, Visibility::Public);

        let debug = find_node(&result, "debug");
        assert_eq!(debug.kind, NodeKind::Field);
        assert_eq!(debug.visibility, Visibility::Public);

        let name = find_node(&result, "name");
        assert_eq!(name.kind, NodeKind::Field);
        assert_eq!(name.visibility, Visibility::Private);

        assert!(has_edge(&result, &config.id, &debug.id, EdgeKind::Contains));
        assert!(has_edge(&result, &config.id, &name.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_enum_with_variants() {
        let result = extract(
            r#"
            pub enum Color {
                Red,
                Green,
                Blue,
            }
            "#,
        );
        let color = find_node(&result, "Color");
        assert_eq!(color.kind, NodeKind::Enum);

        let red = find_node(&result, "Red");
        assert_eq!(red.kind, NodeKind::Variant);

        assert!(has_edge(&result, &color.id, &red.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_trait() {
        let result = extract(
            r#"
            pub trait Drawable {
                fn draw(&self);
            }
            "#,
        );
        let drawable = find_node(&result, "Drawable");
        assert_eq!(drawable.kind, NodeKind::Trait);
        assert_eq!(drawable.visibility, Visibility::Public);

        let draw = find_node(&result, "draw");
        assert_eq!(draw.kind, NodeKind::Function);

        assert!(has_edge(
            &result,
            &drawable.id,
            &draw.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn extracts_impl_block() {
        let result = extract(
            r#"
            struct Foo;
            impl Foo {
                pub fn new() -> Self { Foo }
            }
            "#,
        );
        let impl_node = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Impl)
            .expect("impl node not found");
        assert_eq!(impl_node.name, "Foo");

        let new_fn = find_node(&result, "new");
        assert!(has_edge(
            &result,
            &impl_node.id,
            &new_fn.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn extracts_module() {
        let result = extract(
            r#"
            pub mod utils {
                pub fn helper() {}
            }
            "#,
        );
        let utils = find_node(&result, "utils");
        assert_eq!(utils.kind, NodeKind::Module);
        assert_eq!(utils.visibility, Visibility::Public);

        let helper = find_node(&result, "helper");
        assert!(has_edge(&result, &utils.id, &helper.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_pub_crate_visibility() {
        let result = extract("pub(crate) fn internal() {}");
        let node = find_node(&result, "internal");
        assert_eq!(node.visibility, Visibility::Crate);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib extract::rust`
Expected: FAIL — the extractor returns empty results

- [ ] **Step 3: Implement the full RustExtractor**

Replace the entire `src/extract/rust.rs` implementation (keep the tests) with:

```rust
use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Parser;

use super::{ExtractionResult, LanguageExtractor};
use crate::graph::{Edge, EdgeKind, Node, NodeKind, Span, Visibility};

pub struct RustExtractor;

impl LanguageExtractor for RustExtractor {
    fn language(&self) -> &str {
        "rust"
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| anyhow::anyhow!("failed to load Rust grammar: {e}"))?;

        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse"))?;

        let mut result = ExtractionResult::new();
        let file_str = file_path.to_string_lossy().to_string();

        walk_node(
            tree.root_node(),
            source,
            file_path,
            &file_str,
            &[], // parent module path
            None, // parent node id
            &mut result,
        );

        Ok(result)
    }
}

fn walk_node(
    node: tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "function_item" | "function_signature_item" => {
                if let Some(sym) = extract_function(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    result.nodes.push(sym);
                    // walk inside function body for nested items
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_node(body, source, file_path, file_str, module_path, Some(&id), result);
                    }
                }
            }
            "struct_item" => {
                if let Some(sym) = extract_struct(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    result.nodes.push(sym);
                    extract_fields(&child, source, file_path, file_str, module_path, &id, result);
                }
            }
            "enum_item" => {
                if let Some(sym) = extract_enum(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    result.nodes.push(sym);
                    extract_variants(&child, source, file_path, file_str, module_path, &id, result);
                }
            }
            "trait_item" => {
                if let Some(sym) = extract_trait(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    result.nodes.push(sym);
                    // walk trait body for method signatures
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_node(body, source, file_path, file_str, module_path, Some(&id), result);
                    }
                }
            }
            "impl_item" => {
                if let Some(sym) = extract_impl(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    result.nodes.push(sym);
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_node(body, source, file_path, file_str, module_path, Some(&id), result);
                    }
                }
            }
            "mod_item" => {
                if let Some(sym) = extract_module(&child, source, file_path, file_str, module_path) {
                    let name = sym.name.clone();
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    result.nodes.push(sym);
                    if let Some(body) = child.child_by_field_name("body") {
                        let mut child_path = module_path.to_vec();
                        child_path.push(name);
                        walk_node(body, source, file_path, file_str, &child_path, Some(&id), result);
                    }
                }
            }
            _ => {
                // recurse into unknown nodes to find nested items
                walk_node(child, source, file_path, file_str, module_path, parent_id, result);
            }
        }
    }
}

fn make_id(file_str: &str, module_path: &[String], name: &str) -> String {
    if module_path.is_empty() {
        format!("{file_str}::{name}")
    } else {
        format!("{}::{}::{}", file_str, module_path.join("::"), name)
    }
}

fn make_span(node: &tree_sitter::Node) -> Span {
    let start = node.start_position();
    let end = node.end_position();
    Span {
        start: [start.row, start.column],
        end: [end.row, end.column],
    }
}

fn extract_visibility(node: &tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let text = child.utf8_text(source).unwrap_or("");
            return if text.contains("crate") {
                Visibility::Crate
            } else {
                Visibility::Public
            };
        }
    }
    Visibility::Private
}

fn node_text<'a>(node: &tree_sitter::Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn extract_function(
    node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
) -> Option<Node> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    let mut metadata = HashMap::new();

    // check for async/unsafe by looking at children before the name
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "async" => { metadata.insert("async".to_string(), "true".to_string()); }
            "unsafe" => { metadata.insert("unsafe".to_string(), "true".to_string()); }
            _ => {}
        }
    }

    Some(Node {
        id: make_id(file_str, module_path, &name),
        kind: NodeKind::Function,
        name,
        file: file_path.to_path_buf(),
        span: make_span(node),
        visibility: extract_visibility(node, source),
        metadata,
    })
}

fn extract_struct(
    node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
) -> Option<Node> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    Some(Node {
        id: make_id(file_str, module_path, &name),
        kind: NodeKind::Struct,
        name,
        file: file_path.to_path_buf(),
        span: make_span(node),
        visibility: extract_visibility(node, source),
        metadata: HashMap::new(),
    })
}

fn extract_enum(
    node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
) -> Option<Node> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    Some(Node {
        id: make_id(file_str, module_path, &name),
        kind: NodeKind::Enum,
        name,
        file: file_path.to_path_buf(),
        span: make_span(node),
        visibility: extract_visibility(node, source),
        metadata: HashMap::new(),
    })
}

fn extract_trait(
    node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
) -> Option<Node> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    Some(Node {
        id: make_id(file_str, module_path, &name),
        kind: NodeKind::Trait,
        name,
        file: file_path.to_path_buf(),
        span: make_span(node),
        visibility: extract_visibility(node, source),
        metadata: HashMap::new(),
    })
}

fn extract_impl(
    node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
) -> Option<Node> {
    let type_node = node.child_by_field_name("type")?;
    let name = node_text(&type_node, source).to_string();
    let id = make_id(file_str, module_path, &format!("impl_{name}"));
    Some(Node {
        id,
        kind: NodeKind::Impl,
        name,
        file: file_path.to_path_buf(),
        span: make_span(node),
        visibility: Visibility::Private,
        metadata: HashMap::new(),
    })
}

fn extract_module(
    node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
) -> Option<Node> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(&name_node, source).to_string();
    Some(Node {
        id: make_id(file_str, module_path, &name),
        kind: NodeKind::Module,
        name,
        file: file_path.to_path_buf(),
        span: make_span(node),
        visibility: extract_visibility(node, source),
        metadata: HashMap::new(),
    })
}

fn extract_fields(
    struct_node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
    parent_id: &str,
    result: &mut ExtractionResult,
) {
    let Some(body) = struct_node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "field_declaration" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source).to_string();
                let id = make_id(file_str, module_path, &format!("{}.{name}", parent_id.rsplit("::").next().unwrap_or("")));
                let node = Node {
                    id: id.clone(),
                    kind: NodeKind::Field,
                    name,
                    file: file_path.to_path_buf(),
                    span: make_span(&child),
                    visibility: extract_visibility(&child, source),
                    metadata: HashMap::new(),
                };
                result.edges.push(Edge {
                    source: parent_id.to_string(),
                    target: id,
                    kind: EdgeKind::Contains,
                });
                result.nodes.push(node);
            }
        }
    }
}

fn extract_variants(
    enum_node: &tree_sitter::Node,
    source: &[u8],
    file_path: &Path,
    file_str: &str,
    module_path: &[String],
    parent_id: &str,
    result: &mut ExtractionResult,
) {
    let Some(body) = enum_node.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "enum_variant" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source).to_string();
                let id = make_id(file_str, module_path, &format!("{}.{name}", parent_id.rsplit("::").next().unwrap_or("")));
                let node = Node {
                    id: id.clone(),
                    kind: NodeKind::Variant,
                    name,
                    file: file_path.to_path_buf(),
                    span: make_span(&child),
                    visibility: Visibility::Public,
                    metadata: HashMap::new(),
                };
                result.edges.push(Edge {
                    source: parent_id.to_string(),
                    target: id,
                    kind: EdgeKind::Contains,
                });
                result.nodes.push(node);
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib extract::rust`
Expected: all 8 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/extract/rust.rs
git commit -m "feat: implement RustExtractor symbol extraction with Contains edges"
```

---

### Task 6: RustExtractor — Relationship Extraction (Calls, Uses, Implements, TypeRef, Inherits)

**Files:**
- Modify: `src/extract/rust.rs`

- [ ] **Step 1: Write tests for relationship extraction**

Add these tests to the existing `mod tests` block in `src/extract/rust.rs`:

```rust
    #[test]
    fn extracts_calls_edges() {
        let result = extract(
            r#"
            fn helper() {}
            fn main() {
                helper();
            }
            "#,
        );
        assert!(has_edge(
            &result,
            "test.rs::main",
            "test.rs::helper",
            EdgeKind::Calls,
        ));
    }

    #[test]
    fn extracts_use_edges() {
        let result = extract("use std::collections::HashMap;");
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Uses));
    }

    #[test]
    fn extracts_implements_edge() {
        let result = extract(
            r#"
            trait Drawable { fn draw(&self); }
            struct Circle;
            impl Drawable for Circle {
                fn draw(&self) {}
            }
            "#,
        );
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Implements));
    }

    #[test]
    fn extracts_type_ref_edges() {
        let result = extract(
            r#"
            struct Config { debug: bool }
            fn make_config() -> Config {
                Config { debug: true }
            }
            "#,
        );
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::TypeRef));
    }

    #[test]
    fn extracts_inherits_edge_for_supertraits() {
        let result = extract(
            r#"
            trait Base {}
            trait Child: Base {}
            "#,
        );
        assert!(has_edge(
            &result,
            "test.rs::Child",
            "test.rs::Base",
            EdgeKind::Inherits,
        ));
    }
```

- [ ] **Step 2: Run tests to verify the new ones fail**

Run: `cargo test --lib extract::rust`
Expected: the 5 new tests FAIL, existing 8 still pass

- [ ] **Step 3: Implement relationship extraction**

Add the following functions to `src/extract/rust.rs` and integrate them into `walk_node`:

Add a new function to collect call expressions from a function body:

```rust
fn extract_calls(
    node: &tree_sitter::Node,
    source: &[u8],
    caller_id: &str,
    file_str: &str,
    module_path: &[String],
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            if let Some(func_node) = child.child_by_field_name("function") {
                let func_text = node_text(&func_node, source);
                // Extract the base function name (last segment of path)
                let func_name = func_text.rsplit("::").next().unwrap_or(func_text);
                // Skip macro invocations (e.g., println!, format!)
                if !func_name.ends_with('!') && !func_name.is_empty() {
                    let target_id = make_id(file_str, module_path, func_name);
                    result.edges.push(Edge {
                        source: caller_id.to_string(),
                        target: target_id,
                        kind: EdgeKind::Calls,
                    });
                }
            }
        }
        // recurse into children to find nested calls
        extract_calls(&child, source, caller_id, file_str, module_path, result);
    }
}
```

Add a function to extract `use` declarations:

```rust
fn extract_use(
    node: &tree_sitter::Node,
    source: &[u8],
    file_str: &str,
    result: &mut ExtractionResult,
) {
    let text = node_text(node, source).to_string();
    result.edges.push(Edge {
        source: file_str.to_string(),
        target: text,
        kind: EdgeKind::Uses,
    });
}
```

Add to `extract_impl` to detect trait impls and emit `Implements` edges:

```rust
fn extract_impl_relationships(
    node: &tree_sitter::Node,
    source: &[u8],
    file_str: &str,
    module_path: &[String],
    result: &mut ExtractionResult,
) {
    // Check if this is `impl Trait for Type`
    if let Some(trait_node) = node.child_by_field_name("trait") {
        if let Some(type_node) = node.child_by_field_name("type") {
            let trait_name = node_text(&trait_node, source);
            let type_name = node_text(&type_node, source);
            let type_id = make_id(file_str, module_path, type_name);
            let trait_id = make_id(file_str, module_path, trait_name);
            result.edges.push(Edge {
                source: type_id,
                target: trait_id,
                kind: EdgeKind::Implements,
            });
        }
    }
}
```

Add function to extract return type references:

```rust
fn extract_return_type_ref(
    node: &tree_sitter::Node,
    source: &[u8],
    func_id: &str,
    file_str: &str,
    module_path: &[String],
    result: &mut ExtractionResult,
) {
    if let Some(ret_type) = node.child_by_field_name("return_type") {
        let type_text = node_text(&ret_type, source);
        // Strip leading `-> ` if present
        let type_name = type_text.trim().trim_start_matches("->").trim();
        // Only create TypeRef for non-primitive, non-Self types
        if !is_primitive(type_name) && type_name != "Self" {
            let target_id = make_id(file_str, module_path, type_name);
            result.edges.push(Edge {
                source: func_id.to_string(),
                target: target_id,
                kind: EdgeKind::TypeRef,
            });
        }
    }
}

fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "bool" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
            | "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
            | "f32" | "f64" | "char" | "str" | "()"
    )
}
```

Add function to extract supertrait bounds (Inherits edges):

```rust
fn extract_supertraits(
    node: &tree_sitter::Node,
    source: &[u8],
    trait_id: &str,
    file_str: &str,
    module_path: &[String],
    result: &mut ExtractionResult,
) {
    if let Some(bounds) = node.child_by_field_name("bounds") {
        let mut cursor = bounds.walk();
        for child in bounds.named_children(&mut cursor) {
            if child.kind() == "type_identifier" || child.kind() == "scoped_type_identifier" {
                let name = node_text(&child, source);
                let target_id = make_id(file_str, module_path, name);
                result.edges.push(Edge {
                    source: trait_id.to_string(),
                    target: target_id,
                    kind: EdgeKind::Inherits,
                });
            }
        }
    }
}
```

Then update `walk_node` to call these new functions:

- In the `"function_item"` arm, after creating the node, call `extract_calls` on the body and `extract_return_type_ref` on the node.
- Add a `"use_declaration"` arm that calls `extract_use`.
- In the `"impl_item"` arm, call `extract_impl_relationships`.
- In the `"trait_item"` arm, call `extract_supertraits`.

Update the `"function_item" | "function_signature_item"` arm in `walk_node`:

```rust
            "function_item" | "function_signature_item" => {
                if let Some(sym) = extract_function(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    extract_return_type_ref(&child, source, &id, file_str, module_path, result);
                    result.nodes.push(sym);
                    if let Some(body) = child.child_by_field_name("body") {
                        extract_calls(&body, source, &id, file_str, module_path, result);
                        walk_node(body, source, file_path, file_str, module_path, Some(&id), result);
                    }
                }
            }
```

Add `"use_declaration"` arm to `walk_node`:

```rust
            "use_declaration" => {
                extract_use(&child, source, file_str, result);
            }
```

Update the `"impl_item"` arm to call `extract_impl_relationships`:

```rust
            "impl_item" => {
                if let Some(sym) = extract_impl(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    extract_impl_relationships(&child, source, file_str, module_path, result);
                    result.nodes.push(sym);
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_node(body, source, file_path, file_str, module_path, Some(&id), result);
                    }
                }
            }
```

Update the `"trait_item"` arm to call `extract_supertraits`:

```rust
            "trait_item" => {
                if let Some(sym) = extract_trait(&child, source, file_path, file_str, module_path) {
                    let id = sym.id.clone();
                    if let Some(pid) = parent_id {
                        result.edges.push(Edge {
                            source: pid.to_string(),
                            target: id.clone(),
                            kind: EdgeKind::Contains,
                        });
                    }
                    extract_supertraits(&child, source, &id, file_str, module_path, result);
                    result.nodes.push(sym);
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_node(body, source, file_path, file_str, module_path, Some(&id), result);
                    }
                }
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib extract::rust`
Expected: all 13 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/extract/rust.rs
git commit -m "feat: add relationship extraction (Calls, Uses, Implements, TypeRef, Inherits)"
```

---

### Task 7: File Discovery

**Files:**
- Create: `src/discover.rs`
- Modify: `src/main.rs` (add module declaration)

- [ ] **Step 1: Write tests**

Create `src/discover.rs`:

```rust
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Discover source files under `path` matching the given extensions.
/// Respects .gitignore and skips hidden directories.
pub fn discover_files(path: &Path, extensions: &[&str]) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut files = Vec::new();
    let walker = WalkBuilder::new(path).hidden(true).git_ignore(true).build();

    for entry in walker {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() {
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if extensions.iter().any(|e| *e == ext) {
                    files.push(p.to_path_buf());
                }
            }
        }
    }

    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, "fn main() {}").unwrap();

        let result = discover_files(&file, &["rs"]).unwrap();
        assert_eq!(result, vec![file]);
    }

    #[test]
    fn discovers_files_in_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "").unwrap();
        fs::write(dir.path().join("b.rs"), "").unwrap();
        fs::write(dir.path().join("c.txt"), "").unwrap();

        let result = discover_files(dir.path(), &["rs"]).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|p| p.extension().unwrap() == "rs"));
    }

    #[test]
    fn skips_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        let hidden = dir.path().join(".hidden");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("secret.rs"), "").unwrap();
        fs::write(dir.path().join("visible.rs"), "").unwrap();

        let result = discover_files(dir.path(), &["rs"]).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn empty_directory_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let result = discover_files(dir.path(), &["rs"]).unwrap();
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 2: Add tempfile dev-dependency to Cargo.toml**

Add to `[dev-dependencies]`:

```toml
tempfile = "3"
```

- [ ] **Step 3: Add module declaration to main.rs**

```rust
mod discover;
mod error;
mod extract;
mod graph;

fn main() {
    println!("grapha v0.1.0");
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib discover`
Expected: all 4 tests pass

- [ ] **Step 5: Commit**

```bash
git add src/discover.rs src/main.rs Cargo.toml Cargo.lock
git commit -m "feat: add gitignore-aware file discovery"
```

---

### Task 8: Graph Merging

**Files:**
- Create: `src/merge.rs`
- Modify: `src/main.rs` (add module declaration)

- [ ] **Step 1: Write tests and implementation**

Create `src/merge.rs`:

```rust
use std::collections::HashSet;

use crate::extract::ExtractionResult;
use crate::graph::Graph;

/// Merge multiple `ExtractionResult`s into a single `Graph`.
/// Drops edges whose target does not match any node ID in the graph.
pub fn merge(results: Vec<ExtractionResult>) -> Graph {
    let mut graph = Graph::new();

    for r in &results {
        graph.nodes.extend(r.nodes.iter().cloned());
    }

    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();

    for r in results {
        for edge in r.edges {
            // Keep edges where the target is a known node or is a use-path (external reference)
            if node_ids.contains(edge.target.as_str())
                || edge.kind == crate::graph::EdgeKind::Uses
            {
                graph.edges.push(edge);
            }
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, name: &str, kind: NodeKind) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            file: PathBuf::from("test.rs"),
            span: Span {
                start: [0, 0],
                end: [0, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn merges_nodes_from_multiple_results() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::Foo", "Foo", NodeKind::Struct)],
            edges: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::Bar", "Bar", NodeKind::Struct)],
            edges: vec![],
        };
        let graph = merge(vec![r1, r2]);
        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn drops_edges_with_unresolved_targets() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "nonexistent::foo".to_string(),
                kind: EdgeKind::Calls,
            }],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 0);
    }

    #[test]
    fn keeps_edges_with_resolved_targets() {
        let r1 = ExtractionResult {
            nodes: vec![
                make_node("a::main", "main", NodeKind::Function),
                make_node("a::helper", "helper", NodeKind::Function),
            ],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "a::helper".to_string(),
                kind: EdgeKind::Calls,
            }],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn resolves_cross_file_edges() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "b::helper".to_string(),
                kind: EdgeKind::Calls,
            }],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::helper", "helper", NodeKind::Function)],
            edges: vec![],
        };
        let graph = merge(vec![r1, r2]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn keeps_uses_edges_even_if_target_unresolved() {
        let r1 = ExtractionResult {
            nodes: vec![],
            edges: vec![Edge {
                source: "a.rs".to_string(),
                target: "use std::collections::HashMap;".to_string(),
                kind: EdgeKind::Uses,
            }],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 1);
    }
}
```

- [ ] **Step 2: Add module declaration to main.rs**

```rust
mod discover;
mod error;
mod extract;
mod graph;
mod merge;

fn main() {
    println!("grapha v0.1.0");
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib merge`
Expected: all 5 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/merge.rs src/main.rs
git commit -m "feat: add graph merging with cross-file edge resolution"
```

---

### Task 9: Output Filtering

**Files:**
- Create: `src/filter.rs`
- Modify: `src/main.rs` (add module declaration)

- [ ] **Step 1: Write tests and implementation**

Create `src/filter.rs`:

```rust
use std::collections::HashSet;

use crate::graph::{Graph, NodeKind};

/// Parse a comma-separated filter string into a set of `NodeKind`s.
pub fn parse_filter(filter: &str) -> anyhow::Result<HashSet<NodeKind>> {
    let mut kinds = HashSet::new();
    for part in filter.split(',') {
        let kind = match part.trim() {
            "fn" | "function" => NodeKind::Function,
            "struct" => NodeKind::Struct,
            "enum" => NodeKind::Enum,
            "trait" => NodeKind::Trait,
            "impl" => NodeKind::Impl,
            "mod" | "module" => NodeKind::Module,
            "field" => NodeKind::Field,
            "variant" => NodeKind::Variant,
            other => anyhow::bail!("unknown node kind: '{other}'"),
        };
        kinds.insert(kind);
    }
    Ok(kinds)
}

/// Filter a graph to only include nodes of the given kinds.
/// Prunes edges that reference removed nodes.
pub fn filter_graph(graph: Graph, kinds: &HashSet<NodeKind>) -> Graph {
    let kept_ids: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| kinds.contains(&n.kind))
        .map(|n| n.id.as_str())
        .collect();

    let nodes = graph
        .nodes
        .into_iter()
        .filter(|n| kinds.contains(&n.kind))
        .collect();

    let edges = graph
        .edges
        .into_iter()
        .filter(|e| kept_ids.contains(e.source.as_str()) && kept_ids.contains(e.target.as_str()))
        .collect();

    Graph {
        version: graph.version,
        nodes,
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, kind: NodeKind) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: id.to_string(),
            file: PathBuf::from("test.rs"),
            span: Span {
                start: [0, 0],
                end: [0, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn parse_filter_parses_valid_kinds() {
        let kinds = parse_filter("fn,struct").unwrap();
        assert!(kinds.contains(&NodeKind::Function));
        assert!(kinds.contains(&NodeKind::Struct));
        assert_eq!(kinds.len(), 2);
    }

    #[test]
    fn parse_filter_rejects_unknown_kind() {
        let result = parse_filter("fn,bogus");
        assert!(result.is_err());
    }

    #[test]
    fn filter_keeps_only_matching_nodes() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a", NodeKind::Function),
                make_node("b", NodeKind::Struct),
                make_node("c", NodeKind::Enum),
            ],
            edges: vec![],
        };
        let mut kinds = HashSet::new();
        kinds.insert(NodeKind::Function);
        let filtered = filter_graph(graph, &kinds);
        assert_eq!(filtered.nodes.len(), 1);
        assert_eq!(filtered.nodes[0].id, "a");
    }

    #[test]
    fn filter_prunes_orphaned_edges() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a", NodeKind::Function),
                make_node("b", NodeKind::Struct),
            ],
            edges: vec![
                Edge {
                    source: "a".to_string(),
                    target: "b".to_string(),
                    kind: EdgeKind::TypeRef,
                },
                Edge {
                    source: "a".to_string(),
                    target: "a".to_string(),
                    kind: EdgeKind::Calls,
                },
            ],
        };
        let mut kinds = HashSet::new();
        kinds.insert(NodeKind::Function);
        let filtered = filter_graph(graph, &kinds);
        assert_eq!(filtered.edges.len(), 1);
        assert_eq!(filtered.edges[0].kind, EdgeKind::Calls);
    }
}
```

- [ ] **Step 2: Add module declaration to main.rs**

```rust
mod discover;
mod error;
mod extract;
mod filter;
mod graph;
mod merge;

fn main() {
    println!("grapha v0.1.0");
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib filter`
Expected: all 4 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/filter.rs src/main.rs
git commit -m "feat: add output filtering by node kind"
```

---

### Task 10: CLI Wiring

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Write the full CLI implementation**

Replace `src/main.rs` with:

```rust
mod discover;
mod error;
mod extract;
mod filter;
mod graph;
mod merge;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

use extract::rust::RustExtractor;
use extract::LanguageExtractor;

#[derive(Parser)]
#[command(name = "grapha", version, about = "Structural code graph for LLM consumption")]
struct Cli {
    /// File or directory to analyze
    path: PathBuf,

    /// Output file (default: stdout)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Filter node kinds (comma-separated: fn,struct,enum,trait,impl,mod,field,variant)
    #[arg(long)]
    filter: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let extractor = RustExtractor;
    let files = discover::discover_files(&cli.path, extractor.file_extensions())
        .context("failed to discover files")?;

    let mut results = Vec::new();
    for file in &files {
        let source = std::fs::read(file).with_context(|| format!("failed to read {}", file.display()))?;

        // Make path relative to the input path for cleaner IDs
        let relative = if cli.path.is_dir() {
            file.strip_prefix(&cli.path).unwrap_or(file)
        } else {
            file.file_name().map(|n| n.as_ref()).unwrap_or(file.as_path())
        };

        match extractor.extract(&source, relative) {
            Ok(result) => results.push(result),
            Err(e) => eprintln!("warning: skipping {}: {e}", file.display()),
        }
    }

    let mut graph = merge::merge(results);

    if let Some(ref filter_str) = cli.filter {
        let kinds = filter::parse_filter(filter_str)?;
        graph = filter::filter_graph(graph, &kinds);
    }

    let json = match &cli.output {
        Some(_) => serde_json::to_string(&graph)?,
        None => serde_json::to_string_pretty(&graph)?,
    };

    match cli.output {
        Some(path) => {
            std::fs::write(&path, &json)
                .with_context(|| format!("failed to write {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => println!("{json}"),
    }

    Ok(())
}
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build`
Expected: compiles successfully

- [ ] **Step 3: Smoke test with `--help`**

Run: `cargo run -- --help`
Expected: prints help text with `path`, `--output`, `--filter` options

- [ ] **Step 4: Smoke test on Grapha's own source**

Run: `cargo run -- src/`
Expected: JSON output with nodes for the grapha source files

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire up CLI with clap, orchestrate full pipeline"
```

---

### Task 11: Integration Tests

**Files:**
- Create: `tests/fixtures/simple.rs`
- Create: `tests/fixtures/multi/lib.rs`
- Create: `tests/fixtures/multi/utils.rs`
- Create: `tests/integration.rs`

- [ ] **Step 1: Create test fixtures**

Create `tests/fixtures/simple.rs`:

```rust
pub struct Config {
    pub debug: bool,
    pub name: String,
}

pub trait Configurable {
    fn configure(&self, config: &Config);
}

pub fn default_config() -> Config {
    Config {
        debug: false,
        name: String::new(),
    }
}
```

Create `tests/fixtures/multi/lib.rs`:

```rust
mod utils;

pub fn run() {
    let val = utils::helper();
    println!("{val}");
}
```

Create `tests/fixtures/multi/utils.rs`:

```rust
pub fn helper() -> i32 {
    42
}
```

- [ ] **Step 2: Write integration tests**

Create `tests/integration.rs`:

```rust
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
```

- [ ] **Step 3: Run integration tests**

Run: `cargo test --test integration`
Expected: all 7 tests pass

- [ ] **Step 4: Run the full test suite**

Run: `cargo test`
Expected: all unit + integration tests pass

- [ ] **Step 5: Commit**

```bash
git add tests/
git commit -m "test: add integration tests with fixtures"
```

---

### Task 12: Final Verification

- [ ] **Step 1: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: no warnings

- [ ] **Step 2: Run fmt check**

Run: `cargo fmt -- --check`
Expected: no formatting issues

- [ ] **Step 3: Fix any clippy/fmt issues**

If any warnings or formatting issues, fix them.

- [ ] **Step 4: Run grapha on its own source code**

Run: `cargo run -- src/`
Expected: meaningful JSON graph of grapha's own codebase

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore: fix clippy warnings and formatting"
```
