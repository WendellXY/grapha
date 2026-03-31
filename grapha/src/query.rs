pub mod context;
pub mod entries;
pub mod impact;
pub mod reverse;
pub mod trace;

use serde::Serialize;
use thiserror::Error;

use grapha_core::graph::{Node, NodeKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueryCandidate {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Error)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum QueryResolveError {
    #[error("not found")]
    NotFound { query: String },
    #[error("ambiguous query")]
    Ambiguous {
        query: String,
        candidates: Vec<QueryCandidate>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchTier {
    ExactNormalizedName,
    NormalizedPrefix,
    IdSuffix,
    CaseInsensitiveNormalizedExact,
}

pub(crate) fn strip_accessor_prefix(name: &str) -> &str {
    name.strip_prefix("getter:")
        .or_else(|| name.strip_prefix("setter:"))
        .unwrap_or(name)
}

pub(crate) fn normalize_symbol_name(name: &str) -> &str {
    let without_accessor = strip_accessor_prefix(name);
    without_accessor
        .split_once('(')
        .map(|(head, _)| head)
        .unwrap_or(without_accessor)
}

fn kind_preference(kind: NodeKind) -> usize {
    match kind {
        NodeKind::Function => 0,
        NodeKind::Property => 1,
        NodeKind::Variant | NodeKind::Field => 2,
        _ => 3,
    }
}

fn split_file_symbol_query(query: &str) -> Option<(&str, &str)> {
    let (file_part, symbol_part) = query.rsplit_once("::")?;
    if file_part.is_empty() || symbol_part.is_empty() {
        return None;
    }
    Some((file_part, symbol_part))
}

fn match_tier(node: &Node, query: &str) -> Option<MatchTier> {
    let normalized_query = normalize_symbol_name(query);
    let normalized_name = normalize_symbol_name(&node.name);

    if normalized_name == normalized_query {
        Some(MatchTier::ExactNormalizedName)
    } else if normalized_name.starts_with(normalized_query) {
        Some(MatchTier::NormalizedPrefix)
    } else if node.id.ends_with(query) {
        Some(MatchTier::IdSuffix)
    } else if normalized_name.eq_ignore_ascii_case(normalized_query) {
        Some(MatchTier::CaseInsensitiveNormalizedExact)
    } else {
        None
    }
}

fn to_candidate(node: &Node) -> QueryCandidate {
    QueryCandidate {
        id: node.id.clone(),
        name: node.name.clone(),
        kind: node.kind,
        file: node.file.to_string_lossy().to_string(),
    }
}

pub fn ambiguity_hint() -> &'static str {
    "Retry with file.swift::symbol or the full symbol id."
}

/// Resolve a node by query using scored matching and explicit ambiguity
/// reporting.
pub fn resolve_node<'a>(nodes: &'a [Node], query: &str) -> Result<&'a Node, QueryResolveError> {
    if let Some(node) = nodes.iter().find(|node| node.id == query) {
        return Ok(node);
    }

    let (candidate_nodes, match_query): (Vec<&Node>, &str) = match split_file_symbol_query(query) {
        Some((file_part, symbol_part)) => (
            nodes
                .iter()
                .filter(|node| node.file.to_string_lossy().ends_with(file_part))
                .collect(),
            symbol_part,
        ),
        None => (nodes.iter().collect(), query),
    };

    let matches: Vec<_> = candidate_nodes
        .into_iter()
        .filter_map(|node| {
            let tier = match_tier(node, match_query)?;
            Some((node, tier, kind_preference(node.kind)))
        })
        .collect();

    if matches.is_empty() {
        return Err(QueryResolveError::NotFound {
            query: query.to_string(),
        });
    }

    let best_tier = matches
        .iter()
        .map(|(_, tier, _)| *tier)
        .min()
        .expect("matches is not empty");
    let best_kind = matches
        .iter()
        .filter(|(_, tier, _)| *tier == best_tier)
        .map(|(_, _, kind)| *kind)
        .min()
        .expect("matches is not empty");

    let best_matches: Vec<&Node> = matches
        .iter()
        .filter(|(_, tier, kind)| *tier == best_tier && *kind == best_kind)
        .map(|(node, _, _)| *node)
        .collect();

    if best_matches.len() == 1 {
        return Ok(best_matches[0]);
    }

    Err(QueryResolveError::Ambiguous {
        query: query.to_string(),
        candidates: best_matches.into_iter().map(to_candidate).collect(),
    })
}

#[derive(Debug, Serialize)]
pub struct ContextResult {
    pub symbol: SymbolInfo,
    pub callers: Vec<SymbolRef>,
    pub callees: Vec<SymbolRef>,
    pub implementors: Vec<SymbolRef>,
    pub implements: Vec<SymbolRef>,
    pub type_refs: Vec<SymbolRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolInfo {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub file: String,
    pub span: [usize; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SymbolRef {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub file: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::{NodeRole, Span, Visibility};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, name: &str, kind: NodeKind, file: &str) -> Node {
        Node {
            id: id.into(),
            kind,
            name: name.into(),
            file: PathBuf::from(file),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None::<NodeRole>,
            signature: None,
            doc_comment: None,
            module: None,
        }
    }

    #[test]
    fn bare_send_gift_prefers_functions_over_variants_and_properties() {
        let nodes = vec![
            make_node(
                "variant-id",
                "sendGift",
                NodeKind::Variant,
                "FamilyServiceCore.swift",
            ),
            make_node(
                "property-id",
                "sendGift",
                NodeKind::Property,
                "GiftView.swift",
            ),
            make_node(
                "function-id",
                "sendGift(req:)",
                NodeKind::Function,
                "GiftServiceCore.swift",
            ),
        ];

        let resolved = resolve_node(&nodes, "sendGift").unwrap();
        assert_eq!(resolved.id, "function-id");
    }

    #[test]
    fn bare_send_gift_returns_ambiguous_when_functions_share_top_rank() {
        let nodes = vec![
            make_node(
                "function-1",
                "sendGift(req:)",
                NodeKind::Function,
                "GiftServiceCore.swift",
            ),
            make_node(
                "function-2",
                "sendGift(goods:targetId:)",
                NodeKind::Function,
                "StoreModule.swift",
            ),
            make_node(
                "variant-id",
                "sendGift",
                NodeKind::Variant,
                "HeadlineData.swift",
            ),
        ];

        let err = resolve_node(&nodes, "sendGift").unwrap_err();
        match err {
            QueryResolveError::Ambiguous { query, candidates } => {
                assert_eq!(query, "sendGift");
                assert_eq!(candidates.len(), 2);
                assert!(
                    candidates
                        .iter()
                        .all(|candidate| candidate.kind == NodeKind::Function)
                );
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn swift_file_symbol_query_matches_against_node_file_suffix() {
        let nodes = vec![
            make_node(
                "s:12ModuleExport15GiftServiceCoreC04sendC03reqy...",
                "sendGift(req:)",
                NodeKind::Function,
                "GiftServiceCore.swift",
            ),
            make_node(
                "s:5Store0A6ModuleC8sendGift5goods8targetIdy...",
                "sendGift(goods:targetId:)",
                NodeKind::Function,
                "StoreModule.swift",
            ),
        ];

        let resolved = resolve_node(&nodes, "GiftServiceCore.swift::sendGift").unwrap();
        assert_eq!(resolved.file, PathBuf::from("GiftServiceCore.swift"));
    }
}
