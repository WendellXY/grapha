use std::path::Path;

use grapha_core::ExtractionResult;

use crate::bridge;

/// Try to extract using SwiftSyntax bridge. Returns None if unavailable.
pub fn extract_with_swiftsyntax(
    _source: &[u8],
    _file_path: &Path,
) -> Option<ExtractionResult> {
    let _bridge = bridge::bridge()?;
    None // Phase 4 will implement
}
