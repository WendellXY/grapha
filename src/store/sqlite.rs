use std::path::PathBuf;

use rusqlite::Connection;

use crate::graph::{Edge, EdgeKind, Graph, Node, NodeKind, Span, Visibility};
use crate::store::Store;

const SCHEMA_VERSION: &str = "1";

pub struct SqliteStore {
    path: PathBuf,
}

impl SqliteStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn open(&self) -> anyhow::Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Ok(conn)
    }

    fn create_tables(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS nodes (
                id         TEXT PRIMARY KEY,
                kind       TEXT NOT NULL,
                name       TEXT NOT NULL,
                file       TEXT NOT NULL,
                span_start_line   INTEGER NOT NULL,
                span_start_col    INTEGER NOT NULL,
                span_end_line     INTEGER NOT NULL,
                span_end_col      INTEGER NOT NULL,
                visibility TEXT NOT NULL,
                metadata   TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS edges (
                source     TEXT NOT NULL,
                target     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                confidence REAL NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source);
            CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target);
            CREATE INDEX IF NOT EXISTS idx_edges_kind   ON edges(kind);
            CREATE INDEX IF NOT EXISTS idx_nodes_name   ON nodes(name);
            CREATE INDEX IF NOT EXISTS idx_nodes_file   ON nodes(file);
            CREATE INDEX IF NOT EXISTS idx_nodes_kind   ON nodes(kind);",
        )?;
        Ok(())
    }
}

/// Serialize a serde enum value to its snake_case string form.
fn enum_to_str<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    let json = serde_json::to_string(value)?;
    Ok(json.trim_matches('"').to_string())
}

/// Deserialize a snake_case string back into a serde enum value.
fn str_to_enum<T: serde::de::DeserializeOwned>(s: &str) -> anyhow::Result<T> {
    let quoted = format!("\"{s}\"");
    Ok(serde_json::from_str(&quoted)?)
}

impl Store for SqliteStore {
    fn save(&self, graph: &Graph) -> anyhow::Result<()> {
        let conn = self.open()?;
        Self::create_tables(&conn)?;

        let tx = conn.unchecked_transaction()?;

        // Clear previous data
        tx.execute_batch(
            "DELETE FROM edges;
             DELETE FROM nodes;
             DELETE FROM meta;",
        )?;

        // Write version
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('version', ?1)",
            [&graph.version],
        )?;

        // Insert nodes
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO nodes (id, kind, name, file,
                    span_start_line, span_start_col, span_end_line, span_end_col,
                    visibility, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for node in &graph.nodes {
                stmt.execute(rusqlite::params![
                    node.id,
                    enum_to_str(&node.kind)?,
                    node.name,
                    node.file.to_string_lossy().as_ref(),
                    node.span.start[0] as i64,
                    node.span.start[1] as i64,
                    node.span.end[0] as i64,
                    node.span.end[1] as i64,
                    enum_to_str(&node.visibility)?,
                    serde_json::to_string(&node.metadata)?,
                ])?;
            }
        }

        // Insert edges
        {
            let mut stmt = tx.prepare(
                "INSERT INTO edges (source, target, kind, confidence)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for edge in &graph.edges {
                stmt.execute(rusqlite::params![
                    edge.source,
                    edge.target,
                    enum_to_str(&edge.kind)?,
                    edge.confidence,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    fn load(&self) -> anyhow::Result<Graph> {
        let conn = self.open()?;
        Self::create_tables(&conn)?;

        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| "0.1.0".to_string());

        let nodes = {
            let mut stmt = conn.prepare(
                "SELECT id, kind, name, file,
                        span_start_line, span_start_col, span_end_line, span_end_col,
                        visibility, metadata
                 FROM nodes",
            )?;
            let rows = stmt.query_map([], |row| {
                let kind_str: String = row.get(1)?;
                let vis_str: String = row.get(8)?;
                let meta_str: String = row.get(9)?;
                let file_str: String = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    kind_str,
                    row.get::<_, String>(2)?,
                    file_str,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    vis_str,
                    meta_str,
                ))
            })?;

            let mut nodes = Vec::new();
            for row in rows {
                let (id, kind_str, name, file_str, sl, sc, el, ec, vis_str, meta_str) = row?;
                let kind: NodeKind = str_to_enum(&kind_str)
                    .map_err(|e| anyhow::anyhow!("invalid node kind '{kind_str}': {e}"))?;
                let visibility: Visibility = str_to_enum(&vis_str)
                    .map_err(|e| anyhow::anyhow!("invalid visibility '{vis_str}': {e}"))?;
                let metadata: std::collections::HashMap<String, String> =
                    serde_json::from_str(&meta_str)?;
                nodes.push(Node {
                    id,
                    kind,
                    name,
                    file: PathBuf::from(file_str),
                    span: Span {
                        start: [sl as usize, sc as usize],
                        end: [el as usize, ec as usize],
                    },
                    visibility,
                    metadata,
                });
            }
            nodes
        };

        let edges = {
            let mut stmt = conn.prepare("SELECT source, target, kind, confidence FROM edges")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })?;

            let mut edges = Vec::new();
            for row in rows {
                let (source, target, kind_str, confidence) = row?;
                let kind: EdgeKind = str_to_enum(&kind_str)
                    .map_err(|e| anyhow::anyhow!("invalid edge kind '{kind_str}': {e}"))?;
                edges.push(Edge {
                    source,
                    target,
                    kind,
                    confidence,
                });
            }
            edges
        };

        Ok(Graph {
            version,
            nodes,
            edges,
        })
    }

    fn exists(&self) -> bool {
        self.path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;

    #[test]
    fn sqlite_store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grapha.db");
        let store = SqliteStore::new(path);

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "test.rs::main".to_string(),
                kind: NodeKind::Function,
                name: "main".to_string(),
                file: "test.rs".into(),
                span: Span {
                    start: [0, 0],
                    end: [5, 1],
                },
                visibility: Visibility::Public,
                metadata: HashMap::from([("async".to_string(), "true".to_string())]),
            }],
            edges: vec![Edge {
                source: "test.rs::main".to_string(),
                target: "test.rs::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.85,
            }],
        };

        store.save(&graph).unwrap();
        let loaded = store.load().unwrap();

        assert_eq!(loaded.version, "0.1.0");
        assert_eq!(loaded.nodes.len(), 1);
        assert_eq!(loaded.nodes[0].name, "main");
        assert_eq!(
            loaded.nodes[0].metadata.get("async").map(|s| s.as_str()),
            Some("true")
        );
        assert_eq!(loaded.edges.len(), 1);
        assert_eq!(loaded.edges[0].confidence, 0.85);
    }

    #[test]
    fn sqlite_save_overwrites_previous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grapha.db");
        let store = SqliteStore::new(path);

        let graph1 = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "a".to_string(),
                kind: NodeKind::Function,
                name: "a".to_string(),
                file: "a.rs".into(),
                span: Span {
                    start: [0, 0],
                    end: [1, 0],
                },
                visibility: Visibility::Public,
                metadata: HashMap::new(),
            }],
            edges: vec![],
        };
        store.save(&graph1).unwrap();

        let graph2 = Graph::new();
        store.save(&graph2).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.nodes.len(), 0);
    }
}
