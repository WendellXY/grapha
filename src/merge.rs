use std::collections::{HashMap, HashSet};

use crate::extract::ExtractionResult;
use crate::graph::{EdgeKind, Graph};

/// Merge multiple `ExtractionResult`s into a single `Graph`.
///
/// Edges whose target matches a known node ID are kept as-is.
/// Uses edges are always kept (external references).
/// For unresolved targets, cross-file resolution is attempted by matching
/// the symbol name (last `::` segment) against all known nodes:
/// - Exactly one match: rewrite target, reduce confidence by 10%
/// - Multiple matches: pick first, reduce confidence by 50%
/// - No matches: drop the edge
pub fn merge(results: Vec<ExtractionResult>) -> Graph {
    let mut graph = Graph::new();

    for r in &results {
        graph.nodes.extend(r.nodes.iter().cloned());
    }

    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();

    // Build name → vec of node IDs for cross-file lookup
    let mut name_to_ids: HashMap<&str, Vec<&str>> = HashMap::new();
    for node in &graph.nodes {
        name_to_ids
            .entry(node.name.as_str())
            .or_default()
            .push(node.id.as_str());
    }

    for r in results {
        for mut edge in r.edges {
            if node_ids.contains(edge.target.as_str()) || edge.kind == EdgeKind::Uses {
                graph.edges.push(edge);
            } else {
                // Try cross-file resolution by symbol name
                let target_name = edge.target.rsplit("::").next().unwrap_or(&edge.target);
                if let Some(candidates) = name_to_ids.get(target_name) {
                    if candidates.len() == 1 {
                        edge.target = candidates[0].to_string();
                        edge.confidence *= 0.9;
                        graph.edges.push(edge);
                    } else if candidates.len() > 1 {
                        edge.target = candidates[0].to_string();
                        edge.confidence *= 0.5;
                        graph.edges.push(edge);
                    }
                    // else: no candidates (empty vec), edge dropped
                }
            }
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, name: &str, kind: NodeKind) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            file: PathBuf::from("test.rs"),
            span: Span {
                start: [0, 0],
                end: [0, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn merges_nodes_from_multiple_results() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::Foo", "Foo", NodeKind::Struct)],
            edges: vec![],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::Bar", "Bar", NodeKind::Struct)],
            edges: vec![],
            imports: vec![],
        };
        let graph = merge(vec![r1, r2]);
        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn drops_edges_with_unresolved_targets() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "nonexistent::foo".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
            }],
            imports: vec![],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 0);
    }

    #[test]
    fn keeps_edges_with_resolved_targets() {
        let r1 = ExtractionResult {
            nodes: vec![
                make_node("a::main", "main", NodeKind::Function),
                make_node("a::helper", "helper", NodeKind::Function),
            ],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "a::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
            }],
            imports: vec![],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn resolves_cross_file_edges() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "b::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
            }],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::helper", "helper", NodeKind::Function)],
            edges: vec![],
            imports: vec![],
        };
        let graph = merge(vec![r1, r2]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn keeps_uses_edges_even_if_target_unresolved() {
        let r1 = ExtractionResult {
            nodes: vec![],
            edges: vec![Edge {
                source: "a.rs".to_string(),
                target: "use std::collections::HashMap;".to_string(),
                kind: EdgeKind::Uses,
                confidence: 1.0,
            }],
            imports: vec![],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn resolves_cross_file_calls_by_name() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a.rs::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a.rs::main".to_string(),
                target: "a.rs::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.8,
            }],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b.rs::helper", "helper", NodeKind::Function)],
            edges: vec![],
            imports: vec![],
        };
        let graph = merge(vec![r1, r2]);
        // Edge target should be rewritten to b.rs::helper
        let call_edge = graph.edges.iter().find(|e| e.kind == EdgeKind::Calls);
        assert!(call_edge.is_some());
        let e = call_edge.unwrap();
        assert_eq!(e.target, "b.rs::helper");
        assert!(e.confidence < 0.8); // reduced confidence
    }
}
