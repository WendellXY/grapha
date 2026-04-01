use std::collections::{HashMap, HashSet};

use grapha_core::GraphPass;
use grapha_core::graph::{EdgeKind, FlowDirection, Graph, NodeRole, TerminalKind};

pub struct SwiftGraphPass;

impl GraphPass for SwiftGraphPass {
    fn apply(&self, graph: Graph) -> Graph {
        classify_swift_usr_targets(graph)
    }
}

fn classify_swift_usr_targets(graph: Graph) -> Graph {
    let node_ids: HashSet<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
    let mut terminal_nodes: HashMap<String, TerminalKind> = HashMap::new();
    let entry_patterns: &[&str] = &["SwiftUI", "ObservableObjectP", "10ObservableP"];
    let mut entry_point_nodes = HashSet::new();

    let edges = graph
        .edges
        .iter()
        .map(|edge| {
            if edge.kind == EdgeKind::Implements
                && entry_patterns
                    .iter()
                    .any(|pattern| edge.target.contains(pattern))
            {
                entry_point_nodes.insert(edge.source.clone());
            }

            if edge.kind != EdgeKind::Calls || edge.direction.is_some() {
                return edge.clone();
            }

            let Some((kind, direction, operation)) = classify_by_module(&edge.target) else {
                return edge.clone();
            };

            let terminal_node_id = if !node_ids.contains(edge.target.as_str()) {
                edge.source.clone()
            } else {
                edge.target.clone()
            };
            terminal_nodes.entry(terminal_node_id).or_insert(kind);

            let mut enriched = edge.clone();
            enriched.direction = Some(direction);
            enriched.operation = Some(operation);
            enriched
        })
        .collect();

    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            let mut enriched = node.clone();
            if let Some(kind) = terminal_nodes.get(&node.id)
                && enriched.role.is_none()
            {
                enriched.role = Some(NodeRole::Terminal { kind: *kind });
            } else if entry_point_nodes.contains(&node.id) && enriched.role.is_none() {
                enriched.role = Some(NodeRole::EntryPoint);
            }
            enriched
        })
        .collect();

    Graph {
        version: graph.version,
        nodes,
        edges,
    }
}

fn module_from_usr(usr: &str) -> Option<&str> {
    let rest = usr.strip_prefix("s:")?;
    let len_end = rest.find(|ch: char| !ch.is_ascii_digit())?;
    let len: usize = rest[..len_end].parse().ok()?;
    let name_start = len_end;
    if name_start + len <= rest.len() {
        Some(&rest[name_start..name_start + len])
    } else {
        None
    }
}

fn classify_by_module(target_usr: &str) -> Option<(TerminalKind, FlowDirection, String)> {
    let module = module_from_usr(target_usr)?;
    match module {
        "FrameNetwork" | "FrameNetworkCore" | "Moya" | "Alamofire" => Some((
            TerminalKind::Network,
            FlowDirection::ReadWrite,
            "request".to_string(),
        )),
        "FrameStorage"
        | "FrameStorageCore"
        | "FrameStorageDatabase"
        | "GRDB"
        | "CoreData"
        | "RealmSwift" => Some((
            TerminalKind::Persistence,
            FlowDirection::ReadWrite,
            "db".to_string(),
        )),
        "FrameDownload" | "Tiercel" => Some((
            TerminalKind::Persistence,
            FlowDirection::Write,
            "download".to_string(),
        )),
        "FrameResources" | "AppResource" | "Kingfisher" | "SDWebImageSwiftUI" | "FrameImage" => {
            Some((
                TerminalKind::Cache,
                FlowDirection::Read,
                "resource".to_string(),
            ))
        }
        "FrameWebView" | "WEKit" => Some((
            TerminalKind::Event,
            FlowDirection::ReadWrite,
            "webview".to_string(),
        )),
        "FrameStat" => Some((
            TerminalKind::Event,
            FlowDirection::Write,
            "stat".to_string(),
        )),
        "FrameMedia" | "FrameMediaShared" => Some((
            TerminalKind::Cache,
            FlowDirection::ReadWrite,
            "media".to_string(),
        )),
        "FrameRouter" => Some((
            TerminalKind::Event,
            FlowDirection::Write,
            "navigate".to_string(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::{Edge, Node, NodeKind, Span, Visibility};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn marks_usr_calls_as_terminals() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "local::caller".to_string(),
                kind: NodeKind::Function,
                name: "caller".to_string(),
                file: PathBuf::from("caller.swift"),
                span: Span {
                    start: [0, 0],
                    end: [1, 0],
                },
                visibility: Visibility::Public,
                metadata: HashMap::new(),
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
                snippet: None,
            }],
            edges: vec![Edge {
                source: "local::caller".to_string(),
                target: "s:12FrameNetwork7request".to_string(),
                kind: EdgeKind::Calls,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: Vec::new(),
            }],
        };

        let enriched = classify_swift_usr_targets(graph);
        assert_eq!(
            enriched.nodes[0].role,
            Some(NodeRole::Terminal {
                kind: TerminalKind::Network,
            })
        );
        assert_eq!(enriched.edges[0].operation.as_deref(), Some("request"));
    }
}
