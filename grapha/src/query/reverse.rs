use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use grapha_core::graph::{EdgeKind, Graph, NodeRole};

use super::SymbolRef;

#[derive(Debug, Serialize)]
pub struct ReverseResult {
    pub symbol: String,
    pub affected_entries: Vec<AffectedEntry>,
    pub total_entries: usize,
}

#[derive(Debug, Serialize)]
pub struct AffectedEntry {
    pub entry: SymbolRef,
    pub distance: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
}

fn is_dataflow_edge(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Calls
            | EdgeKind::Reads
            | EdgeKind::Writes
            | EdgeKind::Publishes
            | EdgeKind::Subscribes
    )
}

pub fn query_reverse(graph: &Graph, symbol: &str) -> Option<ReverseResult> {
    let target_node = graph
        .nodes
        .iter()
        .find(|n| n.id == symbol || n.name == symbol)?;

    let node_index: HashMap<&str, &grapha_core::graph::Node> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Build reverse adjacency: target -> [source_id]
    let mut reverse_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        if is_dataflow_edge(edge.kind) {
            reverse_adj
                .entry(&edge.target)
                .or_default()
                .push(&edge.source);
        }
    }

    let mut visited: HashSet<&str> = HashSet::new();
    visited.insert(&target_node.id);

    // BFS backward: (node_id, distance, path_from_target)
    let mut queue: VecDeque<(&str, usize, Vec<String>)> = VecDeque::new();
    queue.push_back((&target_node.id, 0, vec![target_node.name.clone()]));

    let mut affected_entries = Vec::new();

    while let Some((current, distance, path)) = queue.pop_front() {
        if let Some(sources) = reverse_adj.get(current) {
            for &source_id in sources {
                if visited.contains(source_id) {
                    continue;
                }
                visited.insert(source_id);

                if let Some(source_node) = node_index.get(source_id) {
                    let mut new_path = path.clone();
                    new_path.push(source_node.name.clone());
                    let new_distance = distance + 1;

                    if source_node.role == Some(NodeRole::EntryPoint) {
                        // Reverse the path so it goes entry -> ... -> symbol
                        let reversed_path: Vec<String> = new_path.into_iter().rev().collect();
                        affected_entries.push(AffectedEntry {
                            entry: SymbolRef {
                                id: source_node.id.clone(),
                                name: source_node.name.clone(),
                                kind: source_node.kind,
                                file: source_node.file.to_string_lossy().to_string(),
                            },
                            distance: new_distance,
                            path: reversed_path.clone(),
                        });
                        // Continue BFS past entry points
                        queue.push_back((
                            source_id,
                            new_distance,
                            reversed_path.into_iter().rev().collect(),
                        ));
                    } else {
                        queue.push_back((source_id, new_distance, new_path));
                    }
                }
            }
        }
    }

    let total_entries = affected_entries.len();
    Some(ReverseResult {
        symbol: target_node.id.clone(),
        affected_entries,
        total_entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
    use std::collections::HashMap as StdHashMap;
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
            metadata: StdHashMap::new(),
            role,
            signature: None,
            doc_comment: None,
            module: None,
        }
    }

    fn make_edge(source: &str, target: &str) -> Edge {
        Edge {
            source: source.into(),
            target: target.into(),
            kind: EdgeKind::Calls,
            confidence: 0.9,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
        }
    }

    #[test]
    fn finds_entry_points_that_reach_symbol() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("entry1", Some(NodeRole::EntryPoint)),
                make_node("service", None),
                make_node(
                    "db",
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Persistence,
                    }),
                ),
            ],
            edges: vec![make_edge("entry1", "service"), make_edge("service", "db")],
        };

        let result = query_reverse(&graph, "db").unwrap();
        assert_eq!(result.symbol, "db");
        assert_eq!(result.affected_entries.len(), 1);
        assert_eq!(result.affected_entries[0].entry.name, "entry1");
        assert_eq!(result.affected_entries[0].distance, 2);
        assert_eq!(
            result.affected_entries[0].path,
            vec!["entry1", "service", "db"]
        );
    }

    #[test]
    fn finds_multiple_entry_points() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("entry1", Some(NodeRole::EntryPoint)),
                make_node("entry2", Some(NodeRole::EntryPoint)),
                make_node("shared", None),
            ],
            edges: vec![make_edge("entry1", "shared"), make_edge("entry2", "shared")],
        };

        let result = query_reverse(&graph, "shared").unwrap();
        assert_eq!(result.total_entries, 2);
        let names: Vec<&str> = result
            .affected_entries
            .iter()
            .map(|e| e.entry.name.as_str())
            .collect();
        assert!(names.contains(&"entry1"));
        assert!(names.contains(&"entry2"));
    }

    #[test]
    fn returns_none_for_unknown_symbol() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![make_node("a", None)],
            edges: vec![],
        };
        assert!(query_reverse(&graph, "nonexistent").is_none());
    }

    #[test]
    fn includes_path_from_entry_to_symbol() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("entry", Some(NodeRole::EntryPoint)),
                make_node("mid", None),
                make_node("target", None),
            ],
            edges: vec![make_edge("entry", "mid"), make_edge("mid", "target")],
        };

        let result = query_reverse(&graph, "target").unwrap();
        assert_eq!(result.affected_entries.len(), 1);
        assert_eq!(
            result.affected_entries[0].path,
            vec!["entry", "mid", "target"]
        );
    }
}
