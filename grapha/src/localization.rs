use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use grapha_core::graph::{Edge, EdgeKind, Graph, Node};
use langcodec::Codec;
use langcodec::types::Translation;
use serde::Serialize;

const META_REF_KIND: &str = "l10n.ref_kind";
const META_WRAPPER_NAME: &str = "l10n.wrapper_name";
const META_WRAPPER_SYMBOL: &str = "l10n.wrapper_symbol";
const META_TABLE: &str = "l10n.table";
const META_KEY: &str = "l10n.key";
const META_FALLBACK: &str = "l10n.fallback";
const META_ARG_COUNT: &str = "l10n.arg_count";
const META_LITERAL: &str = "l10n.literal";
const META_WRAPPER_TABLE: &str = "l10n.wrapper.table";
const META_WRAPPER_KEY: &str = "l10n.wrapper.key";
const META_WRAPPER_FALLBACK: &str = "l10n.wrapper.fallback";
const META_WRAPPER_ARG_COUNT: &str = "l10n.wrapper.arg_count";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalizationCatalogRecord {
    pub table: String,
    pub key: String,
    pub file: String,
    pub source_language: String,
    pub source_value: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalizationReference {
    pub ref_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arg_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizationWrapperBinding {
    pub table: String,
    pub key: String,
    pub fallback: Option<String>,
    pub arg_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLocalizationMatch {
    pub reference: LocalizationReference,
    pub record: LocalizationCatalogRecord,
    pub match_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmatchedLocalizationReference {
    pub reference: LocalizationReference,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageResolution {
    pub matches: Vec<ResolvedLocalizationMatch>,
    pub unmatched: Option<UnmatchedLocalizationReference>,
}

#[derive(Debug, Default, Clone)]
pub struct LocalizationCatalogIndex {
    records: Vec<LocalizationCatalogRecord>,
    by_table_key: HashMap<(String, String), Vec<usize>>,
    by_key: HashMap<String, Vec<usize>>,
}

impl LocalizationCatalogIndex {
    pub(crate) fn insert(&mut self, record: LocalizationCatalogRecord) {
        let index = self.records.len();
        self.by_table_key
            .entry((record.table.clone(), record.key.clone()))
            .or_default()
            .push(index);
        self.by_key
            .entry(record.key.clone())
            .or_default()
            .push(index);
        self.records.push(record);
    }

    pub fn records_for(&self, table: &str, key: &str) -> Vec<LocalizationCatalogRecord> {
        self.by_table_key
            .get(&(table.to_string(), key.to_string()))
            .into_iter()
            .flatten()
            .filter_map(|index| self.records.get(*index))
            .cloned()
            .collect()
    }

    pub fn records_for_key(&self, key: &str) -> Vec<LocalizationCatalogRecord> {
        self.by_key
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|index| self.records.get(*index))
            .cloned()
            .collect()
    }
}

pub fn load_catalog_index(root: &Path) -> anyhow::Result<LocalizationCatalogIndex> {
    let files = crate::discover::discover_files(root, &["xcstrings"])?;
    let mut index = LocalizationCatalogIndex::default();

    for file in files {
        let mut codec = Codec::new();
        codec
            .read_file_by_extension(&file, None)
            .with_context(|| format!("failed to read xcstrings catalog {}", file.display()))?;

        let Some(source_resource) = source_resource_for_codec(&codec) else {
            continue;
        };
        let source_language = source_resource.metadata.language.clone();
        let table = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Localizable")
            .to_string();
        let file_string = file.to_string_lossy().to_string();

        for entry in &source_resource.entries {
            index.insert(LocalizationCatalogRecord {
                table: table.clone(),
                key: entry.id.clone(),
                file: file_string.clone(),
                source_language: source_language.clone(),
                source_value: translation_plain_string(&entry.value),
                status: serde_json::to_string(&entry.status)
                    .unwrap_or_else(|_| "\"unknown\"".to_string())
                    .trim_matches('"')
                    .to_string(),
                comment: entry.comment.clone(),
            });
        }
    }

    Ok(index)
}

fn source_resource_for_codec(codec: &Codec) -> Option<&langcodec::types::Resource> {
    let source_language = codec
        .resources
        .iter()
        .find_map(|resource| resource.metadata.custom.get("source_language").cloned())
        .unwrap_or_else(|| "en".to_string());

    codec
        .resources
        .iter()
        .find(|resource| resource.metadata.language == source_language)
        .or_else(|| {
            codec
                .resources
                .iter()
                .find(|resource| resource.has_language(&source_language))
        })
        .or_else(|| codec.resources.first())
}

fn translation_plain_string(value: &Translation) -> String {
    value.plain_translation_string()
}

pub fn localization_usage_nodes<'a>(graph: &'a Graph) -> Vec<&'a Node> {
    graph
        .nodes
        .iter()
        .filter(|node| node.metadata.contains_key(META_REF_KIND))
        .collect()
}

pub fn parse_usage_reference(node: &Node) -> Option<LocalizationReference> {
    let ref_kind = node.metadata.get(META_REF_KIND)?.clone();
    Some(LocalizationReference {
        ref_kind,
        wrapper_name: node.metadata.get(META_WRAPPER_NAME).cloned(),
        wrapper_symbol: node.metadata.get(META_WRAPPER_SYMBOL).cloned(),
        table: node.metadata.get(META_TABLE).cloned(),
        key: node.metadata.get(META_KEY).cloned(),
        fallback: node.metadata.get(META_FALLBACK).cloned(),
        arg_count: node
            .metadata
            .get(META_ARG_COUNT)
            .and_then(|value| value.parse::<usize>().ok()),
        literal: node.metadata.get(META_LITERAL).cloned(),
    })
}

pub fn parse_wrapper_binding(node: &Node) -> Option<LocalizationWrapperBinding> {
    Some(LocalizationWrapperBinding {
        table: node.metadata.get(META_WRAPPER_TABLE)?.clone(),
        key: node.metadata.get(META_WRAPPER_KEY)?.clone(),
        fallback: node.metadata.get(META_WRAPPER_FALLBACK).cloned(),
        arg_count: node
            .metadata
            .get(META_WRAPPER_ARG_COUNT)
            .and_then(|value| value.parse::<usize>().ok()),
    })
}

pub fn edges_by_source<'a>(graph: &'a Graph) -> HashMap<&'a str, Vec<&'a Edge>> {
    let mut map: HashMap<&str, Vec<&Edge>> = HashMap::new();
    for edge in &graph.edges {
        map.entry(edge.source.as_str()).or_default().push(edge);
    }
    map
}

pub fn node_index<'a>(graph: &'a Graph) -> HashMap<&'a str, &'a Node> {
    graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect()
}

pub fn resolve_usage(
    usage_node: &Node,
    edges_by_source: &HashMap<&str, Vec<&Edge>>,
    node_index: &HashMap<&str, &Node>,
    catalogs: &LocalizationCatalogIndex,
) -> Option<UsageResolution> {
    let base_reference = parse_usage_reference(usage_node)?;
    let mut matches = Vec::new();
    let mut seen = HashSet::new();

    if let (Some(table), Some(key)) = (
        base_reference.table.as_deref(),
        base_reference.key.as_deref(),
    ) {
        for record in catalogs.records_for(table, key) {
            let dedupe_key = (
                String::new(),
                record.file.clone(),
                record.table.clone(),
                record.key.clone(),
            );
            if seen.insert(dedupe_key) {
                matches.push(ResolvedLocalizationMatch {
                    reference: base_reference.clone(),
                    record,
                    match_kind: "direct_metadata".to_string(),
                });
            }
        }
    }

    if let Some(edges) = edges_by_source.get(usage_node.id.as_str()) {
        for edge in edges {
            if edge.kind != EdgeKind::TypeRef {
                continue;
            }
            let Some(wrapper_node) = node_index.get(edge.target.as_str()).copied() else {
                continue;
            };
            let Some(binding) = parse_wrapper_binding(wrapper_node) else {
                continue;
            };

            for record in catalogs.records_for(&binding.table, &binding.key) {
                let mut reference = base_reference.clone();
                reference.wrapper_symbol = Some(wrapper_node.id.clone());
                reference.table = Some(binding.table.clone());
                reference.key = Some(binding.key.clone());
                if reference.fallback.is_none() {
                    reference.fallback = binding.fallback.clone();
                }
                if reference.arg_count.is_none() {
                    reference.arg_count = binding.arg_count;
                }

                let dedupe_key = (
                    wrapper_node.id.clone(),
                    record.file.clone(),
                    record.table.clone(),
                    record.key.clone(),
                );
                if seen.insert(dedupe_key) {
                    matches.push(ResolvedLocalizationMatch {
                        reference,
                        record,
                        match_kind: "wrapper_symbol".to_string(),
                    });
                }
            }
        }
    }

    let unmatched = if matches.is_empty() {
        Some(UnmatchedLocalizationReference {
            reference: base_reference.clone(),
            reason: unmatched_reason(&base_reference, edges_by_source, usage_node, node_index),
        })
    } else {
        None
    };

    Some(UsageResolution { matches, unmatched })
}

fn unmatched_reason(
    reference: &LocalizationReference,
    edges_by_source: &HashMap<&str, Vec<&Edge>>,
    usage_node: &Node,
    node_index: &HashMap<&str, &Node>,
) -> String {
    if reference.ref_kind == "literal" {
        return "literal text has no stable catalog key".to_string();
    }

    if let Some(edges) = edges_by_source.get(usage_node.id.as_str()) {
        let has_wrapper_target = edges.iter().any(|edge| {
            edge.kind == EdgeKind::TypeRef
                && node_index
                    .get(edge.target.as_str())
                    .is_some_and(|node| parse_wrapper_binding(node).is_some())
        });
        if has_wrapper_target {
            return "catalog record not found".to_string();
        }
    }

    "no wrapper symbol resolved".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn loads_xcstrings_catalog_records() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Localizable.xcstrings");
        fs::write(
            &file,
            r#"{
              "sourceLanguage" : "en",
              "strings" : {
                "welcome_title" : {
                  "comment" : "Shown on the welcome screen",
                  "localizations" : {
                    "en" : {
                      "stringUnit" : {
                        "state" : "translated",
                        "value" : "Welcome"
                      }
                    }
                  }
                }
              },
              "version" : "1.0"
            }"#,
        )
        .unwrap();

        let index = load_catalog_index(dir.path()).unwrap();
        let records = index.records_for("Localizable", "welcome_title");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_value, "Welcome");
        assert_eq!(records[0].status, "translated");
        assert_eq!(
            records[0].comment.as_deref(),
            Some("Shown on the welcome screen")
        );
    }
}
