use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use grapha_core::graph::Graph;

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
}
