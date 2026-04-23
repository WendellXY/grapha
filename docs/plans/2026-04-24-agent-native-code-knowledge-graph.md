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

## Roadmap Status

### Completed

1. **Architecture guard** — done
   Added `grapha repo arch` with `grapha.toml` architecture rules. It detects forbidden dependencies between configured layers/components using existing graph edges, without adding new database tables.

Evidence:

- Config model: `grapha/src/config.rs`
- Checker: `grapha/src/query/arch.rs`
- CLI wiring: `grapha/src/main.rs`, `grapha/src/app/query.rs`
- Public docs: `README.md`
- Tests: config parsing plus architecture matching and violation detection tests

4. **Brief agent output** — done for first narrow slice
   Added compact `--format brief` output for `symbol context` and `repo arch`. Other query commands still keep their existing `json|tree` surface until they receive command-specific brief renderers.

Evidence:

- Context brief CLI: `grapha/src/main.rs`, `grapha/src/app/query.rs`
- Brief renderers: `grapha/src/render.rs`
- Public docs: `README.md`
- Tests: focused renderer tests for context and architecture brief output
- Playground smoke: indexed `/Users/wendell/developer/WeNext/worktree/lama-ludo-ios/refactor/follow-module-swift-6-support` and ran `repo arch --format brief` plus `symbol context --format brief`

5. **Graph quality benchmark** — done for first harness slice
   Added a repeatable ignored integration-test harness with a fixed Rust fixture. It measures impact traversal behavior, architecture violation detection, output-size proxy, and command latency without depending on the external playground project.

Evidence:

- Harness: `grapha/tests/quality_benchmark.rs`
- Fixture: `grapha/tests/fixtures/quality/`
- Run command: `cargo test -p grapha --test quality_benchmark -- --ignored --nocapture`

### Remaining

2. **Persistent history**
   Add an event/history store for commit, build, test, deploy, and incident-like records, linked back to files, modules, and symbols.

3. **Repo-origin metadata**
   Stamp nodes and edges with repo identity for multi-repo graphs so external repos can be queried explicitly without relying on absolute paths.

4. **Brief agent output expansion**
   Extend compact `--format brief` output beyond the first `context`/`arch` slice to additional high-use commands such as `impact`, `trace`, and `smells`.

6. **Optional inferred enrichment**
   Add opt-in LLM or heuristic enrichment for module summaries, ownership, and doc-code links. Store these as inferred metadata with confidence.

7. **Self-maintenance checks**
   Add checks for stale inferred links, orphan entities, missing relations, and inconsistent graph provenance.

## Completed Slice 1: `grapha repo arch`

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
