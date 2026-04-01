use std::collections::{HashMap, HashSet};

use serde::Serialize;

use grapha_core::graph::{Graph, Node};

use crate::localization::{
    LocalizationCatalogIndex, LocalizationReference, edges_by_source, node_index, resolve_usage,
};

use super::l10n::{contains_adjacency, contains_parents, to_symbol_info, ui_path};
use super::{QueryResolveError, SymbolInfo, resolve_node};

#[derive(Debug, Serialize)]
pub struct LocalizeResult {
    pub symbol: SymbolInfo,
    pub matches: Vec<LocalizationMatch>,
    pub unmatched: Vec<UnmatchedLocalizationUsage>,
}

#[derive(Debug, Serialize)]
pub struct LocalizationMatch {
    pub view: SymbolInfo,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ui_path: Vec<String>,
    pub reference: LocalizationReference,
    pub record: crate::localization::LocalizationCatalogRecord,
    pub match_kind: String,
}

#[derive(Debug, Serialize)]
pub struct UnmatchedLocalizationUsage {
    pub view: SymbolInfo,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ui_path: Vec<String>,
    pub reference: LocalizationReference,
    pub reason: String,
}

pub fn query_localize(
    graph: &Graph,
    catalogs: &LocalizationCatalogIndex,
    symbol: &str,
) -> Result<LocalizeResult, QueryResolveError> {
    let root = resolve_node(&graph.nodes, symbol)?;
    let node_index = node_index(graph);
    let edges_by_source = edges_by_source(graph);
    let contains_adj = contains_adjacency(graph);
    let parents = contains_parents(graph);
    let usage_ids = usage_ids_in_subtree(root.id.as_str(), &contains_adj, &node_index);

    let mut matches = Vec::new();
    let mut unmatched = Vec::new();

    for usage_id in usage_ids {
        let Some(usage_node) = node_index.get(usage_id).copied() else {
            continue;
        };
        let Some(resolution) = resolve_usage(usage_node, &edges_by_source, &node_index, catalogs)
        else {
            continue;
        };

        let ui_path = ui_path(usage_id, root.id.as_str(), &parents, &node_index);
        for item in resolution.matches {
            matches.push(LocalizationMatch {
                view: to_symbol_info(usage_node),
                ui_path: ui_path.clone(),
                reference: item.reference,
                record: item.record,
                match_kind: item.match_kind,
            });
        }
        if let Some(item) = resolution.unmatched {
            unmatched.push(UnmatchedLocalizationUsage {
                view: to_symbol_info(usage_node),
                ui_path,
                reference: item.reference,
                reason: item.reason,
            });
        }
    }

    Ok(LocalizeResult {
        symbol: to_symbol_info(root),
        matches,
        unmatched,
    })
}

fn usage_ids_in_subtree<'a>(
    root_id: &'a str,
    contains_adj: &HashMap<&'a str, Vec<&'a str>>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> Vec<&'a str> {
    let mut stack = vec![root_id];
    let mut seen = HashSet::new();
    let mut usage_ids = Vec::new();

    while let Some(current_id) = stack.pop() {
        if !seen.insert(current_id) {
            continue;
        }
        if node_index
            .get(current_id)
            .is_some_and(|node| node.metadata.contains_key("l10n.ref_kind"))
        {
            usage_ids.push(current_id);
        }

        if let Some(children) = contains_adj.get(current_id) {
            for &child in children.iter().rev() {
                stack.push(child);
            }
        }
    }

    usage_ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::LocalizationCatalogRecord;
    use grapha_core::graph::{Edge, EdgeKind, NodeKind, Span, Visibility};
    use std::collections::HashMap as StdHashMap;
    use std::path::PathBuf;

    fn node(id: &str, name: &str, kind: NodeKind) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            file: PathBuf::from("ContentView.swift"),
            span: Span {
                start: [0, 0],
                end: [0, 1],
            },
            visibility: Visibility::Private,
            metadata: StdHashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        }
    }

    #[test]
    fn localize_reports_matches_for_usage_nodes() {
        let mut body = node("body", "body", NodeKind::Property);
        body.role = Some(grapha_core::graph::NodeRole::EntryPoint);
        let mut text = node("text", "Text", NodeKind::View);
        text.metadata
            .insert("l10n.ref_kind".to_string(), "wrapper".to_string());
        text.metadata
            .insert("l10n.wrapper_name".to_string(), "welcomeTitle".to_string());
        let mut wrapper = node("wrapper", "welcomeTitle", NodeKind::Property);
        wrapper
            .metadata
            .insert("l10n.wrapper.table".to_string(), "Localizable".to_string());
        wrapper
            .metadata
            .insert("l10n.wrapper.key".to_string(), "welcome_title".to_string());

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![body, text, wrapper],
            edges: vec![
                Edge {
                    source: "body".to_string(),
                    target: "text".to_string(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: vec![],
                },
                Edge {
                    source: "text".to_string(),
                    target: "wrapper".to_string(),
                    kind: EdgeKind::TypeRef,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: vec![],
                },
            ],
        };

        let mut catalogs = LocalizationCatalogIndex::default();
        catalogs.insert(LocalizationCatalogRecord {
            table: "Localizable".to_string(),
            key: "welcome_title".to_string(),
            catalog_file: "Localizable.xcstrings".to_string(),
            catalog_dir: ".".to_string(),
            source_language: "en".to_string(),
            source_value: "Welcome".to_string(),
            status: "translated".to_string(),
            comment: None,
        });

        let result = query_localize(&graph, &catalogs, "body").unwrap();
        assert_eq!(result.matches.len(), 1);
        assert!(result.unmatched.is_empty());
        assert_eq!(result.matches[0].record.key, "welcome_title");
    }
}
