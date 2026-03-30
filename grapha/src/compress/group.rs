use grapha_core::graph::{Edge, EdgeKind, Graph, NodeKind};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<grapha_core::graph::NodeRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
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
        let mut reads = Vec::new();
        let mut writes = Vec::new();

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
                    EdgeKind::Reads => reads.push(target_name.to_string()),
                    EdgeKind::Writes => writes.push(target_name.to_string()),
                    EdgeKind::Publishes => writes.push(format!("publish:{target_name}")),
                    EdgeKind::Subscribes => reads.push(format!("subscribe:{target_name}")),
                    EdgeKind::Uses => {}
                }
            }
        }

        let file_key = node.file.to_string_lossy().to_string();
        files.entry(file_key).or_default().push(SymbolSummary {
            name: node.name.clone(),
            kind: node.kind,
            span: [node.span.start[0], node.span.end[0]],
            role: node.role.clone(),
            signature: node.signature.clone(),
            module: node.module.clone(),
            members,
            calls,
            implements,
            inherits,
            type_refs,
            reads,
            writes,
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
    use grapha_core::graph::*;
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
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
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
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
        };
        let grouped = group(&graph);
        let file = &grouped.files["a.rs"];
        let main_sym = file.symbols.iter().find(|s| s.name == "main").unwrap();
        assert_eq!(main_sym.calls, vec!["helper"]);
    }

    #[test]
    fn grouped_output_includes_role_and_signature() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![{
                let mut n = make_node("a.rs::main", "main", NodeKind::Function, "a.rs", 0);
                n.role = Some(grapha_core::graph::NodeRole::EntryPoint);
                n.signature = Some("fn main()".to_string());
                n.module = Some("app".to_string());
                n
            }],
            edges: vec![],
        };
        let grouped = group(&graph);
        let json = serde_json::to_string(&grouped).unwrap();
        assert!(json.contains("entry_point"));
        assert!(json.contains("fn main()"));
        assert!(json.contains("app"));
    }

    #[test]
    fn grouped_output_includes_reads_and_writes() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a.rs::handler", "handler", NodeKind::Function, "a.rs", 0),
                make_node("a.rs::db", "db", NodeKind::Function, "a.rs", 10),
                make_node("a.rs::cache", "cache", NodeKind::Function, "a.rs", 20),
                make_node("a.rs::topic", "topic", NodeKind::Function, "a.rs", 30),
            ],
            edges: vec![
                Edge {
                    source: "a.rs::handler".to_string(),
                    target: "a.rs::db".to_string(),
                    kind: EdgeKind::Reads,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                },
                Edge {
                    source: "a.rs::handler".to_string(),
                    target: "a.rs::cache".to_string(),
                    kind: EdgeKind::Writes,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                },
                Edge {
                    source: "a.rs::handler".to_string(),
                    target: "a.rs::topic".to_string(),
                    kind: EdgeKind::Publishes,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                },
            ],
        };
        let grouped = group(&graph);
        let file = &grouped.files["a.rs"];
        let handler = file.symbols.iter().find(|s| s.name == "handler").unwrap();
        assert_eq!(handler.reads, vec!["db"]);
        assert_eq!(handler.writes, vec!["cache", "publish:topic"]);
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
