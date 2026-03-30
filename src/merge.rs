use std::collections::{HashMap, HashSet};

use crate::extract::ExtractionResult;
use crate::graph::{EdgeKind, Graph};

/// Entry in the name-to-candidates index: (node_id, module).
struct NameEntry {
    id: String,
    module: Option<String>,
}

/// Merge multiple `ExtractionResult`s into a single `Graph`.
///
/// Edges whose target matches a known node ID are kept as-is.
/// Uses edges are always kept (external references).
/// For unresolved targets, cross-file resolution is attempted by matching
/// the symbol name (last `::` segment) against all known nodes.
///
/// Module-aware confidence scoring:
/// - Single candidate, same module as source: 0.9x
/// - Single candidate, different module: 0.8x
/// - Multiple candidates, same-module match preferred: 0.7x
/// - Multiple candidates, cross-module: 0.5x
/// - No matches: edge dropped
pub fn merge(results: Vec<ExtractionResult>) -> Graph {
    let mut graph = Graph::new();

    for r in &results {
        graph.nodes.extend(r.nodes.iter().cloned());
    }

    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();

    // Build name → vec of (node_id, module) for cross-file lookup
    let mut name_to_entries: HashMap<&str, Vec<NameEntry>> = HashMap::new();
    for node in &graph.nodes {
        name_to_entries
            .entry(node.name.as_str())
            .or_default()
            .push(NameEntry {
                id: node.id.clone(),
                module: node.module.clone(),
            });
    }

    // Build id → module for source lookups
    let id_to_module: HashMap<&str, Option<&str>> = graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.module.as_deref()))
        .collect();

    for r in results {
        for mut edge in r.edges {
            if node_ids.contains(edge.target.as_str()) || edge.kind == EdgeKind::Uses {
                graph.edges.push(edge);
            } else {
                // Try cross-file resolution by symbol name
                let target_name = edge.target.rsplit("::").next().unwrap_or(&edge.target);
                if let Some(candidates) = name_to_entries.get(target_name) {
                    if candidates.is_empty() {
                        continue;
                    }

                    let source_module = id_to_module.get(edge.source.as_str()).copied().flatten();

                    if candidates.len() == 1 {
                        let candidate = &candidates[0];
                        let same_module = modules_match(source_module, candidate.module.as_deref());
                        edge.target = candidate.id.clone();
                        edge.confidence *= if same_module { 0.9 } else { 0.8 };
                        graph.edges.push(edge);
                    } else {
                        // Multiple candidates — prefer same-module match
                        let same_module_candidate = candidates
                            .iter()
                            .find(|c| modules_match(source_module, c.module.as_deref()));

                        if let Some(candidate) = same_module_candidate {
                            edge.target = candidate.id.clone();
                            edge.confidence *= 0.7;
                        } else {
                            edge.target = candidates[0].id.clone();
                            edge.confidence *= 0.5;
                        }
                        graph.edges.push(edge);
                    }
                }
            }
        }
    }

    graph
}

/// Check if two modules match. Two `None` modules are considered matching
/// (both in the default/root module).
fn modules_match(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, name: &str, kind: NodeKind) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: name.to_string(),
            file: PathBuf::from("test.rs"),
            span: Span {
                start: [0, 0],
                end: [0, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
        }
    }

    #[test]
    fn merges_nodes_from_multiple_results() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::Foo", "Foo", NodeKind::Struct)],
            edges: vec![],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::Bar", "Bar", NodeKind::Struct)],
            edges: vec![],
            imports: vec![],
        };
        let graph = merge(vec![r1, r2]);
        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn drops_edges_with_unresolved_targets() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "nonexistent::foo".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 0);
    }

    #[test]
    fn keeps_edges_with_resolved_targets() {
        let r1 = ExtractionResult {
            nodes: vec![
                make_node("a::main", "main", NodeKind::Function),
                make_node("a::helper", "helper", NodeKind::Function),
            ],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "a::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn resolves_cross_file_edges() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a::main".to_string(),
                target: "b::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b::helper", "helper", NodeKind::Function)],
            edges: vec![],
            imports: vec![],
        };
        let graph = merge(vec![r1, r2]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn keeps_uses_edges_even_if_target_unresolved() {
        let r1 = ExtractionResult {
            nodes: vec![],
            edges: vec![Edge {
                source: "a.rs".to_string(),
                target: "use std::collections::HashMap;".to_string(),
                kind: EdgeKind::Uses,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let graph = merge(vec![r1]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn cross_module_resolution_gets_lower_confidence() {
        // Two nodes with the same name in different modules
        let mut node_a = make_node("mod_a::helper", "helper", NodeKind::Function);
        node_a.module = Some("mod_a".to_string());

        let mut node_b = make_node("mod_b::helper", "helper", NodeKind::Function);
        node_b.module = Some("mod_b".to_string());

        let mut caller = make_node("mod_a::main", "main", NodeKind::Function);
        caller.module = Some("mod_a".to_string());

        let r1 = ExtractionResult {
            nodes: vec![caller],
            edges: vec![Edge {
                source: "mod_a::main".to_string(),
                target: "unknown::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![node_a, node_b],
            edges: vec![],
            imports: vec![],
        };

        let graph = merge(vec![r1, r2]);
        let call_edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .expect("should have a call edge");

        // Multiple candidates but same-module match preferred → 0.7x
        assert_eq!(call_edge.target, "mod_a::helper");
        assert!(
            (call_edge.confidence - 0.7).abs() < 0.001,
            "expected 0.7, got {}",
            call_edge.confidence
        );
    }

    #[test]
    fn cross_module_single_candidate_different_module() {
        let mut node = make_node("mod_b::helper", "helper", NodeKind::Function);
        node.module = Some("mod_b".to_string());

        let mut caller = make_node("mod_a::main", "main", NodeKind::Function);
        caller.module = Some("mod_a".to_string());

        let r1 = ExtractionResult {
            nodes: vec![caller],
            edges: vec![Edge {
                source: "mod_a::main".to_string(),
                target: "unknown::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![node],
            edges: vec![],
            imports: vec![],
        };

        let graph = merge(vec![r1, r2]);
        let call_edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .expect("should have a call edge");

        // Single candidate, different module → 0.8x
        assert_eq!(call_edge.target, "mod_b::helper");
        assert!(
            (call_edge.confidence - 0.8).abs() < 0.001,
            "expected 0.8, got {}",
            call_edge.confidence
        );
    }

    #[test]
    fn same_module_single_candidate_gets_highest_confidence() {
        let mut node = make_node("mod_a::helper", "helper", NodeKind::Function);
        node.module = Some("mod_a".to_string());

        let mut caller = make_node("mod_a::main", "main", NodeKind::Function);
        caller.module = Some("mod_a".to_string());

        let r1 = ExtractionResult {
            nodes: vec![caller],
            edges: vec![Edge {
                source: "mod_a::main".to_string(),
                target: "unknown::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![node],
            edges: vec![],
            imports: vec![],
        };

        let graph = merge(vec![r1, r2]);
        let call_edge = graph
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .expect("should have a call edge");

        // Single candidate, same module → 0.9x
        assert_eq!(call_edge.target, "mod_a::helper");
        assert!(
            (call_edge.confidence - 0.9).abs() < 0.001,
            "expected 0.9, got {}",
            call_edge.confidence
        );
    }

    #[test]
    fn resolves_cross_file_calls_by_name() {
        let r1 = ExtractionResult {
            nodes: vec![make_node("a.rs::main", "main", NodeKind::Function)],
            edges: vec![Edge {
                source: "a.rs::main".to_string(),
                target: "a.rs::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.8,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
            }],
            imports: vec![],
        };
        let r2 = ExtractionResult {
            nodes: vec![make_node("b.rs::helper", "helper", NodeKind::Function)],
            edges: vec![],
            imports: vec![],
        };
        let graph = merge(vec![r1, r2]);
        // Edge target should be rewritten to b.rs::helper
        let call_edge = graph.edges.iter().find(|e| e.kind == EdgeKind::Calls);
        assert!(call_edge.is_some());
        let e = call_edge.unwrap();
        assert_eq!(e.target, "b.rs::helper");
        assert!(e.confidence < 0.8); // reduced confidence
    }
}
