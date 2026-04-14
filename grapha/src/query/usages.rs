use std::collections::{HashMap, HashSet};

use serde::Serialize;

use grapha_core::graph::{Edge, EdgeKind, Graph, Node, NodeKind};

use crate::localization::{
    LocalizationCatalogIndex, LocalizationCatalogRecord, LocalizationReference, edges_by_source,
    localization_usage_nodes, node_index, parse_wrapper_binding, resolve_usage_with,
    wrapper_binding_nodes,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_key: Option<String>,
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
    let (records, matched_by, resolved_key) = resolve_records(catalogs, key, table);

    let node_index = node_index(graph);
    let edges_by_source = edges_by_source(graph);
    let parents = contains_parents(graph);
    let wrapper_nodes = wrapper_binding_nodes(&node_index);

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

    let edges_by_target = edges_by_target(graph);

    let mut record_groups = Vec::new();
    for record in records {
        let mut usages: Vec<UsageSite> = resolved_usages
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

        let wrapper_usages = wrapper_caller_usages(
            &record,
            &wrapper_nodes,
            &edges_by_target,
            &node_index,
            &parents,
        );
        usages.extend(wrapper_usages);

        record_groups.push(RecordUsages { record, usages });
    }

    UsagesResult {
        query: UsageQuery {
            key: key.to_string(),
            table: table.map(ToString::to_string),
            matched_by,
            resolved_key,
        },
        records: record_groups,
    }
}

fn resolve_records(
    catalogs: &LocalizationCatalogIndex,
    input: &str,
    table: Option<&str>,
) -> (Vec<LocalizationCatalogRecord>, Option<String>, Option<String>) {
    let by_key = if let Some(table) = table {
        catalogs.records_for(table, input)
    } else {
        catalogs.records_for_key(input)
    };
    if !by_key.is_empty() {
        return (by_key, None, None);
    }

    let by_value = catalogs.records_for_value(input);
    if !by_value.is_empty() {
        let resolved_key = by_value[0].key.clone();
        return (
            by_value,
            Some("value".to_string()),
            Some(resolved_key),
        );
    }

    (Vec::new(), None, None)
}

fn edges_by_target(graph: &Graph) -> HashMap<&str, Vec<&Edge>> {
    let mut map: HashMap<&str, Vec<&Edge>> = HashMap::new();
    for edge in &graph.edges {
        map.entry(edge.target.as_str()).or_default().push(edge);
    }
    map
}

fn wrapper_caller_usages<'a>(
    record: &LocalizationCatalogRecord,
    wrapper_nodes: &[&'a Node],
    edges_by_target: &HashMap<&str, Vec<&'a Edge>>,
    node_index: &HashMap<&'a str, &'a Node>,
    parents: &HashMap<&'a str, &'a str>,
) -> Vec<UsageSite> {
    let matching_wrappers: Vec<&Node> = wrapper_nodes
        .iter()
        .copied()
        .filter(|node| {
            parse_wrapper_binding(node)
                .is_some_and(|b| b.table == record.table && b.key == record.key)
        })
        .collect();

    let mut usages = Vec::new();
    for wrapper in &matching_wrappers {
        let binding = parse_wrapper_binding(wrapper).unwrap();
        let reference = LocalizationReference {
            ref_kind: "wrapper".to_string(),
            wrapper_name: Some(wrapper.name.clone()),
            wrapper_base: None,
            wrapper_symbol: Some(wrapper.id.clone()),
            table: Some(binding.table.clone()),
            key: Some(binding.key.clone()),
            fallback: binding.fallback.clone(),
            arg_count: binding.arg_count,
            literal: None,
        };

        let accessor_ids = wrapper_and_accessor_ids(wrapper.id.as_str(), edges_by_target);
        let accessor_set: HashSet<&str> = accessor_ids.iter().map(String::as_str).collect();

        for target_id in &accessor_ids {
            let Some(incoming) = edges_by_target.get(target_id.as_str()) else {
                continue;
            };
            for edge in incoming {
                if !matches!(edge.kind, EdgeKind::Calls | EdgeKind::Uses) {
                    continue;
                }
                if accessor_set.contains(edge.source.as_str()) {
                    continue;
                }
                let Some(caller) = node_index.get(edge.source.as_str()) else {
                    continue;
                };
                if caller.metadata.contains_key("l10n.ref_kind")
                    || caller.metadata.contains_key("l10n.wrapper.key")
                {
                    continue;
                }
                let owner = owning_symbol(caller.id.as_str(), parents, node_index)
                    .unwrap_or(caller);
                usages.push(UsageSite {
                    owner: to_symbol_info(owner),
                    view: to_symbol_info(caller),
                    ui_path: Vec::new(),
                    reference: reference.clone(),
                });
            }
        }
    }
    usages
}

fn wrapper_and_accessor_ids(
    wrapper_id: &str,
    edges_by_target: &HashMap<&str, Vec<&Edge>>,
) -> Vec<String> {
    let mut ids = vec![wrapper_id.to_string()];
    if let Some(incoming) = edges_by_target.get(wrapper_id) {
        for edge in incoming {
            if edge.kind == EdgeKind::Implements {
                ids.push(edge.source.clone());
            }
        }
    }
    ids
}

fn owning_symbol<'a>(
    node_id: &'a str,
    parents: &HashMap<&'a str, &'a str>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> Option<&'a Node> {
    let mut current = Some(node_id);
    while let Some(id) = current {
        let node = node_index.get(id).copied()?;
        if !matches!(node.kind, NodeKind::View | NodeKind::Branch)
            && !node.metadata.contains_key("l10n.ref_kind")
        {
            return Some(node);
        }
        current = parents.get(id).copied();
    }
    None
}
