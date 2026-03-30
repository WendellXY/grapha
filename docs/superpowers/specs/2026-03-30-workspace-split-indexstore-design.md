# Workspace Split + Index Store Integration — Design Spec

**Date:** 2026-03-30
**Goal:** Split grapha into a Cargo workspace with per-language crates, and integrate Xcode's index store + SwiftSyntax for accurate Swift symbol resolution.
**Motivation:** tree-sitter-swift has ~97% parse accuracy but zero type resolution. Xcode's index store provides compiler-grade, fully type-resolved symbol data that's already on disk from normal Xcode builds.

---

## 1. Workspace Structure

```
grapha/
├── Cargo.toml                    # [workspace] members
├── grapha-core/                  # Shared types (Node, Edge, Graph, ExtractionResult)
│   ├── Cargo.toml                # serde, serde_json, anyhow only
│   └── src/
│       ├── lib.rs
│       ├── graph.rs
│       ├── resolve.rs
│       └── extract.rs
│
├── grapha-swift/                 # Swift extraction (all strategies)
│   ├── Cargo.toml                # depends on grapha-core, tree-sitter-swift
│   ├── build.rs                  # compiles Swift bridge dylib if toolchain available
│   ├── src/
│   │   ├── lib.rs                # pub fn extract_swift(...) → ExtractionResult
│   │   ├── bridge.rs             # dlopen + FFI function pointers
│   │   ├── indexstore.rs         # index-store reader (calls bridge)
│   │   ├── swiftsyntax.rs        # SwiftSyntax parser (calls bridge)
│   │   └── treesitter.rs         # tree-sitter fallback (current swift.rs)
│   └── swift-bridge/
│       ├── Package.swift
│       └── Sources/GraphaSwiftBridge/
│           ├── IndexStoreReader.swift
│           ├── SwiftSyntaxExtractor.swift
│           └── Bridge.swift      # @c exported functions
│
├── grapha/                       # CLI binary + pipeline
│   ├── Cargo.toml                # depends on grapha-core, grapha-swift
│   └── src/
│       ├── main.rs
│       ├── extract/rust.rs       # tree-sitter Rust extractor (stays here)
│       ├── merge.rs
│       ├── classify/
│       ├── query/
│       ├── serve.rs
│       └── ...
│
└── docs/
```

### Crate Responsibilities

| Crate | Purpose | Heavy deps |
|-------|---------|-----------|
| `grapha-core` | Shared types: `Node`, `Edge`, `Graph`, `ExtractionResult`, `LanguageExtractor` trait, `Import` | serde, anyhow only |
| `grapha-swift` | All Swift extraction strategies behind one API | tree-sitter-swift, dlopen for bridge |
| `grapha` | CLI, pipeline, Rust extractor, merge, query, serve | everything else |

Future crates follow the same pattern: `grapha-java`, `grapha-kotlin`, `grapha-csharp`.

---

## 2. Swift Bridge FFI Interface

Single dylib (`libGraphaSwiftBridge.dylib`) exposes two function sets via Swift 6.3's `@c` attribute.

### Index Store Functions

```swift
@c(grapha_indexstore_open)
func indexstoreOpen(path: UnsafePointer<CChar>) -> OpaquePointer?

@c(grapha_indexstore_extract)
func indexstoreExtract(handle: OpaquePointer, filePath: UnsafePointer<CChar>) -> UnsafePointer<CChar>?

@c(grapha_indexstore_close)
func indexstoreClose(handle: OpaquePointer)
```

### SwiftSyntax Functions

```swift
@c(grapha_swiftsyntax_extract)
func swiftsyntaxExtract(source: UnsafePointer<CChar>, sourceLen: Int, filePath: UnsafePointer<CChar>) -> UnsafePointer<CChar>?
```

### Memory Management

```swift
@c(grapha_free_string)
func freeString(ptr: UnsafeMutablePointer<CChar>)
```

### JSON Response Format

Both functions return the same JSON shape — identical to `ExtractionResult`:

```json
{
  "nodes": [
    {
      "id": "s:7WebGame0aB7RuntimeC",
      "kind": "struct",
      "name": "WebGameRuntime",
      "file": "WebGameRuntime.swift",
      "span": { "start": [17, 0], "end": [94, 1] },
      "visibility": "public",
      "module": "WebGame"
    }
  ],
  "edges": [
    {
      "source": "s:7WebGame0aB7RuntimeC13bootstrapGamey...",
      "target": "s:7WebGame0aB7RuntimeC17continueSceneLoad...",
      "kind": "calls",
      "confidence": 1.0
    }
  ],
  "imports": [
    { "path": "import Foundation", "symbols": [], "kind": "module" }
  ]
}
```

Index-store version uses USR as node IDs (globally unique, type-resolved). Confidence 1.0 for all edges. Since all edges have fully resolved source/target USRs, the merge step's cross-file resolution is skipped entirely for index-store-sourced results — edges pass through as-is.

SwiftSyntax version uses `file::name` IDs (same as current tree-sitter). Confidence 0.9 for all edges. These go through normal merge resolution.

---

## 3. Waterfall Logic

Strict waterfall — use the best available source exclusively per file.

```
1. Bridge dylib loaded? (dlopen on first call, cached)
   └─ No → skip to step 3

2. Index store path available?
   └─ Yes → call grapha_indexstore_extract(handle, file_path)
        └─ Returns data → ExtractionResult (confidence 1.0)
        └─ Returns null (file not in index) → continue to 2b
   2b. Call grapha_swiftsyntax_extract(source, file_path)
        └─ Returns data → ExtractionResult (confidence 0.9)
        └─ Returns null → continue to step 3

3. tree-sitter-swift (bundled, always works)
   └─ ExtractionResult (confidence 0.6–0.8)
```

### Index Store Path Auto-Discovery

```
1. .grapha/indexstore_path (cached from last successful discovery)
2. ~/Library/Developer/Xcode/DerivedData/<project>-*/Index.noindex/DataStore
3. .build/index/store (SPM index store)
4. Give up → no index store
```

### build.rs Logic

```rust
fn main() {
    // Check Swift toolchain
    if Command::new("swift").arg("--version").output().is_err() {
        println!("cargo:rustc-cfg=no_swift_bridge");
        return;
    }
    // Build bridge dylib
    match Command::new("swift").args(["build", "-c", "release"]).current_dir("swift-bridge").status() {
        Ok(s) if s.success() => {
            println!("cargo:rustc-env=SWIFT_BRIDGE_PATH=swift-bridge/.build/release");
        }
        _ => {
            println!("cargo:rustc-cfg=no_swift_bridge");
        }
    }
}
```

`#[cfg(no_swift_bridge)]` skips `dlopen` entirely → straight to tree-sitter.

---

## 4. Index Store Reader (Swift Side)

Reads Xcode's pre-built index via `libIndexStore.dylib` C API.

### What the Index Contains

| Data | Example | Use |
|------|---------|-----|
| Symbol USR | `s:7WebGame0aB7RuntimeC` | Globally unique node ID |
| Symbol kind | class, instanceMethod, instanceProperty | NodeKind |
| Symbol name | `WebGameRuntime`, `bootstrapGame` | Node name |
| Occurrence roles | def, call, ref, read, write | Edge kind |
| Line/column | L60:C45 | Span |
| Relations | calledBy, containedBy, conformsTo | Edges with type resolution |

### Extraction Logic

```swift
// For each occurrence in the record:
// definition/declaration → emit Node (USR as ID)
// call → emit Calls edge (caller USR → callee USR, confidence 1.0)
// reference + read → emit Calls edge for property access
// containedBy → Contains edge
// conformsTo → Implements edge
// baseOf → Inherits edge
```

### Key Advantage

For `manager.sendMessage()` where `manager: MessageManager`:
- tree-sitter: `call_expression "sendMessage"` → ambiguous
- Index store: `ref call s:7Message0A7ManagerC11sendMessageyyF` → exact, fully resolved

---

## 5. SwiftSyntax Extractor (Swift Side)

For files not in the index store (new, modified, unbuilt).

### What SwiftSyntax Fixes Over Tree-sitter

- `async/await`, `do/catch` → correct AST (not ERROR nodes)
- Result builders (SwiftUI body) → correct block parsing
- `deinit` → recognized
- `if let ... = try? await` → correct AST
- All function calls in closures → found

### Visitor Pattern

```swift
class GraphaVisitor: SyntaxVisitor {
    // Declarations → nodes
    override func visit(_ node: ClassDeclSyntax) -> SyntaxVisitorContinueKind
    override func visit(_ node: StructDeclSyntax) -> SyntaxVisitorContinueKind
    override func visit(_ node: FunctionDeclSyntax) -> SyntaxVisitorContinueKind
    override func visit(_ node: InitializerDeclSyntax) -> SyntaxVisitorContinueKind
    override func visit(_ node: DeinitializerDeclSyntax) -> SyntaxVisitorContinueKind
    override func visit(_ node: VariableDeclSyntax) -> SyntaxVisitorContinueKind

    // Expressions → edges
    override func visit(_ node: FunctionCallExprSyntax) -> SyntaxVisitorContinueKind
    override func visit(_ node: MemberAccessExprSyntax) -> SyntaxVisitorContinueKind
}
```

### Confidence: 0.9

Correct parsing, no type resolution. Same name-based merge as tree-sitter but without parsing errors.

---

## 6. Migration Path

### Phase 1: Workspace Split (no new features)

1. Create workspace `Cargo.toml`
2. Extract `graph.rs`, `resolve.rs`, `extract.rs` (trait) → `grapha-core/`
3. Move `extract/swift.rs` → `grapha-swift/src/treesitter.rs`
4. Keep `extract/rust.rs` in `grapha/`
5. Update imports across codebase
6. All 173 tests pass, same behavior

### Phase 2: Swift Bridge Scaffolding

1. Add `swift-bridge/` Swift Package in `grapha-swift/`
2. Add `build.rs` for compilation
3. Add `bridge.rs` with `dlopen` + function pointers
4. `#[cfg(no_swift_bridge)]` fallback to tree-sitter
5. Still same behavior — bridge compiles but unused

### Phase 3: Index Store Integration

1. Implement `IndexStoreReader.swift`
2. Implement `indexstore.rs`
3. Index store path auto-discovery
4. Wire into waterfall
5. Test: `grapha impact activityGiftConfigs` on lama-ludo-ios shows real results

### Phase 4: SwiftSyntax Integration

1. Implement `SwiftSyntaxExtractor.swift` with visitor
2. Implement `swiftsyntax.rs`
3. Wire into waterfall
4. Test: new/unbuilt Swift files parse correctly

### What Doesn't Change

- Rust extraction (tree-sitter-rust, works great)
- All query engines, web UI, storage, classifiers, CLI interface
- Same `grapha index .` command, better results

---

## 7. Out of Scope

- Java/Kotlin/C# language support (future crates)
- SwiftSyntax type inference (use index store for that)
- Building Swift projects (we read existing build artifacts)
- Linux support for index store (macOS only, degrades to tree-sitter)
