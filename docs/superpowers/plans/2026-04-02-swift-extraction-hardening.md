# Swift Extraction Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the Swift extraction stack so bridge/index-store failures are diagnosable, multi-project extraction is safe, fallback semantics are correct, and the Swift workflow is reproducible without slowing the hot path.

**Architecture:** Keep the current waterfall (`index-store -> SwiftSyntax -> tree-sitter`), but make the boundaries explicit and testable. Phase 1 adds small Swift<->Rust FFI contract extensions (`status` + `close`) and replaces global cache assumptions with scoped caches. Later phases lock semantic parity with tests, extend the binary payload only after correctness is pinned down, and document bridge-on / bridge-off workflows using `lama-ludo-ios` as the real-world validation target.

**Tech Stack:** Rust 2024, Swift 6.3, SwiftPM/XCTest, libloading, libIndexStore, tree-sitter-swift, Xcode DerivedData, `/Users/wendell/developer/WeNext/lama-ludo-ios`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `grapha-swift/swift-bridge/Package.swift` | Modify | Add SwiftPM test target and toolchain-env wiring |
| `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/Bridge.swift` | Modify | Export explicit close/status FFI functions |
| `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/IndexStoreReader.swift` | Modify | Safer reader behavior and richer binary payload |
| `grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/BridgeExportsTests.swift` | Create | Swift-side FFI contract tests |
| `grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/IndexStoreBinaryEncodingTests.swift` | Create | Swift-side binary payload tests |
| `grapha-swift/src/bridge.rs` | Modify | New function pointers, status decoding, bridge diagnostics |
| `grapha-swift/src/bridge/tests.rs` | Create | Rust-side status decoding tests |
| `grapha-swift/src/indexstore.rs` | Modify | Path-scoped handle cache, typed internal errors, FFI cleanup |
| `grapha-swift/src/indexstore/tests.rs` | Create | Cache and lifecycle tests |
| `grapha-swift/src/lib.rs` | Modify | Project-scoped discovery cache and fallback parity |
| `grapha-swift/src/binary.rs` | Modify | Parse imports and full spans from v2 binary payload |
| `grapha-swift/tests/extract_swift_semantic_parity.rs` | Create | Bridge-on / bridge-off parity checks |
| `grapha-swift/tests/fixtures/semantic_parity.swift` | Create | Shared Swift fixture for semantic assertions |
| `grapha-swift/build.rs` | Modify | Explicit bridge build modes and diagnostics |
| `grapha-swift/build_support.rs` | Create | Pure build-mode decision logic for tests |
| `grapha-swift/tests/build_modes.rs` | Create | Build-mode parser/decision tests |
| `README.md` | Modify | Document Swift bridge workflow |
| `docs/swift-developer-workflow.md` | Create | Detailed local workflow and validation matrix |

---

### Task 1: Make the Swift<->Rust FFI Contract Explicit

**Files:**
- Modify: `grapha-swift/swift-bridge/Package.swift`
- Modify: `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/Bridge.swift`
- Modify: `grapha-swift/src/bridge.rs`
- Create: `grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/BridgeExportsTests.swift`
- Create: `grapha-swift/src/bridge/tests.rs`

This task establishes the smallest contract change that unlocks the rest of the plan: explicit `status` reporting for `open`/`extract`, plus an explicit `close` export. Keep the success path fixed-size and allocation-free.

- [ ] **Step 1: Write the failing Swift and Rust contract tests**

In `grapha-swift/swift-bridge/Package.swift`, add a test target:

```swift
        .testTarget(
            name: "GraphaSwiftBridgeTests",
            dependencies: ["GraphaSwiftBridge"]
        ),
```

Create `grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/BridgeExportsTests.swift`:

```swift
import XCTest
@testable import GraphaSwiftBridge

final class BridgeExportsTests: XCTestCase {
    func testIndexStoreOpenReportsOpenFailureStatus() {
        var status: Int32 = -1
        let handle = "/tmp/missing-index-store".withCString { path in
            indexstoreOpen(path, &status)
        }

        XCTAssertNil(handle)
        XCTAssertEqual(status, 1)
    }

    func testIndexStoreExtractRejectsInvalidHandle() {
        var length: UInt32 = 123
        var status: Int32 = -1
        let buffer = "File.swift".withCString { filePath in
            indexstoreExtract(nil, filePath, &length, &status)
        }

        XCTAssertNil(buffer)
        XCTAssertEqual(length, 0)
        XCTAssertEqual(status, 2)
    }

    func testIndexStoreCloseAcceptsNilHandle() {
        indexstoreClose(nil)
    }
}
```

In `grapha-swift/src/bridge.rs`, add at the bottom:

```rust
#[cfg(test)]
mod tests;
```

Create `grapha-swift/src/bridge/tests.rs`:

```rust
use super::IndexStoreStatus;

#[test]
fn decodes_known_status_codes() {
    assert_eq!(IndexStoreStatus::try_from(0).unwrap(), IndexStoreStatus::Ok);
    assert_eq!(IndexStoreStatus::try_from(1).unwrap(), IndexStoreStatus::OpenFailed);
    assert_eq!(IndexStoreStatus::try_from(2).unwrap(), IndexStoreStatus::InvalidHandle);
    assert_eq!(IndexStoreStatus::try_from(3).unwrap(), IndexStoreStatus::ExtractFailed);
}

#[test]
fn rejects_unknown_status_codes() {
    assert!(IndexStoreStatus::try_from(99).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run from repo root:

```bash
swift test --package-path grapha-swift/swift-bridge --filter BridgeExportsTests
cargo test -p grapha-swift bridge::tests
```

Expected:
- Swift test compile failure because `indexstoreOpen` and `indexstoreExtract` do not take `status` pointers yet and `indexstoreClose` does not exist.
- Rust test compile failure because `IndexStoreStatus` does not exist yet.

- [ ] **Step 3: Implement the minimal explicit-status contract**

In `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/Bridge.swift`, replace the index-store exports with:

```swift
import Foundation
import Synchronization

private enum IndexStoreStatus: Int32 {
    case ok = 0
    case openFailed = 1
    case invalidHandle = 2
    case extractFailed = 3
}

private let _readers = Mutex<[Int: IndexStoreReader]>([:])
private let _nextHandle = Atomic<Int>(1)

@c(grapha_indexstore_open)
public func indexstoreOpen(
    _ path: UnsafePointer<CChar>,
    _ outStatus: UnsafeMutablePointer<Int32>?
) -> UnsafeMutableRawPointer? {
    let pathStr = String(cString: path)
    guard let reader = IndexStoreReader(storePath: pathStr) else {
        outStatus?.pointee = IndexStoreStatus.openFailed.rawValue
        return nil
    }

    let handle = _nextHandle.wrappingAdd(1, ordering: .relaxed).oldValue
    _readers.withLock { $0[handle] = reader }
    outStatus?.pointee = IndexStoreStatus.ok.rawValue
    return UnsafeMutableRawPointer(bitPattern: handle)
}

@c(grapha_indexstore_close)
public func indexstoreClose(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    let key = Int(bitPattern: handle)
    _ = _readers.withLock { $0.removeValue(forKey: key) }
}

@c(grapha_indexstore_extract)
public func indexstoreExtract(
    _ handle: UnsafeMutableRawPointer?,
    _ filePath: UnsafePointer<CChar>,
    _ outLen: UnsafeMutablePointer<UInt32>,
    _ outStatus: UnsafeMutablePointer<Int32>?
) -> UnsafeRawPointer? {
    guard let handle else {
        outLen.pointee = 0
        outStatus?.pointee = IndexStoreStatus.invalidHandle.rawValue
        return nil
    }

    let key = Int(bitPattern: handle)
    let reader = _readers.withLock { $0[key] }
    guard let reader else {
        outLen.pointee = 0
        outStatus?.pointee = IndexStoreStatus.invalidHandle.rawValue
        return nil
    }

    let file = String(cString: filePath)
    guard let (ptr, len) = reader.extractFile(file) else {
        outLen.pointee = 0
        outStatus?.pointee = IndexStoreStatus.extractFailed.rawValue
        return nil
    }

    outLen.pointee = len
    outStatus?.pointee = IndexStoreStatus.ok.rawValue
    return UnsafeRawPointer(ptr)
}
```

In `grapha-swift/src/bridge.rs`, update the ABI types and status decoding:

```rust
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libloading::Library;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexStoreStatus {
    Ok = 0,
    OpenFailed = 1,
    InvalidHandle = 2,
    ExtractFailed = 3,
}

impl TryFrom<i32> for IndexStoreStatus {
    type Error = i32;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::OpenFailed),
            2 => Ok(Self::InvalidHandle),
            3 => Ok(Self::ExtractFailed),
            other => Err(other),
        }
    }
}

type IndexStoreOpenFn = unsafe extern "C" fn(*const i8, *mut i32) -> *mut std::ffi::c_void;
type IndexStoreCloseFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type IndexStoreExtractFn = unsafe extern "C" fn(
    *mut std::ffi::c_void,
    *const i8,
    *mut u32,
    *mut i32,
) -> *const u8;

pub struct SwiftBridge {
    _lib: Library,
    pub indexstore_open: IndexStoreOpenFn,
    pub indexstore_close: IndexStoreCloseFn,
    pub indexstore_extract: IndexStoreExtractFn,
    pub swiftsyntax_extract: SwiftSyntaxExtractFn,
    pub free_string: FreeStringFn,
    pub free_buffer: FreeBufferFn,
}
```

Also load `grapha_indexstore_close` in `SwiftBridge::load()`.

- [ ] **Step 4: Run tests to verify the contract is green**

Run:

```bash
swift test --package-path grapha-swift/swift-bridge --filter BridgeExportsTests
cargo test -p grapha-swift bridge::tests
```

Expected: both test groups pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add grapha-swift/swift-bridge/Package.swift \
  grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/Bridge.swift \
  grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/BridgeExportsTests.swift \
  grapha-swift/src/bridge.rs \
  grapha-swift/src/bridge/tests.rs
git commit -m "fix: make swift bridge index-store contract explicit"
```

---

### Task 2: Replace Global Store Assumptions with Scoped Rust Caches

**Files:**
- Modify: `grapha-swift/src/indexstore.rs`
- Modify: `grapha-swift/src/lib.rs`
- Create: `grapha-swift/src/indexstore/tests.rs`

This task makes multi-project and long-lived extraction safe without making the hot path slower. Use path-keyed caches and `Weak` handles so unused readers can close naturally.

- [ ] **Step 1: Write failing cache-scope tests**

At the bottom of `grapha-swift/src/indexstore.rs`, add:

```rust
#[cfg(test)]
mod tests;
```

Create `grapha-swift/src/indexstore/tests.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use super::HandleCache;

#[derive(Debug)]
struct DummyHandle(&'static str);

#[test]
fn caches_handles_per_store_path() {
    let cache = HandleCache::default();

    let a = cache
        .get_or_insert_with(PathBuf::from("/tmp/store-a").as_path(), || Some(DummyHandle("a")))
        .unwrap();
    let b = cache
        .get_or_insert_with(PathBuf::from("/tmp/store-b").as_path(), || Some(DummyHandle("b")))
        .unwrap();

    assert!(!Arc::ptr_eq(&a, &b));
}

#[test]
fn reuses_live_handle_for_same_path() {
    let cache = HandleCache::default();
    let path = PathBuf::from("/tmp/store-a");

    let first = cache.get_or_insert_with(&path, || Some(DummyHandle("first"))).unwrap();
    let second = cache.get_or_insert_with(&path, || Some(DummyHandle("second"))).unwrap();

    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn reopens_after_last_strong_reference_drops() {
    let cache = HandleCache::default();
    let path = PathBuf::from("/tmp/store-a");

    let first = cache.get_or_insert_with(&path, || Some(DummyHandle("first"))).unwrap();
    drop(first);

    let reopened = cache.get_or_insert_with(&path, || Some(DummyHandle("reopened"))).unwrap();
    assert_eq!(reopened.0, "reopened");
}
```

In `grapha-swift/src/lib.rs`, add an inline regression test module:

```rust
#[cfg(test)]
mod discovery_cache_tests {
    use super::project_cache_key;
    use std::path::Path;

    #[test]
    fn normalizes_file_roots_to_parent_directory() {
        assert_eq!(
            project_cache_key(Path::new("/tmp/MyApp/Sources/File.swift")),
            std::path::PathBuf::from("/tmp/MyApp/Sources")
        );
    }

    #[test]
    fn keeps_directory_roots_stable() {
        assert_eq!(
            project_cache_key(Path::new("/tmp/MyApp")),
            std::path::PathBuf::from("/tmp/MyApp")
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p grapha-swift indexstore::tests
cargo test -p grapha-swift discovery_cache_tests
```

Expected: compile failures because `HandleCache` and `project_cache_key` do not exist.

- [ ] **Step 3: Implement path-scoped handle caching and project keys**

In `grapha-swift/src/indexstore.rs`, replace the single `OnceLock<Option<StoreHandle>>` with a reusable helper:

```rust
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use grapha_core::ExtractionResult;

use crate::binary;
use crate::bridge::{self, IndexStoreStatus};

static STORE_CACHE: LazyLock<HandleCache<StoreHandle>> = LazyLock::new(HandleCache::default);

#[derive(Default)]
struct HandleCache<T> {
    entries: Mutex<HashMap<PathBuf, Weak<T>>>,
}

impl<T> HandleCache<T> {
    fn get_or_insert_with<F>(&self, path: &Path, open: F) -> Option<Arc<T>>
    where
        F: FnOnce() -> Option<T>,
    {
        let mut entries = self.entries.lock().ok()?;
        if let Some(existing) = entries.get(path).and_then(Weak::upgrade) {
            return Some(existing);
        }

        let value = Arc::new(open()?);
        entries.insert(path.to_path_buf(), Arc::downgrade(&value));
        Some(value)
    }
}

struct StoreHandle {
    ptr: *mut std::ffi::c_void,
}

unsafe impl Send for StoreHandle {}
unsafe impl Sync for StoreHandle {}

impl Drop for StoreHandle {
    fn drop(&mut self) {
        if let Some(bridge) = bridge::bridge() {
            unsafe { (bridge.indexstore_close)(self.ptr) };
        }
    }
}

fn get_or_open_store(index_store_path: &Path) -> Option<Arc<StoreHandle>> {
    STORE_CACHE.get_or_insert_with(index_store_path, || {
        let bridge = bridge::bridge()?;
        let path_c = CString::new(index_store_path.to_str()?).ok()?;
        let mut status = -1;
        let ptr = unsafe { (bridge.indexstore_open)(path_c.as_ptr(), &mut status) };
        match IndexStoreStatus::try_from(status).ok()? {
            IndexStoreStatus::Ok if !ptr.is_null() => Some(StoreHandle { ptr }),
            _ => None,
        }
    })
}
```

In `grapha-swift/src/lib.rs`, replace the global path lock with a keyed cache:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::Mutex;

static INDEX_STORE_PATHS: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn project_cache_key(project_root: &Path) -> PathBuf {
    if project_root.is_file() {
        project_root
            .parent()
            .unwrap_or(project_root)
            .to_path_buf()
    } else {
        project_root.to_path_buf()
    }
}

pub fn init_index_store(project_root: &Path) {
    let key = project_cache_key(project_root);
    let mut cache = INDEX_STORE_PATHS.lock().expect("index-store cache poisoned");
    cache.entry(key.clone()).or_insert_with(|| {
        discover_index_store(&key).or_else(|| {
            let mut dir = key.parent().map(Path::to_path_buf);
            while let Some(d) = dir {
                if let Some(store) = discover_index_store(&d) {
                    return Some(store);
                }
                dir = d.parent().map(Path::to_path_buf);
            }
            None
        })
    });
}

pub fn index_store_path(project_root: &Path) -> Option<PathBuf> {
    INDEX_STORE_PATHS
        .lock()
        .expect("index-store cache poisoned")
        .get(&project_cache_key(project_root))
        .cloned()
        .flatten()
}
```

- [ ] **Step 4: Wire `extract_swift` to the scoped cache and re-run tests**

In `grapha-swift/src/lib.rs`, replace the current `effective_store` block with:

```rust
    if let Some(root) = project_root {
        init_index_store(root);
    }

    let effective_store = index_store_path
        .map(Path::to_path_buf)
        .or_else(|| project_root.and_then(index_store_path));

    if let Some(store_path) = effective_store.as_deref() {
        let abs_file = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else if let Some(root) = project_root {
            if root.is_file() {
                root.to_path_buf()
            } else {
                root.join(file_path)
            }
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(file_path))
                .unwrap_or_else(|_| file_path.to_path_buf())
        };

        let t_is = Instant::now();
        let is_result = indexstore::extract_from_indexstore(&abs_file, store_path);
        TIMING_INDEXSTORE_NS.fetch_add(t_is.elapsed().as_nanos() as u64, Ordering::Relaxed);

        if let Some(result) = is_result {
            return Ok(result);
        }
    }
```

Run:

```bash
cargo test -p grapha-swift indexstore::tests
cargo test -p grapha-swift discovery_cache_tests
cargo test -p grapha-swift
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add grapha-swift/src/indexstore.rs \
  grapha-swift/src/indexstore/tests.rs \
  grapha-swift/src/lib.rs
git commit -m "fix: scope swift index-store caches by path"
```

---

### Task 3: Lock Semantic Parity Before Changing More Extraction Logic

**Files:**
- Create: `grapha-swift/tests/extract_swift_semantic_parity.rs`
- Create: `grapha-swift/tests/fixtures/semantic_parity.swift`
- Modify: `grapha-swift/src/treesitter.rs`
- Modify: `grapha-swift/src/lib.rs`

This task defines the target graph shape first, then fixes fallback-only semantic drift and enrichment gaps without introducing unconditional extra parsing.

- [ ] **Step 1: Write the failing semantic-parity fixture and tests**

Create `grapha-swift/tests/fixtures/semantic_parity.swift`:

```swift
import SwiftUI

protocol Runnable {}
class Base {}
class Worker: Base, Runnable {}

struct ContentView: View {
    @State private var count = 0

    var doubled: Int {
        count * 2
    }

    var body: some View {
        Text("\(doubled)")
    }
}
```

Create `grapha-swift/tests/extract_swift_semantic_parity.rs`:

```rust
use std::path::Path;

use grapha_core::graph::EdgeKind;
use grapha_swift::extract_swift;

fn fixture() -> &'static [u8] {
    include_bytes!("fixtures/semantic_parity.swift")
}

fn has_edge(result: &grapha_core::ExtractionResult, source: &str, target: &str, kind: EdgeKind) -> bool {
    result
        .edges
        .iter()
        .any(|edge| edge.source == source && edge.target == target && edge.kind == kind)
}

#[test]
fn extract_swift_distinguishes_inherits_from_implements() {
    let result = extract_swift(fixture(), Path::new("semantic_parity.swift"), None, None).unwrap();

    assert!(has_edge(
        &result,
        "semantic_parity.swift::Worker",
        "semantic_parity.swift::Base",
        EdgeKind::Inherits,
    ));
    assert!(has_edge(
        &result,
        "semantic_parity.swift::Worker",
        "semantic_parity.swift::Runnable",
        EdgeKind::Implements,
    ));
}

#[test]
fn extract_swift_marks_dynamic_properties_as_invalidation_sources() {
    let result = extract_swift(fixture(), Path::new("semantic_parity.swift"), None, None).unwrap();

    let count = result
        .nodes
        .iter()
        .find(|node| node.name == "count")
        .expect("missing count property");

    assert_eq!(
        count.metadata.get("swiftui.dynamic_property.wrapper").map(String::as_str),
        Some("state")
    );
    assert_eq!(
        count.metadata.get("swiftui.invalidation_source").map(String::as_str),
        Some("true")
    );
}
```

- [ ] **Step 2: Run tests to verify they fail in current fallback mode**

Run:

```bash
cargo test -p grapha-swift --test extract_swift_semantic_parity
RUSTFLAGS="--cfg no_swift_bridge" cargo test -p grapha-swift --test extract_swift_semantic_parity
```

Expected:
- the inheritance test fails because tree-sitter currently emits `Implements` for every `inheritance_specifier`,
- the invalidation-source test fails in bridge-off mode because fallback does not run the full SwiftUI enrichment path.

- [ ] **Step 3: Fix tree-sitter inheritance semantics**

In `grapha-swift/src/treesitter.rs`, replace the current blanket `Implements` emission with declaration-aware classification:

```rust
fn extract_inheritance_edges(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    type_id: &str,
    result: &mut ExtractionResult,
) {
    let owner_kind = node.kind();
    let mut saw_first_class_parent = false;
    let mut cursor = node.walk();

    for child in node.named_children(&mut cursor) {
        if child.kind() != "inheritance_specifier" {
            continue;
        }

        let Some(inherited_name) =
            find_user_type_name(child, source).or_else(|| type_identifier_text(child, source))
        else {
            continue;
        };

        let edge_kind = match owner_kind {
            "class_declaration" if !saw_first_class_parent => {
                saw_first_class_parent = true;
                EdgeKind::Inherits
            }
            "protocol_declaration" => EdgeKind::Inherits,
            _ => EdgeKind::Implements,
        };

        result.edges.push(Edge {
            source: type_id.to_string(),
            target: make_id(file, module_path, &inherited_name),
            kind: edge_kind,
            confidence: 0.9,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, child, type_id),
        });
    }
}
```

- [ ] **Step 4: Reuse a single tree for fallback enrichments and re-run the tests**

In `grapha-swift/src/lib.rs`, replace the fallback tail with:

```rust
    let t_fb = Instant::now();
    let extractor = SwiftExtractor;
    let mut result = extractor.extract(source, file_path)?;

    let has_swiftui = source_contains_swiftui_markers(source);
    let has_l10n = source_contains_l10n_markers(source);
    let has_assets = source_contains_asset_markers(source);
    let needs_tree = has_swiftui || has_l10n || has_assets;

    if needs_tree {
        let t_parse = Instant::now();
        let tree = treesitter::parse_swift(source)?;
        TIMING_TS_PARSE_NS.fetch_add(t_parse.elapsed().as_nanos() as u64, Ordering::Relaxed);

        let t_enrich = Instant::now();
        if has_swiftui {
            let _ = treesitter::enrich_swiftui_structure_with_tree(source, file_path, &tree, &mut result);
        }
        if has_l10n {
            let _ = treesitter::enrich_localization_metadata_with_tree(source, file_path, &tree, &mut result);
        }
        if has_assets {
            let _ = treesitter::enrich_asset_references_with_tree(source, file_path, &tree, &mut result);
        }
        TIMING_TS_ENRICH_NS.fetch_add(t_enrich.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    TIMING_TS_FALLBACK_NS.fetch_add(t_fb.elapsed().as_nanos() as u64, Ordering::Relaxed);
    Ok(result)
```

Run:

```bash
cargo test -p grapha-swift --test extract_swift_semantic_parity
RUSTFLAGS="--cfg no_swift_bridge" cargo test -p grapha-swift --test extract_swift_semantic_parity
cargo test -p grapha-swift
```

Expected: parity tests and the full `grapha-swift` suite pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add grapha-swift/tests/fixtures/semantic_parity.swift \
  grapha-swift/tests/extract_swift_semantic_parity.rs \
  grapha-swift/src/treesitter.rs \
  grapha-swift/src/lib.rs
git commit -m "fix: restore swift fallback semantic parity"
```

---

### Task 4: Extend the Binary Index-Store Payload for Imports and Full Spans

**Files:**
- Modify: `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/IndexStoreReader.swift`
- Modify: `grapha-swift/src/binary.rs`
- Create: `grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/IndexStoreBinaryEncodingTests.swift`

Do this only after Task 3 is green. The target semantics are now fixed; this task just lets the highest-confidence path preserve them better.

- [ ] **Step 1: Write failing Swift and Rust payload tests for imports and end spans**

Create `grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/IndexStoreBinaryEncodingTests.swift`:

```swift
import XCTest
@testable import GraphaSwiftBridge

final class IndexStoreBinaryEncodingTests: XCTestCase {
    func testBinaryFixtureIncludesImportRecordsAndEndPositions() {
        let (_, length) = makeBinaryFixtureForTests()
        XCTAssertGreaterThan(length, 0)
    }
}
```

In `grapha-swift/src/binary.rs`, add:

```rust
#[test]
fn parses_imports_and_full_spans_from_v2_payload() {
    fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    let string_table = b"usr://demoDemoFile.swiftFoundation";
    let string_offset = 24 + 52 + 12;

    let mut buffer = Vec::new();
    push_u32(&mut buffer, 0x47524148);
    buffer.push(2);
    buffer.extend_from_slice(&[0, 0, 0]);
    push_u32(&mut buffer, 1);
    push_u32(&mut buffer, 0);
    push_u32(&mut buffer, 1);
    push_u32(&mut buffer, string_offset as u32);

    push_u32(&mut buffer, 0);
    push_u32(&mut buffer, 10);
    push_u32(&mut buffer, 10);
    push_u32(&mut buffer, 4);
    push_u32(&mut buffer, 14);
    push_u32(&mut buffer, 9);
    push_u32(&mut buffer, 0xFFFF_FFFF);
    push_u32(&mut buffer, 0);
    push_u32(&mut buffer, 4);
    push_u32(&mut buffer, 2);
    push_u32(&mut buffer, 4);
    push_u32(&mut buffer, 15);
    buffer.push(0);
    buffer.push(0);
    buffer.extend_from_slice(&[0, 0]);

    push_u32(&mut buffer, 23);
    push_u32(&mut buffer, 10);
    buffer.push(2);
    buffer.extend_from_slice(&[0, 0, 0]);

    buffer.extend_from_slice(string_table);

    let result = parse_binary_buffer(&buffer).expect("binary payload should parse");

    assert_eq!(result.imports.len(), 1);
    assert_eq!(result.imports[0].path, "Foundation");
    assert_eq!(result.nodes[0].span.start, [4, 2]);
    assert_eq!(result.nodes[0].span.end, [4, 15]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
swift test --package-path grapha-swift/swift-bridge --filter IndexStoreBinaryEncodingTests
cargo test -p grapha-swift binary::tests::parses_imports_and_full_spans_from_v2_payload
```

Expected: compile failures because `ExtractedImport`, `endLine`, `endCol`, and the new binary shape do not exist yet.

- [ ] **Step 3: Update the Swift binary writer to emit v2 payloads**

In `grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/IndexStoreReader.swift`, extend the packed node and import types:

```swift
private struct ExtractedNode {
    let id: String
    let kind: UInt8
    let name: String
    let file: String
    let line: UInt32
    let col: UInt32
    let endLine: UInt32
    let endCol: UInt32
    let visibility: UInt8
    let module: String?
}

private struct ExtractedImport {
    let path: String
    let kind: UInt8
}

private final class OccCollector: @unchecked Sendable {
    var nodes: [String: ExtractedNode]
    var edges: Set<ExtractedEdge>
    var imports: [ExtractedImport]
    let fileName: String
    let moduleName: String?

    init(fileName: String, moduleName: String?) {
        self.fileName = fileName
        self.moduleName = moduleName
        self.nodes = Dictionary(minimumCapacity: 128)
        self.edges = Set(minimumCapacity: 512)
        self.imports = []
    }
}

private let BINARY_VERSION: UInt8 = 2
private let HEADER_SIZE = 24
private let PACKED_NODE_SIZE = 52
private let PACKED_EDGE_SIZE = 20
private let PACKED_IMPORT_SIZE = 12
```

Then update `buildBinaryBuffer` to accept `imports` and pack them after edges:

```swift
func makeBinaryFixtureForTests() -> (UnsafeMutableRawPointer, UInt32) {
    let node = ExtractedNode(
        id: "usr://demo",
        kind: 0,
        name: "demo",
        file: "File.swift",
        line: 4,
        col: 2,
        endLine: 4,
        endCol: 15,
        visibility: 0,
        module: nil
    )
    let imports = [ExtractedImport(path: "Foundation", kind: 2)]
    let nodes = Dictionary(uniqueKeysWithValues: [(node.id, node)])
    return buildBinaryBuffer(nodes: nodes.values, edges: [], imports: imports)
}

private func buildBinaryBuffer(
    nodes: Dictionary<String, ExtractedNode>.Values,
    edges: Set<ExtractedEdge>,
    imports: [ExtractedImport]
) -> (UnsafeMutableRawPointer, UInt32) {
    var stringTable = Data()
    var stringIndex: [String: (UInt32, UInt32)] = [:]

    func intern(_ value: String) -> (UInt32, UInt32) {
        if let existing = stringIndex[value] { return existing }
        let offset = UInt32(stringTable.count)
        let data = Array(value.utf8)
        stringTable.append(contentsOf: data)
        let entry = (offset, UInt32(data.count))
        stringIndex[value] = entry
        return entry
    }

    let nodeRefs = nodes.map { node in
        (
            id: intern(node.id),
            name: intern(node.name),
            file: intern(node.file),
            module: node.module.map(intern),
            line: node.line,
            col: node.col,
            endLine: node.endLine,
            endCol: node.endCol,
            kind: node.kind,
            visibility: node.visibility
        )
    }
    let edgeRefs = edges.map { edge in
        (source: intern(edge.source), target: intern(edge.target), kind: edge.kind, confidencePct: edge.confidencePct)
    }
    let importRefs = imports.map { entry in
        (path: intern(entry.path), kind: entry.kind)
    }

    let stringTableOffset = UInt32(
        HEADER_SIZE
            + nodeRefs.count * PACKED_NODE_SIZE
            + edgeRefs.count * PACKED_EDGE_SIZE
            + importRefs.count * PACKED_IMPORT_SIZE
    )
    let totalSize = Int(stringTableOffset) + stringTable.count
    let buffer = malloc(totalSize)!
    var offset = 0

    func writeU32(_ value: UInt32) {
        buffer.storeBytes(of: value.littleEndian, toByteOffset: offset, as: UInt32.self)
        offset += 4
    }

    func writeU8(_ value: UInt8) {
        buffer.storeBytes(of: value, toByteOffset: offset, as: UInt8.self)
        offset += 1
    }

    func pad(_ count: Int) {
        memset(buffer.advanced(by: offset), 0, count)
        offset += count
    }

    writeU32(BINARY_MAGIC)
    writeU8(BINARY_VERSION)
    pad(3)
    writeU32(UInt32(nodeRefs.count))
    writeU32(UInt32(edgeRefs.count))
    writeU32(UInt32(importRefs.count))
    writeU32(stringTableOffset)

    for node in nodeRefs {
        writeU32(node.id.0); writeU32(node.id.1)
        writeU32(node.name.0); writeU32(node.name.1)
        writeU32(node.file.0); writeU32(node.file.1)
        if let module = node.module {
            writeU32(module.0); writeU32(module.1)
        } else {
            writeU32(NO_MODULE); writeU32(0)
        }
        writeU32(node.line); writeU32(node.col)
        writeU32(node.endLine); writeU32(node.endCol)
        writeU8(node.kind); writeU8(node.visibility)
        pad(2)
    }

    for edge in edgeRefs {
        writeU32(edge.source.0); writeU32(edge.source.1)
        writeU32(edge.target.0); writeU32(edge.target.1)
        writeU8(edge.kind); writeU8(edge.confidencePct)
        pad(2)
    }

    for entry in importRefs {
        writeU32(entry.path.0); writeU32(entry.path.1)
        writeU8(entry.kind)
        pad(3)
    }

    stringTable.withUnsafeBytes { raw in
        buffer.advanced(by: Int(stringTableOffset)).copyMemory(from: raw.baseAddress!, byteCount: stringTable.count)
    }

    return (buffer, UInt32(totalSize))
}
```

While updating extraction, append `ExtractedImport(path: moduleName, kind: 2)` for each module dependency you want to preserve, and set `endLine` / `endCol` at the point where each `ExtractedNode` is created.

- [ ] **Step 4: Update the Rust parser and re-run all payload tests**

In `grapha-swift/src/binary.rs`, upgrade the parser constants and decode logic:

```rust
const VERSION: u8 = 2;
const HEADER_SIZE: usize = 24;
const PACKED_NODE_SIZE: usize = 52;
const PACKED_EDGE_SIZE: usize = 20;
const PACKED_IMPORT_SIZE: usize = 12;

fn read_import(chunk: &[u8], string_table: &[u8]) -> Option<grapha_core::resolve::Import> {
    let path = read_str(string_table, read_u32(chunk, 0), read_u32(chunk, 4))?.to_string();
    let kind = match chunk[8] {
        2 => grapha_core::resolve::ImportKind::Module,
        0 => grapha_core::resolve::ImportKind::Named,
        1 => grapha_core::resolve::ImportKind::Wildcard,
        3 => grapha_core::resolve::ImportKind::Relative,
        _ => return None,
    };

    Some(grapha_core::resolve::Import {
        path,
        symbols: vec![],
        kind,
    })
}
```

Also update `read_node()` to set:

```rust
        span: Span {
            start: [line, col],
            end: [end_line, end_col],
        },
```

Run:

```bash
swift test --package-path grapha-swift/swift-bridge --filter IndexStoreBinaryEncodingTests
cargo test -p grapha-swift --lib binary
cargo test -p grapha-swift
```

Expected: all payload tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add grapha-swift/swift-bridge/Sources/GraphaSwiftBridge/IndexStoreReader.swift \
  grapha-swift/swift-bridge/Tests/GraphaSwiftBridgeTests/IndexStoreBinaryEncodingTests.swift \
  grapha-swift/src/binary.rs
git commit -m "feat: preserve swift index-store imports and spans"
```

---

### Task 5: Make Bridge Build Modes Explicit and Document the Swift Workflow

**Files:**
- Modify: `grapha-swift/build.rs`
- Create: `grapha-swift/build_support.rs`
- Create: `grapha-swift/tests/build_modes.rs`
- Modify: `README.md`
- Create: `docs/swift-developer-workflow.md`

This task makes it obvious whether contributors are building bridge-on or bridge-off, and it captures the `lama-ludo-ios` validation flow so performance checks do not get lost.

- [ ] **Step 1: Write failing build-mode tests first**

Create `grapha-swift/build_support.rs` with just the type declaration stub:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMode {
    Auto,
    Off,
    Required,
}

pub fn parse_bridge_mode(_raw: Option<&str>) -> Result<BridgeMode, String> {
    todo!()
}
```

Create `grapha-swift/tests/build_modes.rs`:

```rust
#[path = "../build_support.rs"]
mod build_support;

use build_support::{parse_bridge_mode, BridgeMode};

#[test]
fn parses_default_mode_as_auto() {
    assert_eq!(parse_bridge_mode(None).unwrap(), BridgeMode::Auto);
}

#[test]
fn parses_known_modes() {
    assert_eq!(parse_bridge_mode(Some("auto")).unwrap(), BridgeMode::Auto);
    assert_eq!(parse_bridge_mode(Some("off")).unwrap(), BridgeMode::Off);
    assert_eq!(parse_bridge_mode(Some("required")).unwrap(), BridgeMode::Required);
}

#[test]
fn rejects_unknown_modes() {
    assert!(parse_bridge_mode(Some("sometimes")).is_err());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p grapha-swift --test build_modes
```

Expected: test failures because `parse_bridge_mode()` still calls `todo!()`.

- [ ] **Step 3: Implement explicit bridge modes and wire them into `build.rs`**

Replace `grapha-swift/build_support.rs` with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeMode {
    Auto,
    Off,
    Required,
}

pub fn parse_bridge_mode(raw: Option<&str>) -> Result<BridgeMode, String> {
    match raw.unwrap_or("auto") {
        "auto" => Ok(BridgeMode::Auto),
        "off" => Ok(BridgeMode::Off),
        "required" => Ok(BridgeMode::Required),
        other => Err(format!("unsupported GRAPHA_SWIFT_BRIDGE_MODE: {other}")),
    }
}
```

Then update `grapha-swift/build.rs`:

```rust
mod build_support;

use std::process::Command;

use build_support::{parse_bridge_mode, BridgeMode};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(no_swift_bridge)");

    let mode = parse_bridge_mode(std::env::var("GRAPHA_SWIFT_BRIDGE_MODE").ok().as_deref())
        .unwrap_or_else(|err| panic!("{err}"));

    if mode == BridgeMode::Off {
        println!("cargo:warning=Swift bridge mode: off");
        println!("cargo:rustc-cfg=no_swift_bridge");
        return;
    }

    let swift_version = Command::new("swift").arg("--version").output();
    if swift_version.is_err() {
        match mode {
            BridgeMode::Auto => {
                println!("cargo:warning=Swift bridge mode: auto (toolchain unavailable, using fallback)");
                println!("cargo:rustc-cfg=no_swift_bridge");
                return;
            }
            BridgeMode::Required => panic!("Swift bridge mode required, but `swift --version` failed"),
            BridgeMode::Off => unreachable!(),
        }
    }

    let bridge_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("swift-bridge");
    if !bridge_dir.join("Package.swift").exists() {
        match mode {
            BridgeMode::Auto => {
                println!("cargo:warning=Swift bridge mode: auto (missing swift-bridge/Package.swift, using fallback)");
                println!("cargo:rustc-cfg=no_swift_bridge");
                return;
            }
            BridgeMode::Required => panic!("Swift bridge mode required, but swift-bridge/Package.swift is missing"),
            BridgeMode::Off => unreachable!(),
        }
    }

    let status = Command::new("swift")
        .args(["build", "-c", "release"])
        .current_dir(&bridge_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            let lib_path = bridge_dir.join(".build/release");
            println!("cargo:warning=Swift bridge mode: {:?}", mode);
            println!("cargo:rustc-env=SWIFT_BRIDGE_PATH={}", lib_path.display());
            println!("cargo:rerun-if-changed=swift-bridge/Sources/");
            println!("cargo:rerun-if-changed=swift-bridge/Package.swift");
        }
        Ok(s) if mode == BridgeMode::Auto => {
            println!("cargo:warning=Swift bridge mode: auto (swift build failed with {s}, using fallback)");
            println!("cargo:rustc-cfg=no_swift_bridge");
        }
        Ok(s) => panic!("Swift bridge mode required, but swift build failed with {s}"),
        Err(err) if mode == BridgeMode::Auto => {
            println!("cargo:warning=Swift bridge mode: auto (failed to launch swift build: {err}, using fallback)");
            println!("cargo:rustc-cfg=no_swift_bridge");
        }
        Err(err) => panic!("Swift bridge mode required, but failed to launch swift build: {err}"),
    }
}
```

- [ ] **Step 4: Document the local workflow and verify with the real project**

Create `docs/swift-developer-workflow.md`:

```md
# Swift Developer Workflow

## Build Modes

- `GRAPHA_SWIFT_BRIDGE_MODE=auto` - try to build/load the Swift bridge, fall back if unavailable
- `GRAPHA_SWIFT_BRIDGE_MODE=off` - skip Swift bridge build and compile bridge-off intentionally
- `GRAPHA_SWIFT_BRIDGE_MODE=required` - fail the build if the bridge cannot be built or loaded

## Fast Validation

    cargo test -p grapha-swift
    GRAPHA_SWIFT_BRIDGE_MODE=off cargo test -p grapha-swift
    swift test --package-path grapha-swift/swift-bridge

## Real-World Validation

    cargo run -p grapha -- index /Users/wendell/developer/WeNext/lama-ludo-ios --timing
    GRAPHA_SWIFT_BRIDGE_MODE=off cargo run -p grapha -- index /Users/wendell/developer/WeNext/lama-ludo-ios --timing
```

Update `README.md` to link to the new workflow doc from the development section.

Run:

```bash
cargo test -p grapha-swift --test build_modes
CARGO_TARGET_DIR=target/swift-auto cargo build -p grapha-swift
CARGO_TARGET_DIR=target/swift-off GRAPHA_SWIFT_BRIDGE_MODE=off cargo build -p grapha-swift
CARGO_TARGET_DIR=target/swift-required GRAPHA_SWIFT_BRIDGE_MODE=required cargo build -p grapha-swift
cargo run -p grapha -- index /Users/wendell/developer/WeNext/lama-ludo-ios --timing
GRAPHA_SWIFT_BRIDGE_MODE=off cargo run -p grapha -- index /Users/wendell/developer/WeNext/lama-ludo-ios --timing
```

Expected:
- build-mode tests pass,
- the three build invocations clearly show which mode was selected,
- the two `lama-ludo-ios` runs complete and provide comparable timing output.

- [ ] **Step 5: Commit**

Run:

```bash
git add grapha-swift/build.rs \
  grapha-swift/build_support.rs \
  grapha-swift/tests/build_modes.rs \
  README.md \
  docs/swift-developer-workflow.md
git commit -m "docs: codify swift bridge build modes and validation"
```

---

## Final Verification Matrix

Run this full matrix after the last task:

```bash
swift test --package-path grapha-swift/swift-bridge
cargo test -p grapha-swift
GRAPHA_SWIFT_BRIDGE_MODE=off cargo test -p grapha-swift
RUSTFLAGS="--cfg no_swift_bridge" cargo test -p grapha-swift --test extract_swift_semantic_parity
cargo run -p grapha -- index /Users/wendell/developer/WeNext/lama-ludo-ios --timing
GRAPHA_SWIFT_BRIDGE_MODE=off cargo run -p grapha -- index /Users/wendell/developer/WeNext/lama-ludo-ios --timing
```

Expected:

- SwiftPM bridge tests pass.
- Rust Swift tests pass in bridge-on and bridge-off modes.
- The semantic-parity integration test passes even with `no_swift_bridge`.
- `lama-ludo-ios` indexing succeeds in both modes.
- Hot-path timing is no worse than baseline on the bridge-enabled run.
