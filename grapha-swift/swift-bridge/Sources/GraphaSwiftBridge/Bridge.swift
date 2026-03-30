import Foundation
import Synchronization

// MARK: - Reader Storage

private let _readers = Mutex<[Int: IndexStoreReader]>([:])
private let _nextHandle = Atomic<Int>(1)

// MARK: - Index Store

@c(grapha_indexstore_open)
public func indexstoreOpen(_ path: UnsafePointer<CChar>) -> UnsafeMutableRawPointer? {
    let pathStr = String(cString: path)
    guard let reader = IndexStoreReader(storePath: pathStr) else { return nil }
    let handle = _nextHandle.wrappingAdd(1, ordering: .relaxed).oldValue
    _readers.withLock { $0[handle] = reader }
    return UnsafeMutableRawPointer(bitPattern: handle)
}

@c(grapha_indexstore_extract)
public func indexstoreExtract(
    _ handle: UnsafeMutableRawPointer,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    let key = Int(bitPattern: handle)
    let reader = _readers.withLock { $0[key] }
    guard let reader else { return nil }
    let file = String(cString: filePath)
    guard let json = reader.extractFile(file) else { return nil }
    return strdup(json).map { UnsafePointer($0) }
}

@c(grapha_indexstore_close)
public func indexstoreClose(_ handle: UnsafeMutableRawPointer) {
    let key = Int(bitPattern: handle)
    _ = _readers.withLock { $0.removeValue(forKey: key) }
}

// MARK: - SwiftSyntax

@c(grapha_swiftsyntax_extract)
public func swiftsyntaxExtract(
    _ source: UnsafePointer<CChar>,
    _ sourceLen: Int,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    return nil // Phase 4
}

// MARK: - Memory

@c(grapha_free_string)
public func freeString(_ ptr: UnsafeMutablePointer<CChar>) {
    ptr.deallocate()
}
