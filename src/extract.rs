use std::path::Path;

use crate::graph::{Edge, Node};

pub mod rust;

#[derive(Debug, Clone)]
pub struct ExtractionResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl ExtractionResult {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }
}

pub trait LanguageExtractor {
    #[allow(dead_code)]
    fn language(&self) -> &str;
    fn file_extensions(&self) -> &[&str];
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult>;
}
