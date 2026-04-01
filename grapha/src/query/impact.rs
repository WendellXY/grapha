use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use grapha_core::graph::{EdgeKind, Graph, Node, NodeKind};

use super::{QueryResolveError, SymbolRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImpactTreeNode {
    pub symbol: SymbolRef,
    pub children: Vec<ImpactTreeNode>,
}

#[derive(Debug, Serialize)]
pub struct ImpactResult {
    pub source: String,
    pub depth_1: Vec<SymbolRef>,
    pub depth_2: Vec<SymbolRef>,
    pub depth_3_plus: Vec<SymbolRef>,
    pub total_affected: usize,
    #[serde(skip)]
    pub(crate) source_ref: SymbolRef,
    #[serde(skip)]
    pub(crate) tree: ImpactTreeNode,
}

fn to_symbol_ref(node: &Node) -> SymbolRef {
    SymbolRef {
        id: node.id.clone(),
        name: node.name.clone(),
        kind: node.kind,
        file: node.file.to_string_lossy().to_string(),
    }
}

fn is_structural_node(node: &Node) -> bool {
    matches!(node.kind, NodeKind::View | NodeKind::Branch)
}

fn node_sort_key(node_id: &str, node_index: &HashMap<&str, &Node>) -> (String, String, String) {
    match node_index.get(node_id).copied() {
        Some(node) => (
            node.name.clone(),
            node.file.to_string_lossy().to_string(),
            node.id.clone(),
        ),
        None => (node_id.to_string(), String::new(), node_id.to_string()),
    }
}

fn build_impact_tree<'a>(
    node_id: &'a str,
    node_index: &HashMap<&'a str, &'a Node>,
    children_by_parent: &HashMap<&'a str, Vec<&'a str>>,
) -> ImpactTreeNode {
    let node = node_index
        .get(node_id)
        .copied()
        .expect("tree nodes must exist in the node index");
    let children = children_by_parent
        .get(node_id)
        .into_iter()
        .flat_map(|children| children.iter().copied())
        .map(|child_id| build_impact_tree(child_id, node_index, children_by_parent))
        .collect();

    ImpactTreeNode {
        symbol: to_symbol_ref(node),
        children,
    }
}

pub fn query_impact(
    graph: &Graph,
    symbol: &str,
    max_depth: usize,
) -> Result<ImpactResult, QueryResolveError> {
    let node = crate::query::resolve_node(&graph.nodes, symbol)?;

    let node_index: HashMap<&str, &grapha_core::graph::Node> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut reverse_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &graph.edges {
        if !matches!(
            edge.kind,
            EdgeKind::Calls
                | EdgeKind::Reads
                | EdgeKind::Implements
                | EdgeKind::TypeRef
                | EdgeKind::Inherits
        ) {
            continue;
        }

        let Some(source_node) = node_index.get(edge.source.as_str()).copied() else {
            continue;
        };
        let Some(target_node) = node_index.get(edge.target.as_str()).copied() else {
            continue;
        };
        if is_structural_node(source_node) || is_structural_node(target_node) {
            continue;
        }

        reverse_adj
            .entry(&edge.target)
            .or_default()
            .push(&edge.source);
    }
    for dependents in reverse_adj.values_mut() {
        dependents.sort_unstable_by_key(|node_id| node_sort_key(node_id, &node_index));
    }

    let mut visited: HashSet<&str> = HashSet::new();
    visited.insert(&node.id);

    let mut depth_1 = Vec::new();
    let mut depth_2 = Vec::new();
    let mut depth_3_plus = Vec::new();
    let mut parents: HashMap<&str, &str> = HashMap::new();

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
                    parents.insert(dep_id, current);
                    let sym_ref = to_symbol_ref(dep_node);
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
    let mut children_by_parent: HashMap<&str, Vec<&str>> = HashMap::new();
    for (child, parent) in parents {
        children_by_parent.entry(parent).or_default().push(child);
    }
    for children in children_by_parent.values_mut() {
        children.sort_unstable_by_key(|node_id| node_sort_key(node_id, &node_index));
    }
    let source_ref = to_symbol_ref(node);
    let tree = build_impact_tree(&node.id, &node_index, &children_by_parent);

    Ok(ImpactResult {
        source: node.id.clone(),
        depth_1,
        depth_2,
        depth_3_plus,
        total_affected: total,
        source_ref,
        tree,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
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
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
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
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "b".into(),
                    target: "c".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "c".into(),
                    target: "d".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
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
    fn impact_traverses_read_dependencies() {
        let mk = |id: &str, name: &str| Node {
            id: id.into(),
            kind: NodeKind::Property,
            name: name.into(),
            file: "view.swift".into(),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: StdHashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        };

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                mk("view.swift::RoomPage::roomMode", "roomMode"),
                mk("view.swift::RoomPage::canShowGameRoom", "canShowGameRoom"),
                mk("view.swift::RoomPage::body", "body"),
            ],
            edges: vec![
                Edge {
                    source: "view.swift::RoomPage::canShowGameRoom".into(),
                    target: "view.swift::RoomPage::roomMode".into(),
                    kind: EdgeKind::Reads,
                    confidence: 0.85,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::RoomPage::body".into(),
                    target: "view.swift::RoomPage::canShowGameRoom".into(),
                    kind: EdgeKind::Reads,
                    confidence: 0.85,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let result = query_impact(&graph, "roomMode", 5).unwrap();
        assert_eq!(result.depth_1.len(), 1);
        assert_eq!(result.depth_1[0].name, "canShowGameRoom");
        assert_eq!(result.depth_2.len(), 1);
        assert_eq!(result.depth_2[0].name, "body");
    }

    #[test]
    fn impact_returns_none_for_unknown() {
        let graph = make_chain_graph();
        assert!(matches!(
            query_impact(&graph, "z", 5),
            Err(QueryResolveError::NotFound { .. })
        ));
    }

    #[test]
    fn impact_tree_reflects_bfs_parentage() {
        let graph = make_chain_graph();
        let result = query_impact(&graph, "d", 5).unwrap();

        assert_eq!(result.source_ref.name, "d");
        assert_eq!(result.tree.symbol.name, "d");
        assert_eq!(result.tree.children.len(), 1);
        assert_eq!(result.tree.children[0].symbol.name, "c");
        assert_eq!(result.tree.children[0].children[0].symbol.name, "b");
        assert_eq!(
            result.tree.children[0].children[0].children[0].symbol.name,
            "a"
        );
    }

    #[test]
    fn impact_tree_is_deterministic_when_multiple_dependents_exist() {
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
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        };
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![mk("alpha"), mk("beta"), mk("source")],
            edges: vec![
                Edge {
                    source: "beta".into(),
                    target: "source".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "alpha".into(),
                    target: "source".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let result = query_impact(&graph, "source", 5).unwrap();
        let child_names: Vec<_> = result
            .tree
            .children
            .iter()
            .map(|child| child.symbol.name.as_str())
            .collect();
        assert_eq!(child_names, vec!["alpha", "beta"]);
    }

    #[test]
    fn impact_ignores_unresolved_dependents_without_panicking_during_sort() {
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
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        };
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![mk("alpha"), mk("source")],
            edges: vec![
                Edge {
                    source: "ghost".into(),
                    target: "source".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "alpha".into(),
                    target: "source".into(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let result = query_impact(&graph, "source", 5).unwrap();
        assert_eq!(result.total_affected, 1);
        assert_eq!(result.depth_1[0].name, "alpha");
    }

    #[test]
    fn impact_ignores_swiftui_structural_nodes() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "source".into(),
                    kind: NodeKind::Function,
                    name: "source".into(),
                    file: "test.swift".into(),
                    span: Span {
                        start: [0, 0],
                        end: [1, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "body::view:Row@10:12".into(),
                    kind: NodeKind::View,
                    name: "Row".into(),
                    file: "test.swift".into(),
                    span: Span {
                        start: [10, 12],
                        end: [10, 28],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
            ],
            edges: vec![Edge {
                source: "body::view:Row@10:12".into(),
                target: "source".into(),
                kind: EdgeKind::TypeRef,
                confidence: 0.9,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        };

        let result = query_impact(&graph, "source", 5).unwrap();
        assert_eq!(result.total_affected, 0);
        assert!(result.depth_1.is_empty());
        assert!(result.depth_2.is_empty());
        assert!(result.depth_3_plus.is_empty());
    }
}
