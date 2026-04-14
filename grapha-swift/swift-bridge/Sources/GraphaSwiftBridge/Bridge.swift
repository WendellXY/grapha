import Foundation
import Synchronization

private enum IndexStoreStatus: Int32 {
    case ok = 0
    case openFailed = 1
    case invalidHandle = 2
    case extractFailed = 3
}

// MARK: - Reader Storage

private let _readers = Mutex<[Int: IndexStoreReader]>([:])
private let _nextHandle = Atomic<Int>(1)

// MARK: - Index Store

@c(grapha_indexstore_open)
public func indexstoreOpen(
    _ path: UnsafePointer<CChar>,
    _ outStatus: UnsafeMutablePointer<Int32>?
) -> UnsafeMutableRawPointer? {
    let pathStr = String(cString: path)
    guard let reader = IndexStoreReader(storePath: pathStr) else {
        outStatus?.pointee = IndexStoreStatus.openFailed.rawValue
        return nil
    }

    let handle = _nextHandle.wrappingAdd(1, ordering: .relaxed).oldValue
    _readers.withLock { $0[handle] = reader }
    outStatus?.pointee = IndexStoreStatus.ok.rawValue
    return UnsafeMutableRawPointer(bitPattern: handle)
}

@c(grapha_indexstore_close)
public func indexstoreClose(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    let key = Int(bitPattern: handle)
    _ = _readers.withLock { $0.removeValue(forKey: key) }
}

@c(grapha_indexstore_extract)
public func indexstoreExtract(
    _ handle: UnsafeMutableRawPointer?,
    _ filePath: UnsafePointer<CChar>,
    _ outLen: UnsafeMutablePointer<UInt32>,
    _ outStatus: UnsafeMutablePointer<Int32>?
) -> UnsafeRawPointer? {
    guard let handle else {
        outLen.pointee = 0
        outStatus?.pointee = IndexStoreStatus.invalidHandle.rawValue
        return nil
    }

    let key = Int(bitPattern: handle)
    let reader = _readers.withLock { $0[key] }
    guard let reader else {
        outLen.pointee = 0
        outStatus?.pointee = IndexStoreStatus.invalidHandle.rawValue
        return nil
    }

    let file = String(cString: filePath)
    guard let (ptr, len) = reader.extractFile(file) else {
        outLen.pointee = 0
        outStatus?.pointee = IndexStoreStatus.extractFailed.rawValue
        return nil
    }

    outLen.pointee = len
    outStatus?.pointee = IndexStoreStatus.ok.rawValue
    return UnsafeRawPointer(ptr)
}

@c(grapha_indexstore_warmup)
public func indexstoreWarmup(_ handle: UnsafeMutableRawPointer?) {
    guard let handle else { return }
    let key = Int(bitPattern: handle)
    let reader = _readers.withLock { $0[key] }
    reader?.warmup()
}

// MARK: - SwiftSyntax

@c(grapha_swiftsyntax_extract)
public func swiftsyntaxExtract(
    _ source: UnsafePointer<CChar>,
    _ sourceLen: Int,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    return extractWithSwiftSyntax(source: source, sourceLen: sourceLen, filePath: filePath)
}

// MARK: - Memory

@c(grapha_free_string)
public func freeString(_ ptr: UnsafeMutablePointer<CChar>) {
    free(ptr)
}

@c(grapha_free_buffer)
public func freeBuffer(_ ptr: UnsafeMutableRawPointer) {
    free(ptr)
}
