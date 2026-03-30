# Workspace Split + Swift Bridge Scaffolding — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the single-crate `grapha` into a Cargo workspace (`grapha-core`, `grapha-swift`, `grapha`) and scaffold the Swift bridge dylib with `build.rs` auto-compilation.

**Architecture:** Extract shared types into `grapha-core` (zero heavy deps). Move Swift extraction into `grapha-swift` with tree-sitter fallback + `dlopen` bridge scaffolding. Keep Rust extractor and all pipeline logic in `grapha` (CLI binary crate). Swift bridge dylib built automatically via `build.rs` if Swift toolchain is available.

**Tech Stack:** Rust workspace, Cargo, Swift Package Manager, `@c` attribute (Swift 6.3), `libloading` crate for `dlopen`

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `Cargo.toml` (root) | Workspace definition with 3 members |
| `grapha-core/Cargo.toml` | Shared types crate manifest |
| `grapha-core/src/lib.rs` | Re-exports graph, resolve, extract types |
| `grapha-core/src/graph.rs` | Node, Edge, Graph, enums (moved from src/graph.rs) |
| `grapha-core/src/resolve.rs` | Import, ImportKind (moved from src/resolve.rs) |
| `grapha-core/src/extract.rs` | ExtractionResult, LanguageExtractor trait (moved from src/extract.rs) |
| `grapha-swift/Cargo.toml` | Swift extraction crate manifest |
| `grapha-swift/src/lib.rs` | Public API: `extract_swift()` with waterfall |
| `grapha-swift/src/treesitter.rs` | Tree-sitter fallback (moved from src/extract/swift.rs) |
| `grapha-swift/src/bridge.rs` | `dlopen` loading + FFI function pointer types |
| `grapha-swift/build.rs` | Compiles Swift bridge dylib if toolchain available |
| `grapha-swift/swift-bridge/Package.swift` | Swift Package manifest |
| `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/Bridge.swift` | `@c` exported stub functions |
| `grapha/Cargo.toml` | CLI binary crate manifest (renamed from root) |

### Modified Files

| File | Changes |
|------|---------|
| `grapha/src/main.rs` | Remove `mod graph`, `mod resolve`, `mod extract`; add `use grapha_core::*`; replace `SwiftExtractor` with `grapha_swift::extract_swift` |
| `grapha/src/extract/rust.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/merge.rs` | Change `use crate::extract::*` → `use grapha_core::*` |
| `grapha/src/classify/*.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/compress/*.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/query/*.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/store/*.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/serve.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/changes.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/filter.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| `grapha/src/search.rs` | Change `use crate::graph::*` → `use grapha_core::*` |
| All test files | Update imports |

### Deleted Files (moved to new crates)

| File | Moved to |
|------|----------|
| `src/graph.rs` | `grapha-core/src/graph.rs` |
| `src/resolve.rs` | `grapha-core/src/resolve.rs` |
| `src/extract.rs` | `grapha-core/src/extract.rs` |
| `src/extract/swift.rs` | `grapha-swift/src/treesitter.rs` |

---

## Phase 1: Workspace Split

### Task 1: Create workspace root Cargo.toml and grapha-core crate

**Files:**
- Modify: `Cargo.toml` (root — becomes workspace manifest)
- Create: `grapha-core/Cargo.toml`
- Create: `grapha-core/src/lib.rs`
- Create: `grapha-core/src/graph.rs`
- Create: `grapha-core/src/resolve.rs`
- Create: `grapha-core/src/extract.rs`

- [ ] **Step 1: Create workspace root Cargo.toml**

Replace the current root `Cargo.toml` with a workspace manifest:

```toml
[workspace]
members = ["grapha-core", "grapha-swift", "grapha"]
resolver = "3"
```

- [ ] **Step 2: Create grapha-core/Cargo.toml**

```toml
[package]
name = "grapha-core"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
```

- [ ] **Step 3: Copy graph.rs, resolve.rs to grapha-core/src/**

Copy `src/graph.rs` → `grapha-core/src/graph.rs`. No changes needed — it only depends on `serde`, `HashMap`, `PathBuf`.

Copy `src/resolve.rs` → `grapha-core/src/resolve.rs`. No changes needed.

- [ ] **Step 4: Create grapha-core/src/extract.rs**

This is the trait + result type, adapted from `src/extract.rs` to not reference sub-modules:

```rust
use std::path::Path;

use crate::graph::{Edge, Node};
use crate::resolve::Import;

#[derive(Debug, Clone)]
pub struct ExtractionResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub imports: Vec<Import>,
}

impl ExtractionResult {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            imports: Vec::new(),
        }
    }
}

pub trait LanguageExtractor {
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult>;
}
```

- [ ] **Step 5: Create grapha-core/src/lib.rs**

```rust
pub mod graph;
pub mod resolve;
pub mod extract;

// Re-export commonly used types at crate root for convenience
pub use graph::*;
pub use resolve::*;
pub use extract::{ExtractionResult, LanguageExtractor};
```

- [ ] **Step 6: Verify grapha-core compiles**

Run: `cargo build -p grapha-core`
Expected: Compiles with no errors

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml grapha-core/
git commit -m "feat: create grapha-core crate with shared types"
```

### Task 2: Move CLI crate into grapha/ subdirectory

**Files:**
- Create: `grapha/Cargo.toml`
- Move: `src/` → `grapha/src/`
- Move: `tests/` → `grapha/tests/`
- Move: `tests/fixtures/` → `grapha/tests/fixtures/`

- [ ] **Step 1: Create grapha/Cargo.toml**

```toml
[package]
name = "grapha"
version = "0.1.0"
edition = "2024"

[dependencies]
grapha-core = { path = "../grapha-core" }
anyhow = "1"
clap = { version = "4", features = ["derive"] }
ignore = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tree-sitter = "0.25"
tree-sitter-rust = "0.23"
tree-sitter-swift = "0.7"
git2 = { version = "0.20", default-features = false }
rusqlite = { version = "0.34", features = ["bundled"] }
tantivy = "0.25"
indicatif = "0.17"
toml = "0.8"
regex = "1"
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
tower-http = { version = "0.6", features = ["cors"] }
urlencoding = "2"

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
tempfile = "3"
```

- [ ] **Step 2: Move source and test directories**

```bash
mkdir -p grapha
mv src grapha/src
mv tests grapha/tests
```

- [ ] **Step 3: Delete the old root-level source files that were copied to grapha-core**

```bash
rm grapha/src/graph.rs
rm grapha/src/resolve.rs
```

- [ ] **Step 4: Update grapha/src/main.rs module declarations**

Remove `mod graph;`, `mod resolve;`. Replace with `use grapha_core;`. The `mod extract;` stays but will only contain the Rust extractor (Swift moves in the next task).

Replace the module declarations at the top of `grapha/src/main.rs`:

```rust
mod changes;
mod classify;
mod compress;
mod config;
mod discover;
mod error;
mod extract;
mod filter;
mod merge;
mod module;
mod progress;
mod query;
mod search;
mod serve;
mod store;
```

Remove `mod graph;` and `mod resolve;` (already removed by deleting the files). These types now come from `grapha_core`.

- [ ] **Step 5: Update grapha/src/extract.rs**

Replace the contents with just the Rust extractor (Swift will move to grapha-swift):

```rust
pub mod rust;

// Re-export core types that the Rust extractor uses
pub use grapha_core::extract::{ExtractionResult, LanguageExtractor};
pub use grapha_core::resolve::Import;
```

- [ ] **Step 6: Update all `use crate::graph::*` imports across the codebase**

In every file under `grapha/src/` that imports from `crate::graph`, change to `grapha_core::graph` or `grapha_core`. In every file that imports from `crate::resolve`, change to `grapha_core::resolve`.

Files to update (search for `use crate::graph` and `use crate::resolve`):
- `grapha/src/merge.rs`: `use crate::graph::*` → `use grapha_core::graph::*` and `use crate::extract::ExtractionResult` stays (it re-exports from grapha_core)
- `grapha/src/classify.rs`: `use crate::graph::*` → `use grapha_core::graph::*`
- `grapha/src/classify/pass.rs`: same
- `grapha/src/classify/rust.rs`: same
- `grapha/src/classify/swift.rs`: same
- `grapha/src/classify/toml_rules.rs`: same
- `grapha/src/compress/group.rs`: same
- `grapha/src/compress/prune.rs`: same
- `grapha/src/query.rs`: same
- `grapha/src/query/context.rs`: same
- `grapha/src/query/entries.rs`: same
- `grapha/src/query/impact.rs`: same
- `grapha/src/query/reverse.rs`: same
- `grapha/src/query/trace.rs`: same
- `grapha/src/store.rs`: same
- `grapha/src/store/sqlite.rs`: same
- `grapha/src/store/json.rs`: same (if it imports graph types)
- `grapha/src/serve.rs`: same
- `grapha/src/changes.rs`: same
- `grapha/src/filter.rs`: same
- `grapha/src/search.rs`: same
- `grapha/src/extract/rust.rs`: `use crate::graph::*` → `use grapha_core::graph::*` and `use super::*` stays

Also update test modules inside these files — they often have `use crate::graph::*` in their `#[cfg(test)]` blocks.

- [ ] **Step 7: Update main.rs imports**

Change:
```rust
use extract::LanguageExtractor;
use extract::rust::RustExtractor;
use extract::swift::SwiftExtractor;
use store::Store;
```

To:
```rust
use grapha_core::LanguageExtractor;
use extract::rust::RustExtractor;
use store::Store;
```

Remove `use extract::swift::SwiftExtractor;` — Swift extraction will come from `grapha-swift` after Task 3. For now, keep the Swift extractor in `grapha/src/extract/swift.rs` temporarily so everything compiles.

Actually, to keep this task atomic: keep `grapha/src/extract/swift.rs` for now. We'll move it in Task 3.

Update `grapha/src/extract.rs` to keep swift for now:

```rust
pub mod rust;
pub mod swift; // temporary — moves to grapha-swift in next task

pub use grapha_core::extract::{ExtractionResult, LanguageExtractor};
pub use grapha_core::resolve::Import;
```

- [ ] **Step 8: Verify the workspace compiles**

Run: `cargo build`
Expected: Both `grapha-core` and `grapha` compile

- [ ] **Step 9: Run all tests**

Run: `cargo test`
Expected: All 173 tests pass

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "refactor: move CLI into grapha/ subdirectory, use grapha-core for shared types"
```

### Task 3: Create grapha-swift crate and move Swift extractor

**Files:**
- Create: `grapha-swift/Cargo.toml`
- Create: `grapha-swift/src/lib.rs`
- Move: `grapha/src/extract/swift.rs` → `grapha-swift/src/treesitter.rs`
- Modify: `grapha/src/extract.rs` (remove `pub mod swift;`)
- Modify: `grapha/Cargo.toml` (add `grapha-swift` dependency, remove `tree-sitter-swift`)
- Modify: `grapha/src/main.rs` (use `grapha_swift::SwiftExtractor`)

- [ ] **Step 1: Create grapha-swift/Cargo.toml**

```toml
[package]
name = "grapha-swift"
version = "0.1.0"
edition = "2024"

[dependencies]
grapha-core = { path = "../grapha-core" }
anyhow = "1"
tree-sitter = "0.25"
tree-sitter-swift = "0.7"
regex = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Move swift.rs to grapha-swift/src/treesitter.rs**

```bash
cp grapha/src/extract/swift.rs grapha-swift/src/treesitter.rs
```

- [ ] **Step 3: Update imports in treesitter.rs**

In `grapha-swift/src/treesitter.rs`, replace:
- `use crate::graph::*` → `use grapha_core::graph::*`
- `use super::{ExtractionResult, LanguageExtractor}` → `use grapha_core::extract::{ExtractionResult, LanguageExtractor}`
- In test modules: `use crate::graph::*` → `use grapha_core::graph::*`

- [ ] **Step 4: Create grapha-swift/src/lib.rs**

```rust
mod treesitter;

use std::path::Path;

pub use treesitter::SwiftExtractor;

use grapha_core::extract::ExtractionResult;

/// Extract Swift source code into a graph representation.
///
/// Uses a waterfall strategy:
/// 1. Xcode index store (if available) — compiler-resolved, confidence 1.0
/// 2. SwiftSyntax bridge (if available) — accurate parsing, confidence 0.9
/// 3. tree-sitter-swift (bundled fallback) — fast but limited, confidence 0.6-0.8
pub fn extract_swift(
    source: &[u8],
    file_path: &Path,
    _index_store_path: Option<&Path>,
) -> anyhow::Result<ExtractionResult> {
    // Phase 1: only tree-sitter is implemented
    // Future phases will add index store and SwiftSyntax bridge here
    let extractor = SwiftExtractor;
    use grapha_core::LanguageExtractor;
    extractor.extract(source, file_path)
}
```

- [ ] **Step 5: Verify grapha-swift compiles**

Run: `cargo build -p grapha-swift`
Expected: Compiles

- [ ] **Step 6: Remove swift.rs from grapha crate**

```bash
rm grapha/src/extract/swift.rs
```

Update `grapha/src/extract.rs`:

```rust
pub mod rust;

pub use grapha_core::extract::{ExtractionResult, LanguageExtractor};
pub use grapha_core::resolve::Import;
```

- [ ] **Step 7: Update grapha/Cargo.toml**

Add `grapha-swift` dependency, remove `tree-sitter-swift`:

```toml
[dependencies]
grapha-core = { path = "../grapha-core" }
grapha-swift = { path = "../grapha-swift" }
# ... remove tree-sitter-swift = "0.7"
# Keep tree-sitter and tree-sitter-rust for the Rust extractor
```

- [ ] **Step 8: Update grapha/src/main.rs**

Replace `use extract::swift::SwiftExtractor;` with usage of `grapha_swift`.

In `extractor_for_path`:
```rust
fn extractor_for_path(path: &Path) -> Option<Box<dyn LanguageExtractor>> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "rs" => Some(Box::new(RustExtractor)),
        "swift" => Some(Box::new(grapha_swift::SwiftExtractor)),
        _ => None,
    }
}
```

- [ ] **Step 9: Run all tests**

Run: `cargo test`
Expected: All 173 tests pass (Swift extractor tests are now in grapha-swift crate)

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "refactor: extract Swift extractor into grapha-swift crate"
```

---

## Phase 2: Swift Bridge Scaffolding

### Task 4: Add libloading dependency and bridge module

**Files:**
- Modify: `grapha-swift/Cargo.toml` (add `libloading`)
- Create: `grapha-swift/src/bridge.rs`

- [ ] **Step 1: Add libloading dependency**

In `grapha-swift/Cargo.toml`:

```toml
[dependencies]
# ... existing deps
libloading = "0.8"
```

- [ ] **Step 2: Create bridge.rs with FFI types and dlopen logic**

Create `grapha-swift/src/bridge.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libloading::{Library, Symbol};

/// FFI function signatures matching the Swift bridge's @c exports.
type IndexStoreOpenFn = unsafe extern "C" fn(*const i8) -> *mut std::ffi::c_void;
type IndexStoreExtractFn = unsafe extern "C" fn(*mut std::ffi::c_void, *const i8) -> *const i8;
type IndexStoreCloseFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type SwiftSyntaxExtractFn = unsafe extern "C" fn(*const i8, usize, *const i8) -> *const i8;
type FreeStringFn = unsafe extern "C" fn(*mut i8);

/// Handle to the loaded Swift bridge dylib.
pub struct SwiftBridge {
    _lib: Library,
    pub indexstore_open: IndexStoreOpenFn,
    pub indexstore_extract: IndexStoreExtractFn,
    pub indexstore_close: IndexStoreCloseFn,
    pub swiftsyntax_extract: SwiftSyntaxExtractFn,
    pub free_string: FreeStringFn,
}

static BRIDGE: OnceLock<Option<SwiftBridge>> = OnceLock::new();

impl SwiftBridge {
    /// Try to load the Swift bridge dylib.
    fn load() -> Option<Self> {
        let lib_path = Self::find_dylib()?;
        let lib = unsafe { Library::new(&lib_path) }.ok()?;

        unsafe {
            let indexstore_open: Symbol<IndexStoreOpenFn> =
                lib.get(b"grapha_indexstore_open").ok()?;
            let indexstore_extract: Symbol<IndexStoreExtractFn> =
                lib.get(b"grapha_indexstore_extract").ok()?;
            let indexstore_close: Symbol<IndexStoreCloseFn> =
                lib.get(b"grapha_indexstore_close").ok()?;
            let swiftsyntax_extract: Symbol<SwiftSyntaxExtractFn> =
                lib.get(b"grapha_swiftsyntax_extract").ok()?;
            let free_string: Symbol<FreeStringFn> =
                lib.get(b"grapha_free_string").ok()?;

            Some(SwiftBridge {
                indexstore_open: *indexstore_open,
                indexstore_extract: *indexstore_extract,
                indexstore_close: *indexstore_close,
                swiftsyntax_extract: *swiftsyntax_extract,
                free_string: *free_string,
                _lib: lib,
            })
        }
    }

    fn find_dylib() -> Option<PathBuf> {
        // Check build.rs-provided path first
        let build_path = option_env!("SWIFT_BRIDGE_PATH");
        if let Some(dir) = build_path {
            let dylib = Path::new(dir).join("libGraphaSwiftBridge.dylib");
            if dylib.exists() {
                return Some(dylib);
            }
        }
        None
    }
}

/// Get a reference to the loaded bridge, or None if unavailable.
#[cfg(not(no_swift_bridge))]
pub fn bridge() -> Option<&'static SwiftBridge> {
    BRIDGE.get_or_init(SwiftBridge::load).as_ref()
}

#[cfg(no_swift_bridge)]
pub fn bridge() -> Option<&'static SwiftBridge> {
    None
}
```

- [ ] **Step 3: Register module in lib.rs**

Add `mod bridge;` to `grapha-swift/src/lib.rs`.

- [ ] **Step 4: Verify compiles**

Run: `cargo build -p grapha-swift`
Expected: Compiles (bridge loads nothing yet since dylib doesn't exist)

- [ ] **Step 5: Commit**

```bash
git add grapha-swift/
git commit -m "feat: add Swift bridge dlopen scaffolding"
```

### Task 5: Add build.rs for Swift bridge compilation

**Files:**
- Create: `grapha-swift/build.rs`
- Create: `grapha-swift/swift-bridge/Package.swift`
- Create: `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/Bridge.swift`

- [ ] **Step 1: Create the Swift Package manifest**

Create `grapha-swift/swift-bridge/Package.swift`:

```swift
// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "GraphaSwiftBridge",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "GraphaSwiftBridge", type: .dynamic, targets: ["GraphaSwiftBridge"]),
    ],
    dependencies: [
        .package(url: "https://github.com/swiftlang/swift-syntax.git", from: "601.0.0"),
    ],
    targets: [
        .target(
            name: "GraphaSwiftBridge",
            dependencies: [
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftParser", package: "swift-syntax"),
            ]
        ),
    ]
)
```

- [ ] **Step 2: Create stub Bridge.swift**

Create `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/Bridge.swift`:

```swift
import Foundation

// MARK: - Index Store Functions

@_cdecl("grapha_indexstore_open")
public func indexstoreOpen(_ path: UnsafePointer<CChar>) -> UnsafeMutableRawPointer? {
    // Stub — Phase 3 will implement
    return nil
}

@_cdecl("grapha_indexstore_extract")
public func indexstoreExtract(
    _ handle: UnsafeMutableRawPointer,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    // Stub — Phase 3 will implement
    return nil
}

@_cdecl("grapha_indexstore_close")
public func indexstoreClose(_ handle: UnsafeMutableRawPointer) {
    // Stub — Phase 3 will implement
}

// MARK: - SwiftSyntax Functions

@_cdecl("grapha_swiftsyntax_extract")
public func swiftsyntaxExtract(
    _ source: UnsafePointer<CChar>,
    _ sourceLen: Int,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    // Stub — Phase 4 will implement
    return nil
}

// MARK: - Memory Management

@_cdecl("grapha_free_string")
public func freeString(_ ptr: UnsafeMutablePointer<CChar>) {
    ptr.deallocate()
}
```

Note: Using `@_cdecl` for now since Swift 6.3's `@c` might not be available on all user toolchains yet. `@_cdecl` is supported since Swift 5.x and produces the same C ABI. Can upgrade to `@c` when Swift 6.3 adoption is wider.

- [ ] **Step 3: Create build.rs**

Create `grapha-swift/build.rs`:

```rust
use std::process::Command;

fn main() {
    // Check if Swift toolchain is available
    let swift_version = Command::new("swift").arg("--version").output();
    if swift_version.is_err() {
        println!("cargo:warning=Swift toolchain not found — Swift bridge disabled, using tree-sitter fallback");
        println!("cargo:rustc-cfg=no_swift_bridge");
        return;
    }

    // Build the Swift bridge dylib
    let bridge_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("swift-bridge");
    if !bridge_dir.join("Package.swift").exists() {
        println!("cargo:warning=swift-bridge/Package.swift not found — Swift bridge disabled");
        println!("cargo:rustc-cfg=no_swift_bridge");
        return;
    }

    let status = Command::new("swift")
        .args(["build", "-c", "release"])
        .current_dir(&bridge_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            let lib_path = bridge_dir.join(".build/release");
            println!(
                "cargo:rustc-env=SWIFT_BRIDGE_PATH={}",
                lib_path.display()
            );
            println!("cargo:rerun-if-changed=swift-bridge/Sources/");
            println!("cargo:rerun-if-changed=swift-bridge/Package.swift");
        }
        Ok(s) => {
            println!(
                "cargo:warning=Swift bridge build failed (exit {}), using tree-sitter fallback",
                s
            );
            println!("cargo:rustc-cfg=no_swift_bridge");
        }
        Err(e) => {
            println!(
                "cargo:warning=Swift bridge build error: {e}, using tree-sitter fallback"
            );
            println!("cargo:rustc-cfg=no_swift_bridge");
        }
    }
}
```

- [ ] **Step 4: Add swift-bridge/.build to .gitignore**

Append to root `.gitignore`:

```
grapha-swift/swift-bridge/.build
```

- [ ] **Step 5: Verify full workspace compiles**

Run: `cargo build`
Expected: All three crates compile. Swift bridge builds (or gracefully skips with warning).

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: All 173 tests pass

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: add Swift bridge Package.swift with stub @_cdecl exports and build.rs"
```

### Task 6: Wire waterfall into extract_swift and add integration test

**Files:**
- Modify: `grapha-swift/src/lib.rs`
- Create: `grapha-swift/src/indexstore.rs` (stub)
- Create: `grapha-swift/src/swiftsyntax.rs` (stub)

- [ ] **Step 1: Create stub indexstore.rs**

Create `grapha-swift/src/indexstore.rs`:

```rust
use std::path::Path;

use grapha_core::extract::ExtractionResult;

use crate::bridge;

/// Try to extract Swift symbols from Xcode's index store.
/// Returns None if the bridge isn't available or the file isn't in the index.
pub fn extract_from_indexstore(
    _file_path: &Path,
    _index_store_path: &Path,
) -> Option<ExtractionResult> {
    let _bridge = bridge::bridge()?;
    // Phase 3 will implement: call grapha_indexstore_open + grapha_indexstore_extract
    None
}
```

- [ ] **Step 2: Create stub swiftsyntax.rs**

Create `grapha-swift/src/swiftsyntax.rs`:

```rust
use std::path::Path;

use grapha_core::extract::ExtractionResult;

use crate::bridge;

/// Try to extract Swift symbols using SwiftSyntax via the bridge.
/// Returns None if the bridge isn't available.
pub fn extract_with_swiftsyntax(
    _source: &[u8],
    _file_path: &Path,
) -> Option<ExtractionResult> {
    let _bridge = bridge::bridge()?;
    // Phase 4 will implement: call grapha_swiftsyntax_extract
    None
}
```

- [ ] **Step 3: Update lib.rs with full waterfall**

```rust
mod bridge;
mod indexstore;
mod swiftsyntax;
mod treesitter;

use std::path::Path;

pub use treesitter::SwiftExtractor;

use grapha_core::extract::ExtractionResult;

/// Extract Swift source code into a graph representation.
///
/// Waterfall strategy (strict — best available wins):
/// 1. Xcode index store (if path provided + bridge available) — confidence 1.0
/// 2. SwiftSyntax bridge (if bridge available) — confidence 0.9
/// 3. tree-sitter-swift (bundled fallback) — confidence 0.6-0.8
pub fn extract_swift(
    source: &[u8],
    file_path: &Path,
    index_store_path: Option<&Path>,
) -> anyhow::Result<ExtractionResult> {
    // 1. Try index store
    if let Some(store_path) = index_store_path {
        if let Some(result) = indexstore::extract_from_indexstore(file_path, store_path) {
            return Ok(result);
        }
    }

    // 2. Try SwiftSyntax bridge
    if let Some(result) = swiftsyntax::extract_with_swiftsyntax(source, file_path) {
        return Ok(result);
    }

    // 3. Fall back to tree-sitter
    use grapha_core::LanguageExtractor;
    let extractor = SwiftExtractor;
    extractor.extract(source, file_path)
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All tests pass (waterfall falls through to tree-sitter for everything)

- [ ] **Step 5: Commit**

```bash
git add grapha-swift/
git commit -m "feat: wire waterfall strategy into grapha-swift with stub index-store and SwiftSyntax"
```

### Task 7: Update CLAUDE.md and clean up

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update CLAUDE.md**

Update the Architecture section to reflect the workspace:

```markdown
### Workspace Structure

| Crate | Purpose |
|-------|---------|
| `grapha-core` | Shared types: Node, Edge, Graph, ExtractionResult, LanguageExtractor |
| `grapha-swift` | Swift extraction: index-store → SwiftSyntax → tree-sitter waterfall |
| `grapha` | CLI binary, Rust extractor, pipeline, query engines, web UI |
```

Update the Build & Development Commands:

```markdown
cargo build                    # Build all workspace crates
cargo test                     # Run all tests across workspace
cargo build -p grapha-core     # Build shared types only
cargo build -p grapha-swift    # Build Swift extractor only
cargo run -p grapha -- <cmd>   # Run the CLI
```

Update the Key Modules table to note which crate each module is in.

- [ ] **Step 2: Run final verification**

```bash
cargo build
cargo test
cargo clippy
```

All must pass.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md for workspace structure"
```
