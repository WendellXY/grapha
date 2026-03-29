use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Field,
    Variant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Calls,
    Uses,
    Implements,
    Contains,
    TypeRef,
    Inherits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Crate,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: [usize; 2],
    pub end: [usize; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub file: PathBuf,
    pub span: Span,
    pub visibility: Visibility,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub kind: EdgeKind,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub version: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn new() -> Self {
        Self {
            version: "0.1.0".to_string(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&NodeKind::Function).unwrap();
        assert_eq!(json, "\"function\"");

        let json = serde_json::to_string(&NodeKind::Struct).unwrap();
        assert_eq!(json, "\"struct\"");
    }

    #[test]
    fn edge_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&EdgeKind::TypeRef).unwrap();
        assert_eq!(json, "\"type_ref\"");
    }

    #[test]
    fn visibility_serializes_as_snake_case() {
        let json = serde_json::to_string(&Visibility::Public).unwrap();
        assert_eq!(json, "\"public\"");
    }

    #[test]
    fn span_serializes_as_arrays() {
        let span = Span {
            start: [10, 0],
            end: [15, 1],
        };
        let json = serde_json::to_string(&span).unwrap();
        assert_eq!(json, r#"{"start":[10,0],"end":[15,1]}"#);
    }

    #[test]
    fn graph_serializes_with_version() {
        let graph = Graph::new();
        let json = serde_json::to_string_pretty(&graph).unwrap();
        assert!(json.contains("\"version\": \"0.1.0\""));
        assert!(json.contains("\"nodes\": []"));
        assert!(json.contains("\"edges\": []"));
    }

    #[test]
    fn edge_serializes_with_confidence() {
        let edge = Edge {
            source: "a".to_string(),
            target: "b".to_string(),
            kind: EdgeKind::Calls,
            confidence: 0.95,
        };
        let json = serde_json::to_string(&edge).unwrap();
        assert!(json.contains("\"confidence\":0.95"));
    }

    #[test]
    fn full_graph_round_trips() {
        let graph = Graph {
            version: "0.1.0".to_string(),
            nodes: vec![Node {
                id: "src/main.rs::main".to_string(),
                kind: NodeKind::Function,
                name: "main".to_string(),
                file: PathBuf::from("src/main.rs"),
                span: Span {
                    start: [0, 0],
                    end: [3, 1],
                },
                visibility: Visibility::Private,
                metadata: HashMap::new(),
            }],
            edges: vec![Edge {
                source: "src/main.rs::main".to_string(),
                target: "src/main.rs::helper".to_string(),
                kind: EdgeKind::Calls,
                confidence: 0.8,
            }],
        };
        let json = serde_json::to_string(&graph).unwrap();
        let deserialized: Graph = serde_json::from_str(&json).unwrap();
        assert_eq!(graph, deserialized);
    }
}
