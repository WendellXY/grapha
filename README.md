# Grapha

A lightweight structural abstraction layer that transforms source code into a normalized, graph-based representation optimized for LLM consumption.

Instead of relying on compiler-level semantics, Grapha uses fast syntax parsing via [tree-sitter](https://tree-sitter.github.io/) to extract symbols, relationships, and call patterns, then compresses them into a navigable node graph. This enables agents to efficiently locate, traverse, and reason about code at scale with minimal context.

## Install

```bash
cargo install --path .
```

## Usage

```bash
# Analyze a single file
grapha src/main.rs

# Analyze a directory (recursively, respects .gitignore)
grapha src/

# Write output to a file
grapha src/ -o graph.json

# Filter to specific symbol kinds
grapha src/ --filter fn,struct,trait
```

## Output Format

Grapha outputs a JSON graph with `nodes` (symbols) and `edges` (relationships):

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
      "kind": "type_ref"
    }
  ]
}
```

### Node Kinds

`function`, `struct`, `enum`, `trait`, `impl`, `module`, `field`, `variant`

### Edge Kinds

| Kind | Meaning |
|------|---------|
| `calls` | Function calls another function |
| `uses` | `use` import statement |
| `implements` | `impl Trait for Type` |
| `contains` | Structural nesting (module contains struct, struct contains field) |
| `type_ref` | Type referenced in return type, parameter, or field |
| `inherits` | Supertrait bound (`trait Child: Base`) |

## Supported Languages

- **Rust** (via `tree-sitter-rust`)

The core is language-agnostic. Adding a new language requires implementing the `LanguageExtractor` trait with a tree-sitter grammar.

## Design Principles

- **Structural, not semantic** -- tree-sitter parses syntax, not types. Call resolution is name-based. No type inference, no cross-crate resolution.
- **Optimized for LLMs** -- minimal tokens, deterministic IDs, flat JSON. Designed for agent traversal, not human reading.
- **Graceful degradation** -- partial parses extract what they can. Failing files are skipped with a warning. Unresolved cross-file references are silently dropped.

## Development

```bash
cargo build          # Build
cargo test           # Run all tests (39 unit + integration)
cargo clippy         # Lint
cargo fmt            # Format
cargo run -- src/    # Run on own source
```

## License

MIT
