use std::path::Path;

use grapha_core::ExtractionResult;

use crate::bridge;

/// Try to extract from Xcode's index store. Returns None if unavailable.
pub fn extract_from_indexstore(
    _file_path: &Path,
    _index_store_path: &Path,
) -> Option<ExtractionResult> {
    let _bridge = bridge::bridge()?;
    None // Phase 3 will implement
}
