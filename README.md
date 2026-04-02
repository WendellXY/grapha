# Grapha

[中文文档](docs/README.CN.md)

**Blazingly fast** code intelligence that gives AI agents compiler-grade understanding of your codebase.

Grapha builds a symbol-level dependency graph from source code — not by guessing with regex, but by reading the compiler's own index. For Swift, it taps directly into Xcode's pre-built index store via binary FFI for 100% type-resolved symbols, then enriches with tree-sitter for view structure, docs, and localization. For Rust, it uses tree-sitter with Cargo workspace awareness. The result is a queryable graph with confidence-scored edges, dataflow tracing, impact analysis, and code smell detection — available as both a CLI and an MCP server for AI agent integration.

> **1,991 Swift files — 131K nodes — 784K edges — 8.7 seconds.** Zero-copy binary FFI. Lock-free parallel extraction. No serde on the hot path.

## Why Grapha

| | Grapha | Typical code-context tools |
|---|---|---|
| **Parsing** | Compiler index store (confidence 1.0) + tree-sitter fallback | tree-sitter only |
| **Relationship types** | 10 (calls, reads, writes, publishes, subscribes, inherits, implements, contains, type_ref, uses) | 4-6 |
| **Dataflow tracing** | Forward (entry → terminals) + reverse (symbol → entries) | None |
| **Code quality** | Complexity analysis, smell detection, module coupling metrics | None |
| **Confidence scores** | Per-edge 0.0–1.0 | None |
| **Terminal classification** | Auto-detects network, persistence, cache, event, keychain, search | None |
| **MCP tools** | 11 | 4-6 |
| **Watch mode** | File watcher with debounced incremental re-index | Varies |
| **Recall** | Session disambiguation — ambiguous symbols auto-resolve after first use | None |

## Performance

Benchmarked on a production iOS app (1,991 Swift files, ~300K lines):

| Phase | Time |
|-------|------|
| Extraction (index store + tree-sitter enrichment) | **3.5s** |
| Merge (module-aware cross-file resolution) | 0.3s |
| Classification (entry points + terminals) | 1.7s |
| SQLite persistence (deferred indexing) | 2.0s |
| Search index (BM25 via tantivy) | 1.0s |
| **Total** | **8.7s** |

**Graph:** 131,185 nodes · 783,793 edges · 2,983 entry points · 11,149 terminal operations

**Why it's fast:** zero-copy index store FFI via pointer arithmetic (no serde), lock-free rayon extraction, single shared tree-sitter parse, marker-based enrichment skip, deferred SQLite indexing, USR-scoped edge resolution. Run `grapha index --timing` for a per-phase breakdown.

## Install

```bash
cargo install --path grapha
```

## Quick Start

```bash
# Index a project (incremental by default)
grapha index .

# Search symbols
grapha symbol search "ViewModel" --kind struct --context
grapha symbol search "send" --kind function --module Room --fuzzy

# 360° context — callers, callees, reads, implements
grapha symbol context RoomPage --format tree

# Impact analysis — what breaks if this changes?
grapha symbol impact GiftPanelViewModel --depth 2 --format tree

# Complexity analysis — structural health of a type
grapha symbol complexity RoomPage

# Dataflow: entry point → terminal operations
grapha flow trace RoomPage --format tree

# Reverse: which entry points reach this symbol?
grapha flow trace sendGift --direction reverse

# Code smell detection
grapha repo smells --module Room

# Module metrics — symbol counts, coupling ratios
grapha repo modules

# MCP server for AI agents (with auto-refresh)
grapha serve --mcp --watch
```

## MCP Server — 11 Tools for AI Agents

```bash
grapha serve --mcp              # JSON-RPC over stdio
grapha serve --mcp --watch      # + auto-refresh on file changes
```

Add to `.mcp.json`:

```json
{
  "mcpServers": {
    "grapha": {
      "command": "grapha",
      "args": ["serve", "--mcp", "--watch", "-p", "."]
    }
  }
}
```

| Tool | What it does |
|------|-------------|
| `search_symbols` | BM25 search with kind/module/role/fuzzy filters |
| `get_symbol_context` | 360° view: callers, callees, reads, implements, contains tree |
| `get_impact` | BFS blast radius at configurable depth |
| `trace` | Forward dataflow to terminals, or reverse to entry points |
| `get_file_symbols` | All declarations in a file, by source position |
| `batch_context` | Context for up to 20 symbols in one call |
| `analyze_complexity` | Structural metrics + severity rating for any type |
| `detect_smells` | Graph-wide code smell scan (god types, fan-out, nesting, etc.) |
| `get_module_summary` | Per-module metrics with cross-module coupling ratio |
| `get_file_map` | File/symbol map organized by module and directory |
| `reload` | Hot-reload graph from disk without restarting the server |

**Recall:** The MCP server remembers symbol resolutions within a session. If `helper` is ambiguous the first time, after you disambiguate with `File.swift::helper`, future bare `helper` queries resolve automatically.

## Commands

### Symbols

```bash
grapha symbol search "query" [--kind K] [--module M] [--fuzzy] [--context]
grapha symbol context <symbol> [--format tree]
grapha symbol impact <symbol> [--depth N] [--format tree]
grapha symbol complexity <symbol>          # property/method/dependency counts, severity
grapha symbol file <path>                  # list declarations in a file
```

### Dataflow

```bash
grapha flow trace <symbol> [--direction forward|reverse] [--depth N]
grapha flow graph <symbol> [--depth N]     # semantic effect graph
grapha flow entries                        # list auto-detected entry points
```

### Repository

```bash
grapha repo smells [--module M]            # code smell detection
grapha repo modules                        # per-module metrics
grapha repo map [--module M]               # file/symbol overview
grapha repo changes [scope]                # git diff → affected symbols
```

### Indexing & Serving

```bash
grapha index . [--full-rebuild] [--timing]
grapha analyze <path> [--compact] [--filter fn,struct]
grapha serve [--mcp] [--watch] [--port N]
```

### Localization & Assets

```bash
grapha l10n symbol <symbol>                # resolve l10n records from SwiftUI subtree
grapha l10n usages <key> [--table T]       # find usage sites for a localization key
grapha asset list [--unused]               # image assets from xcassets catalogs
grapha asset usages <name>                 # find Image()/UIImage() references
```

## Configuration

Optional `grapha.toml` at project root:

```toml
[swift]
index_store = true                         # false → tree-sitter only

[output]
default_fields = ["file", "module"]

[[external]]
name = "FrameUI"
path = "/path/to/local/frameui"            # include in cross-repo analysis

[[classifiers]]
pattern = "FirebaseFirestore.*setData"
terminal = "persistence"
direction = "write"
operation = "set"
```

## Architecture

```
grapha-core/     Shared types (Node, Edge, Graph, ExtractionResult)
grapha-swift/    Swift: index store → SwiftSyntax → tree-sitter waterfall
grapha/          CLI, Rust extractor, query engines, MCP server, web UI
nodus/           Agent tooling package (skills, rules, commands)
```

### Extraction Waterfall (Swift)

```
Xcode Index Store (binary FFI)      → compiler-resolved USRs, confidence 1.0
  ↓ fallback
SwiftSyntax (JSON FFI)              → accurate parse, no type resolution, confidence 0.9
  ↓ fallback
tree-sitter-swift (bundled)         → fast, limited accuracy, confidence 0.6–0.8
```

After index store extraction, tree-sitter enriches doc comments, SwiftUI view hierarchy, and localization metadata in a single shared parse.

### Graph Model

**14 node kinds:** function, struct, enum, trait, protocol, extension, property, field, variant, constant, type_alias, impl, module, view, branch

**10 edge kinds:** calls, implements, inherits, contains, type_ref, uses, reads, writes, publishes, subscribes

**Dataflow annotations:** direction (read/write/pure), operation (fetch/save/publish), condition, async_boundary, provenance (source file + span)

**Node roles:** entry_point (SwiftUI View, @Observable, fn main, #[test]) · terminal (network, persistence, cache, event, keychain, search)

### Nodus Package

```bash
nodus add wenext/grapha --adapter claude
```

Installs skills, rules, and slash commands (`/index`, `/search`, `/impact`, `/complexity`, `/smells`) for grapha-aware AI workflows.

## Supported Languages

| Language | Extraction | Type Resolution |
|----------|-----------|----------------|
| **Swift** | Index store + tree-sitter | Compiler-grade (USR) |
| **Rust** | tree-sitter | Name-based |

The per-language crate architecture supports adding new languages with the same waterfall pattern: compiler index → syntax parser → tree-sitter fallback.

## Development

```bash
cargo build                    # Build all workspace crates
cargo test                     # Run all tests (~200)
cargo clippy && cargo fmt      # Lint + format
```

## License

MIT
