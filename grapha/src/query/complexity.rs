use std::collections::{HashMap, HashSet};

use serde::Serialize;

use grapha_core::graph::{EdgeKind, Graph, Node, NodeKind};

use super::{QueryResolveError, SymbolInfo, SymbolRef, is_swiftui_invalidation_source};

#[derive(Debug, Serialize)]
pub struct ComplexityResult {
    pub symbol: SymbolInfo,
    pub metrics: ComplexityMetrics,
    pub severity: String,
}

#[derive(Debug, Serialize)]
pub struct ComplexityMetrics {
    pub property_count: usize,
    pub method_count: usize,
    pub dependency_count: usize,
    pub invalidation_source_count: usize,
    pub init_parameter_count: usize,
    pub extension_count: usize,
    pub contains_depth: usize,
    pub direct_child_count: usize,
    pub blast_radius: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invalidation_sources: Vec<SymbolRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub heaviest_dependencies: Vec<SymbolRef>,
}

fn to_symbol_ref(node: &Node) -> SymbolRef {
    SymbolRef::from_node(node)
}

fn count_init_params(node: &Node) -> usize {
    let name = &node.name;
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

fn severity_from_score(score: usize) -> &'static str {
    match score {
        0..=2 => "low",
        3..=5 => "medium",
        6..=8 => "high",
        _ => "critical",
    }
}

pub fn query_complexity(graph: &Graph, query: &str) -> Result<ComplexityResult, QueryResolveError> {
    let node = super::resolve_node(&graph.nodes, query)?;
    let node_id = &node.id;
    let node_index: HashMap<&str, &Node> = graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Build adjacency maps
    let mut implements_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut contains_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut type_ref_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut reads_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut callee_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut reverse_adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in &graph.edges {
        match edge.kind {
            EdgeKind::Implements => {
                implements_adj
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
            }
            EdgeKind::Contains => {
                contains_adj
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
            }
            EdgeKind::TypeRef => {
                type_ref_adj
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
            }
            EdgeKind::Reads => {
                reads_adj
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
            }
            EdgeKind::Calls => {
                callee_adj
                    .entry(edge.source.as_str())
                    .or_default()
                    .push(edge.target.as_str());
            }
            _ => {}
        }
        // Reverse edges for blast radius (all edge kinds)
        reverse_adj
            .entry(edge.target.as_str())
            .or_default()
            .push(edge.source.as_str());
    }

    // Implementors: symbols that implement this type (properties, methods)
    let implementors: Vec<&str> = implements_adj
        .iter()
        .filter_map(|(source, targets)| {
            if targets.contains(&node_id.as_str()) {
                Some(*source)
            } else {
                None
            }
        })
        .collect();

    let property_count = implementors
        .iter()
        .filter(|id| {
            node_index
                .get(*id)
                .is_some_and(|n| matches!(n.kind, NodeKind::Property | NodeKind::Field))
        })
        .count();

    let method_count = implementors
        .iter()
        .filter(|id| {
            node_index
                .get(*id)
                .is_some_and(|n| n.kind == NodeKind::Function)
        })
        .count();

    // Init parameter count: find the longest init among implementors
    let init_parameter_count = implementors
        .iter()
        .filter_map(|id| node_index.get(*id).copied())
        .filter(|n| n.kind == NodeKind::Function && n.name.starts_with("init("))
        .map(count_init_params)
        .max()
        .unwrap_or(0);

    // Extension count: type_refs that are extensions pointing to this node
    let extension_count = type_ref_adj
        .iter()
        .filter(|(source, _)| {
            node_index
                .get(*source)
                .is_some_and(|n| n.kind == NodeKind::Extension)
        })
        .filter(|(_, targets)| targets.contains(&node_id.as_str()))
        .count();

    // Dependency count: unique symbols read by body or methods of this type
    let mut dependencies: HashSet<&str> = HashSet::new();
    for impl_id in &implementors {
        if let Some(reads) = reads_adj.get(*impl_id) {
            for read in reads {
                dependencies.insert(read);
            }
        }
        if let Some(callees) = callee_adj.get(*impl_id) {
            for callee in callees {
                dependencies.insert(callee);
            }
        }
    }

    // Invalidation sources: observable properties that trigger re-evaluation
    let invalidation_sources: Vec<SymbolRef> = implementors
        .iter()
        .filter_map(|id| node_index.get(*id).copied())
        .filter(|n| is_swiftui_invalidation_source(n))
        .map(to_symbol_ref)
        .collect();
    let invalidation_source_count = invalidation_sources.len();

    // Contains depth
    let contains_depth =
        measure_contains_depth(node_id.as_str(), &contains_adj, &mut HashSet::new());

    // Direct children in contains tree
    let direct_child_count = contains_adj
        .get(node_id.as_str())
        .map(|c| c.len())
        .unwrap_or(0);

    // Blast radius: BFS depth-1 from this node via reverse adjacency
    let mut blast_radius_set: HashSet<&str> = HashSet::new();
    if let Some(neighbors) = reverse_adj.get(node_id.as_str()) {
        for n in neighbors {
            blast_radius_set.insert(n);
        }
    }
    // Also count implementors as depth-1 blast radius
    for impl_id in &implementors {
        blast_radius_set.insert(impl_id);
    }
    let blast_radius = blast_radius_set.len();

    // Heaviest dependencies (top 10 by kind preference: types before functions)
    let mut heaviest_dependencies: Vec<SymbolRef> = dependencies
        .iter()
        .filter_map(|id| node_index.get(*id).copied())
        .filter(|n| !matches!(n.kind, NodeKind::View | NodeKind::Branch))
        .map(to_symbol_ref)
        .collect();
    heaviest_dependencies.sort_by(|a, b| a.name.cmp(&b.name));
    heaviest_dependencies.truncate(10);

    // Severity scoring
    let mut severity_score = 0usize;
    if property_count > 15 {
        severity_score += 3;
    } else if property_count > 8 {
        severity_score += 2;
    } else if property_count > 5 {
        severity_score += 1;
    }
    if invalidation_source_count > 5 {
        severity_score += 3;
    } else if invalidation_source_count > 3 {
        severity_score += 2;
    } else if invalidation_source_count > 1 {
        severity_score += 1;
    }
    if init_parameter_count > 8 {
        severity_score += 2;
    } else if init_parameter_count > 5 {
        severity_score += 1;
    }
    if extension_count > 4 {
        severity_score += 2;
    } else if extension_count > 2 {
        severity_score += 1;
    }
    if contains_depth > 5 {
        severity_score += 2;
    } else if contains_depth > 3 {
        severity_score += 1;
    }

    let metrics = ComplexityMetrics {
        property_count,
        method_count,
        dependency_count: dependencies.len(),
        invalidation_source_count,
        init_parameter_count,
        extension_count,
        contains_depth,
        direct_child_count,
        blast_radius,
        invalidation_sources,
        heaviest_dependencies,
    };

    Ok(ComplexityResult {
        symbol: SymbolInfo::from_node(node),
        metrics,
        severity: severity_from_score(severity_score).to_string(),
    })
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
    fn counts_properties_and_methods() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                make_node("s:MyType", "MyType", NodeKind::Struct, "MyType.swift"),
                make_node("s:prop1", "name", NodeKind::Property, "MyType.swift"),
                make_node("s:prop2", "age", NodeKind::Property, "MyType.swift"),
                make_node("s:func1", "greet()", NodeKind::Function, "MyType.swift"),
            ],
            edges: vec![
                make_edge("s:prop1", "s:MyType", EdgeKind::Implements),
                make_edge("s:prop2", "s:MyType", EdgeKind::Implements),
                make_edge("s:func1", "s:MyType", EdgeKind::Implements),
            ],
        };

        let result = query_complexity(&graph, "MyType").unwrap();
        assert_eq!(result.metrics.property_count, 2);
        assert_eq!(result.metrics.method_count, 1);
        assert_eq!(result.severity, "low");
    }

    #[test]
    fn counts_init_parameters() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                make_node("s:T", "T", NodeKind::Struct, "T.swift"),
                make_node(
                    "s:init",
                    "init(a:b:c:d:e:f:g:h:i:)",
                    NodeKind::Function,
                    "T.swift",
                ),
            ],
            edges: vec![make_edge("s:init", "s:T", EdgeKind::Implements)],
        };

        let result = query_complexity(&graph, "T").unwrap();
        assert_eq!(result.metrics.init_parameter_count, 9);
    }
}
