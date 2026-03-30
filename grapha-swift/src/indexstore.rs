use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::OnceLock;

use grapha_core::ExtractionResult;

use crate::bridge;

/// Cached store handle — opened once, reused for all files.
static STORE_HANDLE: OnceLock<Option<StoreHandle>> = OnceLock::new();

struct StoreHandle {
    ptr: *mut std::ffi::c_void,
}

// Store handles are thread-safe (protected by lock on Swift side)
unsafe impl Send for StoreHandle {}
unsafe impl Sync for StoreHandle {}

fn get_or_open_store(index_store_path: &Path) -> Option<*mut std::ffi::c_void> {
    let handle = STORE_HANDLE.get_or_init(|| {
        let bridge = bridge::bridge()?;
        let path_c = CString::new(index_store_path.to_str()?).ok()?;
        let ptr = unsafe { (bridge.indexstore_open)(path_c.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(StoreHandle { ptr })
        }
    });
    handle.as_ref().map(|h| h.ptr)
}

/// Try to extract Swift symbols from Xcode's index store.
pub fn extract_from_indexstore(
    file_path: &Path,
    index_store_path: &Path,
) -> Option<ExtractionResult> {
    let bridge = bridge::bridge()?;
    let handle = get_or_open_store(index_store_path)?;

    let file_path_c = CString::new(file_path.to_str()?).ok()?;
    let json_ptr = unsafe { (bridge.indexstore_extract)(handle, file_path_c.as_ptr()) };

    if json_ptr.is_null() {
        return None;
    }

    let json_str = unsafe { CStr::from_ptr(json_ptr) }.to_str().ok()?;
    let result: ExtractionResult = serde_json::from_str(json_str).ok()?;
    unsafe { (bridge.free_string)(json_ptr as *mut i8) };

    Some(result)
}
