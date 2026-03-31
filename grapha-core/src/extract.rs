use std::path::Path;

use crate::graph::{Edge, Node};
use crate::resolve::Import;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractionResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub imports: Vec<Import>,
}

impl Default for ExtractionResult {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtractionResult {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            imports: Vec::new(),
        }
    }
}

pub trait LanguageExtractor {
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult>;
}
