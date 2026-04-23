use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use git2::{DiffOptions, Oid, Repository, StatusOptions};
use serde::{Deserialize, Serialize};

use crate::cache::FileStamp;

const INDEX_STATUS_FILENAME: &str = "index_status.json";
const INDEX_STATUS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedRepoFile {
    path: String,
    stamp: Option<FileStamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedRepoState {
    root: String,
    head_oid: Option<String>,
    head_ref: Option<String>,
    dirty_files: Vec<IndexedRepoFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexStatusSnapshot {
    version: u32,
    indexed_at_unix_secs: u64,
    grapha_version: String,
    node_count: usize,
    edge_count: usize,
    repo: Option<IndexedRepoState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoStatus {
    pub root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_head_oid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_head_oid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_head_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_head_ref: Option<String>,
    pub changed_file_count_since_index: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed_files_since_index: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexStatus {
    pub indexed_at_unix_secs: u64,
    pub grapha_version: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub may_be_stale: bool,
    pub freshness_tracking_available: bool,
    pub changed_file_count_since_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn normalize_repo_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_store_artifact(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".grapha")
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn path_mtime_unix_secs(path: &Path) -> anyhow::Result<u64> {
    Ok(fs::metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs())
}

fn head_state(repo: &Repository) -> (Option<String>, Option<String>) {
    let head = repo.head().ok();
    let head_oid = head.as_ref().and_then(|head| head.target()).map(|oid| oid.to_string());
    let head_ref = head
        .as_ref()
        .and_then(|head| head.shorthand())
        .map(str::to_string);
    (head_oid, head_ref)
}

fn repo_root(repo: &Repository) -> Option<PathBuf> {
    repo.workdir()
        .map(Path::to_path_buf)
        .or_else(|| repo.path().parent().map(Path::to_path_buf))
}

fn dirty_repo_files(repo: &Repository) -> anyhow::Result<Vec<IndexedRepoFile>> {
    let Some(root) = repo_root(repo) else {
        return Ok(Vec::new());
    };

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true)
        .include_ignored(false);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut files = BTreeMap::new();
    for entry in statuses.iter() {
        let Some(path) = entry.path() else {
            continue;
        };
        let relative = PathBuf::from(path);
        if is_store_artifact(&relative) {
            continue;
        }
        let stamp = FileStamp::from_path(&root.join(&relative));
        files.insert(
            normalize_repo_path(&relative),
            IndexedRepoFile {
                path: normalize_repo_path(&relative),
                stamp,
            },
        );
    }

    Ok(files.into_values().collect())
}

fn capture_repo_state(project_root: &Path) -> anyhow::Result<Option<IndexedRepoState>> {
    let repo = match Repository::discover(project_root) {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };
    let Some(root) = repo_root(&repo) else {
        return Ok(None);
    };
    let (head_oid, head_ref) = head_state(&repo);
    Ok(Some(IndexedRepoState {
        root: normalize_repo_path(&root),
        head_oid,
        head_ref,
        dirty_files: dirty_repo_files(&repo)?,
    }))
}

fn status_path(store_dir: &Path) -> PathBuf {
    store_dir.join(INDEX_STATUS_FILENAME)
}

fn save_snapshot(store_dir: &Path, snapshot: &IndexStatusSnapshot) -> anyhow::Result<()> {
    if let Some(parent) = store_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(store_dir)?;
    let payload = serde_json::to_string_pretty(snapshot)?;
    fs::write(status_path(store_dir), payload)
        .with_context(|| format!("writing {}", status_path(store_dir).display()))
}

fn load_snapshot(store_dir: &Path) -> anyhow::Result<IndexStatusSnapshot> {
    let payload = fs::read_to_string(status_path(store_dir))
        .with_context(|| format!("reading {}", status_path(store_dir).display()))?;
    let snapshot: IndexStatusSnapshot = serde_json::from_str(&payload)
        .with_context(|| format!("parsing {}", status_path(store_dir).display()))?;
    if snapshot.version != INDEX_STATUS_VERSION {
        anyhow::bail!(
            "unsupported index status version: {} (expected {})",
            snapshot.version,
            INDEX_STATUS_VERSION
        );
    }
    Ok(snapshot)
}

fn legacy_status(store_dir: &Path) -> anyhow::Result<IndexStatus> {
    let db_path = store_dir.join("grapha.db");
    if !db_path.exists() {
        anyhow::bail!("no index found — run `grapha index` first");
    }

    Ok(IndexStatus {
        indexed_at_unix_secs: path_mtime_unix_secs(&db_path)?,
        grapha_version: env!("CARGO_PKG_VERSION").to_string(),
        node_count: 0,
        edge_count: 0,
        may_be_stale: false,
        freshness_tracking_available: false,
        changed_file_count_since_index: 0,
        repo: None,
        note: Some(
            "reindex with the current Grapha build to enable freshness tracking".to_string(),
        ),
    })
}

fn current_dirty_file_map(repo: &Repository) -> anyhow::Result<BTreeMap<String, Option<FileStamp>>> {
    Ok(dirty_repo_files(repo)?
        .into_iter()
        .map(|file| (file.path, file.stamp))
        .collect())
}

fn snapshot_dirty_file_map(
    repo: &IndexedRepoState,
) -> BTreeMap<String, Option<FileStamp>> {
    repo.dirty_files
        .iter()
        .map(|file| (file.path.clone(), file.stamp))
        .collect()
}

fn changed_files_between_heads(
    repo: &Repository,
    old_head: &str,
    new_head: &str,
) -> anyhow::Result<BTreeSet<String>> {
    let old_oid = Oid::from_str(old_head)?;
    let new_oid = Oid::from_str(new_head)?;
    let old_tree = repo.find_commit(old_oid)?.tree()?;
    let new_tree = repo.find_commit(new_oid)?.tree()?;
    let mut opts = DiffOptions::new();
    let diff = repo.diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(&mut opts))?;
    let mut paths = BTreeSet::new();
    diff.foreach(
        &mut |delta, _| {
            if let Some(path) = delta.new_file().path() {
                paths.insert(normalize_repo_path(path));
            }
            if let Some(path) = delta.old_file().path() {
                paths.insert(normalize_repo_path(path));
            }
            true
        },
        None,
        None,
        None,
    )?;
    Ok(paths)
}

fn compute_status(snapshot: IndexStatusSnapshot, project_root: &Path) -> anyhow::Result<IndexStatus> {
    let mut changed_files = BTreeSet::new();
    let mut freshness_tracking_available = false;
    let repo_status = match snapshot.repo.as_ref() {
        Some(indexed_repo) => match Repository::discover(project_root) {
            Ok(repo) => {
                freshness_tracking_available = true;
                let (current_head_oid, current_head_ref) = head_state(&repo);
                if let (Some(indexed_head), Some(current_head)) =
                    (indexed_repo.head_oid.as_deref(), current_head_oid.as_deref())
                {
                    if indexed_head != current_head {
                        changed_files.extend(changed_files_between_heads(
                            &repo,
                            indexed_head,
                            current_head,
                        )?);
                    }
                } else if indexed_repo.head_oid != current_head_oid {
                    changed_files.insert(".git/HEAD".to_string());
                }

                let indexed_dirty = snapshot_dirty_file_map(indexed_repo);
                let current_dirty = current_dirty_file_map(&repo)?;
                for path in indexed_dirty.keys().chain(current_dirty.keys()) {
                    let indexed_stamp = indexed_dirty.get(path);
                    let current_stamp = current_dirty.get(path);
                    if indexed_stamp != current_stamp {
                        changed_files.insert(path.clone());
                    }
                }

                Some(RepoStatus {
                    root: indexed_repo.root.clone(),
                    indexed_head_oid: indexed_repo.head_oid.clone(),
                    current_head_oid,
                    indexed_head_ref: indexed_repo.head_ref.clone(),
                    current_head_ref,
                    changed_file_count_since_index: changed_files.len(),
                    changed_files_since_index: changed_files.iter().cloned().collect(),
                })
            }
            Err(_) => Some(RepoStatus {
                root: indexed_repo.root.clone(),
                indexed_head_oid: indexed_repo.head_oid.clone(),
                current_head_oid: None,
                indexed_head_ref: indexed_repo.head_ref.clone(),
                current_head_ref: None,
                changed_file_count_since_index: 0,
                changed_files_since_index: Vec::new(),
            }),
        },
        None => None,
    };

    let note = if !freshness_tracking_available && snapshot.repo.is_some() {
        Some("git status unavailable for this project root".to_string())
    } else {
        None
    };

    Ok(IndexStatus {
        indexed_at_unix_secs: snapshot.indexed_at_unix_secs,
        grapha_version: snapshot.grapha_version,
        node_count: snapshot.node_count,
        edge_count: snapshot.edge_count,
        may_be_stale: freshness_tracking_available && !changed_files.is_empty(),
        freshness_tracking_available,
        changed_file_count_since_index: changed_files.len(),
        repo: repo_status,
        note,
    })
}

pub fn save_index_status(
    project_root: &Path,
    store_dir: &Path,
    node_count: usize,
    edge_count: usize,
) -> anyhow::Result<()> {
    let snapshot = IndexStatusSnapshot {
        version: INDEX_STATUS_VERSION,
        indexed_at_unix_secs: current_unix_secs(),
        grapha_version: env!("CARGO_PKG_VERSION").to_string(),
        node_count,
        edge_count,
        repo: capture_repo_state(project_root)?,
    };
    save_snapshot(store_dir, &snapshot)
}

pub fn load_index_status(project_root: &Path, store_dir: &Path) -> anyhow::Result<IndexStatus> {
    match load_snapshot(store_dir) {
        Ok(snapshot) => compute_status(snapshot, project_root),
        Err(error) => {
            if status_path(store_dir).exists() {
                Err(error)
            } else {
                legacy_status(store_dir)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{IndexAddOption, Signature};
    use std::time::Duration;
    use tempfile::tempdir;

    fn commit_all(repo: &Repository, message: &str) -> anyhow::Result<()> {
        let mut index = repo.index()?;
        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
        index.write()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let sig = Signature::now("grapha", "grapha@example.com")?;
        let parent = repo
            .head()
            .ok()
            .and_then(|head| head.target())
            .and_then(|oid| repo.find_commit(oid).ok());
        if let Some(parent) = parent {
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])?;
        } else {
            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])?;
        }
        Ok(())
    }

    #[test]
    fn status_reports_clean_repo_as_fresh() {
        let dir = tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("src.rs"), "fn main() {}\n").unwrap();
        commit_all(&repo, "initial").unwrap();

        let store_dir = dir.path().join(".grapha");
        save_index_status(dir.path(), &store_dir, 1, 0).unwrap();

        let status = load_index_status(dir.path(), &store_dir).unwrap();
        assert!(status.freshness_tracking_available);
        assert!(!status.may_be_stale);
        assert_eq!(status.changed_file_count_since_index, 0);
    }

    #[test]
    fn status_detects_dirty_file_changes_since_index() {
        let dir = tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let source = dir.path().join("src.rs");
        fs::write(&source, "fn main() {}\n").unwrap();
        commit_all(&repo, "initial").unwrap();

        let store_dir = dir.path().join(".grapha");
        save_index_status(dir.path(), &store_dir, 1, 0).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        fs::write(&source, "fn main() { println!(\"hi\"); }\n").unwrap();

        let status = load_index_status(dir.path(), &store_dir).unwrap();
        assert!(status.may_be_stale);
        assert_eq!(status.changed_file_count_since_index, 1);
        assert!(
            status
                .repo
                .unwrap()
                .changed_files_since_index
                .contains(&"src.rs".to_string())
        );
    }

    #[test]
    fn status_keeps_same_dirty_snapshot_fresh_until_file_changes_again() {
        let dir = tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let source = dir.path().join("src.rs");
        fs::write(&source, "fn main() {}\n").unwrap();
        commit_all(&repo, "initial").unwrap();

        fs::write(&source, "fn main() { println!(\"indexed\"); }\n").unwrap();
        let store_dir = dir.path().join(".grapha");
        save_index_status(dir.path(), &store_dir, 1, 0).unwrap();

        let status = load_index_status(dir.path(), &store_dir).unwrap();
        assert!(!status.may_be_stale);

        std::thread::sleep(Duration::from_millis(10));
        fs::write(&source, "fn main() { println!(\"changed\"); }\n").unwrap();
        let stale = load_index_status(dir.path(), &store_dir).unwrap();
        assert!(stale.may_be_stale);
        assert_eq!(stale.changed_file_count_since_index, 1);
    }
}
