# Code Smell Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce the highest-maintenance code smells in Grapha without changing user-visible behavior, starting with the broken Swift index-store config and then splitting the largest multi-responsibility modules into focused units.

**Architecture:** Fix correctness first by replacing the dead process-global index-store toggle with explicit configuration flow through the extraction pipeline. Then perform bounded structural refactors in three areas: Swift tree-sitter enrichment, CLI orchestration, and SQLite persistence. Each phase should preserve behavior with targeted tests before any further decomposition.

**Tech Stack:** Rust workspace, clap, rusqlite, rayon, existing `grapha_core` plugin/extraction pipeline, existing integration/unit tests with `assert_cmd`

---

## File Structure

| File | Responsibility |
|------|----------------|
| Modify: `grapha/src/main.rs` | Current CLI entrypoint, pipeline orchestration, command dispatch, watch-mode server path |
| Modify: `grapha/src/config.rs` | `swift.index_store` configuration source of truth |
| Modify: `grapha-swift/src/lib.rs` | Swift extraction waterfall and index-store discovery |
| Modify: `grapha-swift/src/treesitter.rs` | Current tree-sitter extractor plus all Swift enrichment passes |
| Modify: `grapha/src/store/sqlite.rs` | Current SQLite schema, save/load, incremental sync, compatibility decode |
| Create: `grapha-swift/src/treesitter/` | New focused Swift tree-sitter extraction/enrichment modules |
| Create: `grapha/src/app/` or `grapha/src/commands/` | Extracted CLI orchestration and command handlers |
| Create: `grapha/src/store/sqlite/` | Extracted SQLite schema/read/write helpers |
| Modify: `grapha/tests/integration.rs` | CLI behavior regression coverage |
| Modify: `grapha-swift/tests/*.rs` | Swift extraction/config regression coverage |

---

### Task 1: Fix the dead Swift index-store toggle

**Files:**
- Modify: `grapha/src/main.rs`
- Modify: `grapha/src/config.rs`
- Modify: `grapha-swift/src/lib.rs`
- Modify: `grapha-core` extraction context files if pipeline options must be carried through shared context types
- Test: existing Swift extraction tests or new targeted regression coverage in `grapha-swift/tests/`

- [ ] Add a failing regression test that proves `[swift].index_store = false` prevents index-store extraction and falls back to SwiftSyntax/tree-sitter behavior.
- [ ] Remove the `unsafe { std::env::set_var("GRAPHA_SKIP_INDEX_STORE", "1") }` branch from [main.rs](/Users/wendell/Developer/oops-rs/grapha/grapha/src/main.rs#L484) and replace it with explicit pipeline configuration.
- [ ] Thread an `index_store_enabled` flag through the pipeline into `grapha_swift::extract_swift(...)` so the behavior is controlled by typed inputs rather than process-global state.
- [ ] Update `grapha-swift/src/lib.rs` so `extract_swift` only initializes/uses index store when the flag is enabled.
- [ ] Run targeted Swift tests, then `cargo test -p grapha-swift` and `cargo test -p grapha -- integration`.
- [ ] Commit the fix separately before any structural refactor work.

### Task 2: Split Swift tree-sitter extraction from enrichment passes

**Files:**
- Modify: `grapha-swift/src/treesitter.rs`
- Create: `grapha-swift/src/treesitter/mod.rs`
- Create: `grapha-swift/src/treesitter/extract.rs`
- Create: `grapha-swift/src/treesitter/doc_comments.rs`
- Create: `grapha-swift/src/treesitter/swiftui.rs`
- Create: `grapha-swift/src/treesitter/localization.rs`
- Create: `grapha-swift/src/treesitter/assets.rs`
- Create: `grapha-swift/src/treesitter/common.rs` if shared span/index helpers remain cross-cutting
- Test: existing Swift tree-sitter tests plus new module-focused unit tests

- [ ] Freeze behavior first with targeted tests around doc-comment enrichment, SwiftUI structure enrichment, localization metadata, and asset reference tagging.
- [ ] Keep a thin public facade at `grapha-swift/src/treesitter.rs` or `mod.rs` that re-exports the current public API so call sites stay stable during the split.
- [ ] Move generic parser/shared types (`parse_swift`, `EnrichmentContext`, span helpers, dedup helpers) into `common`/`extract` modules.
- [ ] Move each enrichment concern into its own file with the smallest possible public surface.
- [ ] Eliminate duplicated low-level helpers such as line/byte indexing by consolidating them in one shared helper module.
- [ ] Run `cargo test -p grapha-swift` after each extraction to keep the split behaviorally neutral.
- [ ] Commit the module split once tests pass without changing CLI output.

### Task 3: Extract CLI orchestration out of `main.rs`

**Files:**
- Modify: `grapha/src/main.rs`
- Create: `grapha/src/app/mod.rs` or `grapha/src/commands/mod.rs`
- Create: `grapha/src/app/pipeline.rs`
- Create: `grapha/src/app/index.rs`
- Create: `grapha/src/app/query.rs`
- Create: `grapha/src/app/serve.rs`
- Create: `grapha/src/cli.rs` if clap types are split from runtime behavior
- Test: `grapha/tests/integration.rs`

- [ ] Preserve the existing command-line contract with a smoke test matrix for `analyze`, `index`, `symbol`, `flow`, `l10n`, `asset`, `repo`, and `serve --mcp`.
- [ ] Move `run_pipeline` and indexing orchestration out of `main.rs` into a dedicated runtime module.
- [ ] Move command-specific handlers (`handle_symbol_command`, `handle_flow_command`, `handle_l10n_command`, `handle_asset_command`, `handle_repo_command`) into one or more command modules grouped by behavior rather than by output format.
- [ ] Reduce `main()` to parsing CLI args, building render options, and dispatching into extracted runtime functions.
- [ ] Keep watch-mode logic out of the CLI parsing layer by isolating it behind a `serve`/`watch` runtime module.
- [ ] Run `cargo test -p grapha -- integration` and a manual `cargo run -p grapha -- --help` sanity check.
- [ ] Commit this as a pure structure change with no query semantics changes.

### Task 4: Split SQLite persistence into schema, write, and read paths

**Files:**
- Modify: `grapha/src/store/sqlite.rs`
- Create: `grapha/src/store/sqlite/mod.rs`
- Create: `grapha/src/store/sqlite/schema.rs`
- Create: `grapha/src/store/sqlite/write.rs`
- Create: `grapha/src/store/sqlite/read.rs`
- Create: `grapha/src/store/sqlite/compat.rs`
- Test: move or keep existing `#[cfg(test)]` coverage adjacent to new modules

- [ ] Lock in behavior with focused tests for full save/load, incremental sync, `load_filtered`, and legacy schema compatibility.
- [ ] Extract schema/version constants and table creation helpers into `schema.rs`.
- [ ] Extract full/incremental write paths into `write.rs`, keeping `SqliteStore` as the stable facade type.
- [ ] Extract all load/decode logic into `read.rs` and isolate schema-version-specific edge decoding into `compat.rs`.
- [ ] Replace string-built filtering where possible with parameterized SQL or tightly-scoped helper builders so SQL assembly is explicit and testable.
- [ ] Keep `load_filtered` semantics stable for the localization fast path used in [main.rs:825](/Users/wendell/Developer/oops-rs/grapha/grapha/src/main.rs#L825).
- [ ] Run `cargo test -p grapha sqlite_store` and then full `cargo test`.
- [ ] Commit after the persistence tests and integration tests both pass.

### Task 5: Re-run smell analysis and tighten local thresholds

**Files:**
- Modify: `grapha/src/query/smells.rs` only if threshold tuning or exclusions are needed
- Modify: relevant tests if smell output changes
- Use: generated `.grapha/` index for local verification

- [ ] Re-index the repo with `cargo run -p grapha -- index .`.
- [ ] Run `cargo run -p grapha -- repo smells -p .` and compare the before/after warning set.
- [ ] Confirm the original hotspots are reduced: dead config path removed, `treesitter.rs` split, `main.rs` slimmed down, `sqlite.rs` split.
- [ ] Only adjust smell thresholds if the remaining warnings are demonstrably false positives after the refactors.
- [ ] Capture the remaining warnings in the final PR description so future cleanup has an explicit baseline.

### Task 6: Full verification and integration checkpoint

**Files:**
- No new code expected unless verification exposes regressions

- [ ] Run `cargo fmt -- --check`.
- [ ] Run `cargo clippy --all-targets --all-features`.
- [ ] Run `cargo test`.
- [ ] Run one real-world CLI smoke pass: `cargo run -p grapha -- index .` followed by `cargo run -p grapha -- repo smells -p .`.
- [ ] If all checks pass, prepare either a cleanup PR or continue with any remaining smell-specific follow-up in a new plan rather than extending this refactor indefinitely.

---

## Notes

- The highest-priority fix is Task 1 because it is a correctness issue, not just style.
- Tasks 2 through 4 should be reviewed as structural refactors. Avoid mixing new feature behavior into those commits.
- If the tree-sitter or SQLite splits reveal hidden coupling that would require semantic rewrites, stop and create a follow-up plan rather than forcing the decomposition in one pass.
