use crate::graph::{Edge, EdgeKind, Graph, Node, NodeKind, NodeRole, TerminalKind};
use crate::semantic::{
    ArtifactKind, SemanticAnnotation, SemanticArtifact, SemanticDocument, SemanticRelation,
    SemanticSymbol, SemanticTarget,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SymbolSelector {
    pub id: Option<String>,
    pub name: Option<String>,
    pub kind: Option<NodeKind>,
    pub module: Option<String>,
    pub file_suffix: Option<String>,
    pub annotation: Option<AnnotationSelector>,
    pub property_key: Option<String>,
}

impl SymbolSelector {
    pub fn by_kind(kind: NodeKind) -> Self {
        Self {
            kind: Some(kind),
            ..Self::default()
        }
    }

    pub fn with_annotation(mut self, annotation: AnnotationSelector) -> Self {
        self.annotation = Some(annotation);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationSelector {
    EntryPoint,
    Terminal(TerminalKind),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RelationSelector {
    pub source: Option<String>,
    pub relation_kind: Option<EdgeKind>,
    pub target_symbol: Option<String>,
    pub external_only: bool,
    pub terminal_kind: Option<TerminalKind>,
}

impl RelationSelector {
    pub fn calls() -> Self {
        Self {
            relation_kind: Some(EdgeKind::Calls),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArtifactSelector {
    pub kind: Option<ArtifactKind>,
}

pub fn select_semantic_symbols<'a>(
    document: &'a SemanticDocument,
    selector: &SymbolSelector,
) -> Vec<&'a SemanticSymbol> {
    document
        .symbols
        .iter()
        .filter(|symbol| symbol_matches(symbol, selector))
        .collect()
}

pub fn select_semantic_relations<'a>(
    document: &'a SemanticDocument,
    selector: &RelationSelector,
) -> Vec<&'a SemanticRelation> {
    document
        .relations
        .iter()
        .filter(|relation| relation_matches(relation, selector))
        .collect()
}

pub fn select_semantic_artifacts<'a>(
    document: &'a SemanticDocument,
    selector: &ArtifactSelector,
) -> Vec<&'a SemanticArtifact> {
    document
        .artifacts
        .iter()
        .filter(|artifact| selector.kind.is_none_or(|kind| artifact.kind() == kind))
        .collect()
}

pub fn select_graph_nodes<'a>(graph: &'a Graph, selector: &SymbolSelector) -> Vec<&'a Node> {
    graph
        .nodes
        .iter()
        .filter(|node| node_matches(node, selector))
        .collect()
}

pub fn select_graph_edges<'a>(graph: &'a Graph, selector: &RelationSelector) -> Vec<&'a Edge> {
    let node_ids: std::collections::HashSet<&str> =
        graph.nodes.iter().map(|node| node.id.as_str()).collect();
    graph
        .edges
        .iter()
        .filter(|edge| edge_matches(edge, &node_ids, selector))
        .collect()
}

fn symbol_matches(symbol: &SemanticSymbol, selector: &SymbolSelector) -> bool {
    selector.id.as_ref().is_none_or(|id| symbol.id == *id)
        && selector
            .name
            .as_ref()
            .is_none_or(|name| symbol.name == *name)
        && selector.kind.is_none_or(|kind| symbol.kind == kind)
        && selector
            .module
            .as_ref()
            .is_none_or(|module| symbol.module.as_deref() == Some(module.as_str()))
        && selector
            .file_suffix
            .as_ref()
            .is_none_or(|suffix| symbol.file.to_string_lossy().ends_with(suffix))
        && selector
            .annotation
            .is_none_or(|annotation| symbol_annotation_matches(symbol, annotation))
        && selector
            .property_key
            .as_ref()
            .is_none_or(|key| symbol.properties.contains_key(key))
}

fn symbol_annotation_matches(symbol: &SemanticSymbol, selector: AnnotationSelector) -> bool {
    symbol
        .annotations
        .iter()
        .any(|annotation| match (annotation, selector) {
            (SemanticAnnotation::EntryPoint, AnnotationSelector::EntryPoint) => true,
            (SemanticAnnotation::Terminal { kind }, AnnotationSelector::Terminal(expected)) => {
                *kind == expected
            }
            _ => false,
        })
}

fn relation_matches(relation: &SemanticRelation, selector: &RelationSelector) -> bool {
    selector
        .source
        .as_ref()
        .is_none_or(|source| relation.source == *source)
        && selector
            .relation_kind
            .is_none_or(|kind| relation.kind == kind)
        && selector.target_symbol.as_ref().is_none_or(|target| {
            matches!(&relation.target, SemanticTarget::Symbol(symbol_id) if symbol_id == target)
        })
        && (!selector.external_only || matches!(relation.target, SemanticTarget::ExternalRef(_)))
        && selector
            .terminal_kind
            .is_none_or(|kind| relation.terminal_kind == Some(kind))
}

fn node_matches(node: &Node, selector: &SymbolSelector) -> bool {
    selector.id.as_ref().is_none_or(|id| node.id == *id)
        && selector.name.as_ref().is_none_or(|name| node.name == *name)
        && selector.kind.is_none_or(|kind| node.kind == kind)
        && selector
            .module
            .as_ref()
            .is_none_or(|module| node.module.as_deref() == Some(module.as_str()))
        && selector
            .file_suffix
            .as_ref()
            .is_none_or(|suffix| node.file.to_string_lossy().ends_with(suffix))
        && selector
            .annotation
            .is_none_or(|annotation| node_role_matches(node.role.as_ref(), annotation))
        && selector
            .property_key
            .as_ref()
            .is_none_or(|key| node.metadata.contains_key(key))
}

fn node_role_matches(role: Option<&NodeRole>, selector: AnnotationSelector) -> bool {
    match (role, selector) {
        (Some(NodeRole::EntryPoint), AnnotationSelector::EntryPoint) => true,
        (Some(NodeRole::Terminal { kind }), AnnotationSelector::Terminal(expected)) => {
            *kind == expected
        }
        _ => false,
    }
}

fn edge_matches(
    edge: &Edge,
    node_ids: &std::collections::HashSet<&str>,
    selector: &RelationSelector,
) -> bool {
    selector
        .source
        .as_ref()
        .is_none_or(|source| edge.source == *source)
        && selector.relation_kind.is_none_or(|kind| edge.kind == kind)
        && selector
            .target_symbol
            .as_ref()
            .is_none_or(|target| edge.target == *target)
        && (!selector.external_only || !node_ids.contains(edge.target.as_str()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::graph::{Edge, FlowDirection, Graph, Node, Span, Visibility};
    use crate::semantic::{SemanticDocument, SemanticRelation, SemanticSymbol, SemanticTarget};

    use super::*;

    fn test_symbol() -> SemanticSymbol {
        SemanticSymbol {
            id: "body".to_string(),
            kind: NodeKind::View,
            name: "Text".to_string(),
            file: PathBuf::from("ContentView.swift"),
            span: Span {
                start: [1, 0],
                end: [2, 0],
            },
            visibility: Visibility::Public,
            properties: HashMap::from([(
                "swiftui.invalidation_source".to_string(),
                "true".to_string(),
            )]),
            annotations: vec![SemanticAnnotation::EntryPoint],
            signature: None,
            doc_comment: None,
            module: Some("Demo".to_string()),
            snippet: None,
            repo: None,
            synthetic_kind: Some("swiftui_view".to_string()),
        }
    }

    #[test]
    fn selects_semantic_symbols_and_relations() {
        let mut document = SemanticDocument::new();
        document.symbols.push(test_symbol());
        document.relations.push(SemanticRelation {
            source: "body".to_string(),
            target: SemanticTarget::ExternalRef("reqwest::get".to_string()),
            kind: EdgeKind::Calls,
            confidence: 1.0,
            direction: Some(FlowDirection::Read),
            operation: Some("HTTP".to_string()),
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
            repo: None,
            terminal_kind: Some(TerminalKind::Network),
        });

        let symbols = select_semantic_symbols(
            &document,
            &SymbolSelector::by_kind(NodeKind::View)
                .with_annotation(AnnotationSelector::EntryPoint),
        );
        assert_eq!(symbols.len(), 1);

        let relations = select_semantic_relations(
            &document,
            &RelationSelector {
                external_only: true,
                terminal_kind: Some(TerminalKind::Network),
                ..RelationSelector::calls()
            },
        );
        assert_eq!(relations.len(), 1);
    }

    #[test]
    fn selects_only_external_graph_edges_when_requested() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "caller".to_string(),
                    kind: NodeKind::Function,
                    name: "load".to_string(),
                    file: PathBuf::from("main.rs"),
                    span: Span {
                        start: [1, 0],
                        end: [2, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: HashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                    repo: None,
                },
                Node {
                    id: "callee".to_string(),
                    kind: NodeKind::Function,
                    name: "helper".to_string(),
                    file: PathBuf::from("main.rs"),
                    span: Span {
                        start: [3, 0],
                        end: [4, 0],
                    },
                    visibility: Visibility::Private,
                    metadata: HashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                    repo: None,
                },
            ],
            edges: vec![
                Edge {
                    source: "caller".to_string(),
                    target: "callee".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                    repo: None,
                },
                Edge {
                    source: "caller".to_string(),
                    target: "reqwest::get".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 1.0,
                    direction: Some(FlowDirection::Read),
                    operation: Some("HTTP".to_string()),
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                    repo: None,
                },
            ],
        };

        let edges = select_graph_edges(
            &graph,
            &RelationSelector {
                external_only: true,
                ..RelationSelector::calls()
            },
        );

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target, "reqwest::get");
    }
}
