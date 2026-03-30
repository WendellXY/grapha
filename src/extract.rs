use std::path::Path;

use crate::graph::{Edge, Node};
use crate::resolve::Import;

pub mod rust;
pub mod swift;

#[derive(Debug, Clone)]
pub struct ExtractionResult {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub imports: Vec<Import>,
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
    #[allow(dead_code)]
    fn language(&self) -> &str;
    fn file_extensions(&self) -> &[&str];
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult>;
}
