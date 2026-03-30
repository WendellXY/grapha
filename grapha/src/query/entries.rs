use serde::Serialize;

use grapha_core::graph::{Graph, NodeRole};

use super::SymbolRef;

#[derive(Debug, Serialize)]
pub struct EntriesResult {
    pub entries: Vec<SymbolRef>,
    pub total: usize,
}

pub fn query_entries(graph: &Graph) -> EntriesResult {
    let entries: Vec<SymbolRef> = graph
        .nodes
        .iter()
        .filter(|n| n.role == Some(NodeRole::EntryPoint))
        .map(|n| SymbolRef {
            id: n.id.clone(),
            name: n.name.clone(),
            kind: n.kind,
            file: n.file.to_string_lossy().to_string(),
        })
        .collect();

    let total = entries.len();
    EntriesResult { entries, total }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, role: Option<NodeRole>) -> Node {
        Node {
            id: id.into(),
            kind: NodeKind::Function,
            name: id.into(),
            file: PathBuf::from("test.rs"),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role,
            signature: None,
            doc_comment: None,
            module: None,
        }
    }

    #[test]
    fn lists_entry_points() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("entry1", Some(NodeRole::EntryPoint)),
                make_node("entry2", Some(NodeRole::EntryPoint)),
                make_node("internal", Some(NodeRole::Internal)),
            ],
            edges: vec![],
        };

        let result = query_entries(&graph);
        assert_eq!(result.total, 2);
        let names: Vec<&str> = result.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"entry1"));
        assert!(names.contains(&"entry2"));
        assert!(!names.contains(&"internal"));
    }

    #[test]
    fn returns_empty_when_no_entries() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a", None),
                make_node("b", Some(NodeRole::Internal)),
            ],
            edges: vec![],
        };

        let result = query_entries(&graph);
        assert_eq!(result.total, 0);
        assert!(result.entries.is_empty());
    }
}
