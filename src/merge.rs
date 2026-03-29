use std::collections::HashSet;

use crate::extract::ExtractionResult;
use crate::graph::Graph;

/// Merge multiple `ExtractionResult`s into a single `Graph`.
/// Drops edges whose target does not match any node ID in the graph.
pub fn merge(results: Vec<ExtractionResult>) -> Graph {
    let mut graph = Graph::new();

    for r in &results {
        graph.nodes.extend(r.nodes.iter().cloned());
    }

    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();

    for r in results {
        for edge in r.edges {
            // Keep edges where the target is a known node or is a use-path (external reference)
            if node_ids.contains(edge.target.as_str()) || edge.kind == crate::graph::EdgeKind::Uses
            {
                graph.edges.push(edge);
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
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::Bar", "Bar", NodeKind::Struct)],
            edges: vec![],
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
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::helper", "helper", NodeKind::Function)],
            edges: vec![],
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
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 1);
    }
}
