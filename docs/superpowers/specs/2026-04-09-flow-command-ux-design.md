# Flow Command UX Design

## Goal

Improve `grapha flow entries` and `grapha flow trace` so they are more useful on large SwiftUI-heavy repositories without changing the underlying graph model or dataflow semantics.

## Real Repo Findings

Test playground: `/Users/wendell/developer/WeNext/lama-ludo-ios`

- `grapha flow entries --format tree` currently returns `2985` entry points and nearly three thousand lines of output. The command is fast, but the result is too noisy to orient a user in a large app.
- `grapha flow trace RoomPageCenterContentView --depth 2 --format tree` currently resolves the symbol successfully but returns `flows=0`. This is technically consistent with the current dataflow traversal, but it is not helpful for SwiftUI users because the queried view type is not the node that owns the relevant flow edges.
- The current implementation is literal:
  - `flow entries` lists every entry-point node.
  - `flow trace` traces only from the resolved symbol itself across dataflow edges.

## Constraints

- Keep graph storage and indexing semantics unchanged.
- Do not silently redefine what an entry point is.
- Preserve backward compatibility for JSON consumers as much as possible.
- Improve SwiftUI ergonomics in query and render layers, not by introducing repo-specific heuristics.
- Any fallback behavior must be explicit in the result so users can tell which roots were actually traced.

## Proposed Design

### 1. Add scoped filtering to `flow entries`

Extend `grapha flow entries` with optional filters:

- `--module <MODULE>`
- `--file <PATH_OR_SUFFIX>`
- `--limit <N>`

Behavior:

- Filtering happens after entry detection, not during indexing.
- `--module` reuses existing module names on nodes.
- `--file` reuses the same suffix/repo-relative matching strategy used by `symbol file` and scoped smell queries.
- `--limit` limits returned entries after sorting.

Sorting:

- Sort entries by:
  1. module name
  2. file path
  3. symbol name
  4. id

Output:

- JSON keeps `entries` and `total`.
- Add `shown` to report the number of returned entries after filters and limits.
- Tree output renders only the shown entries and includes a summary like `entries=2985, shown=50`.

This keeps the command honest while making it practical for large repos.

### 2. Add SwiftUI-aware fallback roots to `flow trace`

When `query_trace` resolves a symbol but finds no flows from that symbol directly, run a bounded fallback root discovery step before returning the final result.

Fallback discovery rules:

- If the resolved symbol is a SwiftUI type-like declaration (`struct`, `class`, `property`) or another container-like symbol, inspect its local contains tree.
- Collect candidate fallback roots from directly contained members:
  - `body`
  - action-like functions such as `onTap`, `onShare`, `gotoShare`, `handle*`, `did*`
  - other contained functions that already participate in dataflow edges
- Reuse existing symbol relationships in the in-memory graph only; do not parse source again.
- Trace each candidate root with the existing traversal logic.

Selection rules:

- If one or more fallback roots produce flows, return the combined result.
- If none produce flows, return the original zero-flow result.
- Deduplicate identical flows across fallback roots.

### 3. Make fallback behavior explicit in the trace result

Extend `TraceResult` with metadata describing what was traced:

- `requested_symbol`: the symbol the user asked for
- `traced_roots`: the symbol ids actually used for traversal
- `fallback_used`: boolean

Behavior:

- If the requested symbol itself produced flows, `traced_roots` contains just that symbol and `fallback_used=false`.
- If fallback roots were used, include the original requested symbol plus the traced roots in the response metadata.

Tree output:

- Show a short note when fallback was used, for example:
  - `requested: RoomPageCenterContentView`
  - `traced via: body, onShare()`

This keeps the UX understandable and prevents “hidden magic.”

### 4. Improve empty-flow guidance

When `flow trace` still returns zero flows:

- Tree output should include a small hint line rather than only `flows (0)`.
- Example:
  - `hint: no dataflow edges were found from this symbol or its local SwiftUI roots`

JSON output remains structured and machine-readable; the hint can be an optional string field.

## Implementation Plan Shape

### Query layer

- Extend `query::entries::EntriesResult` to support filtered/limited result metadata.
- Add a filtered `query_entries` variant or new parameterized function for module/file/limit.
- Refactor `query::trace` so traversal from a concrete root is separated from fallback-root discovery and result assembly.
- Reuse existing file matching utilities from `query.rs`.

### CLI layer

- Add new flags to `FlowCommands::Entries`.
- Thread those filters into the query layer.

### Render layer

- Update `render_entries_with_options` to show `shown` vs `total`.
- Update `render_trace_with_options` to render fallback metadata and the empty-flow hint.

## Testing Strategy

Write tests first.

### Entries

- Unit tests for filtered entry selection:
  - module filter
  - file filter
  - limit
- Integration test showing that `flow entries --file <repo-relative path>` returns a focused subset rather than the whole repo.

### Trace

- Unit test where tracing a SwiftUI type produces zero direct flows but contained `body` or `onShare` produces flows.
- Unit test proving fallback does not run when direct tracing already finds flows.
- Unit test proving the zero-flow hint remains when neither the requested symbol nor fallback roots produce flows.
- Integration test covering a SwiftUI-style symbol query and checking that fallback metadata appears in output.

## Non-Goals

- Redefining entry points.
- Changing index-time classification.
- Performing semantic flow inference from arbitrary view trees.
- Adding fuzzy symbol resolution to flow commands in this slice.

## Recommendation

Implement the smaller UX-focused improvement now:

- filterable and limitable `flow entries`
- explicit SwiftUI-aware fallback roots for `flow trace`
- visible metadata/hints in output

This directly addresses the `lama-ludo-ios` pain without destabilizing the graph model or widening the scope into a larger flow-system redesign.
