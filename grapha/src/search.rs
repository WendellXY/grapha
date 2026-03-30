use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{STORED, STRING, Schema, TEXT, Value};
use tantivy::{Index, IndexWriter, ReloadPolicy, doc};

use grapha_core::graph::Graph;

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub file: String,
    pub score: f32,
}

pub fn build_index(graph: &Graph, index_path: &Path) -> Result<Index> {
    std::fs::create_dir_all(index_path)?;

    let mut schema_builder = Schema::builder();
    let id_field = schema_builder.add_text_field("id", STRING | STORED);
    let name_field = schema_builder.add_text_field("name", TEXT | STORED);
    let kind_field = schema_builder.add_text_field("kind", STRING | STORED);
    let file_field = schema_builder.add_text_field("file", TEXT | STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_dir(index_path, schema)?;
    let mut writer: IndexWriter = index.writer(50_000_000)?;

    for node in &graph.nodes {
        let kind_str = serde_json::to_string(&node.kind)?
            .trim_matches('"')
            .to_string();
        writer.add_document(doc!(
            id_field => node.id.clone(),
            name_field => node.name.clone(),
            kind_field => kind_str,
            file_field => node.file.to_string_lossy().to_string(),
        ))?;
    }

    writer.commit()?;
    Ok(index)
}

pub fn search(index: &Index, query_str: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()?;
    let searcher = reader.searcher();

    let schema = index.schema();
    let name_field = schema.get_field("name")?;
    let file_field = schema.get_field("file")?;

    let query_parser = QueryParser::for_index(index, vec![name_field, file_field]);
    let query = query_parser.parse_query(query_str)?;

    let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;

    let id_field = schema.get_field("id")?;
    let kind_field = schema.get_field("kind")?;

    let mut results = Vec::new();
    for (score, doc_address) in top_docs {
        let doc: tantivy::TantivyDocument = searcher.doc(doc_address)?;
        let id = doc
            .get_first(id_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let name = doc
            .get_first(name_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = doc
            .get_first(kind_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let file = doc
            .get_first(file_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        results.push(SearchResult {
            id,
            name,
            kind,
            file,
            score,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
    use std::collections::HashMap;

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
}
