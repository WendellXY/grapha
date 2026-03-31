# AGENTS.md

This file provides guidance to agents when working with code in this repository.

## Project Overview

Grapha is a blazingly fast code intelligence CLI that transforms source code into a normalized, graph-based representation with compiler-grade accuracy. For Swift, it reads Xcode's pre-built index store (via `libIndexStore.dylib` FFI) for fully type-resolved symbol graphs, then falls back to SwiftSyntax and finally tree-sitter for instant parsing without a build. For Rust, it uses tree-sitter directly. The resulting graph provides persistence, search, dataflow tracing, and impact analysis for agent-driven and developer workflows.

## Workspace Structure

```
grapha-core/     Shared types (Node, Edge, Graph, ExtractionResult, LanguageExtractor)
grapha-swift/    Swift extraction: index store → SwiftSyntax → tree-sitter waterfall
grapha/          CLI binary, Rust extractor, pipeline, query engines, web UI
```

## Build & Development Commands

```bash
cargo build                    # Build all workspace crates
cargo test                     # Run all tests (173 tests)
cargo build -p grapha-core     # Build shared types only
cargo build -p grapha-swift    # Build Swift extractor (auto-compiles Swift bridge if toolchain available)
cargo run -p grapha -- <cmd>   # Run the CLI
cargo clippy                   # Lint
cargo fmt                      # Format
cargo fmt -- --check           # Check formatting without modifying
```

## Architecture

- **Language**: Rust workspace, CLI-first (using `clap` with subcommands)
- **Swift parsing**: Xcode index store (compiler-grade, binary FFI) → SwiftSyntax (JSON-string FFI) → tree-sitter-swift (fallback)
- **Rust parsing**: tree-sitter-rust
- **Swift bridge**: `grapha-swift/swift-bridge/` — Swift Package compiled by `build.rs`, exports `@c` functions, links `libIndexStore.dylib`, uses binary FFI for index-store extraction and JSON-string FFI for SwiftSyntax extraction
- **Persistence**: SQLite via `rusqlite` (production), JSON (debug)
- **Search**: BM25 full-text search via `tantivy`
- **Web UI**: Embedded axum server with vis-network frontend
- **Change detection**: Git diff via `git2`
- **Parallelism**: rayon for concurrent file extraction

### Core Pipeline

1. **Discover** — find source files, detect Swift packages / Cargo workspaces for module map
2. **Extract** — per-file: index store (confidence 1.0) → SwiftSyntax (0.9) → tree-sitter (0.6-0.8)
3. **Merge** — combine per-file results with module-aware, import-guided cross-file edge resolution
4. **Classify** — auto-detect entry points (SwiftUI View, @Observable, fn main), terminal nodes (network, persistence, cache, event) via USR module matching + string pattern classifiers + user-defined `grapha.toml` rules
5. **Compress** — prune inferrable edges and group by file for LLM consumption (`--compact`)
6. **Persist** — store to SQLite or JSON via the `Store` trait
7. **Query** — context (360° symbol view), impact (BFS blast radius), search (BM25), trace (forward dataflow), reverse (entry point impact), entries (list entry points)
8. **Serve** — embedded web UI with REST API

### CLI Subcommands

```
grapha analyze <path> [--output FILE] [--filter KINDS] [--compact]
grapha index <path> [--format sqlite|json] [--store-dir DIR]
grapha context <symbol> [-p PATH]
grapha impact <symbol> [--depth N] [-p PATH]
grapha search <query> [--limit N] [-p PATH]
grapha changes [SCOPE] [-p PATH]
grapha trace <entry> [--depth N] [-p PATH]   — Forward dataflow trace
grapha reverse <symbol> [-p PATH]            — Reverse impact to entry points
grapha entries [-p PATH]                      — List auto-detected entry points
grapha serve [-p PATH] [--port 8080]         — Launch web UI
```

### Module Style

Use `foo.rs` + `foo/` directory pattern (not `foo/mod.rs`).

### Key Modules

#### grapha-core

| Module | Purpose |
|--------|---------|
| `graph.rs` | Node, Edge, Graph, NodeRole, TerminalKind, FlowDirection enums |
| `resolve.rs` | Import, ImportKind |
| `extract.rs` | ExtractionResult, LanguageExtractor trait |

#### grapha-swift

| Module | Purpose |
|--------|---------|
| `lib.rs` | Public API: `extract_swift()` waterfall, index store auto-discovery |
| `bridge.rs` | `dlopen` + FFI function pointers for Swift bridge dylib |
| `indexstore.rs` | Index store reader (calls bridge, parses binary buffer) |
| `swiftsyntax.rs` | SwiftSyntax parser (calls bridge, parses JSON-string response) |
| `treesitter.rs` | tree-sitter-swift fallback extractor |
| `swift-bridge/` | Swift Package with `@c` exported functions, links `libIndexStore.dylib` |

#### grapha (CLI)

| Module | Purpose |
|--------|---------|
| `extract/rust.rs` | Rust tree-sitter extraction |
| `merge.rs` | Merge per-file results, module-aware cross-file edge resolution |
| `compress/prune.rs` | Drop inferrable edges and noise |
| `compress/group.rs` | Semantic file→type→members grouping |
| `store/sqlite.rs` | SQLite persistence backend |
| `store/json.rs` | JSON persistence backend |
| `query/context.rs` | 360° symbol context (callers, callees, impls) |
| `query/impact.rs` | BFS blast radius analysis |
| `query/trace.rs` | Forward dataflow tracing (entry → terminals) |
| `query/reverse.rs` | Reverse impact to entry points |
| `query/entries.rs` | Entry point listing |
| `search.rs` | BM25 full-text search via tantivy |
| `changes.rs` | Git diff → affected symbols → impact |
| `classify.rs` | Classifier trait, CompositeClassifier |
| `classify/swift.rs` | Built-in Swift framework pattern classifier |
| `classify/rust.rs` | Built-in Rust framework pattern classifier |
| `classify/toml_rules.rs` | User-defined classifier rules from grapha.toml |
| `classify/pass.rs` | Post-merge classification pass (USR module-based + string patterns + entry points) |
| `config.rs` | grapha.toml configuration parsing |
| `module.rs` | Module map discovery (Swift packages, Cargo workspaces) |
| `serve.rs` | Embedded web UI HTTP server (axum) |
| `serve/api.rs` | REST API handlers for web UI |

### Key Design Decisions

- Xcode index store for Swift: compiler-grade accuracy, USR-based IDs, zero re-parsing cost
- tree-sitter as universal fallback: fast, works without build tools, handles Rust well
- Swift bridge via `@c` FFI + `dlopen`: graceful degradation if Swift toolchain unavailable
- Graph nodes represent symbols; edges represent relationships with confidence scores (0.0–1.0)
- USR-based IDs from index store enable 100% accurate cross-module resolution
- Module-based terminal classification: extract module name from USRs to classify external framework calls
- The `--compact` output groups by file and inlines relationships for token-efficient LLM traversal
