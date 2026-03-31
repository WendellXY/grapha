use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};
use grapha_core::graph::{Edge, EdgeKind, Graph, Node};
use langcodec::Codec;
use langcodec::types::Translation;
use serde::{Deserialize, Serialize};

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
const LOCALIZATION_SNAPSHOT_VERSION: &str = "1";
const LOCALIZATION_SNAPSHOT_FILE: &str = "localization.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalizationCatalogRecord {
    pub table: String,
    pub key: String,
    pub catalog_file: String,
    pub catalog_dir: String,
    pub source_language: String,
    pub source_value: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LocalizationSnapshot {
    version: String,
    records: Vec<LocalizationCatalogRecord>,
}

impl LocalizationSnapshot {
    fn new(mut records: Vec<LocalizationCatalogRecord>) -> Self {
        sort_records(&mut records);
        Self {
            version: LOCALIZATION_SNAPSHOT_VERSION.to_string(),
            records,
        }
    }

    fn record_count(&self) -> usize {
        self.records.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizationSnapshotWarning {
    pub catalog_file: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizationSnapshotBuildStats {
    pub record_count: usize,
    pub warnings: Vec<LocalizationSnapshotWarning>,
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

    pub(crate) fn from_records(mut records: Vec<LocalizationCatalogRecord>) -> Self {
        sort_records(&mut records);
        let mut index = Self::default();
        for record in records {
            index.insert(record);
        }
        index
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

pub fn build_and_save_catalog_snapshot(
    root: &Path,
    store_dir: &Path,
) -> anyhow::Result<LocalizationSnapshotBuildStats> {
    let (snapshot, warnings) = build_catalog_snapshot(root)?;
    let count = snapshot.record_count();
    save_catalog_snapshot(store_dir, &snapshot)?;
    Ok(LocalizationSnapshotBuildStats {
        record_count: count,
        warnings,
    })
}

pub fn load_catalog_index(project_root: &Path) -> anyhow::Result<LocalizationCatalogIndex> {
    load_catalog_index_from_store(&project_root.join(".grapha"))
}

pub(crate) fn load_catalog_index_from_store(
    store_dir: &Path,
) -> anyhow::Result<LocalizationCatalogIndex> {
    let snapshot = load_catalog_snapshot(store_dir)?;
    Ok(LocalizationCatalogIndex::from_records(snapshot.records))
}

fn build_catalog_snapshot(
    root: &Path,
) -> anyhow::Result<(LocalizationSnapshot, Vec<LocalizationSnapshotWarning>)> {
    if root.is_file() {
        return Ok((LocalizationSnapshot::new(Vec::new()), Vec::new()));
    }

    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let files = crate::discover::discover_files(&root, &["xcstrings"])?;
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    for file in files {
        let mut codec = Codec::new();
        if let Err(error) = codec
            .read_file_by_extension(&file, None)
            .with_context(|| format!("failed to read xcstrings catalog {}", file.display()))
        {
            warnings.push(LocalizationSnapshotWarning {
                catalog_file: path_to_snapshot_string(&path_relative_to_root(&root, &file)),
                reason: error.to_string(),
            });
            continue;
        }

        let Some(source_resource) = source_resource_for_codec(&codec) else {
            continue;
        };
        let source_language = source_resource.metadata.language.clone();
        let table = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Localizable")
            .to_string();
        let catalog_file = path_relative_to_root(&root, &file);
        let catalog_dir = catalog_file
            .parent()
            .map(path_to_snapshot_string)
            .unwrap_or_else(|| ".".to_string());

        for entry in &source_resource.entries {
            records.push(LocalizationCatalogRecord {
                table: table.clone(),
                key: entry.id.clone(),
                catalog_file: path_to_snapshot_string(&catalog_file),
                catalog_dir: catalog_dir.clone(),
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

    Ok((LocalizationSnapshot::new(records), warnings))
}

fn save_catalog_snapshot(store_dir: &Path, snapshot: &LocalizationSnapshot) -> anyhow::Result<()> {
    std::fs::create_dir_all(store_dir)
        .with_context(|| format!("failed to create store dir {}", store_dir.display()))?;
    let path = catalog_snapshot_path(store_dir);
    let payload = serde_json::to_string_pretty(snapshot)?;
    std::fs::write(&path, payload)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn load_catalog_snapshot(store_dir: &Path) -> anyhow::Result<LocalizationSnapshot> {
    let path = catalog_snapshot_path(store_dir);
    if !path.exists() {
        bail!("no localization index found — run `grapha index` first");
    }

    let payload = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot: LocalizationSnapshot = serde_json::from_str(&payload)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if snapshot.version != LOCALIZATION_SNAPSHOT_VERSION {
        bail!(
            "unsupported localization snapshot version: {} (expected {})",
            snapshot.version,
            LOCALIZATION_SNAPSHOT_VERSION
        );
    }

    Ok(snapshot)
}

fn catalog_snapshot_path(store_dir: &Path) -> PathBuf {
    store_dir.join(LOCALIZATION_SNAPSHOT_FILE)
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

fn path_relative_to_root(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn path_to_snapshot_string(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value.is_empty() {
        ".".to_string()
    } else {
        value.to_string()
    }
}

fn sort_records(records: &mut [LocalizationCatalogRecord]) {
    records.sort_by(|left, right| {
        left.table
            .cmp(&right.table)
            .then_with(|| left.key.cmp(&right.key))
            .then_with(|| left.catalog_file.cmp(&right.catalog_file))
            .then_with(|| left.source_language.cmp(&right.source_language))
    });
}

fn closest_records(
    usage_file: &Path,
    candidates: Vec<LocalizationCatalogRecord>,
) -> Vec<LocalizationCatalogRecord> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut ranked: Vec<(usize, LocalizationCatalogRecord)> = candidates
        .into_iter()
        .map(|record| (directory_distance(usage_file, &record.catalog_dir), record))
        .collect();
    ranked.sort_by(
        |(left_distance, left_record), (right_distance, right_record)| {
            left_distance
                .cmp(right_distance)
                .then_with(|| left_record.catalog_file.cmp(&right_record.catalog_file))
                .then_with(|| left_record.table.cmp(&right_record.table))
                .then_with(|| left_record.key.cmp(&right_record.key))
        },
    );

    let Some(best_distance) = ranked.first().map(|(distance, _)| *distance) else {
        return Vec::new();
    };

    ranked
        .into_iter()
        .take_while(|(distance, _)| *distance == best_distance)
        .map(|(_, record)| record)
        .collect()
}

fn directory_distance(usage_file: &Path, catalog_dir: &str) -> usize {
    let usage_dir = usage_file.parent().unwrap_or_else(|| Path::new("."));
    let usage_components = normalized_components(usage_dir);
    let catalog_components = normalized_components(Path::new(catalog_dir));

    let common_prefix = usage_components
        .iter()
        .zip(&catalog_components)
        .take_while(|(left, right)| left == right)
        .count();

    (usage_components.len() - common_prefix) + (catalog_components.len() - common_prefix)
}

fn normalized_components(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|component| match component {
            Component::CurDir => None,
            Component::Normal(value) => Some(value.to_os_string()),
            Component::ParentDir => Some(OsString::from("..")),
            Component::RootDir => Some(OsString::from("/")),
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_os_string()),
        })
        .collect()
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
        for record in closest_records(&usage_node.file, catalogs.records_for(table, key)) {
            let dedupe_key = (
                String::new(),
                record.catalog_file.clone(),
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

            for record in closest_records(
                &usage_node.file,
                catalogs.records_for(&binding.table, &binding.key),
            ) {
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
                    record.catalog_file.clone(),
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
    fn builds_and_loads_localization_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join(".grapha");
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

        let stats = build_and_save_catalog_snapshot(dir.path(), &store_dir).unwrap();
        assert_eq!(stats.record_count, 1);
        assert!(stats.warnings.is_empty());
        assert!(store_dir.join("localization.json").exists());

        let index = load_catalog_index_from_store(&store_dir).unwrap();
        let records = index.records_for("Localizable", "welcome_title");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_value, "Welcome");
        assert_eq!(records[0].status, "translated");
        assert_eq!(records[0].catalog_file, "Localizable.xcstrings");
        assert_eq!(records[0].catalog_dir, ".");
        assert_eq!(
            records[0].comment.as_deref(),
            Some("Shown on the welcome screen")
        );
    }

    #[test]
    fn rejects_unsupported_snapshot_version() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join(".grapha");
        fs::create_dir_all(&store_dir).unwrap();
        fs::write(
            store_dir.join("localization.json"),
            r#"{"version":"999","records":[]}"#,
        )
        .unwrap();

        let error = load_catalog_index_from_store(&store_dir).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported localization snapshot version"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn skips_invalid_xcstrings_catalogs_and_keeps_valid_ones() {
        let dir = tempfile::tempdir().unwrap();
        let store_dir = dir.path().join(".grapha");
        fs::write(
            dir.path().join("Localizable.xcstrings"),
            r#"{
              "sourceLanguage" : "en",
              "strings" : {
                "welcome_title" : {
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
        fs::write(
            dir.path().join("Broken.xcstrings"),
            r#"{
              "sourceLanguage" : "en",
              "strings" : {
                "broken" : {},
              },
              "version" : "1.0"
            }"#,
        )
        .unwrap();

        let stats = build_and_save_catalog_snapshot(dir.path(), &store_dir).unwrap();
        assert_eq!(stats.record_count, 1);
        assert_eq!(stats.warnings.len(), 1);
        assert_eq!(stats.warnings[0].catalog_file, "Broken.xcstrings");
        assert!(
            stats.warnings[0]
                .reason
                .contains("failed to read xcstrings catalog"),
            "unexpected warning: {}",
            stats.warnings[0].reason
        );

        let index = load_catalog_index_from_store(&store_dir).unwrap();
        let records = index.records_for("Localizable", "welcome_title");
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn closest_records_prefers_nearest_catalog() {
        let candidates = vec![
            LocalizationCatalogRecord {
                table: "Localizable".to_string(),
                key: "shared_title".to_string(),
                catalog_file: "Features/Auth/Localizable.xcstrings".to_string(),
                catalog_dir: "Features/Auth".to_string(),
                source_language: "en".to_string(),
                source_value: "Auth".to_string(),
                status: "translated".to_string(),
                comment: None,
            },
            LocalizationCatalogRecord {
                table: "Localizable".to_string(),
                key: "shared_title".to_string(),
                catalog_file: "Features/Profile/Localizable.xcstrings".to_string(),
                catalog_dir: "Features/Profile".to_string(),
                source_language: "en".to_string(),
                source_value: "Profile".to_string(),
                status: "translated".to_string(),
                comment: None,
            },
        ];

        let matches = closest_records(
            Path::new("Features/Auth/Sources/Login/ContentView.swift"),
            candidates,
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].source_value, "Auth");
    }

    #[test]
    fn closest_records_keep_equal_distance_ties() {
        let candidates = vec![
            LocalizationCatalogRecord {
                table: "Localizable".to_string(),
                key: "shared_title".to_string(),
                catalog_file: "Features/A/Localizable.xcstrings".to_string(),
                catalog_dir: "Features/A".to_string(),
                source_language: "en".to_string(),
                source_value: "A".to_string(),
                status: "translated".to_string(),
                comment: None,
            },
            LocalizationCatalogRecord {
                table: "Localizable".to_string(),
                key: "shared_title".to_string(),
                catalog_file: "Features/B/Localizable.xcstrings".to_string(),
                catalog_dir: "Features/B".to_string(),
                source_language: "en".to_string(),
                source_value: "B".to_string(),
                status: "translated".to_string(),
                comment: None,
            },
        ];

        let matches = closest_records(Path::new("Features/Common/ContentView.swift"), candidates);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].catalog_file, "Features/A/Localizable.xcstrings");
        assert_eq!(matches[1].catalog_file, "Features/B/Localizable.xcstrings");
    }
}
