use serde::Serialize;

use grapha_core::graph::{Graph, NodeKind};

use super::SymbolRef;

#[derive(Debug, Serialize)]
pub struct FileSymbolsResult {
    pub file: String,
    pub symbols: Vec<FileSymbol>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct FileSymbol {
    #[serde(flatten)]
    pub symbol: SymbolRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub span: [usize; 2],
}

fn role_string(node: &grapha_core::graph::Node) -> Option<String> {
    node.role.as_ref().map(|r| match r {
        grapha_core::graph::NodeRole::EntryPoint => "entry_point".to_string(),
        grapha_core::graph::NodeRole::Terminal { kind } => {
            format!("terminal:{kind:?}").to_lowercase()
        }
        grapha_core::graph::NodeRole::Internal => "internal".to_string(),
    })
}

pub fn query_file_symbols(graph: &Graph, file_query: &str) -> FileSymbolsResult {
    let mut symbols: Vec<FileSymbol> = graph
        .nodes
        .iter()
        .filter(|node| {
            let file_str = node.file.to_string_lossy();
            file_str.ends_with(file_query) || file_str.contains(file_query)
        })
        .filter(|node| !matches!(node.kind, NodeKind::View | NodeKind::Branch))
        .map(|node| FileSymbol {
            symbol: SymbolRef::from_node(node),
            module: node.module.clone(),
            role: role_string(node),
            span: [node.span.start[0], node.span.end[0]],
        })
        .collect();

    symbols.sort_by(|a, b| {
        a.span[0]
            .cmp(&b.span[0])
            .then_with(|| a.symbol.name.cmp(&b.symbol.name))
    });

    let total = symbols.len();
    FileSymbolsResult {
        file: file_query.to_string(),
        symbols,
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::{Node, Span, Visibility};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, name: &str, kind: NodeKind, file: &str) -> Node {
        Node {
            id: id.into(),
            kind,
            name: name.into(),
            file: PathBuf::from(file),
            span: Span {
                start: [10, 0],
                end: [20, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: Some("Room".into()),
            snippet: None,
        }
    }

    #[test]
    fn finds_symbols_by_file_suffix() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                make_node("a", "Foo", NodeKind::Struct, "src/Foo.swift"),
                make_node("b", "Bar", NodeKind::Function, "src/Bar.swift"),
                make_node("c", "Baz", NodeKind::Property, "src/Foo.swift"),
            ],
            edges: vec![],
        };

        let result = query_file_symbols(&graph, "Foo.swift");
        assert_eq!(result.total, 2);
        // Same span, so sorted by name: Baz < Foo
        assert_eq!(result.symbols[0].symbol.name, "Baz");
        assert_eq!(result.symbols[1].symbol.name, "Foo");
    }

    #[test]
    fn excludes_view_and_branch_nodes() {
        let graph = Graph {
            version: String::new(),
            nodes: vec![
                make_node("a", "Foo", NodeKind::Struct, "src/Foo.swift"),
                make_node("v", "VStack", NodeKind::View, "src/Foo.swift"),
                make_node("br", "if x", NodeKind::Branch, "src/Foo.swift"),
            ],
            edges: vec![],
        };

        let result = query_file_symbols(&graph, "Foo.swift");
        assert_eq!(result.total, 1);
    }
}
