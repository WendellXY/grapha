use std::collections::{HashMap, HashSet};

use serde::Serialize;

use grapha_core::graph::{EdgeKind, FlowDirection, Graph, NodeRole, TerminalKind};

use super::{QueryResolveError, SymbolRef};

#[derive(Debug, Serialize)]
pub struct TraceResult {
    pub entry: String,
    pub flows: Vec<Flow>,
    pub summary: TraceSummary,
    #[serde(skip)]
    pub(crate) entry_ref: SymbolRef,
}

#[derive(Debug, Serialize)]
pub struct Flow {
    pub path: Vec<String>,
    pub terminal: Option<TerminalInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub async_boundaries: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TerminalInfo {
    pub kind: String,
    pub operation: String,
    pub direction: String,
}

#[derive(Debug, Serialize)]
pub struct TraceSummary {
    pub total_flows: usize,
    pub reads: usize,
    pub writes: usize,
    pub async_crossings: usize,
}

fn is_dataflow_edge(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Calls
            | EdgeKind::Reads
            | EdgeKind::Writes
            | EdgeKind::Publishes
            | EdgeKind::Subscribes
    )
}

fn terminal_kind_to_string(kind: &TerminalKind) -> String {
    match kind {
        TerminalKind::Network => "network".to_string(),
        TerminalKind::Persistence => "persistence".to_string(),
        TerminalKind::Cache => "cache".to_string(),
        TerminalKind::Event => "event".to_string(),
        TerminalKind::Keychain => "keychain".to_string(),
        TerminalKind::Search => "search".to_string(),
    }
}

fn direction_from_edge(edge_kind: EdgeKind, direction: Option<&FlowDirection>) -> String {
    match direction {
        Some(FlowDirection::Read) => "read".to_string(),
        Some(FlowDirection::Write) => "write".to_string(),
        Some(FlowDirection::ReadWrite) => "read_write".to_string(),
        Some(FlowDirection::Pure) => "pure".to_string(),
        None => match edge_kind {
            EdgeKind::Reads => "read".to_string(),
            EdgeKind::Writes => "write".to_string(),
            _ => "unknown".to_string(),
        },
    }
}

pub fn query_trace(
    graph: &Graph,
    entry: &str,
    max_depth: usize,
) -> Result<TraceResult, QueryResolveError> {
    let entry_node = crate::query::resolve_node(&graph.nodes, entry)?;

    let node_index: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // Build forward adjacency: source -> [(target_id, edge_index)]
    let mut forward_adj: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
    for (ei, edge) in graph.edges.iter().enumerate() {
        if is_dataflow_edge(edge.kind) {
            forward_adj
                .entry(&edge.source)
                .or_default()
                .push((&edge.target, ei));
        }
    }

    let mut flows = Vec::new();
    let mut total_async_crossings = 0;

    // DFS with explicit stack
    // Stack items: (node_id, path, conditions, async_boundaries, visited_edge_pairs)
    struct StackFrame<'a> {
        node_id: &'a str,
        path: Vec<String>,
        conditions: Vec<String>,
        async_boundaries: Vec<String>,
        visited_edges: HashSet<(&'a str, &'a str)>,
    }

    let mut stack: Vec<StackFrame> = vec![StackFrame {
        node_id: &entry_node.id,
        path: vec![entry_node.name.clone()],
        conditions: Vec::new(),
        async_boundaries: Vec::new(),
        visited_edges: HashSet::new(),
    }];

    while let Some(frame) = stack.pop() {
        if frame.path.len() > max_depth + 1 {
            continue;
        }

        // Check if current node is a terminal (but not the entry itself)
        if frame.path.len() > 1
            && let Some(&ni) = node_index.get(frame.node_id)
        {
            let node = &graph.nodes[ni];
            if let Some(NodeRole::Terminal { kind }) = &node.role {
                // Determine direction from the last edge that led here
                let last_edge_direction = graph
                    .edges
                    .iter()
                    .find(|e| e.target == frame.node_id && is_dataflow_edge(e.kind));
                let direction = last_edge_direction
                    .map(|e| direction_from_edge(e.kind, e.direction.as_ref()))
                    .unwrap_or_else(|| "unknown".to_string());
                let operation = last_edge_direction
                    .and_then(|e| e.operation.clone())
                    .unwrap_or_else(|| "unknown".to_string());

                flows.push(Flow {
                    path: frame.path,
                    terminal: Some(TerminalInfo {
                        kind: terminal_kind_to_string(kind),
                        operation,
                        direction,
                    }),
                    conditions: frame.conditions,
                    async_boundaries: frame.async_boundaries,
                });
                continue;
            }
        }

        // Continue traversal
        if let Some(neighbors) = forward_adj.get(frame.node_id) {
            for &(target_id, ei) in neighbors {
                let edge_pair = (frame.node_id, target_id);
                if frame.visited_edges.contains(&edge_pair) {
                    continue;
                }

                let edge = &graph.edges[ei];
                let target_node = node_index.get(target_id).map(|&i| &graph.nodes[i]);

                let target_name = target_node
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| target_id.to_string());

                let mut new_path = frame.path.clone();
                new_path.push(target_name);

                let mut new_conditions = frame.conditions.clone();
                if let Some(cond) = &edge.condition {
                    new_conditions.push(cond.clone());
                }

                let mut new_async = frame.async_boundaries.clone();
                if edge.async_boundary == Some(true) {
                    let boundary_label = format!("{} -> {}", frame.node_id, target_id);
                    new_async.push(boundary_label);
                }

                let mut new_visited = frame.visited_edges.clone();
                new_visited.insert(edge_pair);

                stack.push(StackFrame {
                    node_id: target_id,
                    path: new_path,
                    conditions: new_conditions,
                    async_boundaries: new_async,
                    visited_edges: new_visited,
                });
            }
        }
    }

    // Compute summary
    let mut reads = 0;
    let mut writes = 0;
    for flow in &flows {
        if let Some(terminal) = &flow.terminal {
            match terminal.direction.as_str() {
                "read" => reads += 1,
                "write" => writes += 1,
                "read_write" => {
                    reads += 1;
                    writes += 1;
                }
                _ => {}
            }
        }
        total_async_crossings += flow.async_boundaries.len();
    }

    let summary = TraceSummary {
        total_flows: flows.len(),
        reads,
        writes,
        async_crossings: total_async_crossings,
    };

    Ok(TraceResult {
        entry: entry_node.id.clone(),
        flows,
        summary,
        entry_ref: SymbolRef {
            id: entry_node.id.clone(),
            name: entry_node.name.clone(),
            kind: entry_node.kind,
            file: entry_node.file.to_string_lossy().to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::*;
    use std::collections::HashMap as StdHashMap;
    use std::path::PathBuf;

    fn make_node(id: &str, role: Option<NodeRole>) -> Node {
        Node {
            id: id.into(),
            kind: NodeKind::Function,
            name: id.into(),
            file: PathBuf::from("test.rs"),
            span: Span {
                start: [0, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: StdHashMap::new(),
            role,
            signature: None,
            doc_comment: None,
            module: None,
        }
    }

    fn make_edge(source: &str, target: &str, kind: EdgeKind) -> Edge {
        Edge {
            source: source.into(),
            target: target.into(),
            kind,
            confidence: 0.9,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
        }
    }

    #[test]
    fn traces_entry_to_terminal() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("entry", Some(NodeRole::EntryPoint)),
                make_node("service", None),
                make_node(
                    "db_save",
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Persistence,
                    }),
                ),
            ],
            edges: vec![make_edge("entry", "service", EdgeKind::Calls), {
                let mut e = make_edge("service", "db_save", EdgeKind::Writes);
                e.direction = Some(FlowDirection::Write);
                e.operation = Some("save".to_string());
                e
            }],
        };

        let result = query_trace(&graph, "entry", 10).unwrap();
        assert_eq!(result.entry, "entry");
        assert_eq!(result.flows.len(), 1);

        let flow = &result.flows[0];
        assert_eq!(flow.path, vec!["entry", "service", "db_save"]);
        let terminal = flow.terminal.as_ref().unwrap();
        assert_eq!(terminal.kind, "persistence");
        assert_eq!(terminal.operation, "save");
        assert_eq!(terminal.direction, "write");

        assert_eq!(result.summary.total_flows, 1);
        assert_eq!(result.summary.writes, 1);
        assert_eq!(result.summary.reads, 0);
    }

    #[test]
    fn captures_conditions_on_edges() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("entry", Some(NodeRole::EntryPoint)),
                make_node(
                    "db",
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Persistence,
                    }),
                ),
            ],
            edges: vec![{
                let mut e = make_edge("entry", "db", EdgeKind::Writes);
                e.condition = Some("user.isAdmin".to_string());
                e.direction = Some(FlowDirection::Write);
                e.operation = Some("INSERT".to_string());
                e
            }],
        };

        let result = query_trace(&graph, "entry", 10).unwrap();
        assert_eq!(result.flows.len(), 1);
        assert_eq!(result.flows[0].conditions, vec!["user.isAdmin"]);
    }

    #[test]
    fn returns_none_for_unknown_entry() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![make_node("a", None)],
            edges: vec![],
        };
        assert!(matches!(
            query_trace(&graph, "nonexistent", 10),
            Err(QueryResolveError::NotFound { .. })
        ));
    }

    #[test]
    fn ignores_swiftui_structural_edges() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                Node {
                    id: "body".into(),
                    kind: NodeKind::Property,
                    name: "body".into(),
                    file: PathBuf::from("ContentView.swift"),
                    span: Span {
                        start: [0, 0],
                        end: [1, 0],
                    },
                    visibility: Visibility::Public,
                    metadata: StdHashMap::new(),
                    role: Some(NodeRole::EntryPoint),
                    signature: None,
                    doc_comment: None,
                    module: None,
                },
                Node {
                    id: "body::view:VStack@1:0".into(),
                    kind: NodeKind::View,
                    name: "VStack".into(),
                    file: PathBuf::from("ContentView.swift"),
                    span: Span {
                        start: [1, 0],
                        end: [2, 0],
                    },
                    visibility: Visibility::Private,
                    metadata: StdHashMap::new(),
                    role: None,
                    signature: None,
                    doc_comment: None,
                    module: None,
                },
                make_node(
                    "db_save",
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Persistence,
                    }),
                ),
            ],
            edges: vec![
                make_edge("body", "body::view:VStack@1:0", EdgeKind::Contains),
                make_edge("body::view:VStack@1:0", "db_save", EdgeKind::TypeRef),
            ],
        };

        let result = query_trace(&graph, "body", 10).unwrap();
        assert!(result.flows.is_empty());
        assert_eq!(result.summary.total_flows, 0);
    }

    #[test]
    fn respects_max_depth() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("a", Some(NodeRole::EntryPoint)),
                make_node("b", None),
                make_node("c", None),
                make_node(
                    "d",
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Network,
                    }),
                ),
            ],
            edges: vec![
                make_edge("a", "b", EdgeKind::Calls),
                make_edge("b", "c", EdgeKind::Calls),
                {
                    let mut e = make_edge("c", "d", EdgeKind::Reads);
                    e.direction = Some(FlowDirection::Read);
                    e.operation = Some("fetch".to_string());
                    e
                },
            ],
        };

        // max_depth=2 means path can be at most 3 nodes (entry + 2 hops)
        // a->b->c is 2 hops, but c->d is hop 3, so depth 1 won't reach d
        let result = query_trace(&graph, "a", 1).unwrap();
        assert_eq!(result.flows.len(), 0);

        // depth 3 should reach d
        let result = query_trace(&graph, "a", 5).unwrap();
        assert_eq!(result.flows.len(), 1);
    }

    #[test]
    fn captures_async_boundaries() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![
                make_node("entry", Some(NodeRole::EntryPoint)),
                make_node(
                    "api",
                    Some(NodeRole::Terminal {
                        kind: TerminalKind::Network,
                    }),
                ),
            ],
            edges: vec![{
                let mut e = make_edge("entry", "api", EdgeKind::Reads);
                e.async_boundary = Some(true);
                e.direction = Some(FlowDirection::Read);
                e.operation = Some("fetch".to_string());
                e
            }],
        };

        let result = query_trace(&graph, "entry", 10).unwrap();
        assert_eq!(result.summary.async_crossings, 1);
        assert_eq!(result.flows[0].async_boundaries.len(), 1);
    }
}
