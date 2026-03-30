use std::collections::HashMap;

use grapha_core::graph::{EdgeKind, Graph};

use super::{ContextResult, SymbolInfo, SymbolRef};

pub fn query_context(graph: &Graph, query: &str) -> Option<ContextResult> {
    let node = graph
        .nodes
        .iter()
        .find(|n| n.id == query || n.name == query)?;

    let node_index: HashMap<&str, &grapha_core::graph::Node> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let mut callers = Vec::new();
    let mut callees = Vec::new();
    let mut implementors = Vec::new();
    let mut implements = Vec::new();
    let mut type_refs = Vec::new();

    for edge in &graph.edges {
        if edge.source == node.id
            && let Some(target) = node_index.get(edge.target.as_str())
        {
            let sym_ref = SymbolRef {
                id: target.id.clone(),
                name: target.name.clone(),
                kind: target.kind,
                file: target.file.to_string_lossy().to_string(),
            };
            match edge.kind {
                EdgeKind::Calls => callees.push(sym_ref),
                EdgeKind::Implements => implements.push(sym_ref),
                EdgeKind::TypeRef => type_refs.push(sym_ref),
                _ => {}
            }
        }
        if edge.target == node.id
            && let Some(source) = node_index.get(edge.source.as_str())
        {
            let sym_ref = SymbolRef {
                id: source.id.clone(),
                name: source.name.clone(),
                kind: source.kind,
                file: source.file.to_string_lossy().to_string(),
            };
            match edge.kind {
                EdgeKind::Calls => callers.push(sym_ref),
                EdgeKind::Implements => implementors.push(sym_ref),
                _ => {}
            }
        }
    }

    Some(ContextResult {
        symbol: SymbolInfo {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind,
            file: node.file.to_string_lossy().to_string(),
            span: [node.span.start[0], node.span.end[0]],
        },
        callers,
        callees,
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
    fn context_returns_none_for_unknown() {
        let graph = make_graph();
        assert!(query_context(&graph, "nonexistent").is_none());
    }
}
