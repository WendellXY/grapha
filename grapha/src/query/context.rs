use std::collections::{HashMap, HashSet};

use grapha_core::graph::{EdgeKind, Graph};

use super::{
    ContextResult, QueryResolveError, SymbolInfo, SymbolRef, SymbolTreeRef,
    is_swiftui_invalidation_source,
};

fn to_symbol_ref(node: &grapha_core::graph::Node) -> SymbolRef {
    SymbolRef {
        id: node.id.clone(),
        name: node.name.clone(),
        kind: node.kind,
        file: node.file.to_string_lossy().to_string(),
    }
}

fn to_symbol_tree_ref(
    node: &grapha_core::graph::Node,
    contains: Vec<SymbolTreeRef>,
) -> SymbolTreeRef {
    SymbolTreeRef {
        id: node.id.clone(),
        name: node.name.clone(),
        kind: node.kind,
        file: node.file.to_string_lossy().to_string(),
        contains,
    }
}

fn sort_refs_by_name(symbols: &mut [SymbolRef]) {
    symbols.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn sort_ids_by_span<'a>(
    node_ids: &mut [&'a str],
    node_index: &HashMap<&'a str, &'a grapha_core::graph::Node>,
) {
    node_ids.sort_by(
        |left, right| match (node_index.get(*left), node_index.get(*right)) {
            (Some(left_node), Some(right_node)) => left_node
                .span
                .start
                .cmp(&right_node.span.start)
                .then_with(|| left_node.span.end.cmp(&right_node.span.end))
                .then_with(|| left_node.name.cmp(&right_node.name))
                .then_with(|| left_node.id.cmp(&right_node.id)),
            _ => left.cmp(right),
        },
    );
}

fn build_contains_tree<'a>(
    node_id: &'a str,
    contains_adj: &HashMap<&'a str, Vec<&'a str>>,
    type_ref_adj: &HashMap<&'a str, Vec<&'a str>>,
    node_index: &HashMap<&'a str, &'a grapha_core::graph::Node>,
    ancestors: &mut Vec<&'a str>,
) -> Option<SymbolTreeRef> {
    if ancestors.contains(&node_id) {
        return None;
    }

    let node = node_index.get(node_id).copied()?;
    ancestors.push(node_id);

    let mut child_ids = contains_adj.get(node_id).cloned().unwrap_or_default();
    sort_ids_by_span(&mut child_ids, node_index);

    let mut contains: Vec<_> = child_ids
        .into_iter()
        .filter_map(|child_id| {
            build_contains_tree(child_id, contains_adj, type_ref_adj, node_index, ancestors)
        })
        .collect();

    if node.kind == grapha_core::graph::NodeKind::View {
        let mut type_ref_ids = type_ref_adj.get(node_id).cloned().unwrap_or_default();
        sort_ids_by_span(&mut type_ref_ids, node_index);
        for type_ref_id in type_ref_ids {
            let Some(target) = node_index.get(type_ref_id).copied() else {
                continue;
            };
            if !matches!(
                target.kind,
                grapha_core::graph::NodeKind::Property | grapha_core::graph::NodeKind::Function
            ) {
                continue;
            }

            let mut inline_child_ids = contains_adj.get(type_ref_id).cloned().unwrap_or_default();
            sort_ids_by_span(&mut inline_child_ids, node_index);
            contains.extend(inline_child_ids.into_iter().filter_map(|child_id| {
                build_contains_tree(child_id, contains_adj, type_ref_adj, node_index, ancestors)
            }));
        }
    }

    ancestors.pop();
    Some(to_symbol_tree_ref(node, contains))
}

fn collect_invalidation_sources<'a>(
    start_id: &'a str,
    reads_adj: &HashMap<&'a str, Vec<&'a str>>,
    node_index: &HashMap<&'a str, &'a grapha_core::graph::Node>,
) -> Vec<SymbolRef> {
    let mut visited = HashSet::new();
    let mut invalidation_sources = Vec::new();
    let mut stack = vec![start_id];

    while let Some(node_id) = stack.pop() {
        if !visited.insert(node_id) {
            continue;
        }

        let Some(node) = node_index.get(node_id).copied() else {
            continue;
        };
        if is_swiftui_invalidation_source(node) {
            invalidation_sources.push(to_symbol_ref(node));
        }

        if let Some(next_ids) = reads_adj.get(node_id) {
            stack.extend(next_ids.iter().copied());
        }
    }

    invalidation_sources
}

pub fn query_context(graph: &Graph, query: &str) -> Result<ContextResult, QueryResolveError> {
    let node = crate::query::resolve_node(&graph.nodes, query)?;

    let node_index: HashMap<&str, &grapha_core::graph::Node> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut contains_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut type_ref_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut reads_adj: HashMap<&str, Vec<&str>> = HashMap::new();

    let mut callers = Vec::new();
    let mut callees = Vec::new();
    let mut reads = Vec::new();
    let mut read_by = Vec::new();
    let mut contains_ids = Vec::new();
    let mut contained_by_ids = Vec::new();
    let mut implementors = Vec::new();
    let mut implements = Vec::new();
    let mut type_refs = Vec::new();

    for edge in &graph.edges {
        if edge.source == node.id
            && let Some(target) = node_index.get(edge.target.as_str())
        {
            let sym_ref = to_symbol_ref(target);
            match edge.kind {
                EdgeKind::Calls => callees.push(sym_ref),
                EdgeKind::Reads => reads.push(sym_ref),
                EdgeKind::Contains => contains_ids.push(target.id.as_str()),
                EdgeKind::Implements => implements.push(sym_ref),
                EdgeKind::TypeRef => type_refs.push(sym_ref),
                _ => {}
            }
        }
        if edge.target == node.id
            && let Some(source) = node_index.get(edge.source.as_str())
        {
            let sym_ref = to_symbol_ref(source);
            match edge.kind {
                EdgeKind::Calls => callers.push(sym_ref),
                EdgeKind::Reads => read_by.push(sym_ref),
                EdgeKind::Contains => contained_by_ids.push(source.id.as_str()),
                EdgeKind::Implements => implementors.push(sym_ref),
                _ => {}
            }
        }

        if edge.kind == EdgeKind::Contains {
            contains_adj
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
        } else if edge.kind == EdgeKind::Reads {
            reads_adj
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
        } else if edge.kind == EdgeKind::TypeRef {
            type_ref_adj
                .entry(edge.source.as_str())
                .or_default()
                .push(edge.target.as_str());
        }
    }

    sort_refs_by_name(&mut callers);
    sort_refs_by_name(&mut callees);
    sort_refs_by_name(&mut reads);
    sort_refs_by_name(&mut read_by);
    sort_refs_by_name(&mut implementors);
    sort_refs_by_name(&mut implements);
    sort_refs_by_name(&mut type_refs);
    let mut invalidation_sources = collect_invalidation_sources(&node.id, &reads_adj, &node_index);
    sort_refs_by_name(&mut invalidation_sources);
    sort_ids_by_span(&mut contains_ids, &node_index);
    sort_ids_by_span(&mut contained_by_ids, &node_index);

    let contains_tree = contains_ids
        .iter()
        .filter_map(|node_id| {
            build_contains_tree(
                node_id,
                &contains_adj,
                &type_ref_adj,
                &node_index,
                &mut vec![],
            )
        })
        .collect();
    let contains = contains_ids
        .into_iter()
        .filter_map(|node_id| node_index.get(node_id).copied())
        .map(to_symbol_ref)
        .collect();
    let contained_by = contained_by_ids
        .into_iter()
        .filter_map(|node_id| node_index.get(node_id).copied())
        .map(to_symbol_ref)
        .collect();

    Ok(ContextResult {
        symbol: SymbolInfo {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind,
            file: node.file.to_string_lossy().to_string(),
            span: [node.span.start[0], node.span.end[0]],
        },
        callers,
        callees,
        reads,
        read_by,
        invalidation_sources,
        contains,
        contains_tree,
        contained_by,
        implementors,
        implements,
        type_refs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
    use std::collections::HashMap as StdHashMap;

    fn make_graph() -> Graph {
        Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "a.rs::main".into(),
                    kind: NodeKind::Function,
                    name: "main".into(),
                    file: "a.rs".into(),
                    span: Span {
                        start: [0, 0],
                        end: [10, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "a.rs::helper".into(),
                    kind: NodeKind::Function,
                    name: "helper".into(),
                    file: "a.rs".into(),
                    span: Span {
                        start: [12, 0],
                        end: [15, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
            ],
            edges: vec![Edge {
                source: "a.rs::main".into(),
                target: "a.rs::helper".into(),
                kind: EdgeKind::Calls,
                confidence: 0.9,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        }
    }

    #[test]
    fn context_finds_callers_and_callees() {
        let graph = make_graph();
        let ctx = query_context(&graph, "main").unwrap();
        assert_eq!(ctx.callees.len(), 1);
        assert_eq!(ctx.callees[0].name, "helper");
        assert_eq!(ctx.callers.len(), 0);

        let ctx2 = query_context(&graph, "helper").unwrap();
        assert_eq!(ctx2.callers.len(), 1);
        assert_eq!(ctx2.callers[0].name, "main");
    }

    #[test]
    fn context_tracks_read_dependencies() {
        let mk = |id: &str, name: &str, metadata: StdHashMap<String, String>| Node {
            id: id.into(),
            kind: NodeKind::Property,
            name: name.into(),
            file: "view.swift".into(),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata,
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
            snippet: None,
        };

        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                mk("view.swift::RoomPage::body", "body", StdHashMap::new()),
                mk(
                    "view.swift::RoomPage::canShowGameRoom",
                    "canShowGameRoom",
                    StdHashMap::new(),
                ),
                mk(
                    "view.swift::RoomPage::roomMode",
                    "roomMode",
                    StdHashMap::from([(
                        "swiftui.invalidation_source".to_string(),
                        "true".to_string(),
                    )]),
                ),
            ],
            edges: vec![
                Edge {
                    source: "view.swift::RoomPage::body".into(),
                    target: "view.swift::RoomPage::canShowGameRoom".into(),
                    kind: EdgeKind::Reads,
                    confidence: 0.85,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::RoomPage::canShowGameRoom".into(),
                    target: "view.swift::RoomPage::roomMode".into(),
                    kind: EdgeKind::Reads,
                    confidence: 0.85,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let body_ctx = query_context(&graph, "body").unwrap();
        assert_eq!(body_ctx.reads.len(), 1);
        assert_eq!(body_ctx.reads[0].name, "canShowGameRoom");
        assert_eq!(body_ctx.invalidation_sources.len(), 1);
        assert_eq!(body_ctx.invalidation_sources[0].name, "roomMode");

        let property_ctx = query_context(&graph, "canShowGameRoom").unwrap();
        assert_eq!(property_ctx.read_by.len(), 1);
        assert_eq!(property_ctx.read_by[0].name, "body");
        assert_eq!(property_ctx.invalidation_sources.len(), 1);
        assert_eq!(property_ctx.invalidation_sources[0].name, "roomMode");

        let source_ctx = query_context(&graph, "roomMode").unwrap();
        assert_eq!(source_ctx.invalidation_sources.len(), 1);
        assert_eq!(source_ctx.invalidation_sources[0].name, "roomMode");
    }

    #[test]
    fn context_includes_structural_relationships_in_span_order() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "view.swift::ContentView".into(),
                    kind: NodeKind::Struct,
                    name: "ContentView".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [0, 0],
                        end: [20, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body".into(),
                    kind: NodeKind::Property,
                    name: "body".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [2, 4],
                        end: [12, 4],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: Some(NodeRole::EntryPoint),
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    kind: NodeKind::View,
                    name: "VStack".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [3, 8],
                        end: [10, 9],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body::view:Text@4:12".into(),
                    kind: NodeKind::View,
                    name: "Text".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [4, 12],
                        end: [4, 25],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body::view:Row@5:12".into(),
                    kind: NodeKind::View,
                    name: "Row".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [5, 12],
                        end: [5, 28],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
            ],
            edges: vec![
                Edge {
                    source: "view.swift::ContentView".into(),
                    target: "view.swift::ContentView::body".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body".into(),
                    target: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    target: "view.swift::ContentView::body::view:Text@4:12".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    target: "view.swift::ContentView::body::view:Row@5:12".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let ctx = query_context(&graph, "body").unwrap();
        assert_eq!(ctx.contains.len(), 1);
        assert_eq!(
            ctx.contains
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            vec!["VStack"]
        );
        assert_eq!(ctx.contains_tree.len(), 1);
        assert_eq!(ctx.contains_tree[0].name, "VStack");
        assert_eq!(
            ctx.contains_tree[0]
                .contains
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Text", "Row"]
        );
        assert_eq!(ctx.contained_by.len(), 1);
        assert_eq!(ctx.contained_by[0].name, "ContentView");
    }

    #[test]
    fn context_inlines_property_and_function_view_helpers_but_not_subview_types() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "view.swift::ContentView".into(),
                    kind: NodeKind::Struct,
                    name: "ContentView".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [0, 0],
                        end: [30, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body".into(),
                    kind: NodeKind::Property,
                    name: "body".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [2, 4],
                        end: [16, 4],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: Some(NodeRole::EntryPoint),
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    kind: NodeKind::View,
                    name: "VStack".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [3, 8],
                        end: [12, 9],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body::view:panel@4:12".into(),
                    kind: NodeKind::View,
                    name: "panel".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [4, 12],
                        end: [4, 17],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::panel".into(),
                    kind: NodeKind::Property,
                    name: "panel".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [18, 4],
                        end: [20, 4],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::panel::view:Text@19:8".into(),
                    kind: NodeKind::View,
                    name: "Text".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [19, 8],
                        end: [19, 20],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::helper@5:12".into(),
                    kind: NodeKind::View,
                    name: "helper".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [5, 12],
                        end: [5, 18],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::helper".into(),
                    kind: NodeKind::Function,
                    name: "helper".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [22, 4],
                        end: [24, 4],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::helper::view:Image@23:8".into(),
                    kind: NodeKind::View,
                    name: "Image".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [23, 8],
                        end: [23, 24],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::ContentView::body::view:Row@6:12".into(),
                    kind: NodeKind::View,
                    name: "Row".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [6, 12],
                        end: [6, 26],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
                Node {
                    id: "view.swift::Row".into(),
                    kind: NodeKind::Struct,
                    name: "Row".into(),
                    file: "view.swift".into(),
                    span: Span {
                        start: [26, 0],
                        end: [29, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                    snippet: None,
                },
            ],
            edges: vec![
                Edge {
                    source: "view.swift::ContentView".into(),
                    target: "view.swift::ContentView::body".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body".into(),
                    target: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    target: "view.swift::ContentView::body::view:panel@4:12".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    target: "view.swift::ContentView::helper@5:12".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body::view:VStack@3:8".into(),
                    target: "view.swift::ContentView::body::view:Row@6:12".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::panel".into(),
                    target: "view.swift::ContentView::panel::view:Text@19:8".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::helper".into(),
                    target: "view.swift::ContentView::helper::view:Image@23:8".into(),
                    kind: EdgeKind::Contains,
                    confidence: 1.0,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body::view:panel@4:12".into(),
                    target: "view.swift::ContentView::panel".into(),
                    kind: EdgeKind::TypeRef,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::helper@5:12".into(),
                    target: "view.swift::ContentView::helper".into(),
                    kind: EdgeKind::TypeRef,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
                Edge {
                    source: "view.swift::ContentView::body::view:Row@6:12".into(),
                    target: "view.swift::Row".into(),
                    kind: EdgeKind::TypeRef,
                    confidence: 0.9,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let ctx = query_context(&graph, "body").unwrap();
        let vstack = &ctx.contains_tree[0];
        assert_eq!(vstack.name, "VStack");
        assert_eq!(vstack.contains.len(), 3);
        assert_eq!(vstack.contains[0].name, "panel");
        assert_eq!(vstack.contains[0].contains[0].name, "Text");
        assert_eq!(vstack.contains[1].name, "helper");
        assert_eq!(vstack.contains[1].contains[0].name, "Image");
        assert_eq!(vstack.contains[2].name, "Row");
        assert!(
            vstack.contains[2].contains.is_empty(),
            "custom subview types should stay as component boundaries"
        );
    }

    #[test]
    fn context_returns_none_for_unknown() {
        let graph = make_graph();
        assert!(matches!(
            query_context(&graph, "nonexistent"),
            Err(QueryResolveError::NotFound { .. })
        ));
    }
}
