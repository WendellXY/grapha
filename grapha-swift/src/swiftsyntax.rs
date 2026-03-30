use std::ffi::{CStr, CString};
use std::path::Path;

use grapha_core::ExtractionResult;

use crate::bridge;

/// Try to extract Swift symbols using SwiftSyntax via the bridge.
pub fn extract_with_swiftsyntax(
    source: &[u8],
    file_path: &Path,
) -> Option<ExtractionResult> {
    let bridge = bridge::bridge()?;

    let file_path_c = CString::new(file_path.to_str()?).ok()?;
    let json_ptr = unsafe {
        (bridge.swiftsyntax_extract)(
            source.as_ptr() as *const i8,
            source.len(),
            file_path_c.as_ptr(),
        )
    };

    if json_ptr.is_null() {
        return None;
    }

    let json_str = unsafe { CStr::from_ptr(json_ptr) }.to_str().ok()?;
    let result: ExtractionResult = serde_json::from_str(json_str).ok()?;
    unsafe { (bridge.free_string)(json_ptr as *mut i8) };

    Some(result)
}
