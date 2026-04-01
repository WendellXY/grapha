use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

const CACHE_FILE: &str = "file_hashes.bin";

/// Per-file content hash for skip-extraction optimization.
#[derive(Debug, Clone)]
pub struct HashCache {
    entries: HashMap<PathBuf, [u8; 32]>,
}

/// Result of comparing current file hashes against the cache.
#[derive(Debug)]
pub struct FileChangeset {
    pub changed: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl HashCache {
    pub fn new(entries: HashMap<PathBuf, [u8; 32]>) -> Self {
        Self { entries }
    }

    pub fn empty() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Load from a binary file: [u32 count] then [u32 path_len, path_bytes, 32-byte hash] per entry.
    pub fn load(store_path: &Path) -> Option<Self> {
        let cache_path = store_path.join(CACHE_FILE);
        let data = fs::read(&cache_path).ok()?;
        let mut cursor = &data[..];

        let count = read_u32(&mut cursor)? as usize;
        let mut entries = HashMap::with_capacity(count);

        for _ in 0..count {
            let path_len = read_u32(&mut cursor)? as usize;
            if cursor.len() < path_len + 32 {
                return None;
            }
            let path = PathBuf::from(String::from_utf8_lossy(&cursor[..path_len]).into_owned());
            cursor = &cursor[path_len..];
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&cursor[..32]);
            cursor = &cursor[32..];
            entries.insert(path, hash);
        }

        Some(Self { entries })
    }

    /// Save to binary format.
    pub fn save(&self, store_path: &Path) -> anyhow::Result<()> {
        let cache_path = store_path.join(CACHE_FILE);
        let mut buf = Vec::with_capacity(4 + self.entries.len() * (4 + 100 + 32));

        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (path, hash) in &self.entries {
            let path_bytes = path.to_string_lossy().as_bytes().to_vec();
            buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(&path_bytes);
            buf.extend_from_slice(hash);
        }

        fs::write(&cache_path, &buf)?;
        Ok(())
    }
}

fn read_u32(cursor: &mut &[u8]) -> Option<u32> {
    if cursor.len() < 4 {
        return None;
    }
    let val = u32::from_le_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]);
    *cursor = &cursor[4..];
    Some(val)
}

/// Compute blake3 hash for a single file.
pub fn hash_file(path: &Path) -> anyhow::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// Compute blake3 hashes for all files in parallel using rayon.
pub fn hash_files_parallel(files: &[PathBuf]) -> HashMap<PathBuf, [u8; 32]> {
    files
        .par_iter()
        .filter_map(|path| {
            let hash = hash_file(path).ok()?;
            Some((path.clone(), hash))
        })
        .collect()
}

/// Compare current file hashes against the cached hashes.
pub fn diff_files(previous: &HashCache, current: &HashMap<PathBuf, [u8; 32]>) -> FileChangeset {
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();

    for (path, hash) in current {
        match previous.entries.get(path) {
            Some(prev_hash) if prev_hash == hash => unchanged.push(path.clone()),
            _ => changed.push(path.clone()),
        }
    }

    let deleted: Vec<PathBuf> = previous
        .entries
        .keys()
        .filter(|path| !current.contains_key(*path))
        .cloned()
        .collect();

    FileChangeset {
        changed,
        unchanged,
        deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn hash_file_produces_consistent_output() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, b"hello world").unwrap();

        let h1 = hash_file(&file_path).unwrap();
        let h2 = hash_file(&file_path).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_file_changes_with_content() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");

        fs::write(&file_path, b"version 1").unwrap();
        let h1 = hash_file(&file_path).unwrap();

        fs::write(&file_path, b"version 2").unwrap();
        let h2 = hash_file(&file_path).unwrap();

        assert_ne!(h1, h2);
    }

    #[test]
    fn cache_round_trips() {
        let dir = TempDir::new().unwrap();
        let mut entries = HashMap::new();
        entries.insert(PathBuf::from("src/main.rs"), [42u8; 32]);
        entries.insert(PathBuf::from("src/lib.rs"), [7u8; 32]);

        let cache = HashCache::new(entries);
        cache.save(dir.path()).unwrap();

        let loaded = HashCache::load(dir.path()).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[&PathBuf::from("src/main.rs")], [42u8; 32]);
    }

    #[test]
    fn diff_detects_changes() {
        let mut prev_entries = HashMap::new();
        prev_entries.insert(PathBuf::from("a.rs"), [1u8; 32]);
        prev_entries.insert(PathBuf::from("b.rs"), [2u8; 32]);
        prev_entries.insert(PathBuf::from("deleted.rs"), [3u8; 32]);
        let previous = HashCache::new(prev_entries);

        let mut current = HashMap::new();
        current.insert(PathBuf::from("a.rs"), [1u8; 32]); // unchanged
        current.insert(PathBuf::from("b.rs"), [99u8; 32]); // changed
        current.insert(PathBuf::from("new.rs"), [4u8; 32]); // new

        let changeset = diff_files(&previous, &current);
        assert_eq!(changeset.unchanged.len(), 1);
        assert_eq!(changeset.changed.len(), 2); // b.rs + new.rs
        assert_eq!(changeset.deleted.len(), 1);
    }
}
