use std::collections::HashMap;

use serde::Serialize;

use grapha_core::graph::{EdgeKind, Graph, Node, NodeKind};

use crate::localization::{
    LocalizationCatalogIndex, LocalizationCatalogRecord, LocalizationReference, edges_by_source,
    localization_usage_nodes, node_index, resolve_usage,
};

use super::SymbolInfo;

#[derive(Debug, Serialize)]
pub struct UsagesResult {
    pub query: UsageQuery,
    pub records: Vec<RecordUsages>,
}

#[derive(Debug, Serialize)]
pub struct UsageQuery {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecordUsages {
    pub record: LocalizationCatalogRecord,
    pub usages: Vec<UsageSite>,
}

#[derive(Debug, Serialize)]
pub struct UsageSite {
    pub owner: SymbolInfo,
    pub view: SymbolInfo,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ui_path: Vec<String>,
    pub reference: LocalizationReference,
}

pub fn query_usages(
    graph: &Graph,
    catalogs: &LocalizationCatalogIndex,
    key: &str,
    table: Option<&str>,
) -> UsagesResult {
    let records = if let Some(table) = table {
        catalogs.records_for(table, key)
    } else {
        catalogs.records_for_key(key)
    };

    let node_index = node_index(graph);
    let edges_by_source = edges_by_source(graph);
    let parents = contains_parents(graph);

    let mut record_groups = Vec::new();
    for record in records {
        let usages = localization_usage_nodes(graph)
            .into_iter()
            .filter_map(|usage_node| {
                let resolution =
                    resolve_usage(usage_node, &edges_by_source, &node_index, catalogs)?;
                let matched_reference = resolution.matches.into_iter().find_map(|item| {
                    (item.record.table == record.table
                        && item.record.key == record.key
                        && item.record.file == record.file)
                        .then_some(item.reference)
                })?;

                let owner = owning_symbol(usage_node.id.as_str(), &parents, &node_index)
                    .unwrap_or(usage_node);
                Some(UsageSite {
                    owner: to_symbol_info(owner),
                    view: to_symbol_info(usage_node),
                    ui_path: ui_path(
                        usage_node.id.as_str(),
                        owner.id.as_str(),
                        &parents,
                        &node_index,
                    ),
                    reference: matched_reference,
                })
            })
            .collect();

        record_groups.push(RecordUsages { record, usages });
    }

    UsagesResult {
        query: UsageQuery {
            key: key.to_string(),
            table: table.map(ToString::to_string),
        },
        records: record_groups,
    }
}

fn to_symbol_info(node: &Node) -> SymbolInfo {
    SymbolInfo {
        id: node.id.clone(),
        name: node.name.clone(),
        kind: node.kind,
        file: node.file.to_string_lossy().to_string(),
        span: [node.span.start[0], node.span.end[0]],
    }
}

fn contains_parents<'a>(graph: &'a Graph) -> HashMap<&'a str, &'a str> {
    let mut map = HashMap::new();
    for edge in &graph.edges {
        if edge.kind == EdgeKind::Contains {
            map.insert(edge.target.as_str(), edge.source.as_str());
        }
    }
    map
}

fn owning_symbol<'a>(
    node_id: &'a str,
    parents: &HashMap<&'a str, &'a str>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> Option<&'a Node> {
    let mut current = Some(node_id);
    while let Some(id) = current {
        let node = node_index.get(id).copied()?;
        if !matches!(node.kind, NodeKind::View | NodeKind::Branch) {
            return Some(node);
        }
        current = parents.get(id).copied();
    }
    None
}

fn ui_path<'a>(
    usage_id: &'a str,
    owner_id: &'a str,
    parents: &HashMap<&'a str, &'a str>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> Vec<String> {
    let mut path = Vec::new();
    let mut current = Some(usage_id);

    while let Some(node_id) = current {
        if node_id == owner_id {
            break;
        }
        let Some(node) = node_index.get(node_id).copied() else {
            break;
        };
        if matches!(node.kind, NodeKind::View | NodeKind::Branch) {
            path.push(node.name.clone());
        }
        current = parents.get(node_id).copied();
    }

    path.reverse();
    path
}
