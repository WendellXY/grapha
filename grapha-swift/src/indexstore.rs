use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use grapha_core::ExtractionResult;

use crate::binary;
use crate::bridge::{self, IndexStoreStatus};

static STORE_CACHE: LazyLock<HandleCache<StoreHandle>> = LazyLock::new(HandleCache::default);
const STORE_CACHE_CAPACITY: usize = 8;

struct HandleCache<T> {
    state: RwLock<CacheState<T>>,
    capacity: usize,
}

struct CacheState<T> {
    entries: HashMap<PathBuf, Arc<Mutex<Option<Arc<T>>>>>,
    insertion_order: VecDeque<PathBuf>,
}

impl<T> Default for HandleCache<T> {
    fn default() -> Self {
        Self::with_capacity(STORE_CACHE_CAPACITY)
    }
}

impl<T> HandleCache<T> {
    fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "cache capacity must be positive");
        Self {
            state: RwLock::new(CacheState {
                entries: HashMap::new(),
                insertion_order: VecDeque::new(),
            }),
            capacity,
        }
    }

    fn get_or_insert_with<F>(&self, path: &Path, open: F) -> Option<Arc<T>>
    where
        F: FnOnce() -> Option<T>,
    {
        let path_buf = path.to_path_buf();
        let (slot, inserted) =
            if let Some(existing) = self.state.read().ok()?.entries.get(path).cloned() {
                (existing, false)
            } else {
                let mut state = self.state.write().ok()?;
                if let Some(existing) = state.entries.get(path).cloned() {
                    (existing, false)
                } else {
                    let slot = Arc::new(Mutex::new(None));
                    state.entries.insert(path_buf.clone(), slot.clone());
                    state.insertion_order.push_back(path_buf.clone());
                    (slot, true)
                }
            };

        let mut guard = slot.lock().ok()?;
        if let Some(existing) = guard.as_ref() {
            return Some(existing.clone());
        }

        let value = match open() {
            Some(value) => Arc::new(value),
            None => {
                drop(guard);
                if inserted {
                    self.evict(path);
                }
                return None;
            }
        };
        *guard = Some(value.clone());
        drop(guard);

        if inserted {
            self.evict_over_capacity();
        }

        Some(value)
    }

    fn evict_over_capacity(&self) {
        let mut evicted = Vec::new();
        if let Ok(mut state) = self.state.write() {
            while state.entries.len() > self.capacity {
                let Some(path) = state.insertion_order.pop_front() else {
                    break;
                };
                if state.entries.remove(&path).is_some() {
                    evicted.push(path);
                }
            }
        }

        let _ = evicted;
    }

    fn evict(&self, path: &Path) {
        if let Ok(mut state) = self.state.write() {
            state.entries.remove(path);
            state.insertion_order.retain(|candidate| candidate != path);
        }
    }

    fn clear_all(&self) {
        if let Ok(mut state) = self.state.write() {
            state.entries.clear();
            state.insertion_order.clear();
        }
    }
}

struct StoreHandle {
    ptr: *mut std::ffi::c_void,
    close: unsafe extern "C" fn(*mut std::ffi::c_void),
}

unsafe impl Send for StoreHandle {}
unsafe impl Sync for StoreHandle {}

impl Drop for StoreHandle {
    fn drop(&mut self) {
        unsafe { (self.close)(self.ptr) };
    }
}

fn get_or_open_store(index_store_path: &Path) -> Option<Arc<StoreHandle>> {
    STORE_CACHE.get_or_insert_with(index_store_path, || {
        let bridge = bridge::bridge()?;
        let path_c = CString::new(index_store_path.to_str()?).ok()?;
        let mut status = IndexStoreStatus::Ok as i32;
        let ptr = unsafe { (bridge.indexstore_open)(path_c.as_ptr(), &mut status) };
        match IndexStoreStatus::try_from(status).ok()? {
            IndexStoreStatus::Ok if !ptr.is_null() => Some(StoreHandle {
                ptr,
                close: bridge.indexstore_close,
            }),
            _ => None,
        }
    })
}

pub fn release_store_handles() {
    STORE_CACHE.clear_all();
}

pub fn warmup_indexstore(index_store_path: &Path) {
    let Some(bridge) = bridge::bridge() else {
        return;
    };
    let Some(warmup_fn) = bridge.indexstore_warmup else {
        return;
    };
    let Some(handle) = get_or_open_store(index_store_path) else {
        return;
    };
    unsafe { warmup_fn(handle.ptr) };
}

pub fn extract_from_indexstore(
    file_path: &Path,
    index_store_path: &Path,
) -> Option<ExtractionResult> {
    let bridge = bridge::bridge()?;
    let handle = get_or_open_store(index_store_path)?;

    let file_path_c = CString::new(file_path.to_str()?).ok()?;
    let mut buf_len: u32 = 0;
    let mut status = IndexStoreStatus::Ok as i32;
    let buf_ptr = unsafe {
        (bridge.indexstore_extract)(handle.ptr, file_path_c.as_ptr(), &mut buf_len, &mut status)
    };

    match IndexStoreStatus::try_from(status).ok()? {
        IndexStoreStatus::Ok if !buf_ptr.is_null() && buf_len > 0 => {}
        _ => return None,
    }

    let buf = unsafe { std::slice::from_raw_parts(buf_ptr, buf_len as usize) };
    let result = binary::parse_binary_buffer(buf);
    unsafe { (bridge.free_buffer)(buf_ptr as *mut u8) };

    result
}

#[cfg(test)]
mod tests;
