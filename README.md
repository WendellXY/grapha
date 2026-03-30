# Grapha

[中文文档](docs/README.CN.md)

Blazingly fast code intelligence for LLM agents.

Grapha transforms source code into a normalized, graph-based representation optimized for LLM consumption. It parses via [tree-sitter](https://tree-sitter.github.io/), extracts symbols and relationships, then provides persistence, search, and impact analysis — giving agents structured access to codebases at scale.

> **1,991 Swift files, 306k lines of code — indexed in 2.5 seconds, 146 MB peak memory.**

## Install

```bash
cargo install --path .
```

## Quick Start

```bash
# Index a project (persists to .grapha/)
grapha index .

# Search for symbols
grapha search Config

# Get 360° context for a symbol
grapha context Config

# Analyze blast radius of a change
grapha impact Config --depth 3

# Detect git changes and affected symbols
grapha changes
```

## Commands

### `grapha analyze` — Extract and output graph

```bash
grapha analyze src/              # Analyze a directory (respects .gitignore)
grapha analyze src/main.rs       # Analyze a single file
grapha analyze src/ -o graph.json   # Write to file
grapha analyze src/ --filter fn,struct,trait  # Filter by symbol kind
grapha analyze src/ --compact    # LLM-optimized grouped output
```

### `grapha index` — Persist graph to storage

```bash
grapha index .                         # Index project (SQLite, default)
grapha index . --format json           # Index as JSON (for debugging)
grapha index . --store-dir /tmp/idx    # Custom storage location
```

### `grapha context` — 360° symbol view

```bash
grapha context Config           # Callers, callees, implementors
grapha context Config -p /path/to/project
```

### `grapha impact` — Blast radius analysis

```bash
grapha impact Config            # Who breaks if Config changes?
grapha impact Config --depth 5  # Deeper traversal
```

### `grapha search` — BM25 full-text search

```bash
grapha search "Config"          # Search by name
grapha search "main.rs" --limit 5
```

### `grapha changes` — Git-based change detection

```bash
grapha changes              # All uncommitted changes
grapha changes staged       # Only staged changes
grapha changes main         # Compare against a branch
```

## Output Format

### Standard (JSON graph)

```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "graph.rs::Config",
      "kind": "struct",
      "name": "Config",
      "file": "graph.rs",
      "span": { "start": [10, 0], "end": [15, 1] },
      "visibility": "public",
      "metadata": {}
    }
  ],
  "edges": [
    {
      "source": "main.rs::run",
      "target": "graph.rs::Config",
      "kind": "type_ref",
      "confidence": 0.85
    }
  ]
}
```

### Compact (`--compact`) — LLM-optimized

```json
{
  "version": "0.1.0",
  "files": {
    "graph.rs": {
      "symbols": [
        {
          "name": "Config",
          "kind": "struct",
          "span": [10, 15],
          "type_refs": ["Node"]
        }
      ]
    }
  }
}
```

### Node Kinds

`function`, `struct`, `enum`, `trait`, `impl`, `module`, `field`, `variant`, `property`, `constant`, `type_alias`, `protocol`, `extension`

### Edge Kinds

| Kind | Meaning | Confidence |
|------|---------|------------|
| `calls` | Function calls another function | 0.8 |
| `uses` | `use`/`import` statement | 0.7 |
| `implements` | `impl Trait for Type` / protocol conformance | 0.9 |
| `contains` | Structural nesting (module > struct > field) | 1.0 |
| `type_ref` | Type referenced in signature or field | 0.85 |
| `inherits` | Supertrait bound (`trait Child: Base`) | 0.9 |

## Supported Languages

- **Rust** (via `tree-sitter-rust`)
- **Swift** (via `tree-sitter-swift`)

The core is language-agnostic. Adding a new language requires implementing the `LanguageExtractor` trait with a tree-sitter grammar.

## Design Principles

- **Structural, not semantic** — tree-sitter parses syntax, not types. Call resolution is name-based with confidence scoring. No type inference, no cross-crate resolution.
- **Optimized for LLMs** — minimal tokens, deterministic IDs, flat JSON. The `--compact` mode groups by file for agent-friendly traversal.
- **Graceful degradation** — partial parses extract what they can. Failing files are skipped with a warning. Cross-file references are resolved by name with reduced confidence.
- **Persistence + Query** — index once, query many times. SQLite for production, JSON for debugging.

## Development

```bash
cargo build                    # Build
cargo test                     # Run all tests (79 tests)
cargo clippy                   # Lint
cargo fmt                      # Format
cargo run -- analyze src/      # Run on own source
cargo run -- index .           # Index this project
```

## License

MIT
