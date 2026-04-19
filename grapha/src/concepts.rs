use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use tantivy::Index;

use grapha_core::graph::{Edge, EdgeKind, Graph, Node, NodeKind};

use crate::assets::{self, AssetCatalogIndex, AssetRecord};
use crate::localization::{LocalizationCatalogIndex, LocalizationCatalogRecord};
use crate::query::{self, SymbolInfo};
use crate::search::{self, SearchOptions};
use crate::symbol_locator::SymbolLocatorIndex;

const CONCEPTS_SNAPSHOT_VERSION: &str = "1";
const CONCEPTS_SNAPSHOT_FILE: &str = "concepts.json";

const STATUS_CONFIRMED: &str = "confirmed";
const STATUS_CANDIDATE: &str = "candidate";

const SCORE_CONCEPT_STORE: f32 = 1000.0;
const SCORE_L10N_VALUE_EXACT: f32 = 920.0;
const SCORE_L10N_VALUE_CONTAINS: f32 = 880.0;
const SCORE_L10N_KEY_EXACT: f32 = 840.0;
const SCORE_L10N_KEY_CONTAINS: f32 = 800.0;
const SCORE_ASSET_EXACT: f32 = 760.0;
const SCORE_ASSET_CONTAINS: f32 = 720.0;
const SCORE_FALLBACK_CALLER_BONUS: f32 = 15.0;
const SCORE_FALLBACK_SEED_PENALTY: f32 = 25.0;
const SCORE_SYMBOL_EXACT: f32 = 660.0;
const SCORE_SYMBOL_PREFIX: f32 = 620.0;
const SCORE_SYMBOL_BM25: f32 = 560.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConceptRecord {
    pub concept: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<ConceptBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConceptBinding {
    pub symbol_id: String,
    #[serde(default = "default_binding_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ConceptEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConceptEvidence {
    pub kind: String,
    pub value: String,
    pub match_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ui_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConceptSearchResult {
    pub query: String,
    pub resolved_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_concept: Option<String>,
    pub scopes: Vec<ConceptScopeMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConceptScopeMatch {
    pub symbol: SymbolInfo,
    pub score: f32,
    pub status: String,
    pub evidence: Vec<ConceptEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConceptShowResult {
    pub query: String,
    pub concept: String,
    pub aliases: Vec<String>,
    pub bindings: Vec<ConceptBindingView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConceptBindingView {
    pub symbol_id: String,
    pub status: String,
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<SymbolInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<ConceptEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConceptBindResult {
    pub concept: String,
    pub added_bindings: usize,
    pub total_bindings: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConceptAliasResult {
    pub concept: String,
    pub added_aliases: Vec<String>,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConceptRemoveResult {
    pub concept: String,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConceptPruneResult {
    pub pruned_bindings: usize,
    pub touched_concepts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConceptLookup {
    pub concept: String,
    pub matched_term: String,
    pub match_kind: String,
}

#[derive(Debug, Default, Clone)]
pub struct ConceptIndex {
    records: Vec<ConceptRecord>,
    lookup: HashMap<String, ConceptLookupEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConceptLookupEntry {
    record_index: usize,
    matched_term: String,
    match_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ConceptSnapshot {
    version: String,
    #[serde(default)]
    concepts: Vec<ConceptRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextMatch {
    Exact,
    Contains,
}

#[derive(Debug)]
struct ScopeAccumulator {
    symbol: SymbolInfo,
    score: f32,
    status: String,
    evidence: Vec<ConceptEvidence>,
    evidence_set: HashSet<ConceptEvidence>,
}

struct ScopeSearchContext<'a> {
    graph: &'a Graph,
    node_index: &'a HashMap<&'a str, &'a Node>,
    parents: &'a HashMap<&'a str, &'a str>,
    edges_by_target: &'a HashMap<&'a str, Vec<&'a Edge>>,
    locators: &'a SymbolLocatorIndex,
    search_index: &'a Index,
}

fn default_binding_status() -> String {
    STATUS_CONFIRMED.to_string()
}

impl ConceptSnapshot {
    fn new(mut concepts: Vec<ConceptRecord>) -> Self {
        sort_concepts(&mut concepts);
        Self {
            version: CONCEPTS_SNAPSHOT_VERSION.to_string(),
            concepts,
        }
    }
}

impl ConceptIndex {
    pub fn from_records(records: Vec<ConceptRecord>) -> Self {
        let mut index = Self {
            records,
            lookup: HashMap::new(),
        };
        index.sort_and_rebuild();
        index
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn record_for_term(&self, term: &str) -> Option<(&ConceptRecord, ConceptLookup)> {
        let normalized = normalize_concept(term);
        let entry = self.lookup.get(&normalized)?;
        let record = self.records.get(entry.record_index)?;
        Some((
            record,
            ConceptLookup {
                concept: record.concept.clone(),
                matched_term: entry.matched_term.clone(),
                match_kind: entry.match_kind.clone(),
            },
        ))
    }

    pub fn bind_concept(
        &mut self,
        term: &str,
        symbol_ids: &[String],
        evidence: Vec<ConceptEvidence>,
    ) -> anyhow::Result<ConceptBindResult> {
        let record_index = self.ensure_record(term)?;
        let canonical = self
            .records
            .get(record_index)
            .map(|record| record.concept.clone())
            .unwrap_or_else(|| term.trim().to_string());
        let record = self
            .records
            .get_mut(record_index)
            .expect("record index should exist");
        let mut added_bindings = 0;

        for symbol_id in symbol_ids {
            match record
                .bindings
                .iter_mut()
                .find(|binding| binding.symbol_id == *symbol_id)
            {
                Some(binding) => {
                    merge_evidence(&mut binding.evidence, &evidence);
                    if binding.status.is_empty() {
                        binding.status = STATUS_CONFIRMED.to_string();
                    }
                }
                None => {
                    record.bindings.push(ConceptBinding {
                        symbol_id: symbol_id.clone(),
                        status: STATUS_CONFIRMED.to_string(),
                        evidence: evidence.clone(),
                    });
                    added_bindings += 1;
                }
            }
        }

        self.sort_and_rebuild();
        let total_bindings = self
            .record_for_term(&canonical)
            .map(|(record, _)| record.bindings.len())
            .unwrap_or_default();

        Ok(ConceptBindResult {
            concept: canonical,
            added_bindings,
            total_bindings,
        })
    }

    pub fn add_aliases(
        &mut self,
        term: &str,
        aliases: &[String],
    ) -> anyhow::Result<ConceptAliasResult> {
        let record_index = self.ensure_record(term)?;
        let canonical = self
            .records
            .get(record_index)
            .map(|record| record.concept.clone())
            .unwrap_or_else(|| term.trim().to_string());

        let mut added = Vec::new();
        for alias in aliases {
            let normalized_alias = normalize_concept(alias);
            if normalized_alias.is_empty() {
                continue;
            }

            if let Some(entry) = self.lookup.get(&normalized_alias)
                && entry.record_index != record_index
            {
                bail!(
                    "alias '{}' already belongs to concept '{}'",
                    alias,
                    self.records[entry.record_index].concept
                );
            }

            let record = self
                .records
                .get_mut(record_index)
                .expect("record index should exist");
            if normalize_concept(&record.concept) == normalized_alias
                || record
                    .aliases
                    .iter()
                    .any(|existing| normalize_concept(existing) == normalized_alias)
            {
                continue;
            }

            record.aliases.push(alias.trim().to_string());
            added.push(alias.trim().to_string());
        }

        self.sort_and_rebuild();

        let aliases = self
            .record_for_term(&canonical)
            .map(|(record, _)| record.aliases.clone())
            .unwrap_or_default();

        Ok(ConceptAliasResult {
            concept: canonical,
            added_aliases: added,
            aliases,
        })
    }

    pub fn remove_concept(&mut self, term: &str) -> ConceptRemoveResult {
        let Some((_, lookup)) = self.record_for_term(term) else {
            return ConceptRemoveResult {
                concept: term.trim().to_string(),
                removed: false,
            };
        };
        let normalized = normalize_concept(&lookup.concept);
        let removed =
            if let Some(index) = self.lookup.get(&normalized).map(|entry| entry.record_index) {
                self.records.remove(index);
                true
            } else {
                false
            };
        self.sort_and_rebuild();
        ConceptRemoveResult {
            concept: lookup.concept,
            removed,
        }
    }

    pub fn prune(&mut self, valid_ids: &HashSet<&str>) -> ConceptPruneResult {
        let mut pruned_bindings = 0;
        let mut touched_concepts = 0;

        for record in &mut self.records {
            let before = record.bindings.len();
            record
                .bindings
                .retain(|binding| valid_ids.contains(binding.symbol_id.as_str()));
            let removed = before.saturating_sub(record.bindings.len());
            if removed > 0 {
                pruned_bindings += removed;
                touched_concepts += 1;
            }
        }

        self.sort_and_rebuild();

        ConceptPruneResult {
            pruned_bindings,
            touched_concepts,
        }
    }

    fn ensure_record(&mut self, term: &str) -> anyhow::Result<usize> {
        let normalized = normalize_concept(term);
        if normalized.is_empty() {
            bail!("concept term cannot be empty");
        }

        if let Some(entry) = self.lookup.get(&normalized) {
            return Ok(entry.record_index);
        }

        self.records.push(ConceptRecord {
            concept: term.trim().to_string(),
            aliases: Vec::new(),
            bindings: Vec::new(),
            notes: None,
        });
        self.sort_and_rebuild();
        self.lookup
            .get(&normalized)
            .map(|entry| entry.record_index)
            .context("new concept should be indexed")
    }

    fn sort_and_rebuild(&mut self) {
        sort_concepts(&mut self.records);
        self.lookup.clear();
        for (index, record) in self.records.iter().enumerate() {
            let normalized_concept = normalize_concept(&record.concept);
            if !normalized_concept.is_empty() {
                self.lookup.insert(
                    normalized_concept,
                    ConceptLookupEntry {
                        record_index: index,
                        matched_term: record.concept.clone(),
                        match_kind: "concept".to_string(),
                    },
                );
            }
            for alias in &record.aliases {
                let normalized_alias = normalize_concept(alias);
                if normalized_alias.is_empty() {
                    continue;
                }
                self.lookup.insert(
                    normalized_alias,
                    ConceptLookupEntry {
                        record_index: index,
                        matched_term: alias.clone(),
                        match_kind: "alias".to_string(),
                    },
                );
            }
        }
    }
}

pub fn load_concept_index(project_root: &Path) -> anyhow::Result<ConceptIndex> {
    load_concept_index_from_store(&project_root.join(".grapha"))
}

pub(crate) fn load_concept_index_from_store(store_dir: &Path) -> anyhow::Result<ConceptIndex> {
    let path = snapshot_path(store_dir);
    if !path.exists() {
        return Ok(ConceptIndex::default());
    }

    let payload = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let snapshot: ConceptSnapshot = serde_json::from_str(&payload)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if snapshot.version != CONCEPTS_SNAPSHOT_VERSION {
        bail!(
            "unsupported concept snapshot version: {} (expected {})",
            snapshot.version,
            CONCEPTS_SNAPSHOT_VERSION
        );
    }
    Ok(ConceptIndex::from_records(snapshot.concepts))
}

pub fn save_concept_index(project_root: &Path, index: &ConceptIndex) -> anyhow::Result<()> {
    save_concept_index_to_store(&project_root.join(".grapha"), index)
}

pub(crate) fn save_concept_index_to_store(
    store_dir: &Path,
    index: &ConceptIndex,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(store_dir)
        .with_context(|| format!("failed to create store dir {}", store_dir.display()))?;
    let path = snapshot_path(store_dir);
    let snapshot = ConceptSnapshot::new(index.records.clone());
    let payload = serde_json::to_string_pretty(&snapshot)?;
    std::fs::write(&path, payload)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn show_concept(
    graph: &Graph,
    concepts: &ConceptIndex,
    term: &str,
) -> anyhow::Result<ConceptShowResult> {
    let Some((record, _)) = concepts.record_for_term(term) else {
        bail!("concept not found: {}", term.trim());
    };
    let node_index = graph_node_index(graph);
    let locators = SymbolLocatorIndex::new(graph);

    let mut bindings = Vec::new();
    for binding in &record.bindings {
        let symbol = node_index
            .get(binding.symbol_id.as_str())
            .copied()
            .map(|node| symbol_info(node, &locators));
        bindings.push(ConceptBindingView {
            symbol_id: binding.symbol_id.clone(),
            status: binding.status.clone(),
            stale: symbol.is_none(),
            symbol,
            evidence: binding.evidence.clone(),
        });
    }

    bindings.sort_by(|left, right| {
        left.stale
            .cmp(&right.stale)
            .then_with(|| left.status.cmp(&right.status))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });

    Ok(ConceptShowResult {
        query: term.trim().to_string(),
        concept: record.concept.clone(),
        aliases: record.aliases.clone(),
        bindings,
        notes: record.notes.clone(),
    })
}

pub fn search_concepts(
    graph: &Graph,
    search_index: &Index,
    concepts: &ConceptIndex,
    catalogs: &LocalizationCatalogIndex,
    assets_index: &AssetCatalogIndex,
    query: &str,
    limit: usize,
) -> anyhow::Result<ConceptSearchResult> {
    let locators = SymbolLocatorIndex::new(graph);
    let node_index = graph_node_index(graph);
    let parents = contains_parents(graph);
    let edges_by_target = graph_edges_by_target(graph);
    let scope_context = ScopeSearchContext {
        graph,
        node_index: &node_index,
        parents: &parents,
        edges_by_target: &edges_by_target,
        locators: &locators,
        search_index,
    };

    if let Some((record, lookup)) = concepts.record_for_term(query) {
        let scopes = direct_concept_scopes(record, &lookup, &node_index, &locators, limit);
        if !scopes.is_empty() {
            return Ok(ConceptSearchResult {
                query: query.trim().to_string(),
                resolved_from: "concept_store".to_string(),
                matched_concept: Some(record.concept.clone()),
                scopes,
            });
        }
    }

    let mut scopes = HashMap::<String, ScopeAccumulator>::new();
    let normalized_query = normalize_match_text(query);

    add_localization_value_scopes(
        &mut scopes,
        &scope_context,
        catalogs,
        &normalized_query,
        query,
        TextMatch::Exact,
    );
    add_localization_value_scopes(
        &mut scopes,
        &scope_context,
        catalogs,
        &normalized_query,
        query,
        TextMatch::Contains,
    );
    add_localization_key_scopes(
        &mut scopes,
        &scope_context,
        catalogs,
        &normalized_query,
        query,
        TextMatch::Exact,
    );
    add_localization_key_scopes(
        &mut scopes,
        &scope_context,
        catalogs,
        &normalized_query,
        query,
        TextMatch::Contains,
    );
    add_asset_scopes(
        &mut scopes,
        &scope_context,
        assets_index,
        &normalized_query,
        TextMatch::Exact,
    );
    add_asset_scopes(
        &mut scopes,
        &scope_context,
        assets_index,
        &normalized_query,
        TextMatch::Contains,
    );
    add_symbol_scopes(&mut scopes, &scope_context, query, limit)?;

    let mut matches: Vec<_> = scopes
        .into_values()
        .map(|scope| ConceptScopeMatch {
            symbol: scope.symbol,
            score: scope.score + ((scope.evidence.len().saturating_sub(1)) as f32 * 5.0),
            status: scope.status,
            evidence: scope.evidence,
        })
        .collect();
    matches.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.symbol.name.cmp(&right.symbol.name))
            .then_with(|| left.symbol.file.cmp(&right.symbol.file))
    });
    matches.truncate(limit);

    Ok(ConceptSearchResult {
        query: query.trim().to_string(),
        resolved_from: "heuristics".to_string(),
        matched_concept: None,
        scopes: matches,
    })
}

fn direct_concept_scopes(
    record: &ConceptRecord,
    lookup: &ConceptLookup,
    node_index: &HashMap<&str, &Node>,
    locators: &SymbolLocatorIndex,
    limit: usize,
) -> Vec<ConceptScopeMatch> {
    let mut scopes = Vec::new();
    for binding in &record.bindings {
        let Some(node) = node_index.get(binding.symbol_id.as_str()).copied() else {
            continue;
        };
        let mut evidence = vec![ConceptEvidence {
            kind: "concept_binding".to_string(),
            value: lookup.matched_term.clone(),
            match_kind: lookup.match_kind.clone(),
            table: None,
            key: None,
            source_value: None,
            ui_path: Vec::new(),
            note: Some(record.concept.clone()),
        }];
        merge_evidence(&mut evidence, &binding.evidence);
        scopes.push(ConceptScopeMatch {
            symbol: symbol_info(node, locators),
            score: SCORE_CONCEPT_STORE,
            status: if binding.status.is_empty() {
                STATUS_CONFIRMED.to_string()
            } else {
                binding.status.clone()
            },
            evidence,
        });
    }
    scopes.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.symbol.name.cmp(&right.symbol.name))
    });
    scopes.truncate(limit);
    scopes
}

fn add_localization_value_scopes(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    catalogs: &LocalizationCatalogIndex,
    normalized_query: &str,
    raw_query: &str,
    match_type: TextMatch,
) {
    let mut seen_records = HashSet::new();
    for record in catalogs.all_records() {
        if !matches_localization_value(record, normalized_query, match_type) {
            continue;
        }
        let record_key = (
            record.table.clone(),
            record.key.clone(),
            record.catalog_file.clone(),
        );
        if !seen_records.insert(record_key) {
            continue;
        }

        add_record_usage_scopes(
            scopes,
            context,
            catalogs,
            record,
            if match_type == TextMatch::Exact {
                SCORE_L10N_VALUE_EXACT
            } else {
                SCORE_L10N_VALUE_CONTAINS
            },
            ConceptEvidence {
                kind: "l10n_value".to_string(),
                value: raw_query.trim().to_string(),
                match_kind: match_kind_label(match_type).to_string(),
                table: Some(record.table.clone()),
                key: Some(record.key.clone()),
                source_value: Some(record.source_value.clone()),
                ui_path: Vec::new(),
                note: None,
            },
        );
    }
}

fn add_localization_key_scopes(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    catalogs: &LocalizationCatalogIndex,
    normalized_query: &str,
    raw_query: &str,
    match_type: TextMatch,
) {
    let mut seen_records = HashSet::new();
    for record in catalogs.all_records() {
        if !matches_localization_key(record, normalized_query, match_type) {
            continue;
        }
        let record_key = (
            record.table.clone(),
            record.key.clone(),
            record.catalog_file.clone(),
        );
        if !seen_records.insert(record_key) {
            continue;
        }

        add_record_usage_scopes(
            scopes,
            context,
            catalogs,
            record,
            if match_type == TextMatch::Exact {
                SCORE_L10N_KEY_EXACT
            } else {
                SCORE_L10N_KEY_CONTAINS
            },
            ConceptEvidence {
                kind: "l10n_key".to_string(),
                value: raw_query.trim().to_string(),
                match_kind: match_kind_label(match_type).to_string(),
                table: Some(record.table.clone()),
                key: Some(record.key.clone()),
                source_value: Some(record.source_value.clone()),
                ui_path: Vec::new(),
                note: None,
            },
        );
    }
}

fn add_record_usage_scopes(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    catalogs: &LocalizationCatalogIndex,
    record: &LocalizationCatalogRecord,
    score: f32,
    base_evidence: ConceptEvidence,
) {
    let result =
        query::usages::query_usages(context.graph, catalogs, &record.key, Some(&record.table));
    let mut usage_count = 0;
    for record_group in result.records {
        if record_group.record.table != record.table || record_group.record.key != record.key {
            continue;
        }
        for usage in record_group.usages {
            usage_count += 1;
            let Some(scope_node) = context.node_index.get(usage.owner.id.as_str()).copied() else {
                continue;
            };
            let mut evidence = base_evidence.clone();
            evidence.ui_path = usage.ui_path.clone();
            add_scope(
                scopes,
                scope_node,
                context.locators,
                score,
                STATUS_CANDIDATE,
                evidence,
            );
        }
    }

    if usage_count == 0 {
        add_l10n_fallback_scopes(scopes, context, record, score, &base_evidence);
    }
}

fn add_asset_scopes(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    assets_index: &AssetCatalogIndex,
    normalized_query: &str,
    match_type: TextMatch,
) {
    let mut seen_records = HashSet::new();
    for record in assets_index.all_records() {
        if !matches_asset_name(record, normalized_query, match_type) {
            continue;
        }
        let record_key = (
            record.catalog.clone(),
            record.catalog_dir.clone(),
            record.name.clone(),
        );
        if !seen_records.insert(record_key) {
            continue;
        }

        let mut usage_count = 0;
        for usage in assets::find_usages(context.graph, &record.name) {
            usage_count += 1;
            let Some(node) = context.node_index.get(usage.node_id.as_str()).copied() else {
                continue;
            };
            let scope = scope_for_node(node, context.parents, context.node_index);
            add_scope(
                scopes,
                scope,
                context.locators,
                if match_type == TextMatch::Exact {
                    SCORE_ASSET_EXACT
                } else {
                    SCORE_ASSET_CONTAINS
                },
                STATUS_CANDIDATE,
                ConceptEvidence {
                    kind: "asset_name".to_string(),
                    value: record.name.clone(),
                    match_kind: match_kind_label(match_type).to_string(),
                    table: None,
                    key: None,
                    source_value: None,
                    ui_path: Vec::new(),
                    note: Some(record.catalog.clone()),
                },
            );
        }

        if usage_count == 0 {
            add_asset_fallback_scopes(
                scopes,
                context,
                record,
                if match_type == TextMatch::Exact {
                    SCORE_ASSET_EXACT
                } else {
                    SCORE_ASSET_CONTAINS
                },
            );
        }
    }
}

fn add_symbol_scopes(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    query: &str,
    limit: usize,
) -> anyhow::Result<()> {
    let results = search::search_filtered(
        context.search_index,
        query,
        limit.saturating_mul(4).max(8),
        &SearchOptions::default(),
    )?;
    let normalized_query = normalize_match_text(query);

    for (rank, result) in results.iter().enumerate() {
        let Some(node) = context.node_index.get(result.id.as_str()).copied() else {
            continue;
        };
        let scope = scope_for_node(node, context.parents, context.node_index);
        let normalized_name = normalize_match_text(query::normalize_symbol_name(&node.name));
        let (match_kind, base_score) = if normalized_name == normalized_query {
            ("exact", SCORE_SYMBOL_EXACT)
        } else if normalized_name.starts_with(&normalized_query) {
            ("prefix", SCORE_SYMBOL_PREFIX)
        } else {
            ("bm25", SCORE_SYMBOL_BM25)
        };
        add_scope(
            scopes,
            scope,
            context.locators,
            (base_score - rank as f32).max(0.0),
            STATUS_CANDIDATE,
            ConceptEvidence {
                kind: "symbol_query".to_string(),
                value: query.trim().to_string(),
                match_kind: match_kind.to_string(),
                table: None,
                key: None,
                source_value: None,
                ui_path: Vec::new(),
                note: Some(node.name.clone()),
            },
        );
    }
    Ok(())
}

fn add_l10n_fallback_scopes(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    record: &LocalizationCatalogRecord,
    score: f32,
    base_evidence: &ConceptEvidence,
) {
    let queries = l10n_symbol_queries(record);
    add_seed_symbol_scopes(
        scopes,
        context,
        &queries,
        score,
        |candidate, node_name, is_caller| ConceptEvidence {
            kind: "l10n_wrapper".to_string(),
            value: candidate.to_string(),
            match_kind: if is_caller {
                "wrapper_caller".to_string()
            } else {
                "wrapper_symbol".to_string()
            },
            table: base_evidence.table.clone(),
            key: base_evidence.key.clone(),
            source_value: base_evidence.source_value.clone(),
            ui_path: Vec::new(),
            note: Some(node_name.to_string()),
        },
    );
}

fn add_asset_fallback_scopes(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    record: &AssetRecord,
    score: f32,
) {
    let queries = asset_symbol_queries(record);
    add_seed_symbol_scopes(
        scopes,
        context,
        &queries,
        score,
        |candidate, node_name, is_caller| ConceptEvidence {
            kind: "asset_wrapper".to_string(),
            value: candidate.to_string(),
            match_kind: if is_caller {
                "wrapper_caller".to_string()
            } else {
                "wrapper_symbol".to_string()
            },
            table: None,
            key: None,
            source_value: None,
            ui_path: Vec::new(),
            note: Some(node_name.to_string()),
        },
    );
}

fn add_seed_symbol_scopes<F>(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    context: &ScopeSearchContext<'_>,
    queries: &[String],
    score: f32,
    evidence_builder: F,
) where
    F: Fn(&str, &str, bool) -> ConceptEvidence,
{
    let mut seen_seed_ids = HashSet::new();
    for query in queries {
        let normalized_query = normalize_match_text(query);
        if normalized_query.is_empty() {
            continue;
        }

        let Ok(results) =
            search::search_filtered(context.search_index, query, 8, &SearchOptions::default())
        else {
            continue;
        };
        let matching_seeds: Vec<&Node> = results
            .into_iter()
            .filter_map(|result| context.node_index.get(result.id.as_str()).copied())
            .filter(|seed| seed_matches_query(seed, query, &normalized_query))
            .collect();
        let preferred_non_accessor_bases: HashSet<String> = matching_seeds
            .iter()
            .copied()
            .filter(|seed| !is_accessor_symbol(seed))
            .map(|seed| normalize_match_text(query::normalize_symbol_name(&seed.name)))
            .collect();

        for seed in matching_seeds {
            if is_accessor_symbol(seed)
                && preferred_non_accessor_bases.contains(&normalize_match_text(
                    query::normalize_symbol_name(&seed.name),
                ))
            {
                continue;
            }
            if !seen_seed_ids.insert(seed.id.clone()) {
                continue;
            }

            let seed_scope = seed_scope_for_node(seed, context.parents, context.node_index);
            add_scope(
                scopes,
                seed_scope,
                context.locators,
                (score - SCORE_FALLBACK_SEED_PENALTY).max(0.0),
                STATUS_CANDIDATE,
                evidence_builder(query, &seed.name, false),
            );

            for caller in related_caller_nodes(
                seed.id.as_str(),
                context.edges_by_target,
                context.node_index,
            ) {
                let caller_scope = scope_for_node(caller, context.parents, context.node_index);
                if should_skip_generated_container_scope(seed, caller_scope) {
                    continue;
                }
                add_scope(
                    scopes,
                    caller_scope,
                    context.locators,
                    score + SCORE_FALLBACK_CALLER_BONUS,
                    STATUS_CANDIDATE,
                    evidence_builder(query, &caller.name, true),
                );
            }
        }
    }
}

fn add_scope(
    scopes: &mut HashMap<String, ScopeAccumulator>,
    node: &Node,
    locators: &SymbolLocatorIndex,
    score: f32,
    status: &str,
    evidence: ConceptEvidence,
) {
    let symbol = symbol_info(node, locators);
    match scopes.get_mut(symbol.id.as_str()) {
        Some(existing) => {
            if score > existing.score {
                existing.score = score;
            }
            if existing.status != STATUS_CONFIRMED && status == STATUS_CONFIRMED {
                existing.status = status.to_string();
            }
            if existing.evidence_set.insert(evidence.clone()) {
                existing.evidence.push(evidence);
            }
        }
        None => {
            let mut evidence_set = HashSet::new();
            evidence_set.insert(evidence.clone());
            scopes.insert(
                symbol.id.clone(),
                ScopeAccumulator {
                    symbol,
                    score,
                    status: status.to_string(),
                    evidence: vec![evidence],
                    evidence_set,
                },
            );
        }
    }
}

fn scope_for_node<'a>(
    node: &'a Node,
    parents: &HashMap<&'a str, &'a str>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> &'a Node {
    match node.kind {
        NodeKind::Branch => {
            first_non_branch_ancestor(node.id.as_str(), parents, node_index).unwrap_or(node)
        }
        NodeKind::Property | NodeKind::Field | NodeKind::Variant => {
            first_scope_ancestor(node.id.as_str(), parents, node_index).unwrap_or(node)
        }
        NodeKind::Function => {
            let Some(parent) = first_non_branch_ancestor(node.id.as_str(), parents, node_index)
            else {
                return node;
            };
            if matches!(
                parent.kind,
                NodeKind::Class
                    | NodeKind::Struct
                    | NodeKind::Enum
                    | NodeKind::Trait
                    | NodeKind::Protocol
                    | NodeKind::Impl
                    | NodeKind::Extension
                    | NodeKind::View
            ) {
                parent
            } else {
                node
            }
        }
        _ => node,
    }
}

fn seed_scope_for_node<'a>(
    node: &'a Node,
    parents: &HashMap<&'a str, &'a str>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> &'a Node {
    let file_name = node
        .file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if file_name.ends_with("Strings.generated.swift")
        || file_name.ends_with("Assets.generated.swift")
    {
        return node;
    }
    scope_for_node(node, parents, node_index)
}

fn first_non_branch_ancestor<'a>(
    node_id: &'a str,
    parents: &HashMap<&'a str, &'a str>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> Option<&'a Node> {
    let mut current = parents.get(node_id).copied();
    while let Some(id) = current {
        let node = node_index.get(id).copied()?;
        if node.kind != NodeKind::Branch {
            return Some(node);
        }
        current = parents.get(id).copied();
    }
    None
}

fn first_scope_ancestor<'a>(
    node_id: &'a str,
    parents: &HashMap<&'a str, &'a str>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> Option<&'a Node> {
    let mut current = parents.get(node_id).copied();
    while let Some(id) = current {
        let node = node_index.get(id).copied()?;
        if matches!(
            node.kind,
            NodeKind::Class
                | NodeKind::Struct
                | NodeKind::Enum
                | NodeKind::Trait
                | NodeKind::Protocol
                | NodeKind::Impl
                | NodeKind::Extension
                | NodeKind::View
                | NodeKind::Function
        ) {
            return Some(node);
        }
        if node.kind != NodeKind::Branch && node.kind != NodeKind::Module {
            return Some(node);
        }
        current = parents.get(id).copied();
    }
    None
}

fn symbol_info(node: &Node, locators: &SymbolLocatorIndex) -> SymbolInfo {
    let locator = locators.locator_for_id(&node.id);
    let info = SymbolInfo::from_node(node);
    match locator {
        Some(locator) => info.with_locator(locator.to_string()),
        None => info,
    }
}

fn contains_parents(graph: &Graph) -> HashMap<&str, &str> {
    let mut map = HashMap::new();
    for edge in &graph.edges {
        if edge.kind == EdgeKind::Contains {
            map.insert(edge.target.as_str(), edge.source.as_str());
        }
    }
    map
}

fn graph_edges_by_target(graph: &Graph) -> HashMap<&str, Vec<&Edge>> {
    let mut map: HashMap<&str, Vec<&Edge>> = HashMap::new();
    for edge in &graph.edges {
        map.entry(edge.target.as_str()).or_default().push(edge);
    }
    map
}

fn graph_node_index(graph: &Graph) -> HashMap<&str, &Node> {
    graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect()
}

fn matches_localization_value(
    record: &LocalizationCatalogRecord,
    normalized_query: &str,
    match_type: TextMatch,
) -> bool {
    if normalized_query.is_empty() {
        return false;
    }
    localization_values(record).into_iter().any(|value| {
        let normalized_value = normalize_match_text(value);
        match match_type {
            TextMatch::Exact => normalized_value == normalized_query,
            TextMatch::Contains => {
                normalized_value.contains(normalized_query) && normalized_value != normalized_query
            }
        }
    })
}

fn matches_localization_key(
    record: &LocalizationCatalogRecord,
    normalized_query: &str,
    match_type: TextMatch,
) -> bool {
    matches_text(record.key.as_str(), normalized_query, match_type)
}

fn matches_asset_name(record: &AssetRecord, normalized_query: &str, match_type: TextMatch) -> bool {
    matches_text(record.name.as_str(), normalized_query, match_type)
}

fn matches_text(value: &str, normalized_query: &str, match_type: TextMatch) -> bool {
    if normalized_query.is_empty() {
        return false;
    }
    let normalized_value = normalize_match_text(value);
    match match_type {
        TextMatch::Exact => normalized_value == normalized_query,
        TextMatch::Contains => {
            normalized_value.contains(normalized_query) && normalized_value != normalized_query
        }
    }
}

fn localization_values(record: &LocalizationCatalogRecord) -> Vec<&str> {
    let mut values = Vec::new();
    if !record.source_value.is_empty() {
        values.push(record.source_value.as_str());
    }
    values.extend(
        record
            .translations
            .values()
            .filter(|value| !value.is_empty())
            .map(String::as_str),
    );
    values
}

fn seed_matches_query(node: &Node, raw_query: &str, normalized_query: &str) -> bool {
    let normalized_name = normalize_match_text(query::normalize_symbol_name(&node.name));
    if normalized_name == normalized_query || normalized_name.contains(normalized_query) {
        return true;
    }

    let snippet = node.snippet.as_deref().unwrap_or_default();
    snippet.contains(raw_query)
}

fn related_caller_nodes<'a>(
    seed_id: &'a str,
    edges_by_target: &HashMap<&'a str, Vec<&'a Edge>>,
    node_index: &HashMap<&'a str, &'a Node>,
) -> Vec<&'a Node> {
    let mut related = Vec::new();
    let mut related_ids = HashSet::<String>::new();
    let mut frontier = vec![seed_id];
    let mut visited = HashSet::<String>::new();

    while let Some(current_target) = frontier.pop() {
        if !visited.insert(current_target.to_string()) {
            continue;
        }
        let Some(edges) = edges_by_target.get(current_target) else {
            continue;
        };
        for edge in edges {
            match edge.kind {
                EdgeKind::Implements => frontier.push(edge.source.as_str()),
                EdgeKind::Calls | EdgeKind::Uses | EdgeKind::Reads | EdgeKind::TypeRef => {
                    let Some(node) = node_index.get(edge.source.as_str()).copied() else {
                        continue;
                    };
                    if related_ids.insert(node.id.clone()) {
                        related.push(node);
                    }
                }
                _ => {}
            }
        }
    }

    related
}

fn is_accessor_symbol(node: &Node) -> bool {
    node.kind == NodeKind::Function
        && (node.name.starts_with("getter:") || node.name.starts_with("setter:"))
}

fn should_skip_generated_container_scope(seed: &Node, scope: &Node) -> bool {
    if seed.file != scope.file {
        return false;
    }
    let file_name = seed
        .file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !(file_name.ends_with("Strings.generated.swift")
        || file_name.ends_with("Assets.generated.swift"))
    {
        return false;
    }

    matches!(
        scope.kind,
        NodeKind::Enum
            | NodeKind::Struct
            | NodeKind::Class
            | NodeKind::Extension
            | NodeKind::Module
    )
}

fn l10n_symbol_queries(record: &LocalizationCatalogRecord) -> Vec<String> {
    dedup_preserve_order(vec![
        record.key.clone(),
        snake_or_path_to_camel(&record.key),
        snake_or_path_to_pascal(&record.key),
    ])
}

fn asset_symbol_queries(record: &AssetRecord) -> Vec<String> {
    dedup_preserve_order(vec![
        record.name.clone(),
        snake_or_path_to_camel(&record.name),
        snake_or_path_to_pascal(&record.name),
        record
            .name
            .rsplit('/')
            .next()
            .map(snake_or_path_to_camel)
            .unwrap_or_default(),
    ])
}

fn dedup_preserve_order(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for value in values {
        let normalized = normalize_match_text(&value);
        if normalized.is_empty() || !seen.insert(normalized) {
            continue;
        }
        deduped.push(value);
    }
    deduped
}

fn snake_or_path_to_camel(value: &str) -> String {
    let mut output = String::new();
    let mut upper_next = false;
    for ch in value.chars() {
        if !ch.is_alphanumeric() {
            upper_next = true;
            continue;
        }
        if output.is_empty() {
            output.extend(ch.to_lowercase());
            upper_next = false;
            continue;
        }
        if upper_next {
            output.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            output.extend(ch.to_lowercase());
        }
    }
    output
}

fn snake_or_path_to_pascal(value: &str) -> String {
    let camel = snake_or_path_to_camel(value);
    let mut chars = camel.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut output = String::new();
    output.extend(first.to_uppercase());
    output.extend(chars);
    output
}

fn normalize_match_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut normalized = String::new();
    let mut previous: Option<char> = None;
    let mut last_was_space = false;

    for ch in trimmed.chars() {
        if !ch.is_alphanumeric() {
            if !last_was_space && !normalized.is_empty() {
                normalized.push(' ');
                last_was_space = true;
            }
            previous = None;
            continue;
        }

        let starts_new_token = previous.is_some_and(|prev| {
            (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                || (prev.is_ascii_alphabetic() && ch.is_ascii_digit())
                || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
        });
        if starts_new_token && !last_was_space && !normalized.is_empty() {
            normalized.push(' ');
        }

        for lower in ch.to_lowercase() {
            normalized.push(lower);
        }
        last_was_space = false;
        previous = Some(ch);
    }

    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_concept(value: &str) -> String {
    normalize_match_text(value)
}

fn match_kind_label(match_type: TextMatch) -> &'static str {
    match match_type {
        TextMatch::Exact => "exact",
        TextMatch::Contains => "contains",
    }
}

fn merge_evidence(target: &mut Vec<ConceptEvidence>, incoming: &[ConceptEvidence]) {
    let mut seen: HashSet<ConceptEvidence> = target.iter().cloned().collect();
    for evidence in incoming {
        if seen.insert(evidence.clone()) {
            target.push(evidence.clone());
        }
    }
}

fn sort_concepts(records: &mut [ConceptRecord]) {
    for record in records.iter_mut() {
        record.aliases.sort();
        record
            .aliases
            .dedup_by(|left, right| normalize_concept(left) == normalize_concept(right));
        record
            .bindings
            .sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
        record
            .bindings
            .dedup_by(|left, right| left.symbol_id == right.symbol_id);
    }

    records.sort_by(|left, right| left.concept.cmp(&right.concept));
}

fn snapshot_path(store_dir: &Path) -> PathBuf {
    store_dir.join(CONCEPTS_SNAPSHOT_FILE)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use tempfile::tempdir;

    use grapha_core::graph::{Edge, Graph, Node, NodeKind, Span, Visibility};

    use super::*;

    fn make_node(id: &str, name: &str, kind: NodeKind, file: &str) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            file: PathBuf::from(file),
            span: Span {
                start: [1, 1],
                end: [1, 2],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: Some("Gift".to_string()),
            snippet: None,
        }
    }

    fn build_search_index(graph: &Graph) -> (tempfile::TempDir, Index) {
        let dir = tempdir().unwrap();
        let index = search::build_index(graph, &dir.path().join("search_index")).unwrap();
        (dir, index)
    }

    #[test]
    fn concept_index_loads_missing_store_as_empty() {
        let dir = tempdir().unwrap();
        let index = load_concept_index_from_store(dir.path()).unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn concept_index_persists_bindings_and_aliases() {
        let dir = tempdir().unwrap();
        let mut index = ConceptIndex::default();
        index
            .bind_concept(
                "送礼横幅",
                &[String::from("gift-banner-page")],
                vec![ConceptEvidence {
                    kind: "manual".to_string(),
                    value: "送礼横幅".to_string(),
                    match_kind: "confirmed".to_string(),
                    table: None,
                    key: None,
                    source_value: None,
                    ui_path: Vec::new(),
                    note: Some("manual".to_string()),
                }],
            )
            .unwrap();
        index
            .add_aliases("送礼横幅", &[String::from("礼物 banner")])
            .unwrap();
        save_concept_index_to_store(dir.path(), &index).unwrap();

        let loaded = load_concept_index_from_store(dir.path()).unwrap();
        let (record, lookup) = loaded.record_for_term("礼物 banner").unwrap();
        assert_eq!(record.concept, "送礼横幅");
        assert_eq!(lookup.match_kind, "alias");
        assert_eq!(record.bindings.len(), 1);
    }

    #[test]
    fn search_concepts_prefers_confirmed_binding_over_heuristics() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![make_node(
                "gift-banner-page",
                "GiftBannerPage",
                NodeKind::Struct,
                "GiftBannerPage.swift",
            )],
            edges: Vec::new(),
        };
        let (_dir, search_index) = build_search_index(&graph);
        let mut concepts = ConceptIndex::default();
        concepts
            .bind_concept(
                "送礼横幅",
                &[String::from("gift-banner-page")],
                vec![ConceptEvidence {
                    kind: "manual".to_string(),
                    value: "送礼横幅".to_string(),
                    match_kind: "confirmed".to_string(),
                    table: None,
                    key: None,
                    source_value: None,
                    ui_path: Vec::new(),
                    note: Some("seed".to_string()),
                }],
            )
            .unwrap();

        let result = search_concepts(
            &graph,
            &search_index,
            &concepts,
            &LocalizationCatalogIndex::default(),
            &AssetCatalogIndex::default(),
            "送礼横幅",
            5,
        )
        .unwrap();

        assert_eq!(result.resolved_from, "concept_store");
        assert_eq!(result.scopes.len(), 1);
        assert_eq!(result.scopes[0].symbol.id, "gift-banner-page");
        assert_eq!(result.scopes[0].status, STATUS_CONFIRMED);
    }

    #[test]
    fn search_concepts_resolves_localized_value_to_owner_scope() {
        let owner = make_node(
            "gift-banner-page",
            "GiftBannerPage",
            NodeKind::Struct,
            "GiftBannerPage.swift",
        );
        let mut usage = make_node(
            "gift-banner-title",
            "bannerTitle",
            NodeKind::Property,
            "GiftBannerPage.swift",
        );
        usage
            .metadata
            .insert("l10n.ref_kind".to_string(), "literal".to_string());
        usage
            .metadata
            .insert("l10n.literal".to_string(), "gift_banner_title".to_string());

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![owner.clone(), usage],
            edges: vec![Edge {
                source: owner.id.clone(),
                target: "gift-banner-title".to_string(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        };
        let (_dir, search_index) = build_search_index(&graph);
        let catalogs = LocalizationCatalogIndex::from_records(vec![LocalizationCatalogRecord {
            table: "Localizable".to_string(),
            key: "gift_banner_title".to_string(),
            catalog_file: "Resources/Localizable.xcstrings".to_string(),
            catalog_dir: "Resources".to_string(),
            source_language: "zh-Hans".to_string(),
            source_value: "送礼横幅".to_string(),
            status: "translated".to_string(),
            comment: None,
            translations: BTreeMap::new(),
        }]);

        let result = search_concepts(
            &graph,
            &search_index,
            &ConceptIndex::default(),
            &catalogs,
            &AssetCatalogIndex::default(),
            "送礼横幅",
            5,
        )
        .unwrap();

        assert_eq!(result.resolved_from, "heuristics");
        assert_eq!(result.scopes[0].symbol.id, owner.id);
        assert!(
            result.scopes[0]
                .evidence
                .iter()
                .any(|evidence| evidence.kind == "l10n_value")
        );
    }

    #[test]
    fn search_concepts_resolves_asset_usage_to_owner_scope() {
        let owner = make_node(
            "gift-banner-page",
            "GiftBannerPage",
            NodeKind::Struct,
            "GiftBannerPage.swift",
        );
        let mut asset_usage = make_node(
            "gift-banner-icon",
            "giftIcon",
            NodeKind::Property,
            "GiftBannerPage.swift",
        );
        asset_usage
            .metadata
            .insert("asset.ref_kind".to_string(), "image".to_string());
        asset_usage
            .metadata
            .insert("asset.name".to_string(), "gift/banner".to_string());

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![owner.clone(), asset_usage],
            edges: vec![Edge {
                source: owner.id.clone(),
                target: "gift-banner-icon".to_string(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        };
        let (_dir, search_index) = build_search_index(&graph);
        let assets_index = AssetCatalogIndex::from_records(vec![AssetRecord {
            name: "gift/banner".to_string(),
            group_path: "gift".to_string(),
            catalog: "Assets".to_string(),
            catalog_dir: "Resources".to_string(),
            template_intent: None,
            provides_namespace: None,
        }]);

        let result = search_concepts(
            &graph,
            &search_index,
            &ConceptIndex::default(),
            &LocalizationCatalogIndex::default(),
            &assets_index,
            "gift/banner",
            5,
        )
        .unwrap();

        assert_eq!(result.scopes[0].symbol.id, owner.id);
        assert!(
            result.scopes[0]
                .evidence
                .iter()
                .any(|evidence| evidence.kind == "asset_name")
        );
    }

    #[test]
    fn search_concepts_falls_back_to_l10n_wrapper_when_record_has_no_usage_sites() {
        let wrapper = make_node(
            "l10n-gift-record",
            "taskHelpTabGiftRecord",
            NodeKind::Property,
            "Strings.generated.swift",
        );
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![wrapper.clone()],
            edges: Vec::new(),
        };
        let (_dir, search_index) = build_search_index(&graph);
        let catalogs = LocalizationCatalogIndex::from_records(vec![LocalizationCatalogRecord {
            table: "Localizable".to_string(),
            key: "task_help_tab_gift_record".to_string(),
            catalog_file: "Resources/Localizable.xcstrings".to_string(),
            catalog_dir: "Resources".to_string(),
            source_language: "zh-Hans".to_string(),
            source_value: "Gift records".to_string(),
            status: "translated".to_string(),
            comment: None,
            translations: BTreeMap::from([(String::from("zh-Hans"), String::from("送礼记录"))]),
        }]);

        let result = search_concepts(
            &graph,
            &search_index,
            &ConceptIndex::default(),
            &catalogs,
            &AssetCatalogIndex::default(),
            "送礼记录",
            5,
        )
        .unwrap();

        assert_eq!(result.resolved_from, "heuristics");
        assert_eq!(result.scopes[0].symbol.id, wrapper.id);
        assert!(
            result.scopes[0]
                .evidence
                .iter()
                .any(|evidence| evidence.kind == "l10n_wrapper")
        );
    }

    #[test]
    fn search_concepts_matches_asset_tokens_and_lifts_to_caller_scope() {
        let owner = make_node(
            "gift-banner-view",
            "GiftNotifyBannerView",
            NodeKind::Struct,
            "GiftNotifyBannerView.swift",
        );
        let caller = make_node(
            "gift-banner-image",
            "bannerImage",
            NodeKind::Property,
            "GiftNotifyBannerView.swift",
        );
        let asset = make_node(
            "room-gift-banner-1",
            "roomGiftBanner1",
            NodeKind::Property,
            "Assets.generated.swift",
        );

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![owner.clone(), caller.clone(), asset],
            edges: vec![
                Edge {
                    source: owner.id.clone(),
                    target: caller.id.clone(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: caller.id.clone(),
                    target: "room-gift-banner-1".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };
        let (_dir, search_index) = build_search_index(&graph);
        let assets_index = AssetCatalogIndex::from_records(vec![AssetRecord {
            name: "room_gift_banner_1".to_string(),
            group_path: "Room".to_string(),
            catalog: "Assets".to_string(),
            catalog_dir: "Resources".to_string(),
            template_intent: None,
            provides_namespace: None,
        }]);

        let result = search_concepts(
            &graph,
            &search_index,
            &ConceptIndex::default(),
            &LocalizationCatalogIndex::default(),
            &assets_index,
            "gift banner",
            5,
        )
        .unwrap();

        assert_eq!(result.scopes[0].symbol.id, owner.id);
        assert!(
            result.scopes[0]
                .evidence
                .iter()
                .any(|evidence| evidence.kind == "asset_wrapper")
        );
    }

    #[test]
    fn prune_removes_stale_bindings_without_dropping_record() {
        let mut index = ConceptIndex::from_records(vec![ConceptRecord {
            concept: "送礼横幅".to_string(),
            aliases: Vec::new(),
            bindings: vec![ConceptBinding {
                symbol_id: "stale-id".to_string(),
                status: STATUS_CONFIRMED.to_string(),
                evidence: Vec::new(),
            }],
            notes: None,
        }]);
        let valid_ids: HashSet<&str> = HashSet::new();

        let result = index.prune(&valid_ids);

        assert_eq!(result.pruned_bindings, 1);
        let (record, _) = index.record_for_term("送礼横幅").unwrap();
        assert!(record.bindings.is_empty());
    }
}
