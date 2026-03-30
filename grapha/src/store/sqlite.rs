use std::path::PathBuf;

use rusqlite::Connection;

use grapha_core::graph::{Edge, EdgeKind, Graph, Node, NodeKind, NodeRole, Span, Visibility};
use crate::store::Store;

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
                metadata   TEXT NOT NULL,
                role       TEXT,
                signature  TEXT,
                doc_comment TEXT,
                module     TEXT
            );
            CREATE TABLE IF NOT EXISTS edges (
                source     TEXT NOT NULL,
                target     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                confidence REAL NOT NULL,
                direction  TEXT,
                operation  TEXT,
                condition  TEXT,
                async_boundary INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source);
            CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target);
            CREATE INDEX IF NOT EXISTS idx_edges_kind   ON edges(kind);
            CREATE INDEX IF NOT EXISTS idx_nodes_name   ON nodes(name);
            CREATE INDEX IF NOT EXISTS idx_nodes_file   ON nodes(file);
            CREATE INDEX IF NOT EXISTS idx_nodes_kind   ON nodes(kind);
            CREATE INDEX IF NOT EXISTS idx_nodes_role   ON nodes(role);
            CREATE INDEX IF NOT EXISTS idx_nodes_module ON nodes(module);",
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
                    visibility, metadata, role, signature, doc_comment, module)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            )?;
            for node in &graph.nodes {
                let role_json: Option<String> =
                    node.role.as_ref().map(serde_json::to_string).transpose()?;
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
                    role_json,
                    node.signature,
                    node.doc_comment,
                    node.module,
                ])?;
            }
        }

        // Insert edges
        {
            let mut stmt = tx.prepare(
                "INSERT INTO edges (source, target, kind, confidence,
                    direction, operation, condition, async_boundary)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for edge in &graph.edges {
                let direction_str: Option<String> =
                    edge.direction.as_ref().map(enum_to_str).transpose()?;
                let async_boundary_int: Option<i64> =
                    edge.async_boundary.map(|b| if b { 1 } else { 0 });
                stmt.execute(rusqlite::params![
                    edge.source,
                    edge.target,
                    enum_to_str(&edge.kind)?,
                    edge.confidence,
                    direction_str,
                    edge.operation,
                    edge.condition,
                    async_boundary_int,
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
                        visibility, metadata, role, signature, doc_comment, module
                 FROM nodes",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            })?;

            let mut nodes = Vec::new();
            for row in rows {
                let (
                    id,
                    kind_str,
                    name,
                    file_str,
                    sl,
                    sc,
                    el,
                    ec,
                    vis_str,
                    meta_str,
                    role_str,
                    signature,
                    doc_comment,
                    module,
                ) = row?;
                let kind: NodeKind = str_to_enum(&kind_str)
                    .map_err(|e| anyhow::anyhow!("invalid node kind '{kind_str}': {e}"))?;
                let visibility: Visibility = str_to_enum(&vis_str)
                    .map_err(|e| anyhow::anyhow!("invalid visibility '{vis_str}': {e}"))?;
                let metadata: std::collections::HashMap<String, String> =
                    serde_json::from_str(&meta_str)?;
                let role: Option<NodeRole> = role_str
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(|e| anyhow::anyhow!("invalid node role: {e}"))?;
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
                    role,
                    signature,
                    doc_comment,
                    module,
                });
            }
            nodes
        };

        let edges = {
            let mut stmt = conn.prepare(
                "SELECT source, target, kind, confidence,
                        direction, operation, condition, async_boundary
                 FROM edges",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            })?;

            let mut edges = Vec::new();
            for row in rows {
                let (
                    source,
                    target,
                    kind_str,
                    confidence,
                    direction_str,
                    operation,
                    condition,
                    async_boundary_int,
                ) = row?;
                let kind: EdgeKind = str_to_enum(&kind_str)
                    .map_err(|e| anyhow::anyhow!("invalid edge kind '{kind_str}': {e}"))?;
                let direction = direction_str
                    .map(|s| str_to_enum(&s))
                    .transpose()
                    .map_err(|e| anyhow::anyhow!("invalid flow direction: {e}"))?;
                let async_boundary = async_boundary_int.map(|v| v != 0);
                edges.push(Edge {
                    source,
                    target,
                    kind,
                    confidence,
                    direction,
                    operation,
                    condition,
                    async_boundary,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
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
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
            }],
            edges: vec![Edge {
                source: "test.rs::main".to_string(),
                target: "test.rs::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.85,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
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
    fn sqlite_store_round_trips_dataflow_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grapha_dataflow.db");
        let store = SqliteStore::new(path);

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "api::handler".to_string(),
                    kind: NodeKind::Function,
                    name: "handler".to_string(),
                    file: "api.rs".into(),
                    span: Span {
                        start: [0, 0],
                        end: [10, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: HashMap::new(),
                    role: Some(NodeRole::EntryPoint),
                    signature: Some("async fn handler(req: Request) -> Response".to_string()),
                    doc_comment: Some("Handles incoming requests".to_string()),
                    module: Some("api".to_string()),
                },
                Node {
                    id: "db::query".to_string(),
                    kind: NodeKind::Function,
                    name: "query".to_string(),
                    file: "db.rs".into(),
                    span: Span {
                        start: [0, 0],
                        end: [5, 0],
                    },
                    visibility: Visibility::Crate,
                    metadata: HashMap::new(),
                    role: Some(NodeRole::Terminal {
                        kind: TerminalKind::Persistence,
                    }),
                    signature: Some("fn query(sql: &str) -> Vec<Row>".to_string()),
                    doc_comment: None,
                    module: Some("db".to_string()),
                },
                Node {
                    id: "internal::helper".to_string(),
                    kind: NodeKind::Function,
                    name: "helper".to_string(),
                    file: "internal.rs".into(),
                    span: Span {
                        start: [0, 0],
                        end: [3, 0],
                    },
                    visibility: Visibility::Private,
                    metadata: HashMap::new(),
                    role: Some(NodeRole::Internal),
                    signature: None,
                    doc_comment: None,
                    module: None,
                },
            ],
            edges: vec![
                Edge {
                    source: "api::handler".to_string(),
                    target: "db::query".to_string(),
                    kind: EdgeKind::Reads,
                    confidence: 0.9,
                    direction: Some(FlowDirection::Read),
                    operation: Some("SELECT".to_string()),
                    condition: Some("user.isActive".to_string()),
                    async_boundary: Some(true),
                },
                Edge {
                    source: "api::handler".to_string(),
                    target: "db::query".to_string(),
                    kind: EdgeKind::Writes,
                    confidence: 0.85,
                    direction: Some(FlowDirection::Write),
                    operation: Some("INSERT".to_string()),
                    condition: None,
                    async_boundary: Some(false),
                },
                Edge {
                    source: "api::handler".to_string(),
                    target: "internal::helper".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.95,
                    direction: Some(FlowDirection::Pure),
                    operation: None,
                    condition: None,
                    async_boundary: None,
                },
            ],
        };

        store.save(&graph).unwrap();
        let loaded = store.load().unwrap();

        assert_eq!(loaded.version, "0.1.0");
        assert_eq!(loaded.nodes.len(), 3);
        assert_eq!(loaded.edges.len(), 3);

        // Verify node dataflow fields
        let api_node = loaded
            .nodes
            .iter()
            .find(|n| n.id == "api::handler")
            .unwrap();
        assert_eq!(api_node.role, Some(NodeRole::EntryPoint));
        assert_eq!(
            api_node.signature.as_deref(),
            Some("async fn handler(req: Request) -> Response")
        );
        assert_eq!(
            api_node.doc_comment.as_deref(),
            Some("Handles incoming requests")
        );
        assert_eq!(api_node.module.as_deref(), Some("api"));

        let db_node = loaded.nodes.iter().find(|n| n.id == "db::query").unwrap();
        assert_eq!(
            db_node.role,
            Some(NodeRole::Terminal {
                kind: TerminalKind::Persistence,
            })
        );
        assert_eq!(
            db_node.signature.as_deref(),
            Some("fn query(sql: &str) -> Vec<Row>")
        );
        assert_eq!(db_node.doc_comment, None);
        assert_eq!(db_node.module.as_deref(), Some("db"));

        let internal_node = loaded
            .nodes
            .iter()
            .find(|n| n.id == "internal::helper")
            .unwrap();
        assert_eq!(internal_node.role, Some(NodeRole::Internal));
        assert_eq!(internal_node.signature, None);
        assert_eq!(internal_node.module, None);

        // Verify edge dataflow fields
        let read_edge = loaded
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Reads)
            .unwrap();
        assert_eq!(read_edge.direction, Some(FlowDirection::Read));
        assert_eq!(read_edge.operation.as_deref(), Some("SELECT"));
        assert_eq!(read_edge.condition.as_deref(), Some("user.isActive"));
        assert_eq!(read_edge.async_boundary, Some(true));

        let write_edge = loaded
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Writes)
            .unwrap();
        assert_eq!(write_edge.direction, Some(FlowDirection::Write));
        assert_eq!(write_edge.operation.as_deref(), Some("INSERT"));
        assert_eq!(write_edge.condition, None);
        assert_eq!(write_edge.async_boundary, Some(false));

        let call_edge = loaded
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .unwrap();
        assert_eq!(call_edge.direction, Some(FlowDirection::Pure));
        assert_eq!(call_edge.operation, None);
        assert_eq!(call_edge.condition, None);
        assert_eq!(call_edge.async_boundary, None);
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
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
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
