# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Grapha is a lightweight code intelligence CLI that transforms source code into a normalized, graph-based representation optimized for LLM consumption. It uses fast syntax parsing (via tree-sitter) to extract symbols, relationships, and call patterns, compresses them into a navigable node graph, and provides persistence, search, and impact analysis for agent-driven workflows.

## Build & Development Commands

```bash
cargo build                    # Build the project
cargo run -- <subcommand>      # Run the CLI (analyze, index, context, impact, search, changes)
cargo test                     # Run all tests (173 tests)
cargo test <test_name>         # Run a single test
cargo test --lib               # Run unit tests only
cargo test --test <name>       # Run a specific integration test
cargo clippy                   # Lint
cargo fmt                      # Format code
cargo fmt -- --check           # Check formatting without modifying
```

## Architecture

- **Language**: Rust, CLI-first (using `clap` with subcommands)
- **Parsing**: tree-sitter grammars for Rust (`tree-sitter-rust`) and Swift (`tree-sitter-swift`)
- **Persistence**: SQLite via `rusqlite` (production), JSON (debug)
- **Search**: BM25 full-text search via `tantivy`
- **Change detection**: Git diff via `git2`
- **Design principle**: Structural/syntactic analysis only — no compiler-level semantic validation

### Core Pipeline

1. **Parse** — tree-sitter parses source files into concrete syntax trees
2. **Extract** — walk CSTs to identify symbols (functions, types, protocols, properties) and relationships (calls, conformances, imports, inheritance)
3. **Merge** — combine per-file results into a single graph with cross-file edge resolution (name-based, with confidence scoring)
4. **Classify** — auto-detect entry points, terminal nodes (network, persistence, cache, event), and dataflow directions via built-in classifiers and user-defined `grapha.toml` rules
5. **Compress** — prune inferrable edges and group by file for LLM consumption (`--compact`)
6. **Persist** — store to SQLite or JSON via the `Store` trait
7. **Query** — context (360° symbol view), impact (BFS blast radius), search (BM25), trace (forward dataflow), reverse (entry point impact), entries (list entry points)

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

| Module | Purpose |
|--------|---------|
| `extract/rust.rs` | Rust tree-sitter extraction |
| `extract/swift.rs` | Swift tree-sitter extraction |
| `merge.rs` | Merge per-file results, cross-file edge resolution |
| `compress/prune.rs` | Drop inferrable edges and noise |
| `compress/group.rs` | Semantic file→type→members grouping |
| `store/sqlite.rs` | SQLite persistence backend |
| `store/json.rs` | JSON persistence backend |
| `query/context.rs` | 360° symbol context (callers, callees, impls) |
| `query/impact.rs` | BFS blast radius analysis |
| `search.rs` | BM25 full-text search via tantivy |
| `changes.rs` | Git diff → affected symbols → impact |
| `classify.rs` | Classifier trait, CompositeClassifier |
| `classify/swift.rs` | Built-in Swift framework pattern classifier |
| `classify/rust.rs` | Built-in Rust framework pattern classifier |
| `classify/toml_rules.rs` | User-defined classifier rules from grapha.toml |
| `classify/pass.rs` | Post-merge classification pass |
| `config.rs` | grapha.toml configuration parsing |
| `module.rs` | Module map discovery (Swift packages, Cargo workspaces) |
| `query/trace.rs` | Forward dataflow tracing (entry → terminals) |
| `query/reverse.rs` | Reverse impact to entry points |
| `query/entries.rs` | Entry point listing |
| `serve.rs` | Embedded web UI HTTP server (axum) |
| `serve/api.rs` | REST API handlers for web UI |

### Key Design Decisions

- Tree-sitter over full compiler frontends: prioritizes speed and language coverage over semantic correctness
- Graph nodes represent symbols; edges represent relationships with confidence scores (0.0–1.0)
- Cross-file resolution is name-based: unambiguous matches get 0.9x confidence, ambiguous get 0.5x
- The `--compact` output groups by file and inlines relationships for token-efficient LLM traversal
