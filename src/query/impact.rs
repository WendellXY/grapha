use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::graph::{EdgeKind, Graph};

use super::SymbolRef;

#[derive(Debug, Serialize)]
pub struct ImpactResult {
    pub source: String,
    pub depth_1: Vec<SymbolRef>,
    pub depth_2: Vec<SymbolRef>,
    pub depth_3_plus: Vec<SymbolRef>,
    pub total_affected: usize,
}

pub fn query_impact(graph: &Graph, symbol: &str, max_depth: usize) -> Option<ImpactResult> {
    let node = graph
        .nodes
        .iter()
        .find(|n| n.id == symbol || n.name == symbol)?;

    let node_index: HashMap<&str, &crate::graph::Node> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut reverse_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        if matches!(
            edge.kind,
            EdgeKind::Calls | EdgeKind::Implements | EdgeKind::TypeRef | EdgeKind::Inherits
        ) {
            reverse_adj
                .entry(&edge.target)
                .or_default()
                .push(&edge.source);
        }
    }

    let mut visited: HashSet<&str> = HashSet::new();
    visited.insert(&node.id);

    let mut depth_1 = Vec::new();
    let mut depth_2 = Vec::new();
    let mut depth_3_plus = Vec::new();

    let mut queue: VecDeque<(&str, usize)> = VecDeque::new();
    queue.push_back((&node.id, 0));

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        if let Some(dependents) = reverse_adj.get(current) {
            for dep_id in dependents {
                if visited.contains(dep_id) {
                    continue;
                }
                visited.insert(dep_id);
                if let Some(dep_node) = node_index.get(dep_id) {
                    let sym_ref = SymbolRef {
                        id: dep_node.id.clone(),
                        name: dep_node.name.clone(),
                        kind: dep_node.kind,
                        file: dep_node.file.to_string_lossy().to_string(),
                    };
                    match depth + 1 {
                        1 => depth_1.push(sym_ref),
                        2 => depth_2.push(sym_ref),
                        _ => depth_3_plus.push(sym_ref),
                    }
                    queue.push_back((dep_id, depth + 1));
                }
            }
        }
    }

    let total = depth_1.len() + depth_2.len() + depth_3_plus.len();
    Some(ImpactResult {
        source: node.id.clone(),
        depth_1,
        depth_2,
        depth_3_plus,
        total_affected: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap as StdHashMap;

    fn make_chain_graph() -> Graph {
        let mk = |id: &str| Node {
            id: id.into(),
            kind: NodeKind::Function,
            name: id.into(),
            file: "test.rs".into(),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: StdHashMap::new(),
        };
        Graph {
            version: "0.1.0".to_string(),
            nodes: vec![mk("a"), mk("b"), mk("c"), mk("d")],
            edges: vec![
                Edge {
                    source: "a".into(),
                    target: "b".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                },
                Edge {
                    source: "b".into(),
                    target: "c".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                },
                Edge {
                    source: "c".into(),
                    target: "d".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                },
            ],
        }
    }

    #[test]
    fn impact_finds_transitive_dependents() {
        let graph = make_chain_graph();
        let result = query_impact(&graph, "d", 5).unwrap();
        assert_eq!(result.depth_1.len(), 1);
        assert_eq!(result.depth_1[0].name, "c");
        assert_eq!(result.depth_2.len(), 1);
        assert_eq!(result.depth_2[0].name, "b");
        assert_eq!(result.depth_3_plus.len(), 1);
        assert_eq!(result.depth_3_plus[0].name, "a");
        assert_eq!(result.total_affected, 3);
    }

    #[test]
    fn impact_respects_max_depth() {
        let graph = make_chain_graph();
        let result = query_impact(&graph, "d", 1).unwrap();
        assert_eq!(result.depth_1.len(), 1);
        assert_eq!(result.total_affected, 1);
    }

    #[test]
    fn impact_returns_none_for_unknown() {
        let graph = make_chain_graph();
        assert!(query_impact(&graph, "z", 5).is_none());
    }
}
