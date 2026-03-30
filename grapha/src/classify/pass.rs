use std::collections::HashMap;
use std::path::PathBuf;

use crate::classify::{ClassifyContext, CompositeClassifier};
use grapha_core::graph::{EdgeKind, Graph, NodeRole};

pub fn classify_graph(graph: &Graph, classifier: &CompositeClassifier) -> Graph {
    let node_file_map: HashMap<&str, &PathBuf> = graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), &n.file))
        .collect();

    let mut terminal_nodes: HashMap<String, grapha_core::graph::TerminalKind> = HashMap::new();

    let new_edges: Vec<_> = graph
        .edges
        .iter()
        .map(|edge| {
            if edge.kind != EdgeKind::Calls {
                return edge.clone();
            }

            let target_name = edge.target.rsplit("::").next().unwrap_or(&edge.target);

            let source_file = node_file_map
                .get(edge.source.as_str())
                .cloned()
                .cloned()
                .unwrap_or_default();

            let context = ClassifyContext {
                source_node: edge.source.clone(),
                file: source_file,
                arguments: vec![],
            };

            match classifier.classify(target_name, &context) {
                Some(classification) => {
                    terminal_nodes.insert(edge.target.clone(), classification.terminal_kind);

                    let mut new_edge = edge.clone();
                    new_edge.direction = Some(classification.direction);
                    new_edge.operation = Some(classification.operation);
                    new_edge
                }
                None => edge.clone(),
            }
        })
        .collect();

    let new_nodes: Vec<_> = graph
        .nodes
        .iter()
        .map(|node| match terminal_nodes.get(&node.id) {
            Some(kind) => {
                let mut new_node = node.clone();
                new_node.role = Some(NodeRole::Terminal { kind: *kind });
                new_node
            }
            None => node.clone(),
        })
        .collect();

    Graph {
        version: graph.version.clone(),
        nodes: new_nodes,
        edges: new_edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::{Classification, Classifier};
    use grapha_core::graph::*;
    use std::collections::HashMap;

    struct MockClassifier;

    impl Classifier for MockClassifier {
        fn classify(
            &self,
            call_target: &str,
            _context: &crate::classify::ClassifyContext,
        ) -> Option<Classification> {
            if call_target.contains("fetch") {
                Some(Classification {
                    terminal_kind: TerminalKind::Network,
                    direction: FlowDirection::Read,
                    operation: "HTTP_GET".to_string(),
                })
            } else {
                None
            }
        }
    }

    fn test_graph() -> Graph {
        Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "src::caller".to_string(),
                    kind: NodeKind::Function,
                    name: "caller".to_string(),
                    file: PathBuf::from("src/main.rs"),
                    span: Span {
                        start: [0, 0],
                        end: [5, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: HashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                },
                Node {
                    id: "api::fetch_data".to_string(),
                    kind: NodeKind::Function,
                    name: "fetch_data".to_string(),
                    file: PathBuf::from("src/api.rs"),
                    span: Span {
                        start: [0, 0],
                        end: [10, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: HashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                },
                Node {
                    id: "util::helper".to_string(),
                    kind: NodeKind::Function,
                    name: "helper".to_string(),
                    file: PathBuf::from("src/util.rs"),
                    span: Span {
                        start: [0, 0],
                        end: [3, 0],
                    },
                    visibility: Visibility::Private,
                    metadata: HashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                },
            ],
            edges: vec![
                Edge {
                    source: "src::caller".to_string(),
                    target: "api::fetch_data".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                },
                Edge {
                    source: "src::caller".to_string(),
                    target: "util::helper".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                },
                Edge {
                    source: "src::caller".to_string(),
                    target: "api::fetch_data".to_string(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                },
            ],
        }
    }

    #[test]
    fn enriches_matching_calls_edges() {
        let graph = test_graph();
        let classifier = CompositeClassifier::new(vec![Box::new(MockClassifier)]);
        let result = classify_graph(&graph, &classifier);

        // The first Calls edge targets "fetch_data" which contains "fetch"
        let calls_edge = &result.edges[0];
        assert_eq!(calls_edge.direction, Some(FlowDirection::Read));
        assert_eq!(calls_edge.operation.as_deref(), Some("HTTP_GET"));

        // The second Calls edge targets "helper" — no match
        let helper_edge = &result.edges[1];
        assert!(helper_edge.direction.is_none());
        assert!(helper_edge.operation.is_none());
    }

    #[test]
    fn marks_terminal_nodes() {
        let graph = test_graph();
        let classifier = CompositeClassifier::new(vec![Box::new(MockClassifier)]);
        let result = classify_graph(&graph, &classifier);

        // "api::fetch_data" should be marked as terminal
        let fetch_node = result
            .nodes
            .iter()
            .find(|n| n.id == "api::fetch_data")
            .unwrap();
        assert_eq!(
            fetch_node.role,
            Some(NodeRole::Terminal {
                kind: TerminalKind::Network,
            })
        );

        // Others should not be marked
        let caller_node = result.nodes.iter().find(|n| n.id == "src::caller").unwrap();
        assert!(caller_node.role.is_none());

        let helper_node = result
            .nodes
            .iter()
            .find(|n| n.id == "util::helper")
            .unwrap();
        assert!(helper_node.role.is_none());
    }

    #[test]
    fn skips_non_calls_edges() {
        let graph = test_graph();
        let classifier = CompositeClassifier::new(vec![Box::new(MockClassifier)]);
        let result = classify_graph(&graph, &classifier);

        // The Contains edge should be untouched
        let contains_edge = &result.edges[2];
        assert_eq!(contains_edge.kind, EdgeKind::Contains);
        assert!(contains_edge.direction.is_none());
        assert!(contains_edge.operation.is_none());
    }

    #[test]
    fn preserves_graph_version() {
        let graph = test_graph();
        let classifier = CompositeClassifier::new(vec![]);
        let result = classify_graph(&graph, &classifier);
        assert_eq!(result.version, "0.1.0");
    }

    #[test]
    fn empty_classifier_leaves_graph_unchanged() {
        let graph = test_graph();
        let classifier = CompositeClassifier::new(vec![]);
        let result = classify_graph(&graph, &classifier);

        for node in &result.nodes {
            assert!(node.role.is_none());
        }
        for edge in &result.edges {
            assert!(edge.direction.is_none());
            assert!(edge.operation.is_none());
        }
    }
}
