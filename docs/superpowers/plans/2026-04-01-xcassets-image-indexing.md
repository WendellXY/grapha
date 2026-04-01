# Image Asset Indexing (xcassets) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Discover `.xcassets` image sets, index them, detect `Image()`/`UIImage()` references in Swift source, and enable usage queries + dead asset detection.

**Architecture:** Follows the localization pattern exactly — discovery builds a snapshot in `.grapha/assets.json`, tree-sitter enrichment adds `asset.*` metadata to graph nodes, CLI commands query the snapshot cross-referenced with the graph.

**Tech Stack:** Rust, `xcassets` crate (0.1.0), tree-sitter-swift, serde_json

---

## Task 1: Add xcassets Dependency and Asset Snapshot Module

**Files:**
- Modify: `grapha/Cargo.toml` (add xcassets dependency)
- Create: `grapha/src/assets.rs` (discovery, snapshot, index)

- [ ] **Step 1: Add xcassets dependency**

In `grapha/Cargo.toml`, add under `[dependencies]`:
```toml
xcassets = "0.1"
```

- [ ] **Step 2: Create assets.rs with types**

```rust
// grapha/src/assets.rs
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

const ASSETS_SNAPSHOT_VERSION: &str = "1";
const ASSETS_SNAPSHOT_FILE: &str = "assets.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    pub name: String,
    pub group_path: String,
    pub catalog: String,
    pub catalog_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_intent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AssetSnapshot {
    version: String,
    records: Vec<AssetRecord>,
}

impl AssetSnapshot {
    fn new(mut records: Vec<AssetRecord>) -> Self {
        records.sort_by(|a, b| a.name.cmp(&b.name).then(a.catalog.cmp(&b.catalog)));
        Self {
            version: ASSETS_SNAPSHOT_VERSION.to_string(),
            records,
        }
    }

    fn record_count(&self) -> usize {
        self.records.len()
    }
}

pub struct AssetCatalogIndex {
    records: Vec<AssetRecord>,
    by_name: HashMap<String, Vec<usize>>,
}

impl AssetCatalogIndex {
    pub fn from_records(records: Vec<AssetRecord>) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, record) in records.iter().enumerate() {
            by_name.entry(record.name.clone()).or_default().push(i);
        }
        Self { records, by_name }
    }

    pub fn records_for_name(&self, name: &str) -> Vec<AssetRecord> {
        self.by_name
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|i| self.records.get(*i))
            .cloned()
            .collect()
    }

    pub fn all_records(&self) -> &[AssetRecord] {
        &self.records
    }
}

#[derive(Debug)]
pub struct AssetSnapshotBuildStats {
    pub record_count: usize,
    pub warnings: Vec<AssetSnapshotWarning>,
}

#[derive(Debug)]
pub struct AssetSnapshotWarning {
    pub catalog: String,
    pub reason: String,
}
```

- [ ] **Step 3: Add discovery using xcassets crate**

```rust
// In assets.rs, add:

fn discover_xcassets_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir()
            && path.extension().is_some_and(|ext| ext == "xcassets")
        {
            dirs.push(path.to_path_buf());
        }
    }
    dirs
}

fn build_snapshot(root: &Path) -> anyhow::Result<(AssetSnapshot, Vec<AssetSnapshotWarning>)> {
    if root.is_file() {
        return Ok((AssetSnapshot::new(Vec::new()), Vec::new()));
    }

    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let catalog_dirs = discover_xcassets_dirs(&root);
    let mut records = Vec::new();
    let mut warnings = Vec::new();

    for catalog_path in catalog_dirs {
        let report = match xcassets::parse_catalog(&catalog_path) {
            Ok(r) => r,
            Err(e) => {
                warnings.push(AssetSnapshotWarning {
                    catalog: catalog_path.to_string_lossy().to_string(),
                    reason: e.to_string(),
                });
                continue;
            }
        };

        let catalog_name = catalog_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Assets.xcassets")
            .to_string();
        let catalog_dir = catalog_path
            .parent()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|| ".".to_string());
        let catalog_dir = if catalog_dir.is_empty() {
            ".".to_string()
        } else {
            catalog_dir
        };

        collect_image_sets(
            &report.catalog,
            &catalog_name,
            &catalog_dir,
            "",
            &mut records,
        );

        for diagnostic in &report.diagnostics {
            if diagnostic.severity == xcassets::Severity::Error {
                warnings.push(AssetSnapshotWarning {
                    catalog: catalog_name.clone(),
                    reason: format!("{:?}: {}", diagnostic.code, diagnostic.message),
                });
            }
        }
    }

    Ok((AssetSnapshot::new(records), warnings))
}

fn collect_image_sets(
    catalog: &xcassets::AssetCatalog,
    catalog_name: &str,
    catalog_dir: &str,
    group_prefix: &str,
    records: &mut Vec<AssetRecord>,
) {
    for node in &catalog.children {
        match node {
            xcassets::Node::ImageSet(image_set) => {
                let name = image_set
                    .name
                    .strip_suffix(".imageset")
                    .unwrap_or(&image_set.name)
                    .to_string();
                let template_intent = image_set
                    .contents
                    .properties
                    .as_ref()
                    .and_then(|p| {
                        p.get("template-rendering-intent")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    });
                records.push(AssetRecord {
                    name,
                    group_path: group_prefix.to_string(),
                    catalog: catalog_name.to_string(),
                    catalog_dir: catalog_dir.to_string(),
                    template_intent,
                });
            }
            xcassets::Node::Group(folder) => {
                let child_prefix = if group_prefix.is_empty() {
                    folder.name.clone()
                } else {
                    format!("{}/{}", group_prefix, folder.name)
                };
                // Recurse into the group's children via its inner catalog
                collect_image_sets_from_folder(
                    folder,
                    catalog_name,
                    catalog_dir,
                    &child_prefix,
                    records,
                );
            }
            _ => {} // Skip ColorSet, AppIconSet, etc.
        }
    }
}

fn collect_image_sets_from_folder(
    folder: &xcassets::FolderNode,
    catalog_name: &str,
    catalog_dir: &str,
    group_prefix: &str,
    records: &mut Vec<AssetRecord>,
) {
    for node in &folder.contents.children {
        match node {
            xcassets::Node::ImageSet(image_set) => {
                let name = image_set
                    .name
                    .strip_suffix(".imageset")
                    .unwrap_or(&image_set.name)
                    .to_string();
                let template_intent = image_set
                    .contents
                    .properties
                    .as_ref()
                    .and_then(|p| {
                        p.get("template-rendering-intent")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    });
                records.push(AssetRecord {
                    name,
                    group_path: group_prefix.to_string(),
                    catalog: catalog_name.to_string(),
                    catalog_dir: catalog_dir.to_string(),
                    template_intent,
                });
            }
            xcassets::Node::Group(subfolder) => {
                let child_prefix = format!("{}/{}", group_prefix, subfolder.name);
                collect_image_sets_from_folder(
                    subfolder,
                    catalog_name,
                    catalog_dir,
                    &child_prefix,
                    records,
                );
            }
            _ => {}
        }
    }
}
```

**IMPORTANT:** The `xcassets` crate's API may differ from the above. Check `xcassets::parse_catalog` return type, `AssetCatalog` struct fields, `Node` enum variants, `ImageSetContents` fields, and `FolderNode`/`FolderContents` structure. Adapt the code to match the actual crate API. The intent is: walk the catalog tree recursively, collect all image sets with their group path.

- [ ] **Step 4: Add save/load/public API**

```rust
// In assets.rs, add:

fn snapshot_path(store_dir: &Path) -> PathBuf {
    store_dir.join(ASSETS_SNAPSHOT_FILE)
}

fn save_snapshot(store_dir: &Path, snapshot: &AssetSnapshot) -> anyhow::Result<()> {
    std::fs::create_dir_all(store_dir)
        .with_context(|| format!("failed to create store dir {}", store_dir.display()))?;
    let json = serde_json::to_string_pretty(snapshot)?;
    std::fs::write(snapshot_path(store_dir), json)?;
    Ok(())
}

fn load_snapshot(store_dir: &Path) -> anyhow::Result<AssetSnapshot> {
    let path = snapshot_path(store_dir);
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot: AssetSnapshot = serde_json::from_str(&data)?;
    Ok(snapshot)
}

pub fn build_and_save_snapshot(
    root: &Path,
    store_dir: &Path,
) -> anyhow::Result<AssetSnapshotBuildStats> {
    let (snapshot, warnings) = build_snapshot(root)?;
    let count = snapshot.record_count();
    save_snapshot(store_dir, &snapshot)?;
    Ok(AssetSnapshotBuildStats {
        record_count: count,
        warnings,
    })
}

pub fn load_catalog_index(project_root: &Path) -> anyhow::Result<AssetCatalogIndex> {
    let store_dir = project_root.join(".grapha");
    let snapshot = load_snapshot(&store_dir)?;
    Ok(AssetCatalogIndex::from_records(snapshot.records))
}
```

- [ ] **Step 5: Add `pub mod assets;` to main.rs**

In `grapha/src/main.rs`, add `mod assets;` in the module declarations at the top.

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: All pass (new module has no tests yet but compiles)

- [ ] **Step 7: Commit**

```bash
git add grapha/Cargo.toml grapha/src/assets.rs grapha/src/main.rs
git commit -m "feat(assets): add xcassets discovery and snapshot module"
```

---

## Task 2: Pipeline Integration

**Files:**
- Modify: `grapha/src/main.rs` (add assets_handle to handle_index)

- [ ] **Step 1: Add parallel assets snapshot build**

In `handle_index`, inside the `std::thread::scope`, add an `assets_handle` alongside `localization_handle`:

```rust
let assets_handle = scope.spawn(|| {
    let t = Instant::now();
    let stats = assets::build_and_save_snapshot(&index_root, &store_path)?;
    Ok::<_, anyhow::Error>((t.elapsed(), stats))
});
```

- [ ] **Step 2: Collect and report results**

After the thread scope, destructure the assets result and add progress output:

```rust
let assets = assets_handle.join().expect("assets thread panicked")?;
```

Add to the destructuring pattern and report:
```rust
progress::done_elapsed(
    &format!(
        "saved asset catalog snapshot ({} image sets)",
        assets_stats.record_count
    ),
    assets_elapsed,
);
for warning in &assets_stats.warnings {
    eprintln!(
        "  \x1b[33m!\x1b[0m skipped asset catalog {}: {}",
        warning.catalog, warning.reason
    );
}
```

- [ ] **Step 3: Run tests and verify on lama-ludo**

Run: `cargo test`
Then build release and run `grapha index . --full-rebuild` on lama-ludo-ios to verify the new line appears.

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(assets): integrate asset snapshot into index pipeline"
```

---

## Task 3: Tree-sitter Enrichment for Image References

**Files:**
- Modify: `grapha-swift/src/treesitter.rs` (add `enrich_asset_references_with_tree`)
- Modify: `grapha-swift/src/lib.rs` (add marker check and enrichment call)

- [ ] **Step 1: Add asset metadata constants**

At the top of `grapha-swift/src/treesitter.rs` or in a shared location:

The metadata keys will be used as string constants:
- `asset.ref_kind` = `"image"`
- `asset.name` = the resolved asset name (e.g., `"Room/voiceWave"` or `"icon_gift"`)

- [ ] **Step 2: Implement `enrich_asset_references_with_tree`**

In `grapha-swift/src/treesitter.rs`, add:

```rust
pub fn enrich_asset_references_with_tree(
    source: &[u8],
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let mut asset_refs = Vec::new();
    collect_image_asset_references(tree.root_node(), source, &mut asset_refs);

    for (node_span, asset_name) in asset_refs {
        // Find the graph node that contains this call site
        for node in &mut result.nodes {
            if spans_contain(node, &node_span) {
                node.metadata
                    .insert("asset.ref_kind".to_string(), "image".to_string());
                node.metadata
                    .insert("asset.name".to_string(), asset_name.clone());
                break;
            }
        }
    }

    Ok(())
}
```

The `collect_image_asset_references` function walks the AST looking for:
- `call_expression` where the callee is `Image` or `UIImage`
- Extracts the first argument as either a string literal or dot-expression path

```rust
fn collect_image_asset_references(
    node: tree_sitter::Node,
    source: &[u8],
    out: &mut Vec<(tree_sitter::Range, String)>,
) {
    if node.kind() == "call_expression" {
        if let Some(asset_name) = extract_image_call_asset_name(node, source) {
            out.push((node.range(), asset_name));
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_image_asset_references(child, source, out);
    }
}

fn extract_image_call_asset_name(
    node: tree_sitter::Node,
    source: &[u8],
) -> Option<String> {
    // Get the callee name
    let callee = node.child_by_field_name("function")?;
    let callee_text = node_text(callee, source);

    // Only match Image() and UIImage()
    if callee_text != "Image" && callee_text != "UIImage" {
        return None;
    }

    // Find the arguments
    let args = node.child_by_field_name("arguments")?;

    // Walk arguments looking for:
    // 1. String literal: Image("icon_gift")
    // 2. Dot expression: Image(.Room.voiceWave) or Image(asset: .Room.icon)
    // 3. Named param: UIImage(named: "icon"), UIImage(resource: .Game.btn)
    for i in 0..args.named_child_count() {
        let arg = args.named_child(i)?;

        // Handle labeled arguments: asset:, resource:, named:
        if arg.kind() == "value_argument" {
            let label = arg
                .children(&mut arg.walk())
                .find(|c| c.kind() == "value_argument_label")
                .map(|c| node_text(c, source));

            let value = arg.named_children(&mut arg.walk())
                .find(|c| c.kind() != "value_argument_label");

            if let Some(value) = value {
                match label.as_deref() {
                    Some("asset") | Some("resource") | Some("named") | None => {
                        if let Some(name) = extract_asset_name_from_expr(value, source) {
                            return Some(name);
                        }
                    }
                    _ => {} // Skip unrelated labels like "in:", "compatibleWith:", etc.
                }
            }
        }
    }

    None
}

fn extract_asset_name_from_expr(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let text = node_text(node, source).trim().to_string();

    // String literal: "icon_gift"
    if text.starts_with('"') && text.ends_with('"') && text.len() > 2 {
        return Some(text[1..text.len() - 1].to_string());
    }

    // Dot expression: .Room.voiceWave → "Room/voiceWave"
    if text.starts_with('.') {
        let path = text[1..]
            .split('.')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        if !path.is_empty() {
            return Some(path);
        }
    }

    None
}
```

**IMPORTANT:** The tree-sitter Swift grammar's AST structure for call expressions and arguments may differ from the field names used above (`"function"`, `"arguments"`, `"value_argument"`, `"value_argument_label"`). Check the actual tree-sitter-swift grammar for correct node kinds and field names. Use `tree-sitter parse` on a sample file to verify the AST structure. The logic intent is: find Image/UIImage calls, extract the first meaningful argument as either a string literal or dot-path.

The `spans_contain` helper should check if the call node's span falls within the graph node's span. A simple approach: match by line number range.

- [ ] **Step 3: Add marker check and call in lib.rs**

In `grapha-swift/src/lib.rs`, add:

```rust
fn source_contains_image_asset_markers(source: &[u8]) -> bool {
    bytes_contains(source, b"Image(")
        || bytes_contains(source, b"UIImage(")
}
```

In the enrichment section of `extract_swift` (inside the `if needs_parse` block), add after the l10n check:

```rust
if source_contains_image_asset_markers(source) {
    let _ = treesitter::enrich_asset_references_with_tree(
        source, &tree, &mut result,
    );
}
```

Also update `needs_parse` to include the asset marker check:
```rust
let has_assets = source_contains_image_asset_markers(source);
let needs_parse = needs_doc || has_swiftui || has_l10n || has_assets;
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(assets): detect Image/UIImage references in Swift source"
```

---

## Task 4: CLI Commands

**Files:**
- Modify: `grapha/src/main.rs` (add Asset subcommand group)
- Modify: `grapha/src/assets.rs` (add usage resolution)

- [ ] **Step 1: Add AssetCommands to CLI**

In `grapha/src/main.rs`, add to the `Commands` enum:

```rust
/// Inspect image asset catalogs and usage
Asset {
    #[command(subcommand)]
    command: AssetCommands,
},
```

Add the subcommand enum:

```rust
#[derive(Subcommand)]
enum AssetCommands {
    /// List discovered image assets
    List {
        /// Show only unused assets (no references in graph)
        #[arg(long)]
        unused: bool,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Find code referencing an image asset
    Usages {
        /// Asset name (e.g., "voiceWave" or "icon_gift")
        name: String,
        /// Project directory
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Json)]
        format: QueryOutputFormat,
    },
}
```

- [ ] **Step 2: Add asset usage resolution to assets.rs**

```rust
// In assets.rs, add:

use grapha_core::graph::Graph;

#[derive(Debug, Serialize)]
pub struct AssetUsage {
    pub node_id: String,
    pub node_name: String,
    pub node_kind: String,
    pub file: String,
    pub asset_name: String,
}

pub fn find_usages(graph: &Graph, asset_name: &str) -> Vec<AssetUsage> {
    graph
        .nodes
        .iter()
        .filter(|node| {
            node.metadata
                .get("asset.name")
                .is_some_and(|name| {
                    // Match exact name, or suffix (last path component)
                    name == asset_name
                        || name.rsplit('/').next() == Some(asset_name)
                })
        })
        .map(|node| AssetUsage {
            node_id: node.id.clone(),
            node_name: node.name.clone(),
            node_kind: serde_json::to_string(&node.kind)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string(),
            file: node.file.to_string_lossy().to_string(),
            asset_name: node
                .metadata
                .get("asset.name")
                .cloned()
                .unwrap_or_default(),
        })
        .collect()
}

pub fn find_unused(
    index: &AssetCatalogIndex,
    graph: &Graph,
) -> Vec<AssetRecord> {
    let referenced_names: std::collections::HashSet<String> = graph
        .nodes
        .iter()
        .filter_map(|n| n.metadata.get("asset.name").cloned())
        .flat_map(|name| {
            // Collect both the full path and the base name
            let base = name.rsplit('/').next().unwrap_or(&name).to_string();
            vec![name, base]
        })
        .collect();

    index
        .all_records()
        .iter()
        .filter(|record| !referenced_names.contains(&record.name))
        .cloned()
        .collect()
}
```

- [ ] **Step 3: Add command handlers**

In `grapha/src/main.rs`, add the handler:

```rust
fn handle_asset_command(command: AssetCommands) -> anyhow::Result<()> {
    match command {
        AssetCommands::List { unused, path } => {
            if unused {
                let graph = load_graph(&path)?;
                let index = assets::load_catalog_index(&path)?;
                let unused = assets::find_unused(&index, &graph);
                print_json(&unused)
            } else {
                let index = assets::load_catalog_index(&path)?;
                print_json(index.all_records())
            }
        }
        AssetCommands::Usages { name, path, format } => {
            let graph = load_graph(&path)?;
            let usages = assets::find_usages(&graph, &name);
            match format {
                QueryOutputFormat::Json => print_json(&usages),
                QueryOutputFormat::Tree => {
                    // Simple tree output
                    if usages.is_empty() {
                        eprintln!("no usages found for asset '{name}'");
                        return Ok(());
                    }
                    eprintln!("usages for {name} ({})", usages.len());
                    for usage in &usages {
                        eprintln!(
                            "  {} [{}] ({})",
                            usage.node_name, usage.node_kind, usage.file
                        );
                    }
                    Ok(())
                }
            }
        }
    }
}
```

Wire it into the main match:

```rust
Commands::Asset { command } => handle_asset_command(command)?,
```

- [ ] **Step 4: Run tests and verify on lama-ludo**

Run: `cargo test`
Build release, then on lama-ludo-ios:

```bash
grapha index . --full-rebuild
grapha asset list | head -20
grapha asset usages voiceWave
grapha asset list --unused | head -20
```

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(assets): add asset list, usages, and unused CLI commands"
```

---

## Task 5: Verification and Cleanup

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy`
Expected: No warnings

- [ ] **Step 3: Run fmt**

Run: `cargo fmt -- --check`
Expected: Clean

- [ ] **Step 4: Build release and smoke test on lama-ludo-ios**

```bash
cargo build --release
cd /path/to/lama-ludo-ios
grapha index . --full-rebuild
grapha asset list | wc -l                    # Should show image set count
grapha asset usages commonDefaultAvatarIc    # Should find UIImage(.FrameUI.commonDefaultAvatarIc) references
grapha asset list --unused | head -20        # Should show assets with no references
```

- [ ] **Step 5: Final commit if any fixes needed**

```bash
git commit -m "chore: xcassets integration cleanup"
```
