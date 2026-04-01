use std::collections::{HashMap, HashSet};

use crate::extract::ExtractionResult;
use crate::graph::{EdgeKind, Graph, NodeKind};

struct NameEntry {
    id: String,
    module: Option<String>,
}

pub fn merge(results: Vec<ExtractionResult>) -> Graph {
    let mut graph = Graph::new();

    let mut file_imports: HashMap<String, HashSet<String>> = HashMap::new();
    for result in &results {
        for import in &result.imports {
            if let Some(first_node) = result.nodes.first() {
                let file_key = first_node.file.to_string_lossy().to_string();
                let module_name = import
                    .path
                    .strip_prefix("import ")
                    .unwrap_or(&import.path)
                    .to_string();
                file_imports
                    .entry(file_key)
                    .or_default()
                    .insert(module_name);
            }
        }
    }

    for result in &results {
        graph.nodes.extend(result.nodes.iter().cloned());
    }

    let node_ids: HashSet<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();

    let mut name_to_entries: HashMap<&str, Vec<NameEntry>> = HashMap::new();
    for node in &graph.nodes {
        if matches!(node.kind, NodeKind::View | NodeKind::Branch) {
            continue;
        }
        name_to_entries
            .entry(node.name.as_str())
            .or_default()
            .push(NameEntry {
                id: node.id.clone(),
                module: node.module.clone(),
            });
    }

    let id_to_info: HashMap<&str, (Option<&str>, &str)> = graph
        .nodes
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                (node.module.as_deref(), node.file.to_str().unwrap_or("")),
            )
        })
        .collect();

    let id_to_name: HashMap<&str, &str> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.name.as_str()))
        .collect();
    let mut candidate_to_owner_names: HashMap<String, Vec<String>> = HashMap::new();
    for result in &results {
        for edge in &result.edges {
            if edge.kind == EdgeKind::Contains
                && let Some(parent_name) = id_to_name.get(edge.source.as_str())
            {
                candidate_to_owner_names
                    .entry(edge.target.clone())
                    .or_default()
                    .push(parent_name.to_string());
            } else if edge.kind == EdgeKind::Implements
                && let Some(owner_name) = id_to_name.get(edge.target.as_str())
            {
                candidate_to_owner_names
                    .entry(edge.source.clone())
                    .or_default()
                    .push(owner_name.to_string());
            }
        }
    }

    let all_edges: Vec<_> = results
        .into_iter()
        .flat_map(|result| result.edges)
        .collect();

    // Build child → parent type mapping for scoping Reads edges.
    // If source X is contained by type T, reads from X should prefer
    // targets that are also contained by T (siblings in the same type).
    let mut child_to_parents: HashMap<String, Vec<String>> = HashMap::new();
    for edge in &all_edges {
        if edge.kind == EdgeKind::Contains
            && node_ids.contains(edge.target.as_str())
            && node_ids.contains(edge.source.as_str())
        {
            child_to_parents
                .entry(edge.target.clone())
                .or_default()
                .push(edge.source.clone());
        }
    }

    for mut edge in all_edges {
        let is_external_usr_call = edge.target.starts_with("s:")
            && edge.kind == EdgeKind::Calls
            && !node_ids.contains(edge.target.as_str());

        if node_ids.contains(edge.target.as_str())
            || edge.kind == EdgeKind::Uses
            || edge.kind == EdgeKind::Implements
            || (edge.kind == EdgeKind::Calls
                && (edge.direction.is_some() || edge.operation.is_some()))
            || is_external_usr_call
        {
            graph.edges.push(edge);
            continue;
        }

        let target_name = edge.target.rsplit("::").next().unwrap_or(&edge.target);
        let Some(candidates) = name_to_entries.get(target_name) else {
            continue;
        };
        if candidates.is_empty() {
            continue;
        }

        let (source_module, source_file) = id_to_info
            .get(edge.source.as_str())
            .copied()
            .unwrap_or((None, ""));
        let source_imports = file_imports.get(source_file);
        let prefix_hint = edge.operation.as_deref();

        if candidates.len() == 1 {
            let candidate = &candidates[0];
            let same_module = modules_match(source_module, candidate.module.as_deref());
            if same_module {
                edge.target = candidate.id.clone();
                edge.confidence *= 0.9;
                graph.edges.push(edge);
            } else {
                let imported = source_imports
                    .and_then(|imports| {
                        candidate
                            .module
                            .as_deref()
                            .map(|module| imports.contains(module))
                    })
                    .unwrap_or(false);
                if imported {
                    edge.target = candidate.id.clone();
                    edge.confidence *= 0.7;
                    graph.edges.push(edge);
                }
            }
            continue;
        }

        // For Reads edges: scope resolution to siblings of the same containing type.
        // Without this, "viewModel" resolves to ALL viewModel properties in the module.
        if edge.kind == EdgeKind::Reads {
            if let Some(source_owners) = child_to_parents.get(&edge.source) {
                let sibling_candidates: Vec<&NameEntry> = candidates
                    .iter()
                    .filter(|c| {
                        candidate_to_owner_names
                            .get(&c.id)
                            .is_some_and(|owners| {
                                owners.iter().any(|owner| {
                                    source_owners.iter().any(|so| {
                                        id_to_name.get(so.as_str()).is_some_and(|n| *n == owner)
                                    })
                                })
                            })
                    })
                    .collect();
                if sibling_candidates.len() == 1 {
                    edge.target = sibling_candidates[0].id.clone();
                    edge.confidence *= 0.9;
                    graph.edges.push(edge);
                    continue;
                }
                if !sibling_candidates.is_empty() {
                    // Multiple siblings — pick the one in the same file
                    let same_file: Vec<&&NameEntry> = sibling_candidates
                        .iter()
                        .filter(|c| {
                            id_to_info.get(c.id.as_str())
                                .is_some_and(|(_, f)| *f == source_file)
                        })
                        .collect();
                    if same_file.len() == 1 {
                        edge.target = same_file[0].id.clone();
                        edge.confidence *= 0.9;
                        graph.edges.push(edge);
                        continue;
                    }
                    // Fall through to normal resolution if sibling scoping didn't narrow enough
                }
            }
        }

        let resolved = resolve_candidates(
            candidates,
            source_module,
            source_imports,
            prefix_hint,
            &candidate_to_owner_names,
        );
        for (candidate_id, factor) in resolved {
            let mut resolved_edge = edge.clone();
            resolved_edge.target = candidate_id;
            resolved_edge.confidence *= factor;
            graph.edges.push(resolved_edge);
        }
    }

    graph
}

fn resolve_candidates(
    candidates: &[NameEntry],
    source_module: Option<&str>,
    source_imports: Option<&HashSet<String>>,
    prefix_hint: Option<&str>,
    candidate_to_owner_names: &HashMap<String, Vec<String>>,
) -> Vec<(String, f64)> {
    let same_module: Vec<&NameEntry> = candidates
        .iter()
        .filter(|candidate| modules_match(source_module, candidate.module.as_deref()))
        .collect();
    if same_module.len() == 1 {
        return vec![(same_module[0].id.clone(), 0.9)];
    }

    if same_module.len() > 1 {
        if let Some(hint) = prefix_hint {
            let hint_name = hint.rsplit('.').next().unwrap_or(hint).to_lowercase();
            let exact: Vec<&&NameEntry> = same_module
                .iter()
                .filter(|candidate| {
                    candidate_to_owner_names
                        .get(&candidate.id)
                        .is_some_and(|parents| {
                            parents
                                .iter()
                                .any(|parent| parent.eq_ignore_ascii_case(&hint_name))
                        })
                })
                .collect();
            if exact.len() == 1 {
                return vec![(exact[0].id.clone(), 0.85)];
            }

            let narrowed: Vec<&&NameEntry> = same_module
                .iter()
                .filter(|candidate| {
                    candidate_to_owner_names
                        .get(&candidate.id)
                        .is_some_and(|parents| {
                            parents
                                .iter()
                                .any(|parent| parent.to_lowercase().contains(&hint_name))
                        })
                })
                .collect();
            if narrowed.len() == 1 {
                return vec![(narrowed[0].id.clone(), 0.85)];
            }
        }

        return same_module
            .iter()
            .map(|candidate| (candidate.id.clone(), 0.4))
            .collect();
    }

    if let Some(imports) = source_imports {
        let imported: Vec<&NameEntry> = candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .module
                    .as_deref()
                    .is_some_and(|module| imports.contains(module))
            })
            .collect();
        if imported.len() == 1 {
            return vec![(imported[0].id.clone(), 0.8)];
        }
        if imported.len() > 1 {
            return imported
                .iter()
                .map(|candidate| (candidate.id.clone(), 0.3))
                .collect();
        }
    }

    Vec::new()
}

fn modules_match(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
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
            snippet: None,
        }
    }

    #[test]
    fn merges_nodes_from_multiple_results() {
        let left = ExtractionResult {
            nodes: vec![make_node("a::Foo", "Foo", NodeKind::Struct)],
            edges: vec![],
            imports: vec![],
        };
        let right = ExtractionResult {
            nodes: vec![make_node("b::Bar", "Bar", NodeKind::Struct)],
            edges: vec![],
            imports: vec![],
        };

        let graph = merge(vec![left, right]);
        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn drops_edges_with_unresolved_targets() {
        let result = ExtractionResult {
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
                provenance: Vec::new(),
            }],
            imports: vec![],
        };

        let graph = merge(vec![result]);
        assert_eq!(graph.edges.len(), 0);
    }

    #[test]
    fn keeps_uses_edges_even_if_target_unresolved() {
        let result = ExtractionResult {
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
                provenance: Vec::new(),
            }],
            imports: vec![],
        };

        let graph = merge(vec![result]);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn same_module_candidate_wins() {
        let mut helper = make_node("mod_a::helper", "helper", NodeKind::Function);
        helper.module = Some("mod_a".to_string());
        let mut caller = make_node("mod_a::main", "main", NodeKind::Function);
        caller.module = Some("mod_a".to_string());

        let graph = merge(vec![
            ExtractionResult {
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
                    provenance: Vec::new(),
                }],
                imports: vec![],
            },
            ExtractionResult {
                nodes: vec![helper],
                edges: vec![],
                imports: vec![],
            },
        ]);

        let call_edge = graph
            .edges
            .iter()
            .find(|edge| edge.kind == EdgeKind::Calls)
            .unwrap();
        assert_eq!(call_edge.target, "mod_a::helper");
        assert!((call_edge.confidence - 0.9).abs() < 0.001);
    }

    #[test]
    fn owner_hint_disambiguates_same_module_candidates() {
        let mut room_page_ext = make_node("room::RoomPageExt", "RoomPage", NodeKind::Extension);
        room_page_ext.module = Some("Room".to_string());
        let mut kroom_page_ext = make_node("room::KRoomPageExt", "KRoomPage", NodeKind::Extension);
        kroom_page_ext.module = Some("Room".to_string());

        let mut room_helper = make_node(
            "room::RoomPage::chatRoomFragViewPanel",
            "chatRoomFragViewPanel",
            NodeKind::Property,
        );
        room_helper.module = Some("Room".to_string());
        let mut kroom_helper = make_node(
            "room::KRoomPage::chatRoomFragViewPanel",
            "chatRoomFragViewPanel",
            NodeKind::Property,
        );
        kroom_helper.module = Some("Room".to_string());

        let mut body_view = make_node(
            "room::RoomPage::body::view:chatRoomFragViewPanel",
            "chatRoomFragViewPanel",
            NodeKind::View,
        );
        body_view.module = Some("Room".to_string());

        let source = body_view.id.clone();
        let graph = merge(vec![
            ExtractionResult {
                nodes: vec![body_view],
                edges: vec![Edge {
                    source: source.clone(),
                    target: "room::RoomPage.swift::chatRoomFragViewPanel".to_string(),
                    kind: EdgeKind::TypeRef,
                    confidence: 1.0,
                    direction: None,
                    operation: Some("RoomPage".to_string()),
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                }],
                imports: vec![],
            },
            ExtractionResult {
                nodes: vec![room_page_ext, kroom_page_ext, room_helper, kroom_helper],
                edges: vec![
                    Edge {
                        source: "room::RoomPage::chatRoomFragViewPanel".to_string(),
                        target: "room::RoomPageExt".to_string(),
                        kind: EdgeKind::Implements,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: Vec::new(),
                    },
                    Edge {
                        source: "room::KRoomPage::chatRoomFragViewPanel".to_string(),
                        target: "room::KRoomPageExt".to_string(),
                        kind: EdgeKind::Implements,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: Vec::new(),
                    },
                ],
                imports: vec![],
            },
        ]);

        let type_refs: Vec<_> = graph
            .edges
            .iter()
            .filter(|edge| edge.source == source && edge.kind == EdgeKind::TypeRef)
            .collect();

        assert_eq!(type_refs.len(), 1);
        assert_eq!(type_refs[0].target, "room::RoomPage::chatRoomFragViewPanel");
        assert!((type_refs[0].confidence - 0.85).abs() < 0.001);
    }
}
