use std::collections::HashMap;

use serde::Serialize;

use grapha_core::graph::{Graph, Node, NodeKind};

use crate::localization::{
    LocalizationCatalogIndex, LocalizationCatalogRecord, LocalizationReference, edges_by_source,
    localization_usage_nodes, node_index, resolve_usage_with, wrapper_binding_nodes,
};

use super::SymbolInfo;
use super::l10n::{contains_parents, to_symbol_info, ui_path};

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
    let wrapper_nodes = wrapper_binding_nodes(&node_index);

    // Resolve all usage nodes once (not per record) to avoid redundant work.
    let resolved_usages: Vec<_> = localization_usage_nodes(graph)
        .into_iter()
        .filter_map(|usage_node| {
            let resolution = resolve_usage_with(
                usage_node,
                &edges_by_source,
                &node_index,
                &wrapper_nodes,
                catalogs,
            )?;
            if resolution.matches.is_empty() {
                return None;
            }
            Some((usage_node, resolution.matches))
        })
        .collect();

    let mut record_groups = Vec::new();
    for record in records {
        let usages = resolved_usages
            .iter()
            .filter_map(|(usage_node, matches)| {
                let matched_reference = matches.iter().find_map(|item| {
                    (item.record.table == record.table
                        && item.record.key == record.key
                        && item.record.catalog_file == record.catalog_file)
                        .then_some(item.reference.clone())
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
