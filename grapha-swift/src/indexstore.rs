use std::ffi::{CStr, CString};
use std::path::Path;

use grapha_core::ExtractionResult;

use crate::bridge;

/// Try to extract Swift symbols from Xcode's index store.
pub fn extract_from_indexstore(
    file_path: &Path,
    index_store_path: &Path,
) -> Option<ExtractionResult> {
    let bridge = bridge::bridge()?;

    // Open the store
    let store_path_c = CString::new(index_store_path.to_str()?).ok()?;
    let handle = unsafe { (bridge.indexstore_open)(store_path_c.as_ptr()) };
    if handle.is_null() {
        return None;
    }

    // Extract the file
    let file_path_c = CString::new(file_path.to_str()?).ok()?;
    let json_ptr = unsafe { (bridge.indexstore_extract)(handle, file_path_c.as_ptr()) };

    // Close the store
    unsafe { (bridge.indexstore_close)(handle) };

    if json_ptr.is_null() {
        return None;
    }

    // Parse the JSON
    let json_str = unsafe { CStr::from_ptr(json_ptr) }.to_str().ok()?;
    let result: ExtractionResult = serde_json::from_str(json_str).ok()?;

    // Free the string
    unsafe { (bridge.free_string)(json_ptr as *mut i8) };

    Some(result)
}
