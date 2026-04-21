use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::extract::ExtractionResult;
use crate::graph::{
    Edge, EdgeKind, EdgeProvenance, FlowDirection, Node, NodeKind, NodeRole, Span, TerminalKind,
    Visibility,
};
use crate::resolve::Import;

const META_L10N_REF_KIND: &str = "l10n.ref_kind";
const META_L10N_WRAPPER_NAME: &str = "l10n.wrapper_name";
const META_L10N_WRAPPER_BASE: &str = "l10n.wrapper_base";
const META_L10N_WRAPPER_SYMBOL: &str = "l10n.wrapper_symbol";
const META_L10N_TABLE: &str = "l10n.table";
const META_L10N_KEY: &str = "l10n.key";
const META_L10N_FALLBACK: &str = "l10n.fallback";
const META_L10N_ARG_COUNT: &str = "l10n.arg_count";
const META_L10N_LITERAL: &str = "l10n.literal";
const META_L10N_ARGUMENT_LABEL: &str = "l10n.argument_label";
const META_L10N_WRAPPER_TABLE: &str = "l10n.wrapper.table";
const META_L10N_WRAPPER_KEY: &str = "l10n.wrapper.key";
const META_L10N_WRAPPER_FALLBACK: &str = "l10n.wrapper.fallback";
const META_L10N_WRAPPER_ARG_COUNT: &str = "l10n.wrapper.arg_count";
const META_ASSET_REF_KIND: &str = "asset.ref_kind";
const META_ASSET_NAME: &str = "asset.name";
const META_SWIFTUI_INVALIDATION_SOURCE: &str = "swiftui.invalidation_source";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticDocument {
    pub symbols: Vec<SemanticSymbol>,
    pub relations: Vec<SemanticRelation>,
    pub artifacts: Vec<SemanticArtifact>,
    pub imports: Vec<Import>,
}

impl Default for SemanticDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticDocument {
    pub fn new() -> Self {
        Self {
            symbols: Vec::new(),
            relations: Vec::new(),
            artifacts: Vec::new(),
            imports: Vec::new(),
        }
    }

    pub fn from_extraction_result(result: ExtractionResult) -> Self {
        let mut document = Self::new();
        document.imports = result.imports;
        let symbol_ids: HashSet<String> = result.nodes.iter().map(|node| node.id.clone()).collect();

        for node in result.nodes {
            let (symbol, mut artifacts) = SemanticSymbol::from_node(node);
            document.symbols.push(symbol);
            document.artifacts.append(&mut artifacts);
        }

        for edge in result.edges {
            document
                .relations
                .push(SemanticRelation::from_edge(edge, &symbol_ids));
        }

        document
    }

    pub fn into_extraction_result(self) -> ExtractionResult {
        let mut relation_terminal_roles: HashMap<String, TerminalKind> = HashMap::new();
        let symbol_ids: HashSet<&str> = self
            .symbols
            .iter()
            .map(|symbol| symbol.id.as_str())
            .collect();

        for relation in &self.relations {
            let Some(kind) = relation.terminal_kind else {
                continue;
            };

            let terminal_symbol_id = match &relation.target {
                SemanticTarget::Symbol(symbol_id) if symbol_ids.contains(symbol_id.as_str()) => {
                    symbol_id.clone()
                }
                _ => relation.source.clone(),
            };
            relation_terminal_roles
                .entry(terminal_symbol_id)
                .or_insert(kind);
        }

        let mut artifact_metadata: HashMap<&str, HashMap<String, String>> = HashMap::new();
        for artifact in &self.artifacts {
            let metadata = artifact_metadata.entry(artifact.symbol_id()).or_default();
            artifact.write_metadata(metadata);
        }

        let nodes = self
            .symbols
            .into_iter()
            .map(|symbol| {
                let mut node = symbol.into_node();

                if let Some(metadata) = artifact_metadata.remove(node.id.as_str()) {
                    node.metadata.extend(metadata);
                }

                if node.role.is_none()
                    && let Some(kind) = relation_terminal_roles.get(node.id.as_str())
                {
                    node.role = Some(NodeRole::Terminal { kind: *kind });
                }

                node
            })
            .collect();

        let edges = self
            .relations
            .into_iter()
            .map(SemanticRelation::into_edge)
            .collect();

        ExtractionResult {
            nodes,
            edges,
            imports: self.imports,
        }
    }

    pub fn annotate_call_relations<F>(&mut self, mut classify: F)
    where
        F: FnMut(&SemanticRelation, Option<&SemanticSymbol>) -> Option<TerminalEffect>,
    {
        self.apply_call_relation_effects(&mut classify, false);
    }

    pub fn override_call_relations<F>(&mut self, mut classify: F)
    where
        F: FnMut(&SemanticRelation, Option<&SemanticSymbol>) -> Option<TerminalEffect>,
    {
        self.apply_call_relation_effects(&mut classify, true);
    }

    fn apply_call_relation_effects<F>(&mut self, classify: &mut F, overwrite_existing: bool)
    where
        F: FnMut(&SemanticRelation, Option<&SemanticSymbol>) -> Option<TerminalEffect>,
    {
        let symbols_by_id: HashMap<&str, &SemanticSymbol> = self
            .symbols
            .iter()
            .map(|symbol| (symbol.id.as_str(), symbol))
            .collect();

        for relation in &mut self.relations {
            if relation.kind != EdgeKind::Calls {
                continue;
            }

            if !overwrite_existing
                && (relation.direction.is_some()
                    || relation.operation.is_some()
                    || relation.terminal_kind.is_some())
            {
                continue;
            }

            let Some(effect) = classify(
                relation,
                symbols_by_id.get(relation.source.as_str()).copied(),
            ) else {
                continue;
            };

            relation.terminal_kind = Some(effect.terminal_kind);
            relation.direction = Some(effect.direction);
            relation.operation = Some(effect.operation);
        }
    }

    pub fn stamp_module(mut self, module_name: Option<&str>) -> Self {
        let Some(module_name) = module_name else {
            return self;
        };

        for symbol in &mut self.symbols {
            symbol.module.get_or_insert_with(|| module_name.to_string());
        }

        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticSymbol {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub file: PathBuf,
    pub span: Span,
    pub visibility: Visibility,
    pub properties: HashMap<String, String>,
    pub annotations: Vec<SemanticAnnotation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic_kind: Option<String>,
}

impl SemanticSymbol {
    fn from_node(mut node: Node) -> (Self, Vec<SemanticArtifact>) {
        let mut annotations = Vec::new();

        if let Some(role) = node.role.take() {
            match role {
                NodeRole::EntryPoint => annotations.push(SemanticAnnotation::EntryPoint),
                NodeRole::Terminal { kind } => {
                    annotations.push(SemanticAnnotation::Terminal { kind });
                }
                NodeRole::Internal => annotations.push(SemanticAnnotation::Internal),
            }
        }

        if let Some(value) = node.metadata.remove(META_SWIFTUI_INVALIDATION_SOURCE) {
            annotations.push(SemanticAnnotation::Flag {
                key: META_SWIFTUI_INVALIDATION_SOURCE.to_string(),
                value,
            });
        }

        let artifacts = extract_symbol_artifacts(&mut node.metadata, node.id.clone());
        let synthetic_kind = match node.kind {
            NodeKind::View => Some("swiftui_view".to_string()),
            NodeKind::Branch => Some("swiftui_branch".to_string()),
            _ => None,
        };

        let symbol = Self {
            id: node.id,
            kind: node.kind,
            name: node.name,
            file: node.file,
            span: node.span,
            visibility: node.visibility,
            properties: node.metadata,
            annotations,
            signature: node.signature,
            doc_comment: node.doc_comment,
            module: node.module,
            snippet: node.snippet,
            synthetic_kind,
        };

        (symbol, artifacts)
    }

    fn into_node(self) -> Node {
        let mut role = None;
        let mut metadata = self.properties;

        for annotation in &self.annotations {
            match annotation {
                SemanticAnnotation::EntryPoint if role.is_none() => {
                    role = Some(NodeRole::EntryPoint);
                }
                SemanticAnnotation::Terminal { kind } if role.is_none() => {
                    role = Some(NodeRole::Terminal { kind: *kind });
                }
                SemanticAnnotation::Internal if role.is_none() => {
                    role = Some(NodeRole::Internal);
                }
                SemanticAnnotation::Flag { key, value } => {
                    metadata.insert(key.clone(), value.clone());
                }
                _ => {}
            }
        }

        Node {
            id: self.id,
            kind: self.kind,
            name: self.name,
            file: self.file,
            span: self.span,
            visibility: self.visibility,
            metadata,
            role,
            signature: self.signature,
            doc_comment: self.doc_comment,
            module: self.module,
            snippet: self.snippet,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    LocalizationRef,
    LocalizationWrapperBinding,
    AssetRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SemanticArtifact {
    LocalizationRef {
        symbol_id: String,
        ref_kind: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        wrapper_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        wrapper_base: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        wrapper_symbol: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        table: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fallback: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arg_count: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        literal: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        argument_label: Option<String>,
    },
    LocalizationWrapperBinding {
        symbol_id: String,
        table: String,
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        fallback: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arg_count: Option<usize>,
    },
    AssetRef {
        symbol_id: String,
        ref_kind: String,
        name: String,
    },
}

impl SemanticArtifact {
    pub fn symbol_id(&self) -> &str {
        match self {
            SemanticArtifact::LocalizationRef { symbol_id, .. }
            | SemanticArtifact::LocalizationWrapperBinding { symbol_id, .. }
            | SemanticArtifact::AssetRef { symbol_id, .. } => symbol_id,
        }
    }

    pub fn kind(&self) -> ArtifactKind {
        match self {
            SemanticArtifact::LocalizationRef { .. } => ArtifactKind::LocalizationRef,
            SemanticArtifact::LocalizationWrapperBinding { .. } => {
                ArtifactKind::LocalizationWrapperBinding
            }
            SemanticArtifact::AssetRef { .. } => ArtifactKind::AssetRef,
        }
    }

    fn write_metadata(&self, metadata: &mut HashMap<String, String>) {
        match self {
            SemanticArtifact::LocalizationRef {
                ref_kind,
                wrapper_name,
                wrapper_base,
                wrapper_symbol,
                table,
                key,
                fallback,
                arg_count,
                literal,
                argument_label,
                ..
            } => {
                metadata.insert(META_L10N_REF_KIND.to_string(), ref_kind.clone());
                if let Some(value) = wrapper_name {
                    metadata.insert(META_L10N_WRAPPER_NAME.to_string(), value.clone());
                }
                if let Some(value) = wrapper_base {
                    metadata.insert(META_L10N_WRAPPER_BASE.to_string(), value.clone());
                }
                if let Some(value) = wrapper_symbol {
                    metadata.insert(META_L10N_WRAPPER_SYMBOL.to_string(), value.clone());
                }
                if let Some(value) = table {
                    metadata.insert(META_L10N_TABLE.to_string(), value.clone());
                }
                if let Some(value) = key {
                    metadata.insert(META_L10N_KEY.to_string(), value.clone());
                }
                if let Some(value) = fallback {
                    metadata.insert(META_L10N_FALLBACK.to_string(), value.clone());
                }
                if let Some(value) = arg_count {
                    metadata.insert(META_L10N_ARG_COUNT.to_string(), value.to_string());
                }
                if let Some(value) = literal {
                    metadata.insert(META_L10N_LITERAL.to_string(), value.clone());
                }
                if let Some(value) = argument_label {
                    metadata.insert(META_L10N_ARGUMENT_LABEL.to_string(), value.clone());
                }
            }
            SemanticArtifact::LocalizationWrapperBinding {
                table,
                key,
                fallback,
                arg_count,
                ..
            } => {
                metadata.insert(META_L10N_WRAPPER_TABLE.to_string(), table.clone());
                metadata.insert(META_L10N_WRAPPER_KEY.to_string(), key.clone());
                if let Some(value) = fallback {
                    metadata.insert(META_L10N_WRAPPER_FALLBACK.to_string(), value.clone());
                }
                if let Some(value) = arg_count {
                    metadata.insert(META_L10N_WRAPPER_ARG_COUNT.to_string(), value.to_string());
                }
            }
            SemanticArtifact::AssetRef { ref_kind, name, .. } => {
                metadata.insert(META_ASSET_REF_KIND.to_string(), ref_kind.clone());
                metadata.insert(META_ASSET_NAME.to_string(), name.clone());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum SemanticAnnotation {
    EntryPoint,
    Terminal { kind: TerminalKind },
    Internal,
    Flag { key: String, value: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticRelation {
    pub source: String,
    pub target: SemanticTarget,
    pub kind: EdgeKind,
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<FlowDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub async_boundary: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provenance: Vec<EdgeProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_kind: Option<TerminalKind>,
}

impl SemanticRelation {
    fn from_edge(edge: Edge, symbol_ids: &HashSet<String>) -> Self {
        let target = if symbol_ids.contains(&edge.target) {
            SemanticTarget::Symbol(edge.target)
        } else {
            SemanticTarget::ExternalRef(edge.target)
        };
        Self {
            source: edge.source,
            target,
            kind: edge.kind,
            confidence: edge.confidence,
            direction: edge.direction,
            operation: edge.operation,
            condition: edge.condition,
            async_boundary: edge.async_boundary,
            provenance: edge.provenance,
            terminal_kind: None,
        }
    }

    fn into_edge(self) -> Edge {
        Edge {
            source: self.source,
            target: self.target.into_raw(),
            kind: self.kind,
            confidence: self.confidence,
            direction: self.direction,
            operation: self.operation,
            condition: self.condition,
            async_boundary: self.async_boundary,
            provenance: self.provenance,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum SemanticTarget {
    Symbol(String),
    ExternalRef(String),
}

impl SemanticTarget {
    pub fn as_raw(&self) -> &str {
        match self {
            SemanticTarget::Symbol(value) | SemanticTarget::ExternalRef(value) => value,
        }
    }

    fn into_raw(self) -> String {
        match self {
            SemanticTarget::Symbol(value) | SemanticTarget::ExternalRef(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalEffect {
    pub terminal_kind: TerminalKind,
    pub direction: FlowDirection,
    pub operation: String,
}

fn extract_symbol_artifacts(
    metadata: &mut HashMap<String, String>,
    symbol_id: String,
) -> Vec<SemanticArtifact> {
    let mut artifacts = Vec::new();

    let wrapper_binding = (
        metadata.remove(META_L10N_WRAPPER_TABLE),
        metadata.remove(META_L10N_WRAPPER_KEY),
    );
    if let (Some(table), Some(key)) = wrapper_binding {
        let fallback = metadata.remove(META_L10N_WRAPPER_FALLBACK);
        let arg_count = metadata
            .remove(META_L10N_WRAPPER_ARG_COUNT)
            .and_then(|value| value.parse::<usize>().ok());
        artifacts.push(SemanticArtifact::LocalizationWrapperBinding {
            symbol_id: symbol_id.clone(),
            table,
            key,
            fallback,
            arg_count,
        });
    }

    let localization_ref = metadata.remove(META_L10N_REF_KIND);
    if let Some(ref_kind) = localization_ref {
        let table = metadata.remove(META_L10N_TABLE);
        let key = metadata.remove(META_L10N_KEY);
        let fallback = metadata.remove(META_L10N_FALLBACK);
        let arg_count = metadata
            .remove(META_L10N_ARG_COUNT)
            .and_then(|value| value.parse::<usize>().ok());
        let literal = metadata.remove(META_L10N_LITERAL);
        let wrapper_name = metadata.remove(META_L10N_WRAPPER_NAME);
        let wrapper_base = metadata.remove(META_L10N_WRAPPER_BASE);
        let wrapper_symbol = metadata.remove(META_L10N_WRAPPER_SYMBOL);
        let argument_label = metadata.remove(META_L10N_ARGUMENT_LABEL);

        artifacts.push(SemanticArtifact::LocalizationRef {
            symbol_id: symbol_id.clone(),
            ref_kind,
            wrapper_name,
            wrapper_base,
            wrapper_symbol,
            table,
            key,
            fallback,
            arg_count,
            literal,
            argument_label,
        });
    }

    let asset_ref = (
        metadata.remove(META_ASSET_REF_KIND),
        metadata.remove(META_ASSET_NAME),
    );
    if let (Some(ref_kind), Some(name)) = asset_ref {
        artifacts.push(SemanticArtifact::AssetRef {
            symbol_id,
            ref_kind,
            name,
        });
    }

    artifacts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_span() -> Span {
        Span {
            start: [1, 0],
            end: [2, 0],
        }
    }

    #[test]
    fn round_trips_known_metadata_into_typed_artifacts() {
        let mut metadata = HashMap::new();
        metadata.insert(META_L10N_REF_KIND.to_string(), "literal".to_string());
        metadata.insert(META_L10N_LITERAL.to_string(), "Hello".to_string());
        metadata.insert(META_ASSET_REF_KIND.to_string(), "image".to_string());
        metadata.insert(META_ASSET_NAME.to_string(), "hero".to_string());
        metadata.insert(
            META_SWIFTUI_INVALIDATION_SOURCE.to_string(),
            "true".to_string(),
        );
        metadata.insert("async".to_string(), "true".to_string());

        let result = ExtractionResult {
            nodes: vec![Node {
                id: "body".to_string(),
                kind: NodeKind::View,
                name: "Text".to_string(),
                file: PathBuf::from("ContentView.swift"),
                span: test_span(),
                visibility: Visibility::Public,
                metadata,
                role: Some(NodeRole::EntryPoint),
                signature: Some("var body: some View".to_string()),
                doc_comment: None,
                module: Some("Demo".to_string()),
                snippet: None,
            }],
            edges: Vec::new(),
            imports: Vec::new(),
        };

        let document = SemanticDocument::from_extraction_result(result);
        assert_eq!(document.symbols.len(), 1);
        assert_eq!(document.artifacts.len(), 2);
        assert!(
            document.symbols[0]
                .annotations
                .contains(&SemanticAnnotation::EntryPoint)
        );
        assert!(
            document.symbols[0]
                .annotations
                .contains(&SemanticAnnotation::Flag {
                    key: META_SWIFTUI_INVALIDATION_SOURCE.to_string(),
                    value: "true".to_string(),
                })
        );
        assert_eq!(
            document.symbols[0]
                .properties
                .get("async")
                .map(String::as_str),
            Some("true")
        );

        let lowered = document.into_extraction_result();
        let node = &lowered.nodes[0];
        assert_eq!(node.role, Some(NodeRole::EntryPoint));
        assert_eq!(
            node.metadata.get(META_L10N_REF_KIND).map(String::as_str),
            Some("literal")
        );
        assert_eq!(
            node.metadata.get(META_ASSET_NAME).map(String::as_str),
            Some("hero")
        );
        assert_eq!(
            node.metadata
                .get(META_SWIFTUI_INVALIDATION_SOURCE)
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(node.metadata.get("async").map(String::as_str), Some("true"));
    }

    #[test]
    fn relation_terminal_effect_marks_source_node_when_target_is_external() {
        let mut document = SemanticDocument::new();
        document.symbols.push(SemanticSymbol {
            id: "caller".to_string(),
            kind: NodeKind::Function,
            name: "load".to_string(),
            file: PathBuf::from("main.rs"),
            span: test_span(),
            visibility: Visibility::Public,
            properties: HashMap::new(),
            annotations: Vec::new(),
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
            synthetic_kind: None,
        });
        document.relations.push(SemanticRelation {
            source: "caller".to_string(),
            target: SemanticTarget::ExternalRef("reqwest::get".to_string()),
            kind: EdgeKind::Calls,
            confidence: 1.0,
            direction: Some(FlowDirection::Read),
            operation: Some("HTTP".to_string()),
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
            terminal_kind: Some(TerminalKind::Network),
        });

        let lowered = document.into_extraction_result();
        assert_eq!(
            lowered.nodes[0].role,
            Some(NodeRole::Terminal {
                kind: TerminalKind::Network,
            })
        );
        assert_eq!(lowered.edges[0].direction, Some(FlowDirection::Read));
    }

    #[test]
    fn annotate_call_relations_uses_source_symbol_context() {
        let mut document = SemanticDocument::new();
        document.symbols.push(SemanticSymbol {
            id: "caller".to_string(),
            kind: NodeKind::Function,
            name: "load".to_string(),
            file: PathBuf::from("main.rs"),
            span: test_span(),
            visibility: Visibility::Public,
            properties: HashMap::new(),
            annotations: Vec::new(),
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
            synthetic_kind: None,
        });
        document.relations.push(SemanticRelation {
            source: "caller".to_string(),
            target: SemanticTarget::ExternalRef("reqwest::get".to_string()),
            kind: EdgeKind::Calls,
            confidence: 1.0,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
            terminal_kind: None,
        });

        document.annotate_call_relations(|relation, source| {
            assert_eq!(relation.target.as_raw(), "reqwest::get");
            assert_eq!(source.map(|symbol| symbol.name.as_str()), Some("load"));
            Some(TerminalEffect {
                terminal_kind: TerminalKind::Network,
                direction: FlowDirection::Read,
                operation: "HTTP".to_string(),
            })
        });

        assert_eq!(
            document.relations[0].terminal_kind,
            Some(TerminalKind::Network)
        );
        assert_eq!(document.relations[0].operation.as_deref(), Some("HTTP"));
    }

    #[test]
    fn override_call_relations_replaces_existing_effect() {
        let mut document = SemanticDocument::new();
        document.symbols.push(SemanticSymbol {
            id: "caller".to_string(),
            kind: NodeKind::Function,
            name: "load".to_string(),
            file: PathBuf::from("main.rs"),
            span: test_span(),
            visibility: Visibility::Public,
            properties: HashMap::new(),
            annotations: Vec::new(),
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
            synthetic_kind: None,
        });
        document.relations.push(SemanticRelation {
            source: "caller".to_string(),
            target: SemanticTarget::ExternalRef("reqwest::get".to_string()),
            kind: EdgeKind::Calls,
            confidence: 1.0,
            direction: Some(FlowDirection::Read),
            operation: Some("HTTP".to_string()),
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
            terminal_kind: Some(TerminalKind::Network),
        });

        document.override_call_relations(|_, _| {
            Some(TerminalEffect {
                terminal_kind: TerminalKind::Event,
                direction: FlowDirection::Write,
                operation: "CUSTOM".to_string(),
            })
        });

        assert_eq!(
            document.relations[0].terminal_kind,
            Some(TerminalKind::Event)
        );
        assert_eq!(document.relations[0].direction, Some(FlowDirection::Write));
        assert_eq!(document.relations[0].operation.as_deref(), Some("CUSTOM"));
    }
}
