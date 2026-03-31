# Grapha

[中文文档](docs/README.CN.md)

Blazingly fast code intelligence for LLM agents and developer tooling.

Grapha transforms source code into a normalized, graph-based representation with compiler-grade accuracy. For Swift, it reads Xcode's pre-built index store via a binary FFI bridge for fully type-resolved symbol graphs, then falls back to SwiftSyntax and finally tree-sitter for instant parsing without a build. The resulting graph provides persistence, incremental search/index sync, dataflow tracing, semantic effect graphs, and impact analysis — giving agents and developers structured access to codebases at scale.

> **1,991 Swift files — 123K nodes, 766K edges — indexed in 6 seconds.**

## Performance

Tested on a production iOS app (1,991 Swift files, ~300K lines):

| Phase | Time |
|-------|------|
| Extraction (index store + binary FFI) | **1.8s** |
| Merge (module-aware cross-file resolution) | 0.15s |
| Classification (entry points + terminals) | 0.97s |
| SQLite persistence (889K rows) | 2.1s |
| Search index (BM25 via tantivy) | 0.8s |
| **Total** | **6.0s** |

| Metric | Value |
|--------|-------|
| Nodes | 123,323 |
| Edges (compiler-resolved) | 766,427 |
| Entry points (auto-detected) | 2,985 |
| Terminal operations | 10,548 |

**Why it's fast:**
- **Zero-parse index-store FFI** — the index-store bridge returns packed structs + a deduplicated string table. Rust reads with pointer arithmetic, no serde on the compiler-grade path.
- **Index store reuse** — reads Xcode's already-compiled symbol database. No re-parsing, no re-resolving.
- **Fast fallback tiers** — when no index is available, SwiftSyntax runs through the same bridge and tree-sitter remains the last-resort parser.
- **Deferred indexing** — SQLite indexes built after bulk insert, not during.
- **Parallel extraction** — rayon-powered concurrent file processing.

## Features

- **Compiler-grade accuracy** — reads Xcode's pre-built index store for 100% type-resolved call graphs (Swift). Falls back to SwiftSyntax, then tree-sitter, for instant parsing without a build.
- **Incremental indexing** — SQLite storage and Tantivy search sync incrementally by default. Use `grapha index --full-rebuild` to force a cold rebuild.
- **Dataflow tracing** — trace forward from entry points to terminal operations (network, persistence, cache). Trace backward from any symbol to affected entry points.
- **Semantic dataflow graph** — derive a deduplicated effect graph from an entry point with `grapha dataflow`, including reads, writes, publishes, subscribes, and terminal side effects.
- **Impact analysis** — BFS blast radius: "if I change this function, what breaks?"
- **Provenance-aware change detection** — edges carry source spans, so `grapha changes` can attribute method-body edits even when declaration spans stay fixed.
- **Entry point detection** — auto-detects SwiftUI Views, `@Observable` classes, `fn main()`, `#[test]` functions.
- **Terminal classification** — recognizes network calls, persistence (GRDB, CoreData), cache (Kingfisher), analytics, and more. Extensible via `grapha.toml`.
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

# Force ANSI-highlighted tree output for CI logs
grapha --color always reverse handleSendResult --format tree

# Impact analysis: what breaks if this changes?
grapha impact bootstrapGame --depth 5

# Forward trace: entry point → terminal operations
grapha trace bootstrapGame

# Derived semantic effect graph
grapha dataflow bootstrapGame --format tree

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
grapha index . --full-rebuild          # Force full store/search rebuild
```

Auto-discovers Xcode's index store from DerivedData for compiler-resolved symbols. Falls back to SwiftSyntax and then tree-sitter when no index is available. SQLite storage and the search index sync incrementally by default when a compatible prior index exists.

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
grapha --color always context bootstrapGame --format tree
```

### `grapha impact` — Blast radius

```bash
grapha impact bootstrapGame            # Who depends on this?
grapha impact bootstrapGame --depth 5  # Deeper traversal
grapha impact bootstrapGame --format tree
grapha --color always impact bootstrapGame --format tree
```

### `grapha trace` — Forward dataflow

```bash
grapha trace bootstrapGame             # Entry → service → terminal ops
grapha trace sendMessage --depth 10
grapha trace bootstrapGame --format tree
grapha --color always trace bootstrapGame --format tree
```

### `grapha dataflow` — Derived semantic effect graph

```bash
grapha dataflow bootstrapGame
grapha dataflow sendMessage --depth 10
grapha dataflow bootstrapGame --format tree
grapha --color always dataflow bootstrapGame --format tree
```

### `grapha reverse` — Entry point impact

```bash
grapha reverse handleSendResult        # Which Views/entry points reach this?
grapha reverse handleSendResult --format tree
grapha --color always reverse handleSendResult --format tree
```

### `grapha entries` — List entry points

```bash
grapha entries                         # All detected entry points
grapha entries --format tree
grapha --color always entries --format tree
```

`--color auto|always|never` controls ANSI styling for tree output. `auto` colors only when stdout is a terminal, `always` forces color for CI/log capture, and `never` keeps tree output plain. JSON output is always uncolored.

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
1. Xcode Index Store (binary FFI via Swift bridge)
   → compiler-resolved USRs, confidence 1.0
   → auto-discovered from DerivedData

2. SwiftSyntax (JSON-string FFI via Swift bridge)
   → accurate parsing, no type resolution, confidence 0.9

3. tree-sitter-swift (bundled)
   → fast fallback, limited accuracy, confidence 0.6-0.8
```

The Swift bridge (`libGraphaSwiftBridge.dylib`) is automatically compiled by `build.rs` when a Swift toolchain is detected. The index-store path crosses the FFI boundary as a flat binary buffer (packed structs + string table), while the SwiftSyntax path currently returns a JSON string that Rust decodes into the shared graph model. No Swift required for Rust-only projects.

### Pipeline

```
Discover → Extract → Merge → Classify → Compress → Store → Query/Serve
              ↑          ↑        ↑
      index store /  module-   entry points
      SwiftSyntax /  aware     + terminals
      tree-sitter    resolution
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
| `provenance` | Source file/span evidence for the relationship |

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

## Development

```bash
cargo build                    # Build all workspace crates
cargo test                     # Run all tests
cargo build -p grapha-core     # Build shared types only
cargo build -p grapha-swift    # Build Swift extractor
cargo run -p grapha -- <cmd>   # Run the CLI
cargo clippy                   # Lint
cargo fmt                      # Format
```

## License

MIT
