mod bridge;
mod indexstore;
mod swiftsyntax;
mod treesitter;

use std::path::Path;

pub use treesitter::SwiftExtractor;

use grapha_core::{ExtractionResult, LanguageExtractor};

/// Extract Swift source code with waterfall strategy:
/// 1. Xcode index store (confidence 1.0)
/// 2. SwiftSyntax bridge (confidence 0.9)
/// 3. tree-sitter-swift fallback (confidence 0.6-0.8)
pub fn extract_swift(
    source: &[u8],
    file_path: &Path,
    index_store_path: Option<&Path>,
) -> anyhow::Result<ExtractionResult> {
    if let Some(store_path) = index_store_path {
        if let Some(result) = indexstore::extract_from_indexstore(file_path, store_path) {
            return Ok(result);
        }
    }

    if let Some(result) = swiftsyntax::extract_with_swiftsyntax(source, file_path) {
        return Ok(result);
    }

    let extractor = SwiftExtractor;
    extractor.extract(source, file_path)
}
