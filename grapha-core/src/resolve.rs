use serde::{Deserialize, Serialize};

/// A structured import declaration extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The raw import path as written in source (e.g., "std::collections::HashMap")
    pub path: String,
    /// Specific symbols imported (empty = wildcard/module import)
    pub symbols: Vec<String>,
    /// The kind of import
    pub kind: ImportKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    /// Named import: `use std::collections::HashMap;`
    Named,
    /// Wildcard/glob import: `use std::collections::*;`
    Wildcard,
    /// Module import: `import Foundation` (Swift)
    Module,
    /// Relative import: `use super::foo;`, `use crate::bar;`
    Relative,
}
