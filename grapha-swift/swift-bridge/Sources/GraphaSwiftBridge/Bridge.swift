import Foundation

// MARK: - Index Store

@_cdecl("grapha_indexstore_open")
public func indexstoreOpen(_ path: UnsafePointer<CChar>) -> UnsafeMutableRawPointer? {
    let pathStr = String(cString: path)
    guard let reader = IndexStoreReader(storePath: pathStr) else { return nil }
    return Unmanaged.passRetained(reader).toOpaque()
}

@_cdecl("grapha_indexstore_extract")
public func indexstoreExtract(
    _ handle: UnsafeMutableRawPointer,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    let reader = Unmanaged<IndexStoreReader>.fromOpaque(handle).takeUnretainedValue()
    let file = String(cString: filePath)
    guard let json = reader.extractFile(file) else {
        return nil
    }
    let cStr = strdup(json)
    return cStr.map { UnsafePointer($0) }
}

@_cdecl("grapha_indexstore_close")
public func indexstoreClose(_ handle: UnsafeMutableRawPointer) {
    Unmanaged<IndexStoreReader>.fromOpaque(handle).release()
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
