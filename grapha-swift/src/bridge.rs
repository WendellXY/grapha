#[cfg(not(no_swift_bridge))]
use std::path::{Path, PathBuf};
#[cfg(not(no_swift_bridge))]
use std::sync::OnceLock;

use libloading::Library;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexStoreStatus {
    Ok = 0,
    OpenFailed = 1,
    InvalidHandle = 2,
    ExtractFailed = 3,
}

impl TryFrom<i32> for IndexStoreStatus {
    type Error = i32;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::OpenFailed),
            2 => Ok(Self::InvalidHandle),
            3 => Ok(Self::ExtractFailed),
            other => Err(other),
        }
    }
}

type IndexStoreOpenFn = unsafe extern "C" fn(*const i8, *mut i32) -> *mut std::ffi::c_void;
type IndexStoreCloseFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type IndexStoreExtractFn =
    unsafe extern "C" fn(*mut std::ffi::c_void, *const i8, *mut u32, *mut i32) -> *const u8;
type SwiftSyntaxExtractFn = unsafe extern "C" fn(*const i8, usize, *const i8) -> *const i8;
type FreeStringFn = unsafe extern "C" fn(*mut i8);
type FreeBufferFn = unsafe extern "C" fn(*mut u8);

pub struct SwiftBridge {
    _lib: Library,
    pub indexstore_open: IndexStoreOpenFn,
    pub indexstore_close: IndexStoreCloseFn,
    pub indexstore_extract: IndexStoreExtractFn,
    pub swiftsyntax_extract: SwiftSyntaxExtractFn,
    pub free_string: FreeStringFn,
    pub free_buffer: FreeBufferFn,
}

#[cfg(not(no_swift_bridge))]
static BRIDGE: OnceLock<Option<SwiftBridge>> = OnceLock::new();

impl SwiftBridge {
    #[cfg(not(no_swift_bridge))]
    fn load() -> Option<Self> {
        let lib_path = Self::find_dylib()?;
        let lib = unsafe { Library::new(&lib_path) }.ok()?;

        unsafe {
            let indexstore_open = *lib
                .get::<IndexStoreOpenFn>(b"grapha_indexstore_open")
                .ok()?;
            let indexstore_close = *lib
                .get::<IndexStoreCloseFn>(b"grapha_indexstore_close")
                .ok()?;
            let indexstore_extract = *lib
                .get::<IndexStoreExtractFn>(b"grapha_indexstore_extract")
                .ok()?;
            let swiftsyntax_extract = *lib
                .get::<SwiftSyntaxExtractFn>(b"grapha_swiftsyntax_extract")
                .ok()?;
            let free_string = *lib.get::<FreeStringFn>(b"grapha_free_string").ok()?;
            let free_buffer = *lib.get::<FreeBufferFn>(b"grapha_free_buffer").ok()?;

            Some(SwiftBridge {
                _lib: lib,
                indexstore_open,
                indexstore_close,
                indexstore_extract,
                swiftsyntax_extract,
                free_string,
                free_buffer,
            })
        }
    }

    #[cfg(not(no_swift_bridge))]
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

#[cfg(test)]
mod tests;
