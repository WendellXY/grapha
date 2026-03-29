pub mod context;
pub mod impact;

use serde::Serialize;

use crate::graph::NodeKind;

#[derive(Debug, Serialize)]
pub struct ContextResult {
    pub symbol: SymbolInfo,
    pub callers: Vec<SymbolRef>,
    pub callees: Vec<SymbolRef>,
    pub implementors: Vec<SymbolRef>,
    pub implements: Vec<SymbolRef>,
    pub type_refs: Vec<SymbolRef>,
}

#[derive(Debug, Serialize)]
pub struct SymbolInfo {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub file: String,
    pub span: [usize; 2],
}

#[derive(Debug, Serialize)]
pub struct SymbolRef {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub file: String,
}
