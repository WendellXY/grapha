use crate::graph::{Edge, EdgeKind, Graph, Node, NodeKind, Visibility};
use std::collections::HashSet;

/// Prune a graph for LLM consumption:
/// 1. Drop Contains edges (inferrable from span nesting)
/// 2. Drop Uses edges with raw text targets (not graph-traversable)
/// 3. Optionally drop private leaf nodes (fields, variants)
pub fn prune(graph: Graph, keep_private_leaves: bool) -> Graph {
    let kept_nodes: Vec<Node> = if keep_private_leaves {
        graph.nodes
    } else {
        graph
            .nodes
            .into_iter()
            .filter(|n| {
                !matches!(n.kind, NodeKind::Field | NodeKind::Variant)
                    || n.visibility == Visibility::Public
            })
            .collect()
    };

    let kept_ids: HashSet<&str> = kept_nodes.iter().map(|n| n.id.as_str()).collect();

    let edges: Vec<Edge> = graph
        .edges
        .into_iter()
        .filter(|e| {
            if e.kind == EdgeKind::Contains {
                return false;
            }
            if e.kind == EdgeKind::Uses && !kept_ids.contains(e.target.as_str()) {
                return false;
            }
            kept_ids.contains(e.source.as_str())
                && (kept_ids.contains(e.target.as_str()) || e.kind == EdgeKind::Uses)
        })
        .collect();

    Graph {
        version: graph.version,
        nodes: kept_nodes,
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, kind: NodeKind, vis: Visibility) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: id.to_string(),
            file: PathBuf::from("test.rs"),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: vis,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn prune_drops_contains_edges() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a", NodeKind::Struct, Visibility::Public),
                make_node("b", NodeKind::Function, Visibility::Public),
            ],
            edges: vec![
                Edge {
                    source: "a".into(),
                    target: "b".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                },
                Edge {
                    source: "b".into(),
                    target: "a".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.8,
                },
            ],
        };
        let pruned = prune(graph, true);
        assert_eq!(pruned.edges.len(), 1);
        assert_eq!(pruned.edges[0].kind, EdgeKind::Calls);
    }

    #[test]
    fn prune_drops_unresolved_uses_edges() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![make_node("a", NodeKind::Function, Visibility::Public)],
            edges: vec![Edge {
                source: "file.rs".into(),
                target: "use std::collections::HashMap;".into(),
                kind: EdgeKind::Uses,
                confidence: 0.7,
            }],
        };
        let pruned = prune(graph, true);
        assert_eq!(pruned.edges.len(), 0);
    }

    #[test]
    fn prune_drops_private_leaves_when_requested() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("s", NodeKind::Struct, Visibility::Public),
                make_node("f1", NodeKind::Field, Visibility::Private),
                make_node("f2", NodeKind::Field, Visibility::Public),
            ],
            edges: vec![],
        };
        let pruned = prune(graph, false);
        assert_eq!(pruned.nodes.len(), 2);
    }

    #[test]
    fn prune_keeps_private_leaves_when_requested() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("s", NodeKind::Struct, Visibility::Public),
                make_node("f1", NodeKind::Field, Visibility::Private),
            ],
            edges: vec![],
        };
        let pruned = prune(graph, true);
        assert_eq!(pruned.nodes.len(), 2);
    }
}
