use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::extract::ExtractionResult;
use crate::graph::{EdgeKind, FlowDirection, Graph, NodeRole, TerminalKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub terminal_kind: TerminalKind,
    pub direction: FlowDirection,
    pub operation: String,
}

#[derive(Debug, Clone)]
pub struct ClassifyContext {
    pub source_node: String,
    pub file: PathBuf,
    pub arguments: Vec<String>,
}

pub trait Classifier: Send + Sync {
    fn classify(&self, call_target: &str, context: &ClassifyContext) -> Option<Classification>;
}

pub struct CompositeClassifier {
    classifiers: Vec<Box<dyn Classifier>>,
}

impl CompositeClassifier {
    pub fn new(classifiers: Vec<Box<dyn Classifier>>) -> Self {
        Self { classifiers }
    }

    pub fn classify(&self, call_target: &str, context: &ClassifyContext) -> Option<Classification> {
        self.classifiers
            .iter()
            .find_map(|classifier| classifier.classify(call_target, context))
    }
}

pub fn classify_graph(graph: &Graph, classifier: &CompositeClassifier) -> Graph {
    let node_file_map: HashMap<&str, &PathBuf> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), &node.file))
        .collect();
    let node_ids: HashSet<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut terminal_nodes: HashMap<String, TerminalKind> = HashMap::new();

    let edges = graph
        .edges
        .iter()
        .map(|edge| {
            if edge.kind != EdgeKind::Calls {
                return edge.clone();
            }

            let source_file = node_file_map
                .get(edge.source.as_str())
                .cloned()
                .cloned()
                .unwrap_or_default();
            let context = ClassifyContext {
                source_node: edge.source.clone(),
                file: source_file,
                arguments: Vec::new(),
            };

            let Some(classification) = classifier.classify(&edge.target, &context) else {
                return edge.clone();
            };

            let terminal_node_id = if node_ids.contains(edge.target.as_str()) {
                edge.target.clone()
            } else {
                edge.source.clone()
            };
            terminal_nodes.insert(terminal_node_id, classification.terminal_kind);

            let mut enriched = edge.clone();
            enriched.direction = Some(classification.direction);
            enriched.operation = Some(classification.operation);
            enriched
        })
        .collect();

    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            if let Some(kind) = terminal_nodes.get(&node.id) {
                let mut enriched = node.clone();
                enriched.role = Some(NodeRole::Terminal { kind: *kind });
                enriched
            } else {
                node.clone()
            }
        })
        .collect();

    Graph {
        version: graph.version.clone(),
        nodes,
        edges,
    }
}

pub fn classify_extraction_result(
    mut result: ExtractionResult,
    classifier: &CompositeClassifier,
) -> ExtractionResult {
    let node_ids: HashSet<&str> = result.nodes.iter().map(|node| node.id.as_str()).collect();
    let node_file_map: HashMap<&str, &PathBuf> = result
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), &node.file))
        .collect();
    let mut terminal_nodes: HashMap<String, TerminalKind> = HashMap::new();

    result.edges = result
        .edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind != EdgeKind::Calls {
                return edge;
            }

            let source_file = node_file_map
                .get(edge.source.as_str())
                .cloned()
                .cloned()
                .unwrap_or_default();
            let context = ClassifyContext {
                source_node: edge.source.clone(),
                file: source_file,
                arguments: Vec::new(),
            };

            if let Some(classification) = classifier.classify(&edge.target, &context) {
                let terminal_node_id = if node_ids.contains(edge.target.as_str()) {
                    edge.target.clone()
                } else {
                    edge.source.clone()
                };
                terminal_nodes.insert(terminal_node_id, classification.terminal_kind);
                edge.direction = Some(classification.direction);
                edge.operation = Some(classification.operation);
            }

            edge
        })
        .collect();

    for node in &mut result.nodes {
        if let Some(kind) = terminal_nodes.get(&node.id)
            && node.role.is_none()
        {
            node.role = Some(NodeRole::Terminal { kind: *kind });
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;

    struct AlwaysMatch {
        classification: Classification,
    }

    impl Classifier for AlwaysMatch {
        fn classify(
            &self,
            _call_target: &str,
            _context: &ClassifyContext,
        ) -> Option<Classification> {
            Some(self.classification.clone())
        }
    }

    struct NeverMatch;

    impl Classifier for NeverMatch {
        fn classify(
            &self,
            _call_target: &str,
            _context: &ClassifyContext,
        ) -> Option<Classification> {
            None
        }
    }

    fn test_context() -> ClassifyContext {
        ClassifyContext {
            source_node: "test::caller".to_string(),
            file: PathBuf::from("test.rs"),
            arguments: vec![],
        }
    }

    #[test]
    fn composite_returns_first_match() {
        let classifier = CompositeClassifier::new(vec![Box::new(AlwaysMatch {
            classification: Classification {
                terminal_kind: TerminalKind::Network,
                direction: FlowDirection::Read,
                operation: "HTTP_GET".to_string(),
            },
        })]);
        let result = classifier.classify("something", &test_context());
        assert!(result.is_some());
        assert_eq!(result.unwrap().terminal_kind, TerminalKind::Network);
    }

    #[test]
    fn composite_returns_none_when_no_match() {
        let classifier = CompositeClassifier::new(vec![Box::new(NeverMatch)]);
        assert!(classifier.classify("something", &test_context()).is_none());
    }

    #[test]
    fn classifies_external_call_on_source_node() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "src::caller".to_string(),
                kind: NodeKind::Function,
                name: "caller".to_string(),
                file: PathBuf::from("src/main.rs"),
                span: Span {
                    start: [0, 0],
                    end: [1, 0],
                },
                visibility: Visibility::Public,
                metadata: HashMap::new(),
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
                snippet: None,
            }],
            edges: vec![Edge {
                source: "src::caller".to_string(),
                target: "reqwest::get".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.9,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        };
        let classifier = CompositeClassifier::new(vec![Box::new(AlwaysMatch {
            classification: Classification {
                terminal_kind: TerminalKind::Network,
                direction: FlowDirection::Read,
                operation: "HTTP".to_string(),
            },
        })]);

        let enriched = classify_graph(&graph, &classifier);
        assert_eq!(
            enriched.nodes[0].role,
            Some(NodeRole::Terminal {
                kind: TerminalKind::Network,
            })
        );
        assert_eq!(enriched.edges[0].direction, Some(FlowDirection::Read));
    }
}
