use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use grapha_core::graph::Node;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolAnnotationRecord {
    pub repo: String,
    pub symbol_key: String,
    pub text: String,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub symbol_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolAnnotationView {
    pub symbol_key: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub stale: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AnnotationIndex {
    records: HashMap<(String, String), SymbolAnnotationRecord>,
    records_by_symbol_key: HashMap<String, Vec<(String, String)>>,
}

impl AnnotationIndex {
    pub fn get_for_node(&self, node: &Node) -> Option<SymbolAnnotationView> {
        self.record_for_node(node)
            .map(|record| record.view_for_node(node))
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn from_records(records: HashMap<(String, String), SymbolAnnotationRecord>) -> Self {
        let mut records_by_symbol_key: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (repo, symbol_key) in records.keys() {
            records_by_symbol_key
                .entry(symbol_key.clone())
                .or_default()
                .push((repo.clone(), symbol_key.clone()));
        }
        Self {
            records,
            records_by_symbol_key,
        }
    }

    fn record_for_node(&self, node: &Node) -> Option<&SymbolAnnotationRecord> {
        let key = (repo_key(node).to_string(), symbol_key(node).to_string());
        self.records.get(&key).or_else(|| {
            let matches = self.records_by_symbol_key.get(symbol_key(node))?;
            if matches.len() == 1 {
                self.records.get(&matches[0])
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct AnnotationStore {
    path: PathBuf,
    import_from: Option<PathBuf>,
}

impl AnnotationStore {
    #[allow(dead_code)]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            import_from: None,
        }
    }

    pub fn for_project_root(project_root: &Path) -> Self {
        Self {
            path: crate::data_paths::annotation_db_path(project_root),
            import_from: Some(project_root.join(".grapha").join("annotations.db")),
        }
    }

    #[allow(dead_code)]
    pub fn for_project_root_with_data_root(project_root: &Path, data_root: &Path) -> Self {
        Self {
            path: crate::data_paths::annotation_db_path_with_data_root(project_root, data_root),
            import_from: Some(project_root.join(".grapha").join("annotations.db")),
        }
    }

    #[allow(dead_code)]
    pub fn for_store_dir(store_dir: &Path) -> Self {
        Self::new(store_dir.join("annotations.db"))
    }

    pub fn upsert_for_node(
        &self,
        node: &Node,
        text: &str,
        created_by: Option<&str>,
    ) -> anyhow::Result<SymbolAnnotationView> {
        let text = text.trim();
        if text.is_empty() {
            anyhow::bail!("annotation text cannot be empty");
        }
        let conn = self.open()?;
        create_tables(&conn)?;
        self.import_local_annotations(&conn)?;

        let existing_record = read_record_for_node(&conn, node)?;
        let storage_repo = existing_record
            .as_ref()
            .map(|record| record.repo.as_str())
            .unwrap_or_else(|| repo_key(node));
        let key = symbol_key(node);
        let now = current_timestamp();
        let created_at = existing_record
            .as_ref()
            .map(|record| record.created_at.clone())
            .unwrap_or_else(|| now.clone());
        let fingerprint = symbol_fingerprint(node);

        conn.execute(
            "INSERT INTO symbol_annotations (
                repo, symbol_key, annotation, created_by,
                created_at, updated_at, symbol_fingerprint
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(repo, symbol_key) DO UPDATE SET
                annotation = excluded.annotation,
                created_by = COALESCE(excluded.created_by, symbol_annotations.created_by),
                updated_at = excluded.updated_at,
                symbol_fingerprint = excluded.symbol_fingerprint",
            params![
                storage_repo,
                key,
                text,
                created_by,
                created_at,
                now,
                fingerprint,
            ],
        )?;

        let record = read_record(&conn, storage_repo, key)?
            .ok_or_else(|| anyhow::anyhow!("annotation was not saved for symbol key {key}"))?;
        Ok(record.view_for_node(node))
    }

    pub fn get_for_node(&self, node: &Node) -> anyhow::Result<Option<SymbolAnnotationView>> {
        let Some(conn) = self.open_existing_or_import()? else {
            return Ok(None);
        };
        let record = read_record_for_node(&conn, node)?;
        Ok(record.map(|record| record.view_for_node(node)))
    }

    pub fn load_index(&self) -> anyhow::Result<AnnotationIndex> {
        let Some(conn) = self.open_existing_or_import()? else {
            return Ok(AnnotationIndex::default());
        };
        let mut stmt = conn.prepare(
            "SELECT repo, symbol_key, annotation, created_by,
                    created_at, updated_at, symbol_fingerprint
             FROM symbol_annotations",
        )?;
        let mut rows = stmt.query([])?;
        let mut records = HashMap::new();
        while let Some(row) = rows.next()? {
            let record = SymbolAnnotationRecord {
                repo: row.get(0)?,
                symbol_key: row.get(1)?,
                text: row.get(2)?,
                created_by: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                symbol_fingerprint: row.get(6)?,
            };
            records.insert((record.repo.clone(), record.symbol_key.clone()), record);
        }
        Ok(AnnotationIndex::from_records(records))
    }

    fn open(&self) -> anyhow::Result<Connection> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Connection::open(&self.path)?)
    }

    fn open_existing(&self) -> anyhow::Result<Option<Connection>> {
        if !self.path.exists() {
            return Ok(None);
        }
        Ok(Some(Connection::open(&self.path)?))
    }

    fn open_existing_or_import(&self) -> anyhow::Result<Option<Connection>> {
        let has_global = self.path.exists();
        let has_legacy = self
            .import_from
            .as_ref()
            .is_some_and(|source| source.exists() && !same_path(source, &self.path));

        let conn = if has_global {
            self.open_existing()?
        } else if has_legacy {
            Some(self.open()?)
        } else {
            None
        };

        if let Some(conn) = conn {
            create_tables(&conn)?;
            self.import_local_annotations(&conn)?;
            Ok(Some(conn))
        } else {
            Ok(None)
        }
    }

    fn import_local_annotations(&self, conn: &Connection) -> anyhow::Result<()> {
        let Some(source_path) = self.import_from.as_ref() else {
            return Ok(());
        };
        if !source_path.exists() || same_path(source_path, &self.path) {
            return Ok(());
        }

        create_tables(conn)?;
        let source_id = source_db_id(source_path);
        let already_imported = conn
            .query_row(
                "SELECT 1 FROM annotation_imports WHERE source_id = ?1",
                params![source_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if already_imported {
            return Ok(());
        }

        let legacy =
            Connection::open_with_flags(source_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if !table_exists(&legacy, "symbol_annotations")? {
            mark_imported(conn, &source_id, source_path)?;
            return Ok(());
        }

        let repo_expr = if table_has_column(&legacy, "symbol_annotations", "repo")? {
            "repo"
        } else {
            "''"
        };
        let query = format!(
            "SELECT {repo_expr}, symbol_key, annotation, created_by,
                    created_at, updated_at, symbol_fingerprint
             FROM symbol_annotations"
        );
        let mut stmt = legacy.prepare(&query)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            conn.execute(
                "INSERT OR IGNORE INTO symbol_annotations (
                    repo, symbol_key, annotation, created_by,
                    created_at, updated_at, symbol_fingerprint
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ],
            )?;
        }
        mark_imported(conn, &source_id, source_path)?;
        Ok(())
    }
}

impl SymbolAnnotationRecord {
    fn view_for_node(&self, node: &Node) -> SymbolAnnotationView {
        let current_fingerprint = symbol_fingerprint(node);
        let stale = self
            .symbol_fingerprint
            .as_deref()
            .is_some_and(|stored| stored != current_fingerprint);
        SymbolAnnotationView {
            symbol_key: self.symbol_key.clone(),
            text: self.text.clone(),
            created_by: self.created_by.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            stale,
        }
    }
}

pub fn symbol_key(node: &Node) -> &str {
    node.id.as_str()
}

fn repo_key(node: &Node) -> &str {
    node.repo.as_deref().unwrap_or("")
}

fn create_tables(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS symbol_annotations (
            repo TEXT NOT NULL DEFAULT '',
            symbol_key TEXT NOT NULL,
            annotation TEXT NOT NULL,
            created_by TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            symbol_fingerprint TEXT,
            PRIMARY KEY (repo, symbol_key)
        );
        CREATE INDEX IF NOT EXISTS idx_symbol_annotations_symbol_key
            ON symbol_annotations(symbol_key);
        CREATE TABLE IF NOT EXISTS annotation_imports (
            source_id TEXT PRIMARY KEY,
            source_path TEXT NOT NULL,
            imported_at TEXT NOT NULL
        );",
    )?;
    Ok(())
}

fn read_record_for_node(
    conn: &Connection,
    node: &Node,
) -> anyhow::Result<Option<SymbolAnnotationRecord>> {
    if let Some(record) = read_record(conn, repo_key(node), symbol_key(node))? {
        return Ok(Some(record));
    }

    let mut stmt = conn.prepare(
        "SELECT repo, symbol_key, annotation, created_by,
                created_at, updated_at, symbol_fingerprint
         FROM symbol_annotations
         WHERE symbol_key = ?1
         LIMIT 2",
    )?;
    let mut rows = stmt.query(params![symbol_key(node)])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let record = SymbolAnnotationRecord {
        repo: row.get(0)?,
        symbol_key: row.get(1)?,
        text: row.get(2)?,
        created_by: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        symbol_fingerprint: row.get(6)?,
    };
    if rows.next()?.is_some() {
        return Ok(None);
    }
    Ok(Some(record))
}

fn read_record(
    conn: &Connection,
    repo: &str,
    symbol_key: &str,
) -> anyhow::Result<Option<SymbolAnnotationRecord>> {
    Ok(conn
        .query_row(
            "SELECT repo, symbol_key, annotation, created_by,
                    created_at, updated_at, symbol_fingerprint
             FROM symbol_annotations
             WHERE repo = ?1 AND symbol_key = ?2",
            params![repo, symbol_key],
            |row| {
                Ok(SymbolAnnotationRecord {
                    repo: row.get(0)?,
                    symbol_key: row.get(1)?,
                    text: row.get(2)?,
                    created_by: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                    symbol_fingerprint: row.get(6)?,
                })
            },
        )
        .optional()?)
}

fn current_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    seconds.to_string()
}

fn mark_imported(conn: &Connection, source_id: &str, source_path: &Path) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO annotation_imports (source_id, source_path, imported_at)
         VALUES (?1, ?2, ?3)",
        params![
            source_id,
            source_path.to_string_lossy().as_ref(),
            current_timestamp()
        ],
    )?;
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> anyhow::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn source_db_id(path: &Path) -> String {
    let normalized = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string();
    let mut hasher = Fnv1a64::default();
    hasher.write_component(&normalized);
    format!("{:016x}", hasher.finish())
}

fn symbol_fingerprint(node: &Node) -> String {
    let mut hasher = Fnv1a64::default();
    hasher.write_component(&node.id);
    hasher.write_component(&format!("{:?}", node.kind));
    hasher.write_component(&node.name);
    hasher.write_component(&node.file.to_string_lossy());
    hasher.write_component(&format!("{:?}", node.span.start));
    hasher.write_component(&format!("{:?}", node.span.end));
    hasher.write_component(node.signature.as_deref().unwrap_or(""));
    hasher.write_component(node.doc_comment.as_deref().unwrap_or(""));
    hasher.write_component(node.snippet.as_deref().unwrap_or(""));
    format!("{:016x}", hasher.finish())
}

#[derive(Default)]
struct Fnv1a64(u64);

impl Fnv1a64 {
    fn write_component(&mut self, value: &str) {
        if self.0 == 0 {
            self.0 = 0xcbf29ce484222325;
        }
        for byte in value.as_bytes() {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x100000001b3);
    }

    fn finish(self) -> u64 {
        if self.0 == 0 {
            0xcbf29ce484222325
        } else {
            self.0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use grapha_core::graph::{NodeKind, Span, Visibility};
    use rusqlite::Connection;

    use super::*;

    fn node() -> Node {
        Node {
            id: "s:DemoUSR".to_string(),
            kind: NodeKind::Function,
            name: "sendGift".to_string(),
            file: PathBuf::from("Sources/Gifts.swift"),
            span: Span {
                start: [1, 0],
                end: [5, 1],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: Some("func sendGift()".to_string()),
            doc_comment: None,
            module: Some("Demo".to_string()),
            snippet: Some("func sendGift() {}".to_string()),
            repo: None,
        }
    }

    #[test]
    fn annotations_round_trip_by_symbol_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnnotationStore::for_store_dir(dir.path());
        let node = node();

        let saved = store
            .upsert_for_node(&node, "Coordinates the gift handoff.", Some("codex"))
            .unwrap();
        assert_eq!(saved.symbol_key, "s:DemoUSR");
        assert_eq!(saved.text, "Coordinates the gift handoff.");
        assert_eq!(saved.created_by.as_deref(), Some("codex"));
        assert!(!saved.stale);

        let loaded = store.get_for_node(&node).unwrap().unwrap();
        assert_eq!(loaded.text, saved.text);
        assert!(!loaded.stale);
    }

    #[test]
    fn annotation_view_marks_changed_symbol_stale() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnnotationStore::for_store_dir(dir.path());
        let mut node = node();
        store
            .upsert_for_node(&node, "Builds the checkout payload.", None)
            .unwrap();

        node.signature = Some("func sendGift(cartID: String)".to_string());
        let loaded = store.get_for_node(&node).unwrap().unwrap();
        assert!(loaded.stale);
    }

    #[test]
    fn project_root_store_imports_legacy_annotations_without_mutating_source() {
        let project = tempfile::tempdir().unwrap();
        let global_root = tempfile::tempdir().unwrap();
        let legacy_store = AnnotationStore::for_store_dir(&project.path().join(".grapha"));
        let mut original_node = node();
        legacy_store
            .upsert_for_node(
                &original_node,
                "Legacy note from the local worktree.",
                Some("codex"),
            )
            .unwrap();

        original_node.signature = Some("func sendGift(cartID: String)".to_string());
        let global_store =
            AnnotationStore::for_project_root_with_data_root(project.path(), global_root.path());
        let imported = global_store
            .get_for_node(&original_node)
            .unwrap()
            .expect("legacy annotation should be imported on first read");

        assert_eq!(imported.text, "Legacy note from the local worktree.");
        assert!(imported.stale);
        assert!(
            project
                .path()
                .join(".grapha")
                .join("annotations.db")
                .exists()
        );
        assert!(legacy_store.get_for_node(&node()).unwrap().is_some());
    }

    #[test]
    fn global_annotation_wins_over_imported_legacy_conflict() {
        let project = tempfile::tempdir().unwrap();
        let global_root = tempfile::tempdir().unwrap();
        let global_store =
            AnnotationStore::for_project_root_with_data_root(project.path(), global_root.path());
        let legacy_store = AnnotationStore::for_store_dir(&project.path().join(".grapha"));
        let node = node();

        global_store
            .upsert_for_node(&node, "Global note should win.", Some("codex"))
            .unwrap();
        legacy_store
            .upsert_for_node(
                &node,
                "Legacy note should not replace it.",
                Some("old-agent"),
            )
            .unwrap();

        let loaded = global_store.get_for_node(&node).unwrap().unwrap();
        assert_eq!(loaded.text, "Global note should win.");
        assert_eq!(loaded.created_by.as_deref(), Some("codex"));
    }

    #[test]
    fn imported_sources_are_not_reimported_after_global_delete() {
        let project = tempfile::tempdir().unwrap();
        let global_root = tempfile::tempdir().unwrap();
        let legacy_store = AnnotationStore::for_store_dir(&project.path().join(".grapha"));
        let node = node();
        legacy_store
            .upsert_for_node(&node, "Import me once.", Some("codex"))
            .unwrap();

        let global_store =
            AnnotationStore::for_project_root_with_data_root(project.path(), global_root.path());
        assert!(global_store.get_for_node(&node).unwrap().is_some());

        let conn = Connection::open(&global_store.path).unwrap();
        conn.execute("DELETE FROM symbol_annotations", []).unwrap();
        drop(conn);

        assert!(global_store.get_for_node(&node).unwrap().is_none());
    }

    #[test]
    fn global_index_matches_unique_symbol_key_across_repo_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnnotationStore::for_store_dir(dir.path());
        let mut first_worktree_node = node();
        first_worktree_node.repo = Some("main-worktree".to_string());
        store
            .upsert_for_node(
                &first_worktree_node,
                "Shared across repo display names.",
                Some("codex"),
            )
            .unwrap();

        let mut second_worktree_node = first_worktree_node.clone();
        second_worktree_node.repo = Some("linked-worktree".to_string());
        let loaded = store.get_for_node(&second_worktree_node).unwrap().unwrap();

        assert_eq!(loaded.text, "Shared across repo display names.");
    }
}
