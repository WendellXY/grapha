use std::collections::HashMap;

use grapha_core::graph::{Edge, EdgeKind, FlowDirection, Graph, Node};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    FullRebuild,
    Incremental,
}

impl SyncMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::FullRebuild => "full_rebuild",
            Self::Incremental => "incremental",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EntitySyncStats {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
}

impl EntitySyncStats {
    pub fn from_total(total: usize) -> Self {
        Self {
            added: total,
            updated: 0,
            deleted: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EdgeDelta<'a> {
    pub id: String,
    pub edge: &'a Edge,
}

#[derive(Debug)]
pub struct GraphDelta<'a> {
    pub added_nodes: Vec<&'a Node>,
    pub updated_nodes: Vec<&'a Node>,
    pub deleted_node_ids: Vec<String>,
    pub added_edges: Vec<EdgeDelta<'a>>,
    pub updated_edges: Vec<EdgeDelta<'a>>,
    pub deleted_edge_ids: Vec<String>,
}

impl<'a> GraphDelta<'a> {
    pub fn between(previous: &'a Graph, next: &'a Graph) -> Self {
        let previous_nodes: HashMap<&str, &Node> = previous
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        let next_nodes: HashMap<&str, &Node> = next
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();

        let mut added_nodes = Vec::new();
        let mut updated_nodes = Vec::new();
        let mut deleted_node_ids = Vec::new();

        for node in &next.nodes {
            match previous_nodes.get(node.id.as_str()) {
                None => added_nodes.push(node),
                Some(previous_node) if *previous_node != node => updated_nodes.push(node),
                Some(_) => {}
            }
        }

        for node in &previous.nodes {
            if !next_nodes.contains_key(node.id.as_str()) {
                deleted_node_ids.push(node.id.clone());
            }
        }

        let previous_edges: HashMap<String, &Edge> = previous
            .edges
            .iter()
            .map(|edge| (edge_fingerprint(edge), edge))
            .collect();
        let next_edges: HashMap<String, &Edge> = next
            .edges
            .iter()
            .map(|edge| (edge_fingerprint(edge), edge))
            .collect();

        let mut added_edges = Vec::new();
        let mut updated_edges = Vec::new();
        let mut deleted_edge_ids = Vec::new();

        for edge in &next.edges {
            let edge_id = edge_fingerprint(edge);
            match previous_edges.get(edge_id.as_str()) {
                None => added_edges.push(EdgeDelta { id: edge_id, edge }),
                Some(previous_edge) if *previous_edge != edge => {
                    updated_edges.push(EdgeDelta { id: edge_id, edge })
                }
                Some(_) => {}
            }
        }

        for edge in &previous.edges {
            let edge_id = edge_fingerprint(edge);
            if !next_edges.contains_key(edge_id.as_str()) {
                deleted_edge_ids.push(edge_id);
            }
        }

        added_nodes.sort_by(|left, right| left.id.cmp(&right.id));
        updated_nodes.sort_by(|left, right| left.id.cmp(&right.id));
        deleted_node_ids.sort();
        added_edges.sort_by(|left, right| left.id.cmp(&right.id));
        updated_edges.sort_by(|left, right| left.id.cmp(&right.id));
        deleted_edge_ids.sort();

        Self {
            added_nodes,
            updated_nodes,
            deleted_node_ids,
            added_edges,
            updated_edges,
            deleted_edge_ids,
        }
    }

    pub fn node_stats(&self) -> EntitySyncStats {
        EntitySyncStats {
            added: self.added_nodes.len(),
            updated: self.updated_nodes.len(),
            deleted: self.deleted_node_ids.len(),
        }
    }

    pub fn edge_stats(&self) -> EntitySyncStats {
        EntitySyncStats {
            added: self.added_edges.len(),
            updated: self.updated_edges.len(),
            deleted: self.deleted_edge_ids.len(),
        }
    }
}

pub fn edge_fingerprint(edge: &Edge) -> String {
    let mut hasher = Fnv1a64::default();
    hasher.write_component(&edge.source);
    hasher.write_component(&edge.target);
    hasher.write_component(edge_kind_tag(edge.kind));
    hasher.write_component(direction_tag(edge.direction.as_ref()));
    hasher.write_component(edge.operation.as_deref().unwrap_or(""));
    hasher.write_component(edge.condition.as_deref().unwrap_or(""));
    hasher.write_component(bool_tag(edge.async_boundary));
    format!("{:016x}", hasher.finish())
}

fn edge_kind_tag(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Calls => "calls",
        EdgeKind::Uses => "uses",
        EdgeKind::Implements => "implements",
        EdgeKind::Contains => "contains",
        EdgeKind::TypeRef => "type_ref",
        EdgeKind::Inherits => "inherits",
        EdgeKind::Reads => "reads",
        EdgeKind::Writes => "writes",
        EdgeKind::Publishes => "publishes",
        EdgeKind::Subscribes => "subscribes",
    }
}

fn direction_tag(direction: Option<&FlowDirection>) -> &'static str {
    match direction {
        Some(FlowDirection::Read) => "read",
        Some(FlowDirection::Write) => "write",
        Some(FlowDirection::ReadWrite) => "read_write",
        Some(FlowDirection::Pure) => "pure",
        None => "",
    }
}

fn bool_tag(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "1",
        Some(false) => "0",
        None => "",
    }
}

#[derive(Default)]
struct Fnv1a64 {
    state: u64,
}

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    fn write_component(&mut self, value: &str) {
        if self.state == 0 {
            self.state = Self::OFFSET_BASIS;
        }
        for byte in value.as_bytes() {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(Self::PRIME);
        }
        self.state ^= u64::from(0xff_u8);
        self.state = self.state.wrapping_mul(Self::PRIME);
    }

    fn finish(self) -> u64 {
        if self.state == 0 {
            Self::OFFSET_BASIS
        } else {
            self.state
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use grapha_core::graph::{NodeKind, Span, Visibility};

    use super::*;

    fn node(id: &str, name: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Function,
            name: name.to_string(),
            file: PathBuf::from("main.rs"),
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
        }
    }

    fn edge(source: &str, target: &str, confidence: f64) -> Edge {
        Edge {
            source: source.to_string(),
            target: target.to_string(),
            kind: EdgeKind::Calls,
            confidence,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: Vec::new(),
        }
    }

    #[test]
    fn fingerprint_ignores_confidence() {
        let left = edge("a", "b", 0.8);
        let right = edge("a", "b", 0.9);
        assert_eq!(edge_fingerprint(&left), edge_fingerprint(&right));
    }

    #[test]
    fn graph_delta_tracks_node_and_edge_changes() {
        let previous = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![node("a", "a"), node("b", "b")],
            edges: vec![edge("a", "b", 0.8)],
        };
        let mut changed = node("a", "renamed");
        changed.signature = Some("fn renamed()".to_string());
        let next = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![changed, node("c", "c")],
            edges: vec![
                edge("a", "b", 0.9),
                Edge {
                    source: "a".to_string(),
                    target: "c".to_string(),
                    kind: EdgeKind::Calls,
                    confidence: 0.7,
                    direction: Some(FlowDirection::Pure),
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: Vec::new(),
                },
            ],
        };

        let delta = GraphDelta::between(&previous, &next);
        assert_eq!(
            delta.node_stats(),
            EntitySyncStats {
                added: 1,
                updated: 1,
                deleted: 1,
            }
        );
        assert_eq!(
            delta.edge_stats(),
            EntitySyncStats {
                added: 1,
                updated: 1,
                deleted: 0,
            }
        );
    }
}
