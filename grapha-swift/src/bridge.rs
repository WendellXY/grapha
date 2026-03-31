use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libloading::Library;

type IndexStoreOpenFn = unsafe extern "C" fn(*const i8) -> *mut std::ffi::c_void;
type IndexStoreExtractFn = unsafe extern "C" fn(*mut std::ffi::c_void, *const i8, *mut u32) -> *const u8;
type IndexStoreCloseFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type SwiftSyntaxExtractFn = unsafe extern "C" fn(*const i8, usize, *const i8) -> *const i8;
type FreeStringFn = unsafe extern "C" fn(*mut i8);
type FreeBufferFn = unsafe extern "C" fn(*mut u8);

#[allow(dead_code)] // Fields used in Phase 3/4 implementations
pub struct SwiftBridge {
    _lib: Library,
    pub indexstore_open: IndexStoreOpenFn,
    pub indexstore_extract: IndexStoreExtractFn,
    pub indexstore_close: IndexStoreCloseFn,
    pub swiftsyntax_extract: SwiftSyntaxExtractFn,
    pub free_string: FreeStringFn,
    pub free_buffer: FreeBufferFn,
}

static BRIDGE: OnceLock<Option<SwiftBridge>> = OnceLock::new();

impl SwiftBridge {
    fn load() -> Option<Self> {
        let lib_path = Self::find_dylib()?;
        let lib = unsafe { Library::new(&lib_path) }.ok()?;

        unsafe {
            let indexstore_open = *lib
                .get::<IndexStoreOpenFn>(b"grapha_indexstore_open")
                .ok()?;
            let indexstore_extract = *lib
                .get::<IndexStoreExtractFn>(b"grapha_indexstore_extract")
                .ok()?;
            let indexstore_close = *lib
                .get::<IndexStoreCloseFn>(b"grapha_indexstore_close")
                .ok()?;
            let swiftsyntax_extract = *lib
                .get::<SwiftSyntaxExtractFn>(b"grapha_swiftsyntax_extract")
                .ok()?;
            let free_string = *lib.get::<FreeStringFn>(b"grapha_free_string").ok()?;
            let free_buffer = *lib.get::<FreeBufferFn>(b"grapha_free_buffer").ok()?;

            Some(SwiftBridge {
                _lib: lib,
                indexstore_open,
                indexstore_extract,
                indexstore_close,
                swiftsyntax_extract,
                free_string,
                free_buffer,
            })
        }
    }

    fn find_dylib() -> Option<PathBuf> {
        if let Some(dir) = option_env!("SWIFT_BRIDGE_PATH") {
            let dylib = Path::new(dir).join("libGraphaSwiftBridge.dylib");
            if dylib.exists() {
                return Some(dylib);
            }
        }
        None
    }
}

#[cfg(not(no_swift_bridge))]
pub fn bridge() -> Option<&'static SwiftBridge> {
    BRIDGE.get_or_init(SwiftBridge::load).as_ref()
}

#[cfg(no_swift_bridge)]
pub fn bridge() -> Option<&'static SwiftBridge> {
    None
}
