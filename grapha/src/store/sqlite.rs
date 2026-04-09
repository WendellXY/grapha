use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::delta::{GraphDelta, edge_fingerprint};
use crate::store::{Store, StoreWriteStats};
use grapha_core::graph::{
    Edge, EdgeKind, EdgeProvenance, Graph, Node, NodeKind, NodeRole, Span, Visibility,
};

const STORE_SCHEMA_VERSION: &str = "6";
const BINARY_PROVENANCE_SCHEMA_VERSION: &str = "5";

pub struct SqliteStore {
    path: PathBuf,
}

fn serialize_provenance(provenance: &[EdgeProvenance]) -> anyhow::Result<Vec<u8>> {
    if provenance.is_empty() {
        return Ok(Vec::new());
    }
    Ok(bincode::serialize(provenance)?)
}

fn deserialize_provenance(blob: &[u8]) -> anyhow::Result<Vec<EdgeProvenance>> {
    if blob.is_empty() {
        return Ok(Vec::new());
    }
    Ok(bincode::deserialize(blob)?)
}

fn remove_existing_store_files(path: &Path) -> anyhow::Result<()> {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.to_string_lossy())),
        PathBuf::from(format!("{}-shm", path.to_string_lossy())),
    ] {
        match std::fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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

    fn open_for_write(&self) -> anyhow::Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=MEMORY;
             PRAGMA cache_size=-64000;
             PRAGMA mmap_size=268435456;",
        )?;
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
                module     TEXT,
                snippet    TEXT
            );
            CREATE TABLE IF NOT EXISTS edges (
                edge_id    TEXT PRIMARY KEY,
                source     TEXT NOT NULL,
                target     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                confidence REAL NOT NULL,
                direction  TEXT,
                operation  TEXT,
                condition  TEXT,
                async_boundary INTEGER,
                provenance BLOB NOT NULL
            );",
        )?;
        Ok(())
    }

    fn schema_version(conn: &Connection) -> anyhow::Result<Option<String>> {
        let has_meta = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
                [],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !has_meta {
            return Ok(None);
        }

        Ok(conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'store_schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn write_meta(tx: &rusqlite::Transaction<'_>, graph: &Graph) -> anyhow::Result<()> {
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [&graph.version],
        )?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('store_schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [STORE_SCHEMA_VERSION],
        )?;
        Ok(())
    }

    fn insert_nodes(
        tx: &rusqlite::Transaction<'_>,
        nodes: &[Node],
        replace: bool,
    ) -> anyhow::Result<()> {
        let verb = if replace {
            "INSERT OR REPLACE"
        } else {
            "INSERT"
        };
        let sql = format!(
            "{verb} INTO nodes (id, kind, name, file,
                span_start_line, span_start_col, span_end_line, span_end_col,
                visibility, metadata, role, signature, doc_comment, module, snippet)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)"
        );
        let mut stmt = tx.prepare_cached(&sql)?;
        let empty_meta = "{}".to_string();
        let mut meta_buf = String::new();
        for node in nodes {
            let role_json: Option<String> =
                node.role.as_ref().map(serde_json::to_string).transpose()?;
            let meta_ref: &str = if node.metadata.is_empty() {
                &empty_meta
            } else {
                meta_buf.clear();
                serde_json::to_writer(unsafe { meta_buf.as_mut_vec() }, &node.metadata)?;
                &meta_buf
            };
            let file_str = node.file.to_string_lossy();
            stmt.execute(rusqlite::params![
                node.id,
                node_kind_str(&node.kind),
                node.name,
                file_str.as_ref(),
                node.span.start[0] as i64,
                node.span.start[1] as i64,
                node.span.end[0] as i64,
                node.span.end[1] as i64,
                visibility_str(&node.visibility),
                meta_ref,
                role_json,
                node.signature,
                node.doc_comment,
                node.module,
                node.snippet,
            ])?;
        }
        Ok(())
    }

    fn insert_edges<'a>(
        tx: &rusqlite::Transaction<'_>,
        edges: impl Iterator<Item = (String, &'a Edge)>,
        replace: bool,
    ) -> anyhow::Result<()> {
        let verb = if replace {
            "INSERT OR REPLACE"
        } else {
            "INSERT"
        };
        let sql = format!(
            "{verb} INTO edges (edge_id, source, target, kind, confidence,
                direction, operation, condition, async_boundary, provenance)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"
        );
        let mut stmt = tx.prepare_cached(&sql)?;
        for (edge_id, edge) in edges {
            let direction_str: Option<&str> = edge.direction.as_ref().map(flow_direction_str);
            let async_boundary_int: Option<i64> =
                edge.async_boundary.map(|b| if b { 1 } else { 0 });
            let provenance = serialize_provenance(&edge.provenance)?;
            stmt.execute(rusqlite::params![
                edge_id,
                edge.source,
                edge.target,
                edge_kind_str(&edge.kind),
                edge.confidence,
                direction_str,
                edge.operation,
                edge.condition,
                async_boundary_int,
                provenance,
            ])?;
        }
        Ok(())
    }

    fn save_full(&self, graph: &Graph) -> anyhow::Result<()> {
        remove_existing_store_files(&self.path)?;
        let conn = Connection::open(&self.path)?;
        // For full rebuild: journal OFF (no crash safety needed — just redo),
        // locking_mode EXCLUSIVE (no readers during rebuild),
        // large page size for fewer I/O ops.
        conn.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=MEMORY;
             PRAGMA cache_size=-64000;
             PRAGMA mmap_size=268435456;
             PRAGMA locking_mode=EXCLUSIVE;
             PRAGMA page_size=8192;",
        )?;

        conn.execute_batch(
            "DROP TABLE IF EXISTS edges;
             DROP TABLE IF EXISTS nodes;
             DROP TABLE IF EXISTS meta;",
        )?;

        // Create tables WITHOUT primary key for fast sequential bulk insert.
        // Unique indexes added after data load (much faster than maintaining during insert).
        conn.execute_batch(
            "CREATE TABLE meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE nodes (
                id         TEXT NOT NULL,
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
                module     TEXT,
                snippet    TEXT
            );
            CREATE TABLE edges (
                edge_id    TEXT NOT NULL,
                source     TEXT NOT NULL,
                target     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                confidence REAL NOT NULL,
                direction  TEXT,
                operation  TEXT,
                condition  TEXT,
                async_boundary INTEGER,
                provenance BLOB NOT NULL
            );",
        )?;

        let tx = conn.unchecked_transaction()?;
        Self::write_meta(&tx, graph)?;
        Self::insert_nodes(&tx, &graph.nodes, false)?;
        Self::insert_edges(
            &tx,
            graph
                .edges
                .iter()
                .map(|edge| (edge_fingerprint(edge), edge)),
            false,
        )?;
        tx.commit()?;
        // Add primary key unique indexes after bulk load
        conn.execute_batch(
            "CREATE UNIQUE INDEX idx_nodes_id ON nodes(id);
             CREATE UNIQUE INDEX idx_edges_id ON edges(edge_id);",
        )?;
        conn.execute_batch("PRAGMA optimize;")?;
        Ok(())
    }
}

// Direct enum → &str conversions — avoids ~1M serde_json round-trips during save.

fn node_kind_str(k: &NodeKind) -> &'static str {
    match k {
        NodeKind::Function => "function",
        NodeKind::Class => "class",
        NodeKind::Struct => "struct",
        NodeKind::Enum => "enum",
        NodeKind::Trait => "trait",
        NodeKind::Impl => "impl",
        NodeKind::Module => "module",
        NodeKind::Field => "field",
        NodeKind::Variant => "variant",
        NodeKind::Property => "property",
        NodeKind::Constant => "constant",
        NodeKind::TypeAlias => "type_alias",
        NodeKind::Protocol => "protocol",
        NodeKind::Extension => "extension",
        NodeKind::View => "view",
        NodeKind::Branch => "branch",
    }
}

fn edge_kind_str(k: &EdgeKind) -> &'static str {
    match k {
        EdgeKind::Calls => "calls",
        EdgeKind::Uses => "uses",
        EdgeKind::Implements => "implements",
        EdgeKind::Contains => "contains",
        EdgeKind::TypeRef => "type_ref",
        EdgeKind::Inherits => "inherits",
        EdgeKind::Reads => "reads",
        EdgeKind::Writes => "writes",
        EdgeKind::Publishes => "publishes",
        EdgeKind::Subscribes => "subscribes",
    }
}

fn visibility_str(v: &Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Crate => "crate",
        Visibility::Private => "private",
    }
}

fn flow_direction_str(d: &grapha_core::graph::FlowDirection) -> &'static str {
    use grapha_core::graph::FlowDirection;
    match d {
        FlowDirection::Read => "read",
        FlowDirection::Write => "write",
        FlowDirection::ReadWrite => "read_write",
        FlowDirection::Pure => "pure",
    }
}

/// Deserialize a snake_case string back into a serde enum value.
fn str_to_enum<T: serde::de::DeserializeOwned>(s: &str) -> anyhow::Result<T> {
    let quoted = format!("\"{s}\"");
    Ok(serde_json::from_str(&quoted)?)
}

impl Store for SqliteStore {
    fn save(&self, graph: &Graph) -> anyhow::Result<()> {
        self.save_full(graph)
    }

    fn save_incremental(
        &self,
        previous: Option<&Graph>,
        graph: &Graph,
    ) -> anyhow::Result<StoreWriteStats> {
        let current_stats =
            StoreWriteStats::from_graphs(previous, graph, crate::delta::SyncMode::Incremental);
        let full_stats =
            StoreWriteStats::from_graphs(previous, graph, crate::delta::SyncMode::FullRebuild);

        let conn = self.open_for_write()?;
        let schema_version = Self::schema_version(&conn)?;
        if previous.is_none() || schema_version.as_deref() != Some(STORE_SCHEMA_VERSION) {
            drop(conn);
            self.save_full(graph)?;
            return Ok(full_stats);
        }

        let previous_graph = previous.expect("checked is_some above");
        let delta = GraphDelta::between(previous_graph, graph);
        let tx = conn.unchecked_transaction()?;
        Self::write_meta(&tx, graph)?;

        {
            let mut delete_edges = tx.prepare("DELETE FROM edges WHERE edge_id = ?1")?;
            for edge_id in &delta.deleted_edge_ids {
                delete_edges.execute([edge_id])?;
            }
        }

        {
            let mut delete_nodes = tx.prepare("DELETE FROM nodes WHERE id = ?1")?;
            for node_id in &delta.deleted_node_ids {
                delete_nodes.execute([node_id])?;
            }
        }

        let mut changed_nodes = Vec::new();
        changed_nodes.extend(delta.added_nodes.iter().copied().cloned());
        changed_nodes.extend(delta.updated_nodes.iter().copied().cloned());
        Self::insert_nodes(&tx, &changed_nodes, true)?;

        {
            let mut delete_edges = tx.prepare("DELETE FROM edges WHERE edge_id = ?1")?;
            for edge in &delta.updated_edges {
                delete_edges.execute([&edge.id])?;
            }
        }
        Self::insert_edges(
            &tx,
            delta
                .added_edges
                .iter()
                .chain(delta.updated_edges.iter())
                .map(|edge| (edge.id.clone(), edge.edge)),
            true,
        )?;
        tx.commit()?;
        conn.execute_batch("PRAGMA optimize;")?;

        Ok(current_stats)
    }

    fn load(&self) -> anyhow::Result<Graph> {
        self.load_with_edge_filter(None)
    }
}

impl SqliteStore {
    /// Load the graph with optional filters to reduce I/O and deserialization cost.
    ///
    /// - `edge_kinds`: restrict which edge kinds are loaded (None = all)
    /// - `metadata_key_prefix`: when set, only deserialize metadata for nodes whose raw
    ///   metadata contains this prefix; other nodes get an empty map. Also skips loading
    ///   signature, doc_comment, and snippet columns.
    pub fn load_with_edge_filter(&self, edge_kinds: Option<&[EdgeKind]>) -> anyhow::Result<Graph> {
        self.load_filtered(edge_kinds, None)
    }

    pub fn load_filtered(
        &self,
        edge_kinds: Option<&[EdgeKind]>,
        metadata_key_prefix: Option<&str>,
    ) -> anyhow::Result<Graph> {
        let conn = self.open()?;
        let schema_version = Self::schema_version(&conn)?;
        Self::create_tables(&conn)?;

        let version: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'version'", [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|_| "0.1.0".to_string());

        let nodes = {
            let node_sql = if let Some(prefix) = metadata_key_prefix {
                format!(
                    "SELECT id, kind, name, file,
                            span_start_line, span_start_col, span_end_line, span_end_col,
                            visibility,
                            CASE WHEN metadata LIKE '%{prefix}%' THEN metadata ELSE '{{}}' END,
                            role, NULL, NULL, module, NULL
                     FROM nodes"
                )
            } else {
                "SELECT id, kind, name, file,
                        span_start_line, span_start_col, span_end_line, span_end_col,
                        visibility, metadata, role, signature, doc_comment, module, snippet
                 FROM nodes"
                    .to_string()
            };
            let mut stmt = conn.prepare(&node_sql)?;
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
                    row.get::<_, Option<String>>(14)?,
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
                    snippet,
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
                    snippet,
                });
            }
            nodes
        };

        let edge_where = edge_kinds
            .filter(|kinds| !kinds.is_empty())
            .map(|kinds| {
                let values: Vec<_> = kinds
                    .iter()
                    .map(|k| format!("'{}'", edge_kind_str(k)))
                    .collect();
                format!(" WHERE kind IN ({})", values.join(", "))
            })
            .unwrap_or_default();

        let edges = if matches!(
            schema_version.as_deref(),
            Some(STORE_SCHEMA_VERSION) | Some(BINARY_PROVENANCE_SCHEMA_VERSION)
        ) {
            let mut stmt = conn.prepare(&format!(
                "SELECT source, target, kind, confidence,
                        direction, operation, condition, async_boundary, provenance
                 FROM edges{edge_where}",
            ))?;
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
                    row.get::<_, Vec<u8>>(8)?,
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
                    provenance_blob,
                ) = row?;
                let kind: EdgeKind = str_to_enum(&kind_str)
                    .map_err(|e| anyhow::anyhow!("invalid edge kind '{kind_str}': {e}"))?;
                let direction = direction_str
                    .map(|s| str_to_enum(&s))
                    .transpose()
                    .map_err(|e| anyhow::anyhow!("invalid flow direction: {e}"))?;
                let async_boundary = async_boundary_int.map(|v| v != 0);
                let provenance = deserialize_provenance(&provenance_blob)?;
                edges.push(Edge {
                    source,
                    target,
                    kind,
                    confidence,
                    direction,
                    operation,
                    condition,
                    async_boundary,
                    provenance,
                });
            }
            edges
        } else if schema_version.as_deref() == Some("4") {
            let mut stmt = conn.prepare(&format!(
                "SELECT source, target, kind, confidence,
                        direction, operation, condition, async_boundary, provenance
                 FROM edges{edge_where}",
            ))?;
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
                    row.get::<_, String>(8)?,
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
                    provenance_json,
                ) = row?;
                let kind: EdgeKind = str_to_enum(&kind_str)
                    .map_err(|e| anyhow::anyhow!("invalid edge kind '{kind_str}': {e}"))?;
                let direction = direction_str
                    .map(|s| str_to_enum(&s))
                    .transpose()
                    .map_err(|e| anyhow::anyhow!("invalid flow direction: {e}"))?;
                let async_boundary = async_boundary_int.map(|v| v != 0);
                let provenance: Vec<EdgeProvenance> = serde_json::from_str(&provenance_json)?;
                edges.push(Edge {
                    source,
                    target,
                    kind,
                    confidence,
                    direction,
                    operation,
                    condition,
                    async_boundary,
                    provenance,
                });
            }
            edges
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT source, target, kind, confidence,
                        direction, operation, condition, async_boundary
                 FROM edges{edge_where}",
            ))?;
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
                    provenance: Vec::new(),
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
    use std::path::Path;

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
                snippet: None,
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
                provenance: vec![EdgeProvenance {
                    file: "test.rs".into(),
                    span: Span {
                        start: [2, 4],
                        end: [2, 10],
                    },
                    symbol_id: "test.rs::main".to_string(),
                }],
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
        assert_eq!(loaded.edges[0].provenance, graph.edges[0].provenance);
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
                    snippet: None,
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
                    snippet: None,
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
                    snippet: None,
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
                    provenance: vec![EdgeProvenance {
                        file: "api.rs".into(),
                        span: Span {
                            start: [4, 8],
                            end: [4, 18],
                        },
                        symbol_id: "api::handler".to_string(),
                    }],
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
                    provenance: Vec::new(),
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
                    provenance: Vec::new(),
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
        assert_eq!(read_edge.provenance, graph.edges[0].provenance);

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
                snippet: None,
            }],
            edges: vec![],
        };
        store.save(&graph1).unwrap();

        let graph2 = Graph::new();
        store.save(&graph2).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.nodes.len(), 0);
    }

    #[test]
    fn sqlite_incremental_save_updates_added_updated_and_deleted_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grapha.db");
        let store = SqliteStore::new(path);

        let previous = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
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
                    snippet: None,
                },
                Node {
                    id: "b".to_string(),
                    kind: NodeKind::Function,
                    name: "b".to_string(),
                    file: "b.rs".into(),
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
                    snippet: None,
                },
            ],
            edges: vec![Edge {
                source: "a".to_string(),
                target: "b".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.8,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        };
        store.save(&previous).unwrap();

        let mut updated_a = previous.nodes[0].clone();
        updated_a.signature = Some("fn a()".to_string());
        let next = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                updated_a,
                Node {
                    id: "c".to_string(),
                    kind: NodeKind::Function,
                    name: "c".to_string(),
                    file: "c.rs".into(),
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
                    snippet: None,
                },
            ],
            edges: vec![
                Edge {
                    source: "a".to_string(),
                    target: "b".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.95,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "a".to_string(),
                    target: "c".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.7,
                    direction: Some(FlowDirection::Pure),
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let stats = store.save_incremental(Some(&previous), &next).unwrap();
        assert_eq!(stats.mode, crate::delta::SyncMode::Incremental);
        assert_eq!(
            stats.nodes,
            crate::delta::EntitySyncStats {
                added: 1,
                updated: 1,
                deleted: 1,
            }
        );
        assert_eq!(
            stats.edges,
            crate::delta::EntitySyncStats {
                added: 1,
                updated: 1,
                deleted: 0,
            }
        );

        let loaded = store.load().unwrap();
        assert_eq!(loaded.nodes.len(), 2);
        assert!(loaded.nodes.iter().any(|node| node.id == "c"));
        assert!(loaded.nodes.iter().all(|node| node.id != "b"));
        let edge = loaded
            .edges
            .iter()
            .find(|edge| edge.target == "b")
            .expect("updated edge should exist");
        assert_eq!(edge.confidence, 0.95);
    }

    #[test]
    fn sqlite_incremental_save_rebuilds_legacy_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE nodes (
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
            CREATE TABLE edges (
                source     TEXT NOT NULL,
                target     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                confidence REAL NOT NULL,
                direction  TEXT,
                operation  TEXT,
                condition  TEXT,
                async_boundary INTEGER
            );
            INSERT INTO meta (key, value) VALUES ('version', '0.1.0');",
        )
        .unwrap();
        drop(conn);

        let store = SqliteStore::new(path.clone());
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "main".to_string(),
                kind: NodeKind::Function,
                name: "main".to_string(),
                file: Path::new("main.rs").into(),
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
                snippet: None,
            }],
            edges: vec![],
        };

        let stats = store.save_incremental(None, &graph).unwrap();
        assert_eq!(stats.mode, crate::delta::SyncMode::FullRebuild);

        let conn = Connection::open(path).unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'store_schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, STORE_SCHEMA_VERSION);
        let edge_columns: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('edges') WHERE name = 'edge_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_columns, 1);
    }

    #[test]
    fn sqlite_load_reads_schema_v4_json_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema-v4.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE nodes (
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
                module     TEXT,
                snippet    TEXT
            );
            CREATE TABLE edges (
                edge_id    TEXT PRIMARY KEY,
                source     TEXT NOT NULL,
                target     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                confidence REAL NOT NULL,
                direction  TEXT,
                operation  TEXT,
                condition  TEXT,
                async_boundary INTEGER,
                provenance TEXT NOT NULL
            );
            INSERT INTO meta (key, value) VALUES ('version', '0.1.0');
            INSERT INTO meta (key, value) VALUES ('store_schema_version', '4');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nodes (
                id, kind, name, file,
                span_start_line, span_start_col, span_end_line, span_end_col,
                visibility, metadata, role, signature, doc_comment, module, snippet
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                "main",
                "function",
                "main",
                "main.swift",
                0_i64,
                0_i64,
                1_i64,
                0_i64,
                "public",
                "{}",
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (
                edge_id, source, target, kind, confidence,
                direction, operation, condition, async_boundary, provenance
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                "main::calls::helper",
                "main",
                "helper",
                "calls",
                1.0_f64,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<i64>::None,
                r#"[{"file":"main.swift","span":{"start":[0,0],"end":[0,4]},"symbol_id":"main"}]"#,
            ],
        )
        .unwrap();
        drop(conn);

        let store = SqliteStore::new(path);
        let loaded = store.load().unwrap();

        assert_eq!(loaded.nodes.len(), 1);
        assert_eq!(loaded.edges.len(), 1);
        assert_eq!(loaded.edges[0].source, "main");
        assert_eq!(
            loaded.edges[0].provenance,
            vec![EdgeProvenance {
                file: Path::new("main.swift").into(),
                span: Span {
                    start: [0, 0],
                    end: [0, 4],
                },
                symbol_id: "main".to_string(),
            }]
        );
    }

    #[test]
    fn sqlite_load_reads_schema_v5_binary_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema-v5.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE nodes (
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
                module     TEXT,
                snippet    TEXT
            );
            CREATE TABLE edges (
                edge_id    TEXT PRIMARY KEY,
                source     TEXT NOT NULL,
                target     TEXT NOT NULL,
                kind       TEXT NOT NULL,
                confidence REAL NOT NULL,
                direction  TEXT,
                operation  TEXT,
                condition  TEXT,
                async_boundary INTEGER,
                provenance BLOB NOT NULL
            );
            INSERT INTO meta (key, value) VALUES ('version', '0.1.0');
            INSERT INTO meta (key, value) VALUES ('store_schema_version', '5');",
        )
        .unwrap();
        let provenance = vec![EdgeProvenance {
            file: Path::new("main.swift").into(),
            span: Span {
                start: [0, 0],
                end: [0, 4],
            },
            symbol_id: "main".to_string(),
        }];
        conn.execute(
            "INSERT INTO nodes (
                id, kind, name, file,
                span_start_line, span_start_col, span_end_line, span_end_col,
                visibility, metadata, role, signature, doc_comment, module, snippet
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                "main",
                "function",
                "main",
                "main.swift",
                0_i64,
                0_i64,
                1_i64,
                0_i64,
                "public",
                "{}",
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edges (
                edge_id, source, target, kind, confidence,
                direction, operation, condition, async_boundary, provenance
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                "main::calls::helper",
                "main",
                "helper",
                "calls",
                1.0_f64,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<i64>::None,
                serialize_provenance(&provenance).unwrap(),
            ],
        )
        .unwrap();
        drop(conn);

        let store = SqliteStore::new(path);
        let loaded = store.load().unwrap();

        assert_eq!(loaded.nodes.len(), 1);
        assert_eq!(loaded.edges.len(), 1);
        assert_eq!(loaded.edges[0].provenance, provenance);
    }

    #[test]
    fn sqlite_full_rebuild_uses_large_page_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page-size.db");
        let store = SqliteStore::new(path.clone());
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "main".to_string(),
                kind: NodeKind::Function,
                name: "main".to_string(),
                file: Path::new("main.rs").into(),
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
                snippet: None,
            }],
            edges: vec![],
        };

        store.save(&graph).unwrap();

        let conn = Connection::open(path).unwrap();
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(page_size, 8192);
    }

    #[test]
    fn sqlite_full_rebuild_drops_unused_secondary_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index-shape.db");
        let store = SqliteStore::new(path.clone());
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "main".to_string(),
                kind: NodeKind::Function,
                name: "main".to_string(),
                file: Path::new("main.rs").into(),
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
                snippet: None,
            }],
            edges: vec![Edge {
                source: "main".to_string(),
                target: "helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        };

        store.save(&graph).unwrap();

        let conn = Connection::open(path).unwrap();
        let indexes: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name
                     FROM sqlite_master
                     WHERE type = 'index'
                     ORDER BY name",
                )
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        assert!(indexes.iter().any(|name| name == "idx_nodes_id"));
        assert!(indexes.iter().any(|name| name == "idx_edges_id"));
        assert!(!indexes.iter().any(|name| name == "idx_edges_source"));
        assert!(!indexes.iter().any(|name| name == "idx_edges_target"));
        assert!(!indexes.iter().any(|name| name == "idx_edges_kind"));
        assert!(!indexes.iter().any(|name| name == "idx_nodes_name"));
        assert!(!indexes.iter().any(|name| name == "idx_nodes_file"));
        assert!(!indexes.iter().any(|name| name == "idx_nodes_kind"));
        assert!(!indexes.iter().any(|name| name == "idx_nodes_role"));
        assert!(!indexes.iter().any(|name| name == "idx_nodes_module"));
    }

    #[test]
    fn sqlite_batch_insert_round_trips_large_graph() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("batch.db");
        let store = SqliteStore::new(path);

        let node_count = 1600;
        let edge_count = 800;
        let nodes: Vec<Node> = (0..node_count)
            .map(|i| {
                let snippet = if i % 3 == 0 {
                    Some(format!("fn node_{i}() {{ }}"))
                } else {
                    None
                };
                Node {
                    id: format!("mod::node_{i}"),
                    kind: NodeKind::Function,
                    name: format!("node_{i}"),
                    file: format!("file_{}.rs", i % 10).into(),
                    span: Span {
                        start: [i, 0],
                        end: [i + 5, 1],
                    },
                    visibility: Visibility::Public,
                    metadata: if i % 5 == 0 {
                        HashMap::from([("key".to_string(), format!("val_{i}"))])
                    } else {
                        HashMap::new()
                    },
                    role: if i == 0 {
                        Some(NodeRole::EntryPoint)
                    } else {
                        None
                    },
                    signature: Some(format!("fn node_{i}()")),
                    doc_comment: None,
                    module: Some("mod".to_string()),
                    snippet,
                }
            })
            .collect();
        let edges: Vec<Edge> = (0..edge_count)
            .map(|i| Edge {
                source: format!("mod::node_{i}"),
                target: format!("mod::node_{}", i + 1),
                kind: EdgeKind::Calls,
                confidence: 0.9,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            })
            .collect();

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes,
            edges,
        };

        store.save(&graph).unwrap();
        let loaded = store.load().unwrap();

        assert_eq!(loaded.nodes.len(), node_count);
        assert_eq!(loaded.edges.len(), edge_count);

        // Verify first and last node data
        let first = loaded.nodes.iter().find(|n| n.id == "mod::node_0").unwrap();
        assert_eq!(first.role, Some(NodeRole::EntryPoint));
        assert_eq!(first.signature.as_deref(), Some("fn node_0()"));
        assert_eq!(first.module.as_deref(), Some("mod"));
        assert_eq!(first.snippet.as_deref(), Some("fn node_0() { }"));

        let last = loaded
            .nodes
            .iter()
            .find(|n| n.id == format!("mod::node_{}", node_count - 1))
            .unwrap();
        assert_eq!(
            last.signature.as_deref(),
            Some(format!("fn node_{}()", node_count - 1).as_str())
        );
        // node_count - 1 = 1599, 1599 % 3 == 0, so snippet should be present
        assert_eq!(
            last.snippet.as_deref(),
            Some(format!("fn node_{}() {{ }}", node_count - 1).as_str())
        );

        // Verify node without snippet
        let no_snippet = loaded.nodes.iter().find(|n| n.id == "mod::node_1").unwrap();
        assert_eq!(no_snippet.snippet, None);

        // Verify metadata round-trip
        let with_meta = loaded.nodes.iter().find(|n| n.id == "mod::node_0").unwrap();
        assert_eq!(
            with_meta.metadata.get("key").map(|s| s.as_str()),
            Some("val_0")
        );

        // Verify edge data
        let edge = loaded
            .edges
            .iter()
            .find(|e| e.source == "mod::node_0")
            .unwrap();
        assert_eq!(edge.target, "mod::node_1");
        assert_eq!(edge.confidence, 0.9);
    }

    #[test]
    fn sqlite_snippet_field_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snippet.db");
        let store = SqliteStore::new(path);

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "a".to_string(),
                    kind: NodeKind::Function,
                    name: "a".to_string(),
                    file: "a.rs".into(),
                    span: Span {
                        start: [0, 0],
                        end: [3, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: HashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: Some("fn a() {\n    println!(\"hello\");\n}".to_string()),
                },
                Node {
                    id: "b".to_string(),
                    kind: NodeKind::Struct,
                    name: "b".to_string(),
                    file: "b.rs".into(),
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
                    snippet: None,
                },
            ],
            edges: vec![],
        };

        store.save(&graph).unwrap();
        let loaded = store.load().unwrap();

        let node_a = loaded.nodes.iter().find(|n| n.id == "a").unwrap();
        assert_eq!(
            node_a.snippet.as_deref(),
            Some("fn a() {\n    println!(\"hello\");\n}")
        );

        let node_b = loaded.nodes.iter().find(|n| n.id == "b").unwrap();
        assert_eq!(node_b.snippet, None);
    }

    #[test]
    fn load_with_edge_filter_only_loads_matching_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grapha_filter.db");
        let store = SqliteStore::new(path);

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "a".to_string(),
                kind: NodeKind::Struct,
                name: "A".to_string(),
                file: "a.swift".into(),
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
                snippet: None,
            }],
            edges: vec![
                Edge {
                    source: "a".to_string(),
                    target: "b".to_string(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "a".to_string(),
                    target: "c".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "b".to_string(),
                    target: "d".to_string(),
                    kind: EdgeKind::TypeRef,
                    confidence: 0.8,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        store.save(&graph).unwrap();

        let full = store.load().unwrap();
        assert_eq!(full.edges.len(), 3);

        let filtered = store
            .load_with_edge_filter(Some(&[EdgeKind::Contains, EdgeKind::TypeRef]))
            .unwrap();
        assert_eq!(filtered.nodes.len(), 1, "all nodes should still be loaded");
        assert_eq!(
            filtered.edges.len(),
            2,
            "only Contains and TypeRef edges should be loaded"
        );
        assert!(
            filtered
                .edges
                .iter()
                .all(|e| matches!(e.kind, EdgeKind::Contains | EdgeKind::TypeRef))
        );
    }

    #[test]
    fn load_filtered_skips_metadata_for_non_matching_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grapha_slim.db");
        let store = SqliteStore::new(path);

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "a".to_string(),
                    kind: NodeKind::View,
                    name: "Text".to_string(),
                    file: "a.swift".into(),
                    span: Span {
                        start: [0, 0],
                        end: [1, 0],
                    },
                    visibility: Visibility::Private,
                    metadata: HashMap::from([
                        ("l10n.ref_kind".to_string(), "literal".to_string()),
                        ("l10n.literal".to_string(), "Hello".to_string()),
                    ]),
                    role: None,
                    signature: Some("func body".to_string()),
                    doc_comment: Some("A doc comment".to_string()),
                    module: Some("App".to_string()),
                    snippet: Some("Text(\"Hello\")".to_string()),
                },
                Node {
                    id: "b".to_string(),
                    kind: NodeKind::Struct,
                    name: "ContentView".to_string(),
                    file: "b.swift".into(),
                    span: Span {
                        start: [0, 0],
                        end: [10, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: HashMap::from([("async".to_string(), "false".to_string())]),
                    role: None,
                    signature: Some("struct ContentView: View".to_string()),
                    doc_comment: Some("Main view".to_string()),
                    module: Some("App".to_string()),
                    snippet: Some("struct ContentView: View { ... }".to_string()),
                },
            ],
            edges: Vec::new(),
        };

        store.save(&graph).unwrap();

        let slim = store.load_filtered(None, Some("l10n.")).unwrap();
        assert_eq!(slim.nodes.len(), 2, "all nodes should be loaded");

        let l10n_node = slim.nodes.iter().find(|n| n.id == "a").unwrap();
        assert_eq!(
            l10n_node.metadata.get("l10n.ref_kind").map(|s| s.as_str()),
            Some("literal"),
            "l10n node should retain its metadata"
        );
        assert!(
            l10n_node.signature.is_none(),
            "signature should be skipped in slim mode"
        );
        assert!(
            l10n_node.snippet.is_none(),
            "snippet should be skipped in slim mode"
        );

        let other_node = slim.nodes.iter().find(|n| n.id == "b").unwrap();
        assert!(
            other_node.metadata.is_empty(),
            "non-l10n node should have empty metadata, got: {:?}",
            other_node.metadata
        );
        assert!(
            other_node.signature.is_none(),
            "signature should be skipped in slim mode"
        );
    }
}
