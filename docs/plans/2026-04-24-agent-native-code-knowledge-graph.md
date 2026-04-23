# Agent-Native Code Knowledge Graph Plan

## Source

- Article: https://mp.weixin.qq.com/s/VE8CKfUUj-548DF81ZuLLg
- Core claim: useful agent code understanding needs deterministic structure, cross-domain links, and time-aware history, not only grep, embeddings, or generated documentation.

## First Principles

Facts already true in Grapha:

- Grapha stores a deterministic symbol graph with nodes, edges, confidence scores, provenance, roles, and semantic dataflow effects.
- Swift extraction can use compiler-grade Xcode index store facts; Rust extraction uses tree-sitter with Cargo-aware module context.
- The CLI already exposes search, context, impact, flow, changes, smells, module metrics, concepts, assets, localization, MCP, and watch-mode refresh.
- Persistent storage currently centers on graph snapshots: `nodes`, `edges`, and `meta`.

Constraints:

- Agent-facing commands must stay compact, deterministic, and scriptable.
- New modeling must preserve fast indexing and incremental updates.
- LLM-inferred information, when added, must be marked as lower-confidence or inferred rather than mixed with extracted facts.
- Cross-domain and history features should be optional so normal code indexing remains lightweight.

Invariants:

- Extracted code relationships should remain traceable to source spans or external symbol identifiers.
- Query output must distinguish "known by extraction" from "inferred by heuristic or LLM".
- CI-oriented checks should return stable JSON that can be consumed by agents and shell scripts.
- Configuration should live in `grapha.toml` and be safe to ignore when absent.

## Roadmap

1. **Architecture guard**
   Add `grapha repo arch` with `grapha.toml` architecture rules. Detect forbidden dependencies between configured layers/components using existing graph edges.

2. **Persistent history**
   Add an event/history store for commit, build, test, deploy, and incident-like records, linked back to files, modules, and symbols.

3. **Repo-origin metadata**
   Stamp nodes and edges with repo identity for multi-repo graphs so external repos can be queried explicitly without relying on absolute paths.

4. **Brief agent output**
   Add compact `--format brief` output for high-use commands such as `context`, `impact`, `trace`, `smells`, and `arch`.

5. **Graph quality benchmark**
   Build a repeatable benchmark for impact analysis, call-chain recall, architecture violation detection, query latency, and token cost.

6. **Optional inferred enrichment**
   Add opt-in LLM or heuristic enrichment for module summaries, ownership, and doc-code links. Store these as inferred metadata with confidence.

7. **Self-maintenance checks**
   Add checks for stale inferred links, orphan entities, missing relations, and inconsistent graph provenance.

## Implementation Plan For Slice 1: `grapha repo arch`

Files:

- `grapha/src/config.rs`
- `grapha/src/query/arch.rs`
- `grapha/src/query.rs`
- `grapha/src/main.rs`
- `grapha/src/app/query.rs`
- `README.md`
- Focused unit tests in the changed Rust modules

Acceptance:

- `grapha.toml` can define architecture layers with module/file patterns.
- `grapha.toml` can define denied layer-to-layer dependencies.
- `grapha repo arch` loads the indexed graph and reports matching violations as JSON.
- With no architecture config, `grapha repo arch` returns an empty result instead of failing.
- Tests cover config parsing, rule matching, violation detection, and allowed dependencies.

Initial config shape:

```toml
[[architecture.layers]]
name = "ui"
patterns = ["AppUI*", "Features/*/View*"]

[[architecture.layers]]
name = "infra"
patterns = ["Networking*", "Persistence*"]

[[architecture.deny]]
from = "infra"
to = "ui"
reason = "Infrastructure must not depend on UI."
```

## Non-Goals For Slice 1

- No new database tables.
- No LLM integration.
- No architecture auto-discovery.
- No CI subcommand wrapper beyond stable JSON output.
- No graph schema changes unless rule detection proves impossible without them.
