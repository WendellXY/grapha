use std::collections::{HashMap, HashSet};

use serde::Serialize;

use grapha_core::graph::{EdgeKind, Graph, Node, NodeKind};

use super::{SymbolRef, is_swiftui_invalidation_source};

#[derive(Debug, Serialize)]
pub struct SmellsResult {
    pub smells: Vec<Smell>,
    pub total: usize,
    pub by_severity: HashMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct Smell {
    pub kind: String,
    pub severity: String,
    pub symbol: SymbolRef,
    pub message: String,
    pub metric_value: usize,
    pub threshold: usize,
}

fn to_symbol_ref(node: &Node) -> SymbolRef {
    SymbolRef::from_node(node)
}

fn is_type_node(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Struct
            | NodeKind::Enum
            | NodeKind::Trait
            | NodeKind::Protocol
            | NodeKind::Extension
    )
}

fn count_init_params(name: &str) -> usize {
    if !name.starts_with("init(") {
        return 0;
    }
    let inner = name
        .strip_prefix("init(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or("");
    if inner.is_empty() {
        return 0;
    }
    inner.split(':').filter(|s| !s.is_empty()).count()
}

fn measure_contains_depth<'a>(
    node_id: &'a str,
    contains_adj: &HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
) -> usize {
    if !visited.insert(node_id) {
        return 0;
    }
    let children = match contains_adj.get(node_id) {
        Some(c) => c,
        None => return 0,
    };
    let max_child_depth = children
        .iter()
        .map(|child| measure_contains_depth(child, contains_adj, visited))
        .max()
        .unwrap_or(0);
    1 + max_child_depth
}

struct SmellConfig {
    god_type_property_threshold: usize,
    god_type_dependency_threshold: usize,
    wide_invalidation_threshold: usize,
    massive_init_threshold: usize,
    deep_nesting_threshold: usize,
    high_fan_out_threshold: usize,
    high_fan_in_threshold: usize,
    many_extensions_threshold: usize,
}

impl Default for SmellConfig {
    fn default() -> Self {
        Self {
            god_type_property_threshold: 15,
            god_type_dependency_threshold: 10,
            wide_invalidation_threshold: 5,
            massive_init_threshold: 8,
            deep_nesting_threshold: 5,
            high_fan_out_threshold: 15,
            high_fan_in_threshold: 15,
            many_extensions_threshold: 5,
        }
    }
}

pub fn detect_smells(graph: &Graph) -> SmellsResult {
    let config = SmellConfig::default();
    let node_index: HashMap<&str, &Node> = graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Build adjacency maps
    let mut implements_targets: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut contains_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut type_ref_sources: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut callee_count: HashMap<&str, usize> = HashMap::new();
    let mut caller_count: HashMap<&str, usize> = HashMap::new();
    let mut reads_adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in &graph.edges {
        match edge.kind {
            EdgeKind::Implements => {
                implements_targets
                    .entry(edge.target.as_str())
                    .or_default()
                    .push(edge.source.as_str());
            }
            EdgeKind::Contains => {
                contains_adj
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
            }
            EdgeKind::TypeRef => {
                type_ref_sources
                    .entry(edge.target.as_str())
                    .or_default()
                    .push(edge.source.as_str());
            }
            EdgeKind::Calls => {
                *callee_count.entry(edge.source.as_str()).or_default() += 1;
                *caller_count.entry(edge.target.as_str()).or_default() += 1;
            }
            EdgeKind::Reads => {
                reads_adj
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
            }
            _ => {}
        }
    }

    let mut smells: Vec<Smell> = Vec::new();

    for node in &graph.nodes {
        if !is_type_node(node.kind) {
            continue;
        }

        let implementors = implements_targets
            .get(node.id.as_str())
            .cloned()
            .unwrap_or_default();

        // God type: too many properties
        let property_count = implementors
            .iter()
            .filter(|id| {
                node_index
                    .get(*id)
                    .is_some_and(|n| matches!(n.kind, NodeKind::Property | NodeKind::Field))
            })
            .count();

        if property_count > config.god_type_property_threshold {
            smells.push(Smell {
                kind: "god_type".to_string(),
                severity: if property_count > 25 {
                    "critical"
                } else {
                    "warning"
                }
                .to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} has {property_count} properties (threshold: {})",
                    node.name, config.god_type_property_threshold
                ),
                metric_value: property_count,
                threshold: config.god_type_property_threshold,
            });
        }

        // God type: too many dependencies
        let mut dep_set: HashSet<&str> = HashSet::new();
        for impl_id in &implementors {
            if let Some(reads) = reads_adj.get(*impl_id) {
                for r in reads {
                    dep_set.insert(r);
                }
            }
        }
        if dep_set.len() > config.god_type_dependency_threshold {
            smells.push(Smell {
                kind: "excessive_dependencies".to_string(),
                severity: "warning".to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} reads {} unique dependencies (threshold: {})",
                    node.name,
                    dep_set.len(),
                    config.god_type_dependency_threshold
                ),
                metric_value: dep_set.len(),
                threshold: config.god_type_dependency_threshold,
            });
        }

        // Wide invalidation surface
        let invalidation_count = implementors
            .iter()
            .filter_map(|id| node_index.get(*id).copied())
            .filter(|n| is_swiftui_invalidation_source(n))
            .count();

        if invalidation_count > config.wide_invalidation_threshold {
            smells.push(Smell {
                kind: "wide_invalidation".to_string(),
                severity: if invalidation_count > 8 {
                    "critical"
                } else {
                    "warning"
                }
                .to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} has {invalidation_count} invalidation sources (threshold: {})",
                    node.name, config.wide_invalidation_threshold
                ),
                metric_value: invalidation_count,
                threshold: config.wide_invalidation_threshold,
            });
        }

        // Massive init
        let max_init_params = implementors
            .iter()
            .filter_map(|id| node_index.get(*id).copied())
            .filter(|n| n.kind == NodeKind::Function && n.name.starts_with("init("))
            .map(|n| count_init_params(&n.name))
            .max()
            .unwrap_or(0);

        if max_init_params > config.massive_init_threshold {
            smells.push(Smell {
                kind: "massive_init".to_string(),
                severity: if max_init_params > 12 {
                    "critical"
                } else {
                    "warning"
                }
                .to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} init has {max_init_params} parameters (threshold: {})",
                    node.name, config.massive_init_threshold
                ),
                metric_value: max_init_params,
                threshold: config.massive_init_threshold,
            });
        }

        // Deep nesting (contains tree depth)
        let depth = measure_contains_depth(node.id.as_str(), &contains_adj, &mut HashSet::new());
        if depth > config.deep_nesting_threshold {
            smells.push(Smell {
                kind: "deep_nesting".to_string(),
                severity: "warning".to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} has contains-tree depth {depth} (threshold: {})",
                    node.name, config.deep_nesting_threshold
                ),
                metric_value: depth,
                threshold: config.deep_nesting_threshold,
            });
        }

        // Many extensions
        let ext_count = type_ref_sources
            .get(node.id.as_str())
            .map(|sources| {
                sources
                    .iter()
                    .filter(|id| {
                        node_index
                            .get(*id)
                            .is_some_and(|n| n.kind == NodeKind::Extension)
                    })
                    .count()
            })
            .unwrap_or(0);

        if ext_count > config.many_extensions_threshold {
            smells.push(Smell {
                kind: "many_extensions".to_string(),
                severity: "warning".to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} has {ext_count} extensions (threshold: {})",
                    node.name, config.many_extensions_threshold
                ),
                metric_value: ext_count,
                threshold: config.many_extensions_threshold,
            });
        }
    }

    // Function-level smells: fan-out and fan-in
    for node in &graph.nodes {
        if node.kind != NodeKind::Function {
            continue;
        }

        let fan_out = callee_count.get(node.id.as_str()).copied().unwrap_or(0);
        if fan_out > config.high_fan_out_threshold {
            smells.push(Smell {
                kind: "high_fan_out".to_string(),
                severity: "warning".to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} calls {fan_out} other symbols (threshold: {})",
                    node.name, config.high_fan_out_threshold
                ),
                metric_value: fan_out,
                threshold: config.high_fan_out_threshold,
            });
        }

        let fan_in = caller_count.get(node.id.as_str()).copied().unwrap_or(0);
        if fan_in > config.high_fan_in_threshold {
            smells.push(Smell {
                kind: "high_fan_in".to_string(),
                severity: "warning".to_string(),
                symbol: to_symbol_ref(node),
                message: format!(
                    "{} is called by {fan_in} symbols (threshold: {})",
                    node.name, config.high_fan_in_threshold
                ),
                metric_value: fan_in,
                threshold: config.high_fan_in_threshold,
            });
        }
    }

    // Sort: critical first, then by metric_value desc
    smells.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| b.metric_value.cmp(&a.metric_value))
    });

    let mut by_severity: HashMap<String, usize> = HashMap::new();
    for smell in &smells {
        *by_severity.entry(smell.severity.clone()).or_default() += 1;
    }

    let total = smells.len();
    SmellsResult {
        smells,
        total,
        by_severity,
    }
}

fn severity_rank(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::{Edge, Node, Span, Visibility};
    use std::path::PathBuf;

    fn make_node(id: &str, name: &str, kind: NodeKind, file: &str) -> Node {
        Node {
            id: id.into(),
            kind,
            name: name.into(),
            file: PathBuf::from(file),
            span: Span {
                start: [1, 0],
                end: [10, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        }
    }

    fn make_edge(source: &str, target: &str, kind: EdgeKind) -> Edge {
        Edge {
            source: source.into(),
            target: target.into(),
            kind,
            confidence: 1.0,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: vec![],
        }
    }

    #[test]
    fn detects_god_type() {
        let mut nodes = vec![make_node("s:Big", "BigStruct", NodeKind::Struct, "B.swift")];
        let mut edges = Vec::new();

        for i in 0..20 {
            let id = format!("s:prop{i}");
            nodes.push(make_node(
                &id,
                &format!("prop{i}"),
                NodeKind::Property,
                "B.swift",
            ));
            edges.push(make_edge(&id, "s:Big", EdgeKind::Implements));
        }

        let graph = Graph {
            version: String::new(),
            nodes,
            edges,
        };

        let result = detect_smells(&graph);
        assert!(result.smells.iter().any(|s| s.kind == "god_type"));
    }

    #[test]
    fn no_smells_for_small_type() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                make_node("s:T", "SmallType", NodeKind::Struct, "T.swift"),
                make_node("s:p1", "name", NodeKind::Property, "T.swift"),
            ],
            edges: vec![make_edge("s:p1", "s:T", EdgeKind::Implements)],
        };

        let result = detect_smells(&graph);
        assert_eq!(result.total, 0);
    }
}
