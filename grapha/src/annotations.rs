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
}

impl AnnotationIndex {
    pub fn get_for_node(&self, node: &Node) -> Option<SymbolAnnotationView> {
        self.records
            .get(&(repo_key(node).to_string(), symbol_key(node).to_string()))
            .map(|record| record.view_for_node(node))
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct AnnotationStore {
    path: PathBuf,
}

impl AnnotationStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn for_project_root(project_root: &Path) -> Self {
        Self::for_store_dir(&project_root.join(".grapha"))
    }

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

        let repo = repo_key(node);
        let key = symbol_key(node);
        let now = current_timestamp();
        let created_at = conn
            .query_row(
                "SELECT created_at FROM symbol_annotations
                 WHERE repo = ?1 AND symbol_key = ?2",
                params![repo, key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
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
            params![repo, key, text, created_by, created_at, now, fingerprint,],
        )?;

        let record = read_record(&conn, repo, key)?
            .ok_or_else(|| anyhow::anyhow!("annotation was not saved for symbol key {key}"))?;
        Ok(record.view_for_node(node))
    }

    pub fn get_for_node(&self, node: &Node) -> anyhow::Result<Option<SymbolAnnotationView>> {
        let Some(conn) = self.open_existing()? else {
            return Ok(None);
        };
        create_tables(&conn)?;
        let record = read_record(&conn, repo_key(node), symbol_key(node))?;
        Ok(record.map(|record| record.view_for_node(node)))
    }

    pub fn load_index(&self) -> anyhow::Result<AnnotationIndex> {
        let Some(conn) = self.open_existing()? else {
            return Ok(AnnotationIndex::default());
        };
        create_tables(&conn)?;
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
        Ok(AnnotationIndex { records })
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
            ON symbol_annotations(symbol_key);",
    )?;
    Ok(())
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
}
