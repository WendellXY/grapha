# Swift Extraction Hardening

**Date:** 2026-04-02
**Status:** Draft

---

## Motivation

The Swift side already has the right overall architecture: index store first, SwiftSyntax second, tree-sitter fallback third. The current gaps are not about missing a parser strategy; they are about trustworthiness of the boundaries between those strategies.

The highest-value issues are:

- silent bridge and index-store failures that quietly fall through to lower-confidence extraction,
- process-global index-store state that can become wrong for long-lived multi-project runs,
- FFI lifecycle and safety issues around handles and callbacks,
- semantic drift in fallback extraction,
- incomplete Swift developer workflow coverage and documentation.

This design defines a phased improvement program that allows small Swift<->Rust FFI contract changes, but treats no-performance-regression as a hard constraint.

---

## Goals

- Make bridge and index-store failures observable instead of silent.
- Make Swift extraction safe for long-lived, multi-project, and concurrent use.
- Improve semantic parity between index-store, SwiftSyntax, and tree-sitter paths.
- Keep the hot path as fast as it is now, or faster.
- Make bridge-on and bridge-off Swift development reproducible and testable.

## Non-Goals

- No full rewrite of the Swift bridge or extraction waterfall.
- No broad binary format redesign in phase 1.
- No unconditional extra parsing on successful extraction paths.
- No feature expansion unrelated to reliability, correctness, or developer workflow.

---

## Validation Target

Use `/Users/wendell/developer/WeNext/lama-ludo-ios` as the primary real-world regression target.

Why this project:

- it is a mature production iOS codebase,
- it contains `lamaludo.xcodeproj/`,
- it exercises realistic module structure and source volume,
- it is a better performance and stability signal than synthetic fixtures alone.

Validation will happen at two levels:

1. **Unit and fixture coverage inside `grapha-swift`** for exact graph semantics.
2. **Repo-scale smoke and timing checks** against `lama-ludo-ios` for real extraction behavior.

---

## Performance Guardrails

Performance is a release criterion for every phase.

- Reuse the existing timing counters in `grapha-swift/src/lib.rs` as the baseline instrumentation.
- Record before/after timings for index-store, SwiftSyntax, and tree-sitter paths on the same fixture set and on `lama-ludo-ios`.
- Any new diagnostics must be lazy, failure-only, or verbose-gated.
- Any new FFI metadata must be constant-size or near-constant-size on the success path.
- Avoid repeated canonicalization, repeated JSON copying, and unconditional reparsing.
- If a change improves correctness but regresses hot-path performance, the design requires either a cheaper implementation or a gated fallback-only version.

---

## Workstream A: FFI and Index-Store Reliability

### Problems

- Rust-side bridge loading and extraction collapse many failures into `Option::None`.
- Index-store path and handle caching are process-global instead of project- or path-scoped.
- The Swift bridge currently uses pointer-shaped integer handles without an explicit close path.
- Some Swift bridge code force-unwraps C-returned pointers.

### Design

Introduce minimal contract additions across the Swift/Rust boundary:

- explicit open/extract status reporting,
- explicit handle close/destructor support,
- typed Rust-side error categories internally,
- path-scoped store handle caching rather than one global shared handle.

The success path should remain lean:

- no heavy error payload creation unless a failure occurs,
- no additional parse or decode work when extraction succeeds,
- no extra per-file synchronization beyond what is required for safe handle access.

### Expected Outcome

- Wrong-project cache reuse is eliminated.
- Long-lived CLI/server sessions can safely analyze more than one Swift project.
- Bridge failures become diagnosable without forcing users into tree-sitter blindly.

---

## Workstream B: Extraction Correctness and Semantic Parity

### Problems

- Tree-sitter fallback currently conflates superclass inheritance and protocol conformance.
- Some enrichment behavior is stronger on bridge-enabled paths than on pure fallback paths.
- The index-store binary result currently drops imports and only preserves point spans.

### Design

Treat tree-sitter semantics as the correctness baseline for fallback behavior, then bring the higher-confidence paths closer to that validated shape.

Phased approach:

1. restore missing fallback coverage and parity first,
2. fix semantic mistakes such as `Inherits` vs `Implements`,
3. only then extend the binary/index-store path to preserve more metadata.

This ordering prevents the highest-confidence path from being changed before the desired graph semantics are nailed down by tests.

### Expected Outcome

- Fallback extraction produces more correct graph edges.
- Bridge-off mode remains useful instead of being a best-effort emergency path.
- Index-store output becomes richer without guessing what the target semantics should be.

---

## Workstream C: Swift Developer Workflow

### Problems

- Bridge build behavior silently degrades when prerequisites are missing.
- `swift-bridge/Package.swift` currently hardcodes strict environment assumptions.
- The Swift bridge package has no SwiftPM tests today.
- Docs do not fully explain bridge-on, bridge-off, and real-world validation workflows.

### Design

Make the Swift workflow explicit and testable:

- add clear bridge build modes such as `auto`, `off`, and `required`,
- improve build diagnostics in `grapha-swift/build.rs`,
- add SwiftPM tests for the bridge package,
- add Rust-side bridge-on and bridge-off regression coverage,
- document the supported local workflow and the use of `lama-ludo-ios` as a validation target.

### Expected Outcome

- contributors can tell whether they are exercising the bridge or the fallback,
- CI can intentionally cover both modes,
- Swift-side regressions become much harder to hide.

---

## Phase Ordering

### Phase 0: Baseline and Guardrails

- Lock in timing collection and baseline commands.
- Add the `lama-ludo-ios` repo-scale validation commands.
- Identify the exact bridge-on and bridge-off verification matrix.

### Phase 1: Minimal FFI Hardening

- Add explicit close/error/status behavior.
- Replace process-global store assumptions with path-scoped caching.
- Preserve current success-path performance characteristics.

### Phase 2: Correctness Repairs

- Fix fallback semantic bugs.
- Restore enrichment parity where practical without unconditional extra parsing.
- Strengthen tests that run with and without the bridge.

### Phase 3: Workflow and Documentation

- Add SwiftPM tests.
- Make build modes explicit.
- Document the supported Swift workflow and validation steps.

### Phase 4: Richer Index-Store Metadata

- Extend binary/index-store output only after semantics and tests are stable.
- Preserve imports and better spans if that can be done without measurable hot-path regression.

---

## Verification Strategy

### Unit-Level

- `cargo test -p grapha-swift`
- bridge-disabled variant of the same suite
- targeted tests for inheritance semantics, cache scoping, error propagation, and cleanup

### Swift Package-Level

- `swift build` in `grapha-swift/swift-bridge`
- `swift test` in `grapha-swift/swift-bridge`

### Repo-Scale

- run Grapha against `/Users/wendell/developer/WeNext/lama-ludo-ios`
- verify index-store discovery and extraction stability
- compare phase timings before and after changes

### Acceptance Criteria

- no measurable hot-path regression on the `lama-ludo-ios` validation project,
- better failure visibility for bridge and index-store issues,
- improved fallback correctness,
- repeatable bridge-on and bridge-off workflows.

---

## Risks and Controls

- **Risk:** richer error handling adds overhead on successful runs.  
  **Control:** make error materialization failure-only and keep success payloads unchanged where possible.

- **Risk:** cache fixes introduce contention.  
  **Control:** scope caches by path, keep read-mostly access cheap, and avoid coarse global locks.

- **Risk:** semantic fixes change downstream query behavior.  
  **Control:** add focused graph-shape regression tests before changing broader metadata paths.

- **Risk:** real-world validation depends on local Xcode state.  
  **Control:** separate deterministic unit checks from environment-dependent smoke/perf checks.

---

## Out of Scope for This Design

- rewriting the entire bridge into a new transport mechanism,
- adding new language features unrelated to the current Swift reliability/correctness program,
- solving every index-store metadata gap in the first hardening pass.
