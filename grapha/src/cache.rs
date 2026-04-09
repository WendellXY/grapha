use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use grapha_core::graph::Graph;
use serde::{Deserialize, Serialize};

/// Returns `true` if `cache_path` exists and its modification time is
/// greater than or equal to that of `source_path`.
pub fn cache_is_fresh(source_path: &Path, cache_path: &Path) -> bool {
    let mtime = |p: &Path| -> Option<SystemTime> { fs::metadata(p).ok()?.modified().ok() };

    match (mtime(source_path), mtime(cache_path)) {
        (Some(src), Some(cache)) => cache >= src,
        _ => false,
    }
}

/// Binary (bincode) cache for a [`Graph`] stored alongside the SQLite database.
pub struct GraphCache {
    cache_path: PathBuf,
}

impl GraphCache {
    /// The cache file lives at `store_dir/graph.bincode`.
    pub fn new(store_dir: &Path) -> Self {
        Self {
            cache_path: store_dir.join("graph.bincode"),
        }
    }

    /// Returns `true` when the cache file is at least as new as `db_path`.
    pub fn is_fresh(&self, db_path: &Path) -> bool {
        cache_is_fresh(db_path, &self.cache_path)
    }

    /// Deserialise a [`Graph`] from the binary cache file.
    pub fn load(&self) -> anyhow::Result<Graph> {
        let bytes = fs::read(&self.cache_path)
            .with_context(|| format!("reading cache file {}", self.cache_path.display()))?;
        let graph: Graph = bincode::deserialize(&bytes)
            .with_context(|| format!("deserialising cache file {}", self.cache_path.display()))?;
        Ok(graph)
    }

    /// Serialise `graph` to the binary cache file, creating parent directories
    /// if they do not yet exist.
    pub fn save(&self, graph: &Graph) -> anyhow::Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(graph)
            .with_context(|| "serialising graph to bincode".to_string())?;
        fs::write(&self.cache_path, bytes)
            .with_context(|| format!("writing cache file {}", self.cache_path.display()))?;
        Ok(())
    }
}

const QUERY_CACHE_FILENAME: &str = "query_cache.bin";
const MAX_QUERY_CACHE_ENTRIES: usize = 64;

#[derive(Serialize, Deserialize)]
struct QueryCacheEntry {
    db_mtime_secs: u64,
    output: String,
}

/// Cache for serialized query output, keyed by a string and invalidated when
/// the SQLite database changes.
pub struct QueryCache {
    cache_path: PathBuf,
}

fn mtime_secs(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

impl QueryCache {
    /// The cache file lives at `store_dir/query_cache.bin`.
    pub fn new(store_dir: &Path) -> Self {
        Self {
            cache_path: store_dir.join(QUERY_CACHE_FILENAME),
        }
    }

    fn load_entries(&self) -> HashMap<String, QueryCacheEntry> {
        let Ok(bytes) = fs::read(&self.cache_path) else {
            return HashMap::new();
        };
        bincode::deserialize(&bytes).unwrap_or_default()
    }

    /// Returns the cached output for `key` if the cache entry exists and the
    /// database file has not changed since it was written.
    pub fn get(&self, key: &str, db_path: &Path) -> Option<String> {
        let current_mtime = mtime_secs(db_path)?;
        let entries = self.load_entries();
        let entry = entries.get(key)?;
        if entry.db_mtime_secs == current_mtime {
            Some(entry.output.clone())
        } else {
            None
        }
    }

    /// Inserts or updates the cached output for `key`, evicting stale entries
    /// and enforcing the maximum cache size.
    pub fn put(&self, key: &str, db_path: &Path, output: &str) -> anyhow::Result<()> {
        let current_mtime = mtime_secs(db_path).unwrap_or(0);

        let mut entries = self.load_entries();

        // Evict entries that belong to a different db mtime (stale after index).
        entries.retain(|_, v| v.db_mtime_secs == current_mtime);

        // If the cache is full, clear it rather than implementing LRU.
        if entries.len() >= MAX_QUERY_CACHE_ENTRIES {
            entries.clear();
        }

        entries.insert(
            key.to_owned(),
            QueryCacheEntry {
                db_mtime_secs: current_mtime,
                output: output.to_owned(),
            },
        );

        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = bincode::serialize(&entries).context("serialising query cache to bincode")?;
        fs::write(&self.cache_path, bytes)
            .with_context(|| format!("writing query cache {}", self.cache_path.display()))?;
        Ok(())
    }

    /// Removes the cache file so the next query rebuilds it from scratch.
    pub fn invalidate(&self) {
        let _ = fs::remove_file(&self.cache_path);
    }
}

impl GraphCache {
    /// Removes the cache file so the next load rebuilds from SQLite.
    pub fn invalidate(&self) {
        let _ = fs::remove_file(&self.cache_path);
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    // ── cache_is_fresh ────────────────────────────────────────────────────────

    #[test]
    fn cache_is_stale_when_source_is_newer() {
        let dir = tempfile::tempdir().unwrap();

        let source = dir.path().join("source.db");
        let cache = dir.path().join("graph.bincode");

        // Write the cache first so its mtime is older.
        fs::write(&cache, b"old").unwrap();

        // Sleep long enough that the filesystem records a newer mtime for source.
        thread::sleep(Duration::from_millis(10));
        fs::write(&source, b"new source").unwrap();

        // Cache was written before source → stale.
        assert!(!cache_is_fresh(&source, &cache));
    }

    #[test]
    fn cache_is_stale_when_cache_missing() {
        let dir = tempfile::tempdir().unwrap();

        let source = dir.path().join("source.db");
        let cache = dir.path().join("graph.bincode");

        fs::write(&source, b"data").unwrap();
        // cache does not exist → not fresh.

        assert!(!cache_is_fresh(&source, &cache));
    }

    // ── GraphCache ────────────────────────────────────────────────────────────

    #[test]
    fn graph_cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("grapha.db");
        fs::write(&db_path, b"fake db").unwrap();

        let gc = GraphCache::new(dir.path());

        let original = Graph::new();
        gc.save(&original).unwrap();

        // The cache file must exist and be at least as new as the db file.
        assert!(gc.cache_path.exists());
        assert!(gc.is_fresh(&db_path));

        let loaded = gc.load().unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn graph_cache_returns_none_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("grapha.db");
        fs::write(&db_path, b"db").unwrap();

        let gc = GraphCache::new(dir.path());

        // No cache file written → not fresh.
        assert!(!gc.is_fresh(&db_path));

        // load() should return an error because the file doesn't exist.
        assert!(gc.load().is_err());
    }

    // ── QueryCache ────────────────────────────────────────────────────────────

    #[test]
    fn query_cache_hit_returns_cached_output() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("grapha.db");
        fs::write(&db_path, b"db").unwrap();

        let qc = QueryCache::new(dir.path());

        qc.put("my_key", &db_path, "hello world").unwrap();

        let result = qc.get("my_key", &db_path);
        assert_eq!(result.as_deref(), Some("hello world"));
    }

    #[test]
    fn query_cache_miss_when_db_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("grapha.db");
        fs::write(&db_path, b"db v1").unwrap();

        let qc = QueryCache::new(dir.path());
        qc.put("key", &db_path, "cached output").unwrap();

        // Ensure enough time passes for the filesystem mtime to differ.
        thread::sleep(Duration::from_secs(1));
        fs::write(&db_path, b"db v2").unwrap();

        let result = qc.get("key", &db_path);
        assert!(
            result.is_none(),
            "cache should be invalidated after db write"
        );
    }

    #[test]
    fn query_cache_different_keys_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("grapha.db");
        fs::write(&db_path, b"db").unwrap();

        let qc = QueryCache::new(dir.path());

        qc.put("key_a", &db_path, "output_a").unwrap();
        qc.put("key_b", &db_path, "output_b").unwrap();

        assert_eq!(qc.get("key_a", &db_path).as_deref(), Some("output_a"));
        assert_eq!(qc.get("key_b", &db_path).as_deref(), Some("output_b"));
    }
}
