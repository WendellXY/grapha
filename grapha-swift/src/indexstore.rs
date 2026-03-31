use std::ffi::CString;
use std::path::Path;
use std::sync::OnceLock;

use grapha_core::ExtractionResult;

use crate::binary;
use crate::bridge;

static STORE_HANDLE: OnceLock<Option<StoreHandle>> = OnceLock::new();

struct StoreHandle {
    ptr: *mut std::ffi::c_void,
}

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

pub fn extract_from_indexstore(
    file_path: &Path,
    index_store_path: &Path,
) -> Option<ExtractionResult> {
    let bridge = bridge::bridge()?;
    let handle = get_or_open_store(index_store_path)?;

    let file_path_c = CString::new(file_path.to_str()?).ok()?;
    let mut buf_len: u32 = 0;
    let buf_ptr = unsafe {
        (bridge.indexstore_extract)(handle, file_path_c.as_ptr(), &mut buf_len)
    };

    if buf_ptr.is_null() || buf_len == 0 {
        return None;
    }

    let buf = unsafe { std::slice::from_raw_parts(buf_ptr, buf_len as usize) };
    let result = binary::parse_binary_buffer(buf);
    unsafe { (bridge.free_buffer)(buf_ptr as *mut u8) };

    result
}
