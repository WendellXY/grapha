# Grapha

[中文文档](docs/README.CN.md)

Blazingly fast code intelligence for LLM agents and developer tooling.

Grapha transforms source code into a normalized, graph-based representation with compiler-grade accuracy. It combines [tree-sitter](https://tree-sitter.github.io/) parsing with Xcode's index store for fully type-resolved symbol graphs, then provides persistence, search, dataflow tracing, and impact analysis — giving agents and developers structured access to codebases at scale.

> **1,991 Swift files — 111K nodes, 627K compiler-resolved edges — indexed in 21 seconds.**

## Features

- **Compiler-grade accuracy** — reads Xcode's pre-built index store for 100% type-resolved call graphs (Swift). Falls back to tree-sitter for instant parsing without a build.
- **Dataflow tracing** — trace forward from entry points to terminal operations (network, persistence, cache). Trace backward from any symbol to affected entry points.
- **Impact analysis** — BFS blast radius: "if I change this function, what breaks?"
- **Entry point detection** — auto-detects SwiftUI Views, `@Observable` classes, `fn main()`, `#[test]` functions.
- **Terminal classification** — recognizes network calls (FrameNetwork), persistence (FrameStorage, GRDB), cache (Kingfisher), analytics (FrameStat), and more. Extensible via `grapha.toml`.
- **Cross-module resolution** — import-guided disambiguation with confidence scoring. Module-aware merging for multi-package projects.
- **Web UI** — embedded interactive graph explorer (`grapha serve`).
- **Multi-language** — Rust and Swift today. Architecture supports adding Java, Kotlin, C#, TypeScript.

## Install

```bash
cargo install --path grapha
```

## Quick Start

```bash
# Index a project
grapha index .

# Search for symbols
grapha search sendMessage

# 360° context for a symbol
grapha context sendMessage

# Human-readable tree output for graph queries
grapha reverse handleSendResult --format tree

# Impact analysis: what breaks if this changes?
grapha impact bootstrapGame --depth 5

# Forward trace: entry point → terminal operations
grapha trace bootstrapGame

# Reverse: which entry points reach this symbol?
grapha reverse handleSendResult

# List auto-detected entry points
grapha entries

# Interactive web UI
grapha serve --port 8765
```

## Commands

### `grapha index` — Build the graph

```bash
grapha index .                         # Index project (SQLite)
grapha index . --format json           # JSON output (debugging)
grapha index . --store-dir /tmp/idx    # Custom storage
```

Auto-discovers Xcode's index store from DerivedData for compiler-resolved symbols. Falls back to tree-sitter when no index is available.

### `grapha analyze` — One-shot extraction

```bash
grapha analyze src/                    # Analyze directory
grapha analyze src/main.rs             # Single file
grapha analyze src/ --compact          # LLM-optimized grouped output
grapha analyze src/ --filter fn,struct # Filter by symbol kind
grapha analyze src/ -o graph.json      # Write to file
```

### `grapha context` — 360° symbol view

```bash
grapha context Config                  # Callers, callees, implements
grapha context bootstrapGame           # Fuzzy name matching
grapha context bootstrapGame --format tree
```

### `grapha impact` — Blast radius

```bash
grapha impact bootstrapGame            # Who depends on this?
grapha impact bootstrapGame --depth 5  # Deeper traversal
grapha impact bootstrapGame --format tree
```

### `grapha trace` — Forward dataflow

```bash
grapha trace bootstrapGame             # Entry → service → terminal ops
grapha trace sendMessage --depth 10
grapha trace bootstrapGame --format tree
```

### `grapha reverse` — Entry point impact

```bash
grapha reverse handleSendResult        # Which Views/entry points reach this?
grapha reverse handleSendResult --format tree
```

### `grapha entries` — List entry points

```bash
grapha entries                         # All detected entry points
grapha entries --format tree
```

### `grapha search` — Full-text search

```bash
grapha search "ViewModel"
grapha search "send" --limit 10
```

### `grapha changes` — Git change detection

```bash
grapha changes                         # All uncommitted changes
grapha changes staged                  # Only staged
grapha changes main                    # Compare against branch
```

### `grapha serve` — Web UI

```bash
grapha serve --port 8765               # Open http://localhost:8765
```

Interactive graph visualization with vis-network: click nodes, trace flows, search symbols, filter by kind/role.

## Architecture

### Workspace

```
grapha-core/     Shared types (Node, Edge, Graph, ExtractionResult)
grapha-swift/    Swift extraction: index store → SwiftSyntax → tree-sitter waterfall
grapha/          CLI binary, Rust extractor, pipeline, query engines, web UI
```

### Extraction Waterfall (Swift)

```
1. Xcode Index Store (libIndexStore.dylib via Swift bridge FFI)
   → compiler-resolved USRs, confidence 1.0
   → auto-discovered from DerivedData

2. SwiftSyntax (via Swift bridge FFI)
   → accurate parsing, no type resolution, confidence 0.9

3. tree-sitter-swift (bundled)
   → fast fallback, limited accuracy, confidence 0.6-0.8
```

The Swift bridge (`libGraphaSwiftBridge.dylib`) is automatically compiled by `build.rs` when a Swift toolchain is detected. No Swift required for Rust-only projects.

### Pipeline

```
Discover → Extract → Merge → Classify → Compress → Store → Query/Serve
              ↑          ↑        ↑
         index store  module-   entry points
         or tree-     aware     + terminals
         sitter       resolution
```

### Graph Model

Nodes represent symbols (functions, types, properties). Edges represent relationships with confidence scores.

**Node kinds:** `function`, `struct`, `enum`, `trait`, `protocol`, `extension`, `property`, `field`, `variant`, `constant`, `type_alias`, `impl`, `module`

**Edge kinds:**

| Kind | Meaning |
|------|---------|
| `calls` | Function/method call |
| `implements` | Protocol conformance / trait impl |
| `inherits` | Superclass / supertrait |
| `contains` | Structural nesting |
| `type_ref` | Type reference |
| `uses` | Import statement |
| `reads` / `writes` | Data access direction |
| `publishes` / `subscribes` | Event flow |

**Dataflow annotations on edges:**

| Field | Purpose |
|-------|---------|
| `direction` | `read`, `write`, `read_write`, `pure` |
| `operation` | `fetch`, `save`, `publish`, `navigate`, etc. |
| `condition` | Guard/if condition text (when call is conditional) |
| `async_boundary` | Whether call crosses async boundary |

**Node roles:**
- `entry_point` — SwiftUI View.body, @Observable methods, fn main, #[test]
- `terminal` — network, persistence, cache, event, keychain, search

## Configuration

Optional `grapha.toml` for custom classifiers and entry points:

```toml
[[classifiers]]
pattern = "FirebaseFirestore.*setData"
terminal = "persistence"
direction = "write"
operation = "set"

[[entry_points]]
language = "swift"
pattern = ".*Coordinator.start"
```

## Supported Languages

| Language | Extraction | Type Resolution |
|----------|-----------|----------------|
| **Swift** | tree-sitter + Xcode index store | Compiler-grade (USR) |
| **Rust** | tree-sitter | Name-based |

The per-language crate architecture (`grapha-swift/`, future `grapha-java/`, etc.) supports adding new languages with the same pattern: compiler index → syntax parser → tree-sitter fallback.

## Performance

Tested on a production iOS project (1,991 Swift files):

| Metric | Value |
|--------|-------|
| Index time | **21 seconds** |
| Nodes | 111,096 |
| Edges | 627,558 |
| Entry points | 2,993 |
| Terminals | 2,959 |
| Peak memory | ~200 MB |

## Development

```bash
cargo build                    # Build all workspace crates
cargo test                     # Run all tests (173 tests)
cargo build -p grapha-core     # Build shared types only
cargo build -p grapha-swift    # Build Swift extractor
cargo run -p grapha -- <cmd>   # Run the CLI
cargo clippy                   # Lint
cargo fmt                      # Format
```

## License

MIT
