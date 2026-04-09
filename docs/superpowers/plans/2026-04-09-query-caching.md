# Query Caching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two caching layers to grapha CLI — query result cache (instant repeated queries) and graph binary cache (faster graph loading for all commands).

**Architecture:** Two independent cache files in `.grapha/`: a result cache (`query_cache.bin`) keyed by `(command_string, grapha.db_mtime)` for instant repeated queries, and a graph binary cache (`graph.bincode`) that stores the deserialized `Graph` as bincode, skipping per-row SQLite+JSON deserialization on load. Both invalidate when `grapha.db` modification time changes.

**Tech Stack:** Rust, bincode (already a dependency), rusqlite (existing), serde (existing)

---

## File Structure

| File | Responsibility |
|------|----------------|
| Create: `grapha/src/cache.rs` | Cache primitives: `QueryCache` (result cache) + `GraphCache` (binary graph cache), freshness checks via mtime |
| Modify: `grapha/src/store.rs` | Add `pub mod cache;` (if placed as submodule) — actually, cache is independent of store, so top-level module |
| Modify: `grapha/src/main.rs` | Add `mod cache;`, wire `load_graph`/`load_graph_for_l10n` through `GraphCache`, wrap query commands with `QueryCache` |

---

### Task 1: Cache freshness primitive

**Files:**
- Create: `grapha/src/cache.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test for mtime-based freshness check**

```rust
// In grapha/src/cache.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cache_is_stale_when_source_is_newer() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("grapha.db");
        let cache = dir.path().join("cache.bin");

        fs::write(&source, b"source").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&cache, b"cached").unwrap();

        // cache is newer → fresh
        assert!(cache_is_fresh(&source, &cache));

        // touch source to make it newer
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&source, b"source2").unwrap();

        // source is newer → stale
        assert!(!cache_is_fresh(&source, &cache));
    }

    #[test]
    fn cache_is_stale_when_cache_missing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("grapha.db");
        let cache = dir.path().join("nonexistent.bin");
        fs::write(&source, b"source").unwrap();

        assert!(!cache_is_fresh(&source, &cache));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p grapha -- cache_is_stale`
Expected: FAIL — module `cache` doesn't exist

- [ ] **Step 3: Implement `cache_is_fresh`**

```rust
// grapha/src/cache.rs
use std::path::Path;

/// Returns true if `cache_path` exists and is newer than `source_path`.
pub fn cache_is_fresh(source_path: &Path, cache_path: &Path) -> bool {
    let Ok(source_meta) = source_path.metadata() else {
        return false;
    };
    let Ok(cache_meta) = cache_path.metadata() else {
        return false;
    };
    let Ok(source_mtime) = source_meta.modified() else {
        return false;
    };
    let Ok(cache_mtime) = cache_meta.modified() else {
        return false;
    };
    cache_mtime >= source_mtime
}
```

Add `mod cache;` to `grapha/src/main.rs` (near other mod declarations).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p grapha -- cache_is_stale`
Expected: PASS (both tests)

- [ ] **Step 5: Commit**

```bash
git add grapha/src/cache.rs grapha/src/main.rs
git commit -m "feat(cache): add mtime-based cache freshness check"
```

---

### Task 2: Graph binary cache (Option B)

**Files:**
- Modify: `grapha/src/cache.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test for graph binary cache round-trip**

```rust
// Add to grapha/src/cache.rs tests
#[test]
fn graph_cache_round_trips() {
    use grapha_core::graph::*;
    use std::collections::HashMap;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("grapha.db");
    let cache_dir = dir.path();

    // Create a fake db so mtime check works
    fs::write(&db_path, b"db").unwrap();

    let graph = Graph {
        version: "0.1.0".to_string(),
        nodes: vec![Node {
            id: "a::b".to_string(),
            kind: NodeKind::Function,
            name: "b".to_string(),
            file: "a.rs".into(),
            span: Span { start: [1, 0], end: [5, 1] },
            visibility: Visibility::Public,
            metadata: HashMap::from([("k".into(), "v".into())]),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        }],
        edges: vec![],
    };

    let cache = GraphCache::new(cache_dir);
    cache.save(&graph).unwrap();
    assert!(cache.is_fresh(&db_path));

    let loaded = cache.load().unwrap();
    assert_eq!(loaded.nodes.len(), 1);
    assert_eq!(loaded.nodes[0].id, "a::b");
    assert_eq!(loaded.nodes[0].metadata.get("k").map(|s| s.as_str()), Some("v"));
}

#[test]
fn graph_cache_returns_none_when_stale() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("grapha.db");

    let cache = GraphCache::new(dir.path());

    // No cache file yet
    assert!(!cache.is_fresh(&db_path));
    assert!(cache.load().is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p grapha -- graph_cache`
Expected: FAIL — `GraphCache` not defined

- [ ] **Step 3: Implement `GraphCache`**

```rust
// Add to grapha/src/cache.rs
use grapha_core::graph::Graph;

const GRAPH_CACHE_FILENAME: &str = "graph.bincode";

pub struct GraphCache {
    cache_path: std::path::PathBuf,
}

impl GraphCache {
    pub fn new(store_dir: &Path) -> Self {
        Self {
            cache_path: store_dir.join(GRAPH_CACHE_FILENAME),
        }
    }

    pub fn is_fresh(&self, db_path: &Path) -> bool {
        cache_is_fresh(db_path, &self.cache_path)
    }

    pub fn load(&self) -> anyhow::Result<Graph> {
        let bytes = std::fs::read(&self.cache_path)
            .map_err(|e| anyhow::anyhow!("failed to read graph cache: {e}"))?;
        let graph: Graph = bincode::deserialize(&bytes)
            .map_err(|e| anyhow::anyhow!("failed to deserialize graph cache: {e}"))?;
        Ok(graph)
    }

    pub fn save(&self, graph: &Graph) -> anyhow::Result<()> {
        let bytes = bincode::serialize(graph)
            .map_err(|e| anyhow::anyhow!("failed to serialize graph cache: {e}"))?;
        std::fs::write(&self.cache_path, bytes)
            .map_err(|e| anyhow::anyhow!("failed to write graph cache: {e}"))?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p grapha -- graph_cache`
Expected: PASS (both tests)

- [ ] **Step 5: Commit**

```bash
git add grapha/src/cache.rs
git commit -m "feat(cache): add graph binary cache with bincode serialization"
```

---

### Task 3: Wire graph cache into `load_graph`

**Files:**
- Modify: `grapha/src/main.rs`

- [ ] **Step 1: Replace `load_graph` to try cache first, fall back to SQLite, then populate cache**

```rust
fn load_graph(path: &Path) -> anyhow::Result<grapha_core::graph::Graph> {
    let store_dir = path.join(".grapha");
    let db_path = store_dir.join("grapha.db");

    let graph_cache = cache::GraphCache::new(&store_dir);
    if graph_cache.is_fresh(&db_path) {
        if let Ok(graph) = graph_cache.load() {
            return Ok(graph);
        }
    }

    let s = store::sqlite::SqliteStore::new(db_path);
    let graph = s
        .load()
        .context("no index found — run `grapha index` first")?;

    // Best-effort cache write — don't fail the command if caching fails
    let _ = graph_cache.save(&graph);

    Ok(graph)
}
```

- [ ] **Step 2: Run full test suite to verify nothing breaks**

Run: `cargo test`
Expected: all tests PASS

- [ ] **Step 3: Commit**

```bash
git add grapha/src/main.rs
git commit -m "feat(cache): wire graph binary cache into load_graph"
```

---

### Task 4: Query result cache (Option A)

**Files:**
- Modify: `grapha/src/cache.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test for query result cache**

```rust
// Add to grapha/src/cache.rs tests
#[test]
fn query_cache_hit_returns_cached_output() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("grapha.db");
    fs::write(&db_path, b"db").unwrap();

    let cache = QueryCache::new(dir.path());
    let key = "l10n usages Tournament --format tree";

    assert!(cache.get(key, &db_path).is_none());

    cache.put(key, &db_path, "cached output").unwrap();
    let hit = cache.get(key, &db_path);
    assert_eq!(hit.as_deref(), Some("cached output"));
}

#[test]
fn query_cache_miss_when_db_changes() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("grapha.db");
    fs::write(&db_path, b"db").unwrap();

    let cache = QueryCache::new(dir.path());
    let key = "l10n usages Tournament";

    cache.put(key, &db_path, "old output").unwrap();

    // Simulate re-index: db mtime changes
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(&db_path, b"db2").unwrap();

    assert!(cache.get(key, &db_path).is_none());
}

#[test]
fn query_cache_different_keys_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("grapha.db");
    fs::write(&db_path, b"db").unwrap();

    let cache = QueryCache::new(dir.path());

    cache.put("query_a", &db_path, "output_a").unwrap();
    cache.put("query_b", &db_path, "output_b").unwrap();

    assert_eq!(cache.get("query_a", &db_path).as_deref(), Some("output_a"));
    assert_eq!(cache.get("query_b", &db_path).as_deref(), Some("output_b"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p grapha -- query_cache`
Expected: FAIL — `QueryCache` not defined

- [ ] **Step 3: Implement `QueryCache`**

The cache stores entries in a single bincode file (`query_cache.bin`) as a `HashMap<String, QueryCacheEntry>`. Each entry records the db mtime at write time and the output string. On `get`, we check that the current db mtime matches the stored mtime.

```rust
// Add to grapha/src/cache.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

const QUERY_CACHE_FILENAME: &str = "query_cache.bin";
const MAX_QUERY_CACHE_ENTRIES: usize = 64;

#[derive(Serialize, Deserialize)]
struct QueryCacheEntry {
    db_mtime_secs: u64,
    output: String,
}

pub struct QueryCache {
    cache_path: std::path::PathBuf,
}

impl QueryCache {
    pub fn new(store_dir: &Path) -> Self {
        Self {
            cache_path: store_dir.join(QUERY_CACHE_FILENAME),
        }
    }

    pub fn get(&self, key: &str, db_path: &Path) -> Option<String> {
        let current_mtime = mtime_secs(db_path)?;
        let entries = self.load_entries().ok()?;
        let entry = entries.get(key)?;
        (entry.db_mtime_secs == current_mtime).then(|| entry.output.clone())
    }

    pub fn put(&self, key: &str, db_path: &Path, output: &str) -> anyhow::Result<()> {
        let Some(db_mtime_secs) = mtime_secs(db_path) else {
            return Ok(());
        };
        let mut entries = self.load_entries().unwrap_or_default();

        // Evict stale entries (different mtime) and enforce size limit
        entries.retain(|_, entry| entry.db_mtime_secs == db_mtime_secs);
        if entries.len() >= MAX_QUERY_CACHE_ENTRIES {
            // Drop oldest by just clearing — simple eviction
            entries.clear();
        }

        entries.insert(
            key.to_string(),
            QueryCacheEntry {
                db_mtime_secs,
                output: output.to_string(),
            },
        );
        let bytes = bincode::serialize(&entries)?;
        std::fs::write(&self.cache_path, bytes)?;
        Ok(())
    }

    fn load_entries(&self) -> anyhow::Result<HashMap<String, QueryCacheEntry>> {
        let bytes = std::fs::read(&self.cache_path)?;
        Ok(bincode::deserialize(&bytes)?)
    }
}

fn mtime_secs(path: &Path) -> Option<u64> {
    let meta = path.metadata().ok()?;
    let mtime = meta.modified().ok()?;
    Some(
        mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs(),
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p grapha -- query_cache`
Expected: PASS (all three tests)

- [ ] **Step 5: Commit**

```bash
git add grapha/src/cache.rs
git commit -m "feat(cache): add query result cache with mtime-based invalidation"
```

---

### Task 5: Wire query cache into l10n commands

**Files:**
- Modify: `grapha/src/main.rs`

- [ ] **Step 1: Add a helper to build a query cache key from command args**

```rust
// In grapha/src/main.rs, near the load_graph functions

fn query_cache_key(parts: &[&str]) -> String {
    parts.join("\0")
}
```

- [ ] **Step 2: Wire query cache into `handle_l10n_command` for Usages**

Replace the Usages arm in `handle_l10n_command`:

```rust
L10nCommands::Usages {
    key,
    table,
    path,
    format,
    fields,
} => {
    let store_dir = path.join(".grapha");
    let db_path = store_dir.join("grapha.db");
    let qcache = cache::QueryCache::new(&store_dir);
    let cache_key = query_cache_key(&[
        "l10n",
        "usages",
        &key,
        table.as_deref().unwrap_or(""),
        &format!("{format:?}"),
        fields.as_deref().unwrap_or(""),
    ]);

    if let Some(cached) = qcache.get(&cache_key, &db_path) {
        print!("{cached}");
        return Ok(());
    }

    let render_options = render_options.with_fields(resolve_field_set(&fields, &path));
    let graph = load_graph_for_l10n(&path)?;
    let catalogs = localization::load_catalog_index(&path)?;
    let result = query::usages::query_usages(&graph, &catalogs, &key, table.as_deref());

    let output = match format {
        QueryOutputFormat::Json => serde_json::to_string_pretty(&result)?,
        QueryOutputFormat::Tree => render::render_usages_with_options(&result, render_options),
    };
    print!("{output}");
    let _ = qcache.put(&cache_key, &db_path, &output);
    Ok(())
}
```

- [ ] **Step 3: Wire query cache into `handle_l10n_command` for Symbol**

Apply the same pattern to the Symbol arm:

```rust
L10nCommands::Symbol {
    symbol,
    path,
    format,
    fields,
} => {
    let store_dir = path.join(".grapha");
    let db_path = store_dir.join("grapha.db");
    let qcache = cache::QueryCache::new(&store_dir);
    let cache_key = query_cache_key(&[
        "l10n",
        "symbol",
        &symbol,
        &format!("{format:?}"),
        fields.as_deref().unwrap_or(""),
    ]);

    if let Some(cached) = qcache.get(&cache_key, &db_path) {
        print!("{cached}");
        return Ok(());
    }

    let render_options = render_options.with_fields(resolve_field_set(&fields, &path));
    let graph = load_graph_for_l10n(&path)?;
    let catalogs = localization::load_catalog_index(&path)?;
    let result = resolve_query_result(
        query::localize::query_localize(&graph, &catalogs, &symbol),
        "symbol",
    )?;
    let output = match format {
        QueryOutputFormat::Json => serde_json::to_string_pretty(&result)?,
        QueryOutputFormat::Tree => {
            render::render_localize_with_options(&result, render_options)
        }
    };
    print!("{output}");
    let _ = qcache.put(&cache_key, &db_path, &output);
    Ok(())
}
```

- [ ] **Step 4: Run full test suite**

Run: `cargo test`
Expected: all tests PASS

- [ ] **Step 5: Run clippy**

Run: `cargo clippy`
Expected: no warnings

- [ ] **Step 6: Commit**

```bash
git add grapha/src/main.rs
git commit -m "feat(cache): wire query result cache into l10n commands"
```

---

### Task 6: Invalidate caches on `grapha index`

**Files:**
- Modify: `grapha/src/cache.rs`
- Modify: `grapha/src/main.rs`

- [ ] **Step 1: Add `invalidate` method to both caches**

```rust
// In grapha/src/cache.rs
impl GraphCache {
    pub fn invalidate(&self) {
        let _ = std::fs::remove_file(&self.cache_path);
    }
}

impl QueryCache {
    pub fn invalidate(&self) {
        let _ = std::fs::remove_file(&self.cache_path);
    }
}
```

- [ ] **Step 2: Call invalidate after `grapha index` completes**

In `main.rs`, find the index command handler (where `store.save` or `store.save_incremental` is called) and add cache invalidation right after the save completes:

```rust
// After the index save completes (after the scope.spawn block joins)
cache::GraphCache::new(&store_dir).invalidate();
cache::QueryCache::new(&store_dir).invalidate();
```

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: all tests PASS

- [ ] **Step 4: Commit**

```bash
git add grapha/src/cache.rs grapha/src/main.rs
git commit -m "feat(cache): invalidate caches on grapha index"
```

---

### Task 7: Wire graph cache into `load_graph_for_l10n` and verify end-to-end

**Files:**
- Modify: `grapha/src/main.rs`
- Modify: `grapha/src/cache.rs`

Note: `load_graph_for_l10n` uses `load_filtered` which returns a subset of the graph. The graph cache stores the FULL graph (from `load_graph`). For l10n, the query result cache (Option A) gives the biggest win. The graph cache helps `load_graph` callers (context, impact, search, etc.). We do NOT cache the filtered graph separately — the query result cache handles the l10n fast path.

- [ ] **Step 1: Verify the integration works end-to-end**

Build a release binary and test manually:

Run: `cargo build --release -p grapha`

Test first run (populates both caches):
```bash
time target/release/grapha l10n usages "Tournament" --format tree -p /path/to/project
```

Test second run (should be instant from query cache):
```bash
time target/release/grapha l10n usages "Tournament" --format tree -p /path/to/project
```

Expected: second run completes in <100ms.

Test different query (graph cache helps, no query cache hit):
```bash
time target/release/grapha context SomeSymbol -p /path/to/project
```

Expected: faster than without graph cache (graph loaded from bincode instead of SQLite).

- [ ] **Step 2: Run clippy and full tests**

Run: `cargo clippy && cargo test`
Expected: clean

- [ ] **Step 3: Commit any final adjustments**

```bash
git add -A
git commit -m "feat(cache): end-to-end verification and cleanup"
```
