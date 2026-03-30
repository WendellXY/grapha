import Foundation

// MARK: - Reader Storage

// Store readers in a dictionary to avoid Unmanaged ARC issues across FFI
nonisolated(unsafe) private var _readers: [Int: IndexStoreReader] = [:]
nonisolated(unsafe) private var _nextHandle: Int = 1
nonisolated(unsafe) private let _readerLock = NSLock()

// MARK: - Index Store

@_cdecl("grapha_indexstore_open")
public func indexstoreOpen(_ path: UnsafePointer<CChar>) -> UnsafeMutableRawPointer? {
    let pathStr = String(cString: path)
    guard let reader = IndexStoreReader(storePath: pathStr) else { return nil }
    _readerLock.lock()
    let handle = _nextHandle
    _nextHandle += 1
    _readers[handle] = reader
    _readerLock.unlock()
    return UnsafeMutableRawPointer(bitPattern: handle)
}

@_cdecl("grapha_indexstore_extract")
public func indexstoreExtract(
    _ handle: UnsafeMutableRawPointer,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    let key = Int(bitPattern: handle)
    _readerLock.lock()
    let reader = _readers[key]
    _readerLock.unlock()
    guard let reader else { return nil }
    let file = String(cString: filePath)
    guard let json = reader.extractFile(file) else { return nil }
    return strdup(json).map { UnsafePointer($0) }
}

@_cdecl("grapha_indexstore_close")
public func indexstoreClose(_ handle: UnsafeMutableRawPointer) {
    let key = Int(bitPattern: handle)
    _readerLock.lock()
    _readers.removeValue(forKey: key)
    _readerLock.unlock()
}

// MARK: - SwiftSyntax

@_cdecl("grapha_swiftsyntax_extract")
public func swiftsyntaxExtract(
    _ source: UnsafePointer<CChar>,
    _ sourceLen: Int,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    return nil // Phase 4
}

// MARK: - Memory

@_cdecl("grapha_free_string")
public func freeString(_ ptr: UnsafeMutablePointer<CChar>) {
    ptr.deallocate()
}
