import Foundation

// MARK: - Index Store

@_cdecl("grapha_indexstore_open")
public func indexstoreOpen(_ path: UnsafePointer<CChar>) -> UnsafeMutableRawPointer? {
    return nil // Phase 3
}

@_cdecl("grapha_indexstore_extract")
public func indexstoreExtract(
    _ handle: UnsafeMutableRawPointer,
    _ filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    return nil // Phase 3
}

@_cdecl("grapha_indexstore_close")
public func indexstoreClose(_ handle: UnsafeMutableRawPointer) {
    // Phase 3
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
