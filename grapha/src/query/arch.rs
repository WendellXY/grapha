use std::collections::{HashMap, HashSet};

use grapha_core::graph::{EdgeKind, Graph, Node};
use serde::Serialize;

use crate::config::{ArchitectureConfig, ArchitectureDenyRule, ArchitectureLayer};

use super::SymbolRef;

#[derive(Debug, Serialize)]
pub struct ArchitectureResult {
    pub configured: bool,
    pub total_violations: usize,
    pub layers: Vec<ArchitectureLayerSummary>,
    pub violations: Vec<ArchitectureViolation>,
}

#[derive(Debug, Serialize)]
pub struct ArchitectureLayerSummary {
    pub name: String,
    pub patterns: Vec<String>,
    pub matched_symbols: usize,
}

#[derive(Debug, Serialize)]
pub struct ArchitectureViolation {
    pub source_layer: String,
    pub target_layer: String,
    pub edge_kind: EdgeKind,
    pub confidence: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub source: SymbolRef,
    pub target: SymbolRef,
}

pub fn check_architecture(graph: &Graph, config: &ArchitectureConfig) -> ArchitectureResult {
    let configured = !config.layers.is_empty() || !config.deny.is_empty();
    if config.layers.is_empty() || config.deny.is_empty() {
        let node_layers = if config.layers.is_empty() {
            HashMap::new()
        } else {
            assign_layers(graph, &config.layers)
        };
        return ArchitectureResult {
            configured,
            total_violations: 0,
            layers: layer_summaries(config, &node_layers),
            violations: Vec::new(),
        };
    }

    let node_index: HashMap<&str, &Node> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let node_layers = assign_layers(graph, &config.layers);
    let deny_rules = denied_rule_index(&config.deny);

    let mut violations = Vec::new();
    for edge in &graph.edges {
        if !is_architecture_dependency(edge.kind) {
            continue;
        }

        let Some(source) = node_index.get(edge.source.as_str()).copied() else {
            continue;
        };
        let Some(target) = node_index.get(edge.target.as_str()).copied() else {
            continue;
        };
        let Some(source_layer) = node_layers.get(source.id.as_str()) else {
            continue;
        };
        let Some(target_layer) = node_layers.get(target.id.as_str()) else {
            continue;
        };
        let Some(rule) = deny_rules.get(&deny_key(source_layer, target_layer)) else {
            continue;
        };

        violations.push(ArchitectureViolation {
            source_layer: source_layer.clone(),
            target_layer: target_layer.clone(),
            edge_kind: edge.kind,
            confidence: edge.confidence,
            reason: rule.reason.clone(),
            source: SymbolRef::from_node(source),
            target: SymbolRef::from_node(target),
        });
    }

    violations.sort_by(|left, right| {
        left.source.file.cmp(&right.source.file).then_with(|| {
            left.source
                .name
                .cmp(&right.source.name)
                .then_with(|| left.target.file.cmp(&right.target.file))
                .then_with(|| left.target.name.cmp(&right.target.name))
        })
    });

    let total_violations = violations.len();
    ArchitectureResult {
        configured,
        total_violations,
        layers: layer_summaries(config, &node_layers),
        violations,
    }
}

fn assign_layers(graph: &Graph, layers: &[ArchitectureLayer]) -> HashMap<String, String> {
    let mut node_layers = HashMap::new();
    for node in &graph.nodes {
        if let Some(layer) = layers.iter().find(|layer| node_matches_layer(node, layer)) {
            node_layers.insert(node.id.clone(), layer.name.clone());
        }
    }
    node_layers
}

fn layer_summaries(
    config: &ArchitectureConfig,
    node_layers: &HashMap<String, String>,
) -> Vec<ArchitectureLayerSummary> {
    let mut layer_counts: HashMap<&str, usize> = HashMap::new();
    for layer_name in node_layers.values() {
        *layer_counts.entry(layer_name.as_str()).or_default() += 1;
    }

    config
        .layers
        .iter()
        .map(|layer| ArchitectureLayerSummary {
            name: layer.name.clone(),
            patterns: layer.patterns.clone(),
            matched_symbols: layer_counts.get(layer.name.as_str()).copied().unwrap_or(0),
        })
        .collect()
}

fn denied_rule_index(
    rules: &[ArchitectureDenyRule],
) -> HashMap<(String, String), &ArchitectureDenyRule> {
    rules
        .iter()
        .map(|rule| (deny_key(&rule.from, &rule.to), rule))
        .collect()
}

fn deny_key(from: &str, to: &str) -> (String, String) {
    (from.to_lowercase(), to.to_lowercase())
}

fn is_architecture_dependency(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Calls
            | EdgeKind::Uses
            | EdgeKind::TypeRef
            | EdgeKind::Implements
            | EdgeKind::Inherits
    )
}

fn node_matches_layer(node: &Node, layer: &ArchitectureLayer) -> bool {
    layer.patterns.iter().any(|pattern| {
        node.module
            .as_deref()
            .is_some_and(|module| pattern_matches(pattern, module))
            || pattern_matches(pattern, &node.file.to_string_lossy())
    })
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.replace('\\', "/").to_lowercase();
    let value = value.replace('\\', "/").to_lowercase();

    if !pattern.contains('*') && !pattern.contains('?') {
        return value == pattern || value.ends_with(&format!("/{pattern}"));
    }

    wildcard_matches(&pattern, &value)
        || value
            .match_indices('/')
            .any(|(idx, _)| wildcard_matches(&pattern, &value[idx + 1..]))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut reachable: HashSet<(usize, usize)> = HashSet::from([(0, 0)]);

    for i in 0..=pattern.len() {
        for j in 0..=value.len() {
            if !reachable.contains(&(i, j)) || i == pattern.len() {
                continue;
            }

            match pattern[i] {
                '*' => {
                    reachable.insert((i + 1, j));
                    if j < value.len() {
                        reachable.insert((i, j + 1));
                    }
                }
                '?' if j < value.len() => {
                    reachable.insert((i + 1, j + 1));
                }
                ch if j < value.len() && ch == value[j] => {
                    reachable.insert((i + 1, j + 1));
                }
                _ => {}
            }
        }
    }

    reachable.contains(&(pattern.len(), value.len()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use grapha_core::graph::{Edge, NodeKind, Span, Visibility};

    use super::*;

    fn node(id: &str, module: &str, file: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Function,
            name: id.to_string(),
            file: PathBuf::from(file),
            span: Span {
                start: [1, 0],
                end: [2, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: Some(module.to_string()),
            snippet: None,
        }
    }

    fn edge(source: &str, target: &str, kind: EdgeKind) -> Edge {
        Edge {
            source: source.to_string(),
            target: target.to_string(),
            kind,
            confidence: 1.0,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
        }
    }

    fn config() -> ArchitectureConfig {
        ArchitectureConfig {
            layers: vec![
                ArchitectureLayer {
                    name: "ui".to_string(),
                    patterns: vec!["AppUI*".to_string(), "Features/*/View*".to_string()],
                },
                ArchitectureLayer {
                    name: "infra".to_string(),
                    patterns: vec!["Networking*".to_string()],
                },
            ],
            deny: vec![ArchitectureDenyRule {
                from: "infra".to_string(),
                to: "ui".to_string(),
                reason: Some("Infrastructure must not depend on UI.".to_string()),
            }],
        }
    }

    #[test]
    fn no_config_returns_empty_result() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![node("a", "Networking", "Networking/API.swift")],
            edges: Vec::new(),
        };

        let result = check_architecture(&graph, &ArchitectureConfig::default());

        assert!(!result.configured);
        assert_eq!(result.total_violations, 0);
        assert!(result.violations.is_empty());
    }

    #[test]
    fn configured_layers_without_deny_rules_are_not_violations() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![node("api", "Networking", "Networking/API.swift")],
            edges: Vec::new(),
        };
        let config = ArchitectureConfig {
            layers: vec![ArchitectureLayer {
                name: "infra".to_string(),
                patterns: vec!["Networking*".to_string()],
            }],
            deny: Vec::new(),
        };

        let result = check_architecture(&graph, &config);

        assert!(result.configured);
        assert_eq!(result.layers[0].matched_symbols, 1);
        assert_eq!(result.total_violations, 0);
    }

    #[test]
    fn detects_denied_layer_dependency() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                node("api", "Networking", "Networking/API.swift"),
                node("view", "AppUI", "AppUI/View.swift"),
            ],
            edges: vec![edge("api", "view", EdgeKind::Calls)],
        };

        let result = check_architecture(&graph, &config());

        assert!(result.configured);
        assert_eq!(result.total_violations, 1);
        let violation = &result.violations[0];
        assert_eq!(violation.source_layer, "infra");
        assert_eq!(violation.target_layer, "ui");
        assert_eq!(violation.edge_kind, EdgeKind::Calls);
        assert_eq!(
            violation.reason.as_deref(),
            Some("Infrastructure must not depend on UI.")
        );
    }

    #[test]
    fn allows_dependency_without_matching_deny_rule() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                node("view", "AppUI", "AppUI/View.swift"),
                node("api", "Networking", "Networking/API.swift"),
            ],
            edges: vec![edge("view", "api", EdgeKind::Calls)],
        };

        let result = check_architecture(&graph, &config());

        assert_eq!(result.total_violations, 0);
        assert!(result.violations.is_empty());
    }

    #[test]
    fn matches_file_patterns_when_module_is_not_specific() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                node("api", "Shared", "Sources/Networking/API.swift"),
                node("view", "Shared", "Sources/Features/Login/ViewModel.swift"),
            ],
            edges: vec![edge("api", "view", EdgeKind::TypeRef)],
        };

        let result = check_architecture(&graph, &config());

        assert_eq!(result.total_violations, 1);
        assert_eq!(result.violations[0].edge_kind, EdgeKind::TypeRef);
    }
}
