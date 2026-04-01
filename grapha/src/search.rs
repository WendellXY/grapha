use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, FuzzyTermQuery, Occur, QueryParser, TermQuery};
use tantivy::schema::{IndexRecordOption, STORED, STRING, Schema, TEXT, Value};
use tantivy::{Index, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};

use crate::delta::{EntitySyncStats, GraphDelta, SyncMode};
use grapha_core::graph::Graph;
use grapha_core::graph::{Node, NodeRole};

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub file: String,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Default)]
pub struct SearchOptions {
    pub kind: Option<String>,
    pub module: Option<String>,
    pub file_glob: Option<String>,
    pub role: Option<String>,
    pub fuzzy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchSyncStats {
    pub mode: SyncMode,
    pub documents: EntitySyncStats,
}

impl SearchSyncStats {
    pub fn from_graphs(previous: Option<&Graph>, graph: &Graph, mode: SyncMode) -> Self {
        let documents = match previous {
            Some(previous_graph) => GraphDelta::between(previous_graph, graph).node_stats(),
            None => EntitySyncStats::from_total(graph.nodes.len()),
        };
        Self { mode, documents }
    }

    pub fn summary(self) -> String {
        format!(
            "{} docs +{} ~{} -{}",
            self.mode.label(),
            self.documents.added,
            self.documents.updated,
            self.documents.deleted
        )
    }
}

#[derive(Clone, Copy)]
struct SearchFields {
    id: tantivy::schema::Field,
    name: tantivy::schema::Field,
    kind: tantivy::schema::Field,
    file: tantivy::schema::Field,
    module: tantivy::schema::Field,
    visibility: tantivy::schema::Field,
    role: tantivy::schema::Field,
}

fn schema() -> (Schema, SearchFields) {
    let mut schema_builder = Schema::builder();
    let id = schema_builder.add_text_field("id", STRING | STORED);
    let name = schema_builder.add_text_field("name", TEXT | STORED);
    let kind = schema_builder.add_text_field("kind", STRING | STORED);
    let file = schema_builder.add_text_field("file", TEXT | STORED);
    let module = schema_builder.add_text_field("module", STRING | STORED);
    let visibility = schema_builder.add_text_field("visibility", STRING | STORED);
    let role = schema_builder.add_text_field("role", STRING | STORED);
    (
        schema_builder.build(),
        SearchFields {
            id,
            name,
            kind,
            file,
            module,
            visibility,
            role,
        },
    )
}

fn index_writer(index: &Index) -> Result<IndexWriter> {
    Ok(index.writer(50_000_000)?)
}

fn role_to_string(role: &Option<NodeRole>) -> String {
    match role {
        Some(NodeRole::EntryPoint) => "entry_point".to_string(),
        Some(NodeRole::Terminal { .. }) => "terminal".to_string(),
        _ => String::new(),
    }
}

fn node_document(fields: SearchFields, node: &Node) -> Result<TantivyDocument> {
    let kind_str = serde_json::to_string(&node.kind)?
        .trim_matches('"')
        .to_string();
    let visibility_str = serde_json::to_string(&node.visibility)?
        .trim_matches('"')
        .to_string();
    Ok(doc!(
        fields.id => node.id.clone(),
        fields.name => node.name.clone(),
        fields.kind => kind_str,
        fields.file => node.file.to_string_lossy().to_string(),
        fields.module => node.module.clone().unwrap_or_default(),
        fields.visibility => visibility_str,
        fields.role => role_to_string(&node.role),
    ))
}

fn rebuild_index_impl(graph: &Graph, index_path: &Path) -> Result<Index> {
    if index_path.exists() {
        std::fs::remove_dir_all(index_path)?;
    }
    std::fs::create_dir_all(index_path)?;
    let (schema, fields) = schema();
    let index = Index::create_in_dir(index_path, schema)?;
    let mut writer = index_writer(&index)?;
    for node in &graph.nodes {
        writer.add_document(node_document(fields, node)?)?;
    }
    writer.commit()?;
    Ok(index)
}

pub fn build_index(graph: &Graph, index_path: &Path) -> Result<Index> {
    rebuild_index_impl(graph, index_path)
}

pub fn sync_index(
    previous: Option<&Graph>,
    graph: &Graph,
    index_path: &Path,
    force_full_rebuild: bool,
    precomputed_delta: Option<&GraphDelta>,
) -> Result<SearchSyncStats> {
    let full_stats = SearchSyncStats::from_graphs(previous, graph, SyncMode::FullRebuild);
    if force_full_rebuild || previous.is_none() || !index_path.exists() {
        rebuild_index_impl(graph, index_path)?;
        return Ok(full_stats);
    }

    let previous_graph = previous.expect("checked is_some above");
    let owned_delta;
    let delta = match precomputed_delta {
        Some(d) => d,
        None => {
            owned_delta = GraphDelta::between(previous_graph, graph);
            &owned_delta
        }
    };
    let incremental_stats = SearchSyncStats {
        mode: SyncMode::Incremental,
        documents: delta.node_stats(),
    };
    let index = match Index::open_in_dir(index_path) {
        Ok(index) => index,
        Err(_) => {
            rebuild_index_impl(graph, index_path)?;
            return Ok(full_stats);
        }
    };
    let fields = match resolve_fields(&index) {
        Ok(fields) => fields,
        Err(_) => {
            rebuild_index_impl(graph, index_path)?;
            return Ok(full_stats);
        }
    };

    let mut writer = index_writer(&index)?;
    for node_id in &delta.deleted_node_ids {
        writer.delete_term(Term::from_field_text(fields.id, node_id));
    }
    for node in &delta.updated_nodes {
        writer.delete_term(Term::from_field_text(fields.id, &node.id));
    }
    for node in delta
        .added_nodes
        .iter()
        .copied()
        .chain(delta.updated_nodes.iter().copied())
    {
        writer.add_document(node_document(fields, node)?)?;
    }
    writer.commit()?;

    Ok(incremental_stats)
}

fn resolve_fields(index: &Index) -> Result<SearchFields> {
    let schema = index.schema();
    Ok(SearchFields {
        id: schema.get_field("id")?,
        name: schema.get_field("name")?,
        kind: schema.get_field("kind")?,
        file: schema.get_field("file")?,
        module: schema.get_field("module")?,
        visibility: schema.get_field("visibility")?,
        role: schema.get_field("role")?,
    })
}

pub fn search(index: &Index, query_str: &str, limit: usize) -> Result<Vec<SearchResult>> {
    search_filtered(index, query_str, limit, &SearchOptions::default())
}

pub fn search_filtered(
    index: &Index,
    query_str: &str,
    limit: usize,
    options: &SearchOptions,
) -> Result<Vec<SearchResult>> {
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()?;
    let searcher = reader.searcher();

    let fields = resolve_fields(index)?;

    let text_query: Box<dyn tantivy::query::Query> = if options.fuzzy {
        let term = Term::from_field_text(fields.name, query_str);
        Box::new(FuzzyTermQuery::new(term, 2, true))
    } else {
        let query_parser = QueryParser::for_index(index, vec![fields.name, fields.file]);
        Box::new(query_parser.parse_query(query_str)?)
    };

    let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = vec![(Occur::Must, text_query)];

    if let Some(ref kind_filter) = options.kind {
        let term = Term::from_field_text(fields.kind, kind_filter);
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
        ));
    }
    if let Some(ref module_filter) = options.module {
        let term = Term::from_field_text(fields.module, module_filter);
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
        ));
    }
    if let Some(ref role_filter) = options.role {
        let term = Term::from_field_text(fields.role, role_filter);
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
        ));
    }

    let final_query = BooleanQuery::new(clauses);
    let top_docs = searcher.search(&final_query, &TopDocs::with_limit(limit))?;

    let mut results = Vec::new();
    for (score, doc_address) in top_docs {
        let doc: TantivyDocument = searcher.doc(doc_address)?;
        let get_str = |field| {
            doc.get_first(field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let module_val = get_str(fields.module);
        let role_val = get_str(fields.role);
        results.push(SearchResult {
            id: get_str(fields.id),
            name: get_str(fields.name),
            kind: get_str(fields.kind),
            file: get_str(fields.file),
            score,
            module: if module_val.is_empty() {
                None
            } else {
                Some(module_val)
            },
            role: if role_val.is_empty() {
                None
            } else {
                Some(role_val)
            },
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
    use std::collections::HashMap;
    use tantivy::collector::Count;
    use tantivy::query::AllQuery;

    fn make_test_graph() -> Graph {
        let mk = |id: &str, name: &str, kind: NodeKind, file: &str| Node {
            id: id.into(),
            kind,
            name: name.into(),
            file: file.into(),
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
        };
        Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                mk("a.rs::Config", "Config", NodeKind::Struct, "a.rs"),
                mk(
                    "a.rs::default_config",
                    "default_config",
                    NodeKind::Function,
                    "a.rs",
                ),
                mk("b.rs::run", "run", NodeKind::Function, "b.rs"),
            ],
            edges: vec![],
        }
    }

    fn make_rich_test_graph() -> Graph {
        let mk = |id: &str,
                  name: &str,
                  kind: NodeKind,
                  file: &str,
                  module: Option<&str>,
                  role: Option<NodeRole>| Node {
            id: id.into(),
            kind,
            name: name.into(),
            file: file.into(),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role,
            signature: None,
            doc_comment: None,
            module: module.map(String::from),
            snippet: None,
        };
        Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                mk(
                    "app::AppView",
                    "AppView",
                    NodeKind::Struct,
                    "Sources/App/AppView.swift",
                    Some("App"),
                    Some(NodeRole::EntryPoint),
                ),
                mk(
                    "app::fetch_data",
                    "fetch_data",
                    NodeKind::Function,
                    "Sources/App/Network.swift",
                    Some("App"),
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Network,
                    }),
                ),
                mk(
                    "core::Config",
                    "Config",
                    NodeKind::Struct,
                    "Sources/Core/Config.swift",
                    Some("Core"),
                    None,
                ),
                mk(
                    "core::save_config",
                    "save_config",
                    NodeKind::Function,
                    "Sources/Core/Persist.swift",
                    Some("Core"),
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Persistence,
                    }),
                ),
            ],
            edges: vec![],
        }
    }

    #[test]
    fn search_finds_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let results = search(&index, "Config", 10).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.name == "Config"));
    }

    #[test]
    fn search_returns_empty_for_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let results = search(&index, "zzzznonexistent", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn sync_index_updates_added_updated_and_deleted_documents() {
        let dir = tempfile::tempdir().unwrap();
        let previous = make_test_graph();
        build_index(&previous, dir.path()).unwrap();

        let mut updated_node = previous.nodes[0].clone();
        updated_node.name = "RuntimeConfig".to_string();
        let next = Graph {
            version: previous.version.clone(),
            nodes: vec![
                updated_node,
                previous.nodes[2].clone(),
                Node {
                    id: "c.rs::fresh".to_string(),
                    kind: NodeKind::Function,
                    name: "fresh".to_string(),
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
            edges: vec![],
        };

        let stats = sync_index(Some(&previous), &next, dir.path(), false, None).unwrap();
        assert_eq!(stats.mode, SyncMode::Incremental);
        assert_eq!(
            stats.documents,
            EntitySyncStats {
                added: 1,
                updated: 1,
                deleted: 1,
            }
        );

        let index = Index::open_in_dir(dir.path()).unwrap();
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        assert_eq!(searcher.search(&AllQuery, &Count).unwrap(), 3);

        let results = search(&index, "RuntimeConfig", 10).unwrap();
        assert_eq!(results.len(), 1);
        let deleted = search(&index, "default_config", 10).unwrap();
        assert!(deleted.is_empty());
    }

    #[test]
    fn search_without_filters_backward_compat() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let results = search(&index, "Config", 10).unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.name == "Config"));
    }

    #[test]
    fn filter_by_kind() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let options = SearchOptions {
            kind: Some("struct".into()),
            ..Default::default()
        };
        let results = search_filtered(&index, "Config", 10, &options).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Config");
        assert_eq!(results[0].kind, "struct");
    }

    #[test]
    fn filter_by_module() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let options = SearchOptions {
            module: Some("Core".into()),
            ..Default::default()
        };
        let results = search_filtered(&index, "config", 10, &options).unwrap();
        assert!(!results.is_empty());
        for r in &results {
            assert_eq!(r.module.as_deref(), Some("Core"));
        }
    }

    #[test]
    fn filter_by_role_entry_point() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let options = SearchOptions {
            role: Some("entry_point".into()),
            ..Default::default()
        };
        let results = search_filtered(&index, "AppView", 10, &options).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "AppView");
        assert_eq!(results[0].role.as_deref(), Some("entry_point"));
    }

    #[test]
    fn filter_by_role_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let options = SearchOptions {
            role: Some("terminal".into()),
            ..Default::default()
        };
        let results = search_filtered(&index, "fetch_data", 10, &options).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role.as_deref(), Some("terminal"));
    }

    #[test]
    fn fuzzy_search_finds_misspelled() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let options = SearchOptions {
            fuzzy: true,
            ..Default::default()
        };
        let results = search_filtered(&index, "confg", 10, &options).unwrap();
        assert!(
            results.iter().any(|r| r.name == "Config"),
            "fuzzy search should find 'Config' for misspelling 'confg', got: {:?}",
            results.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn combined_kind_and_module_filter() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        let options = SearchOptions {
            kind: Some("function".into()),
            module: Some("App".into()),
            ..Default::default()
        };
        let results = search_filtered(&index, "fetch_data", 10, &options).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "fetch_data");
        assert_eq!(results[0].module.as_deref(), Some("App"));
    }

    #[test]
    fn filter_excludes_non_matching() {
        let dir = tempfile::tempdir().unwrap();
        let graph = make_rich_test_graph();
        let index = build_index(&graph, dir.path()).unwrap();
        // AppView is a struct; filtering by kind=function should exclude it
        let options = SearchOptions {
            kind: Some("function".into()),
            ..Default::default()
        };
        let results = search_filtered(&index, "AppView", 10, &options).unwrap();
        assert!(results.is_empty());
    }
}
