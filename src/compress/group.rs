use crate::graph::{Edge, EdgeKind, Graph, NodeKind};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct GroupedGraph {
    pub version: String,
    pub files: BTreeMap<String, FileGroup>,
}

#[derive(Debug, Serialize)]
pub struct FileGroup {
    pub symbols: Vec<SymbolSummary>,
}

#[derive(Debug, Serialize)]
pub struct SymbolSummary {
    pub name: String,
    pub kind: NodeKind,
    pub span: [usize; 2],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inherits: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub type_refs: Vec<String>,
}

pub fn group(graph: &Graph) -> GroupedGraph {
    let mut files: BTreeMap<String, Vec<SymbolSummary>> = BTreeMap::new();

    let mut edges_by_source: BTreeMap<&str, Vec<&Edge>> = BTreeMap::new();
    for edge in &graph.edges {
        edges_by_source.entry(&edge.source).or_default().push(edge);
    }

    let id_to_name: BTreeMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.name.as_str()))
        .collect();

    for node in &graph.nodes {
        let edges = edges_by_source.get(node.id.as_str());
        let mut members = Vec::new();
        let mut calls = Vec::new();
        let mut implements = Vec::new();
        let mut inherits = Vec::new();
        let mut type_refs = Vec::new();

        if let Some(edges) = edges {
            for edge in edges {
                let target_name = id_to_name
                    .get(edge.target.as_str())
                    .copied()
                    .unwrap_or_else(|| edge.target.rsplit("::").next().unwrap_or(&edge.target));
                match edge.kind {
                    EdgeKind::Contains => members.push(target_name.to_string()),
                    EdgeKind::Calls => calls.push(target_name.to_string()),
                    EdgeKind::Implements => implements.push(target_name.to_string()),
                    EdgeKind::Inherits => inherits.push(target_name.to_string()),
                    EdgeKind::TypeRef => type_refs.push(target_name.to_string()),
                    EdgeKind::Uses => {}
                }
            }
        }

        let file_key = node.file.to_string_lossy().to_string();
        files.entry(file_key).or_default().push(SymbolSummary {
            name: node.name.clone(),
            kind: node.kind,
            span: [node.span.start[0], node.span.end[0]],
            members,
            calls,
            implements,
            inherits,
            type_refs,
        });
    }

    GroupedGraph {
        version: graph.version.clone(),
        files: files
            .into_iter()
            .map(|(k, v)| (k, FileGroup { symbols: v }))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, name: &str, kind: NodeKind, file: &str, line: usize) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            file: PathBuf::from(file),
            span: Span {
                start: [line, 0],
                end: [line + 5, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn groups_by_file() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a.rs::Foo", "Foo", NodeKind::Struct, "a.rs", 0),
                make_node("b.rs::Bar", "Bar", NodeKind::Struct, "b.rs", 0),
            ],
            edges: vec![],
        };
        let grouped = group(&graph);
        assert_eq!(grouped.files.len(), 2);
        assert!(grouped.files.contains_key("a.rs"));
        assert!(grouped.files.contains_key("b.rs"));
    }

    #[test]
    fn collects_calls_into_symbol() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a.rs::main", "main", NodeKind::Function, "a.rs", 0),
                make_node("a.rs::helper", "helper", NodeKind::Function, "a.rs", 10),
            ],
            edges: vec![Edge {
                source: "a.rs::main".to_string(),
                target: "a.rs::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.9,
            }],
        };
        let grouped = group(&graph);
        let file = &grouped.files["a.rs"];
        let main_sym = file.symbols.iter().find(|s| s.name == "main").unwrap();
        assert_eq!(main_sym.calls, vec!["helper"]);
    }

    #[test]
    fn grouped_output_skips_empty_arrays() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![make_node("a.rs::Foo", "Foo", NodeKind::Struct, "a.rs", 0)],
            edges: vec![],
        };
        let grouped = group(&graph);
        let json = serde_json::to_string(&grouped).unwrap();
        assert!(!json.contains("\"calls\""));
        assert!(!json.contains("\"members\""));
    }
}
