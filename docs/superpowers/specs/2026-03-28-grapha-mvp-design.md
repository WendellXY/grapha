# Grapha MVP Design Spec

## Overview

Grapha is a lightweight structural abstraction layer that transforms source code into a normalized, graph-based representation optimized for LLM consumption. It uses tree-sitter for fast syntax parsing to extract symbols and relationships, compressing them into a navigable node graph.

The MVP targets **Rust** as the first supported language, with a language-agnostic core designed for future language support (Swift, etc.).

## Graph Data Model

All types are language-agnostic.

### Node

Represents a symbol extracted from source code.

| Field        | Type                    | Description                                              |
|-------------|-------------------------|----------------------------------------------------------|
| `id`        | `String`                | Unique identifier: `{file_path}::{module_path}::{name}` |
| `kind`      | `NodeKind` enum         | `Function`, `Struct`, `Enum`, `Trait`, `Impl`, `Module`, `Field`, `Variant` |
| `name`      | `String`                | Symbol name                                              |
| `file`      | `PathBuf`               | Source file path (relative)                              |
| `span`      | `Span`                  | Line/column start and end (0-indexed)                    |
| `visibility`| `Visibility` enum       | `Public`, `Crate`, `Private`                             |
| `metadata`  | `HashMap<String,String>`| Extensible bag for language-specific info (e.g., `async`, `unsafe`) |

### Edge

Represents a relationship between two nodes.

| Field    | Type           | Description        |
|---------|----------------|--------------------|
| `source`| `String`       | Source node ID     |
| `target`| `String`       | Target node ID     |
| `kind`  | `EdgeKind` enum| Relationship type  |

**EdgeKind values:** `Calls`, `Uses`, `Implements`, `Contains`, `TypeRef`, `Inherits`

- `Contains` captures structural nesting (module→struct, struct→field, impl→function)

### Graph

Top-level container.

| Field   | Type         |
|---------|-------------|
| `nodes` | `Vec<Node>` |
| `edges` | `Vec<Edge>` |

## Language Extractor Trait

```
trait LanguageExtractor {
    fn language(&self) -> &str;
    fn file_extensions(&self) -> &[&str];
    fn extract(&self, source: &[u8], file_path: &Path) -> Result<ExtractionResult>;
}
```

`ExtractionResult` holds `Vec<Node>` and `Vec<Edge>` — raw extracted data before merging into a `Graph`.

### RustExtractor

Implements `LanguageExtractor` using `tree-sitter-rust`.

**Symbols extracted:** `fn`, `struct`, `enum`, `trait`, `impl`, `mod`, fields, variants.

**Relationships extracted:**
- `Contains` — structural nesting
- `Calls` — function call expressions resolved to identifiers
- `Uses` — `use` statements
- `Implements` — `impl Trait for Type`
- `TypeRef` — types referenced in fields, parameters, return types
- `Inherits` — supertrait bounds (`trait Foo: Bar`)

**Node ID generation:** `{relative_file_path}::{module_path}::{symbol_name}`. For ambiguous cases (e.g., multiple impls), append the impl target type.

**Limitation:** Call resolution is name-based only (no type inference, no cross-crate resolution, no method dispatch). This is intentional — structural, not semantic.

## CLI Interface

```
grapha <path>                     # analyze file or directory, JSON to stdout
grapha <path> -o output.json      # write to file
grapha <path> --filter fn,struct  # filter output to specific node kinds
```

### File Discovery

When `<path>` is a directory:
- Recursively walk using `ignore` crate (gitignore-aware, skips hidden dirs)
- Filter files by extension, matched against registered extractors
- Each file extracted independently, then merged into a single graph

### Graph Merging

- All nodes from all files collected into one graph
- Cross-file edges resolved by matching node IDs across files
- Unresolved references silently dropped (no phantom nodes)

### Output Filtering (`--filter`)

- Comma-separated list of node kinds: `fn`, `struct`, `enum`, `trait`, `impl`, `mod`
- Filters the nodes list, then prunes edges referencing removed nodes
- Applied after graph construction

## JSON Output Format

```json
{
  "version": "0.1.0",
  "nodes": [
    {
      "id": "src/graph.rs::Graph",
      "kind": "struct",
      "name": "Graph",
      "file": "src/graph.rs",
      "span": { "start": [10, 0], "end": [15, 1] },
      "visibility": "public",
      "metadata": {}
    }
  ],
  "edges": [
    {
      "source": "src/main.rs::main",
      "target": "src/graph.rs::Graph::new",
      "kind": "calls"
    }
  ]
}
```

- `version` field for forward compatibility
- `span` uses `[line, column]` pairs, 0-indexed
- `kind` values are lowercase strings in JSON
- `metadata` always present (empty object if nothing extra)
- Pretty-printed to stdout by default, compact when writing to file

## Project Structure

```
src/
  main.rs          — CLI entry point, clap setup
  graph.rs         — Node, Edge, Graph types + serialization
  extract.rs       — LanguageExtractor trait + ExtractionResult
  extract/
    rust.rs        — RustExtractor implementation
  discover.rs      — file discovery, walking directories
  merge.rs         — merge per-file ExtractionResults into Graph, resolve cross-file edges
  filter.rs        — --filter logic, node/edge pruning
```

Module style: `foo.rs` + `foo/` (not `foo/mod.rs`).

## Dependencies

| Crate              | Purpose                          |
|-------------------|----------------------------------|
| `clap` (derive)   | CLI argument parsing             |
| `serde` / `serde_json` | Serialization               |
| `tree-sitter`     | Parsing infrastructure           |
| `tree-sitter-rust`| Rust grammar                     |
| `ignore`          | Gitignore-aware directory walking |
| `anyhow`          | Error handling (CLI layer)       |
| `thiserror`       | Typed errors (library layer)     |

## Error Handling

- `anyhow` at the CLI layer for top-level orchestration
- `thiserror` for typed errors in library code: `ParseError`, `IoError`, `UnsupportedLanguage`
- Errors are per-file, not fatal: if one file fails to parse, report to stderr and continue
- Binary/non-UTF8 files skipped silently during discovery
- Partial parses: extract what we can, skip tree-sitter `ERROR` nodes
- Empty directory produces an empty graph
- Macro-generated code is not visible in the CST — documented limitation
