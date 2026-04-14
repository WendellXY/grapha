import CIndexStore
import Foundation

// MARK: - Role Constants

private struct Roles {
    static let declaration: UInt64 = 1
    static let definition: UInt64 = 2
    static let reference: UInt64 = 4
    static let call: UInt64 = 32
    static let containedBy: UInt64 = 128
    static let baseOf: UInt64 = 256
    static let overrideOf: UInt64 = 512
    static let conformsTo: UInt64 = 1 << 19
}

// MARK: - String Conversion

private func str(_ ref: indexstore_string_ref_t) -> String {
    guard ref.length > 0, let data = ref.data else { return "" }
    return String(
        decoding: UnsafeRawBufferPointer(start: data, count: ref.length),
        as: UTF8.self
    )
}

@_silgen_name("indexstore_unit_dependency_get_modulename")
private func indexstore_unit_dependency_get_modulename_shim(
    _ dependency: indexstore_unit_dependency_t?
) -> indexstore_string_ref_t

// MARK: - Extracted Data

/// Node data collected from index store. Strings use interned offsets
/// to avoid redundant copies — the raw string is interned once into the
/// string table, and only the (offset, length) pair is stored here.
private struct ExtractedNode {
    let id: String      // USR — also used as dict key (shared via CoW)
    let kind: UInt8
    let name: String
    let file: String    // shared across all nodes in same file (CoW)
    let line: UInt32
    let col: UInt32
    let endLine: UInt32
    let endCol: UInt32
    let visibility: UInt8
    let module: String? // shared across all nodes in same file (CoW)
}

private struct ExtractedImport: Hashable {
    let path: String
    let kind: UInt8
}

private struct ExtractedEdge: Hashable {
    let source: String
    let target: String
    let kind: UInt8
    let confidencePct: UInt8

    func hash(into hasher: inout Hasher) {
        hasher.combine(source)
        hasher.combine(target)
        hasher.combine(kind)
    }

    static func == (lhs: ExtractedEdge, rhs: ExtractedEdge) -> Bool {
        lhs.source == rhs.source && lhs.target == rhs.target && lhs.kind == rhs.kind
    }
}

// MARK: - Callback Context Types

private final class OccCollector: @unchecked Sendable {
    var nodes: [String: ExtractedNode]
    /// Set gives O(1) insertion with automatic deduplication (no post-pass needed).
    var edges: Set<ExtractedEdge>
    var imports: [ExtractedImport]
    let fileName: String
    let moduleName: String?

    init(fileName: String, moduleName: String?) {
        self.fileName = fileName
        self.moduleName = moduleName
        // Pre-size to avoid rehashing during collection.
        // Typical Swift file: ~50-100 symbols, ~200-500 edges.
        self.nodes = Dictionary(minimumCapacity: 128)
        self.edges = Set(minimumCapacity: 512)
        self.imports = []
    }
}

// MARK: - IndexStoreReader

import Synchronization

// MARK: - Callback State

// buildFileIndex globals — serialized by _buildIndexLock.
private let _buildIndexLock = Mutex<Void>(())
nonisolated(unsafe) private var _cbStore: indexstore_t? = nil
nonisolated(unsafe) private var _cbFileIndex: [String: UnitInfo] = [:]
nonisolated(unsafe) private var _cbRecordName: String? = nil
nonisolated(unsafe) private var _cbImports: [ExtractedImport] = []

/// Shared file index cache keyed by store path. Multiple IndexStoreReader
/// instances for the same store share a single file index, avoiding
/// redundant unit scans when threads each open their own handle.
private let _sharedFileIndexCache = Mutex<[String: [String: UnitInfo]]>([:])

/// Per-extraction context passed through the `ctx` pointer of _apply_f callbacks.
/// This eliminates global mutable state, allowing concurrent extractions.
private final class ExtractionContext {
    let collector: OccCollector
    var relSymbolUSR: String = ""
    var relRoles: UInt64 = 0

    init(collector: OccCollector) {
        self.collector = collector
    }
}

/// Pre-built lookup: mainFile path → (unitName, moduleName, recordName).
/// recordName is pre-fetched during buildFileIndex to avoid a second unit reader open per extraction.
private struct UnitInfo {
    let unitName: String
    let moduleName: String?
    let recordName: String?
    let imports: [ExtractedImport]
}

// MARK: - File-level dependency callback
// Extracted from buildFileIndex to satisfy @convention(c)'s no-capture requirement.

private func _collectRecordName(_ ctx: UnsafeMutableRawPointer?, _ dep: indexstore_unit_dependency_t?) -> Bool {
    guard let dep else { return true }
    if indexstore_unit_dependency_get_kind(dep) == 2 {
        let name = str(indexstore_unit_dependency_get_name(dep))
        if !name.isEmpty { _cbRecordName = name }
        return true
    }

    let moduleName = str(indexstore_unit_dependency_get_modulename_shim(dep))
    if !moduleName.isEmpty,
        !_cbImports.contains(where: { $0.path == moduleName && $0.kind == BinaryImportKind.module })
    {
        _cbImports.append(ExtractedImport(path: moduleName, kind: BinaryImportKind.module))
    }

    return true
}

final class IndexStoreReader: @unchecked Sendable {
    private let store: indexstore_t
    private let storePath: String
    /// Lazy file→unit index, built on first access
    private var fileIndex: [String: UnitInfo]?

    init?(storePath: String) {
        var err: indexstore_error_t?
        guard let store = storePath.withCString({ indexstore_store_create($0, &err) }) else {
            return nil
        }
        self.store = store
        self.storePath = storePath
    }

    deinit {
        indexstore_store_dispose(store)
    }

    // MARK: - Public

    func warmup() {
        ensureFileIndex()
    }

    func extractFile(_ filePath: String) -> (UnsafeMutableRawPointer, UInt32)? {
        ensureFileIndex()

        let resolved = resolvePath(filePath)
        // Fast last-path-component extraction without Foundation URL
        let fileName: String
        if let slashIdx = resolved.lastIndex(of: "/") {
            fileName = String(resolved[resolved.index(after: slashIdx)...])
        } else {
            fileName = resolved
        }

        let unitInfo = fileIndex?[resolved] ?? findByFileName(fileName)

        guard let unitInfo else { return nil }
        guard let recordName = unitInfo.recordName else { return nil }

        // No global lock — each extraction uses its own context via ctx pointer
        let collector = readOccurrences(
            recordName: recordName,
            fileName: fileName,
            moduleName: unitInfo.moduleName
        )

        return buildBinaryBuffer(
            nodes: collector.nodes.values,
            edges: collector.edges,
            imports: unitInfo.imports
        )
    }

    // MARK: - File Index (built once)

    private func ensureFileIndex() {
        if fileIndex == nil {
            if let cached = _sharedFileIndexCache.withLock({ $0[storePath] }) {
                fileIndex = cached
            } else {
                _buildIndexLock.withLock { _ in
                    if let cached = _sharedFileIndexCache.withLock({ $0[storePath] }) {
                        fileIndex = cached
                    } else {
                        fileIndex = buildFileIndex()
                        let idx = fileIndex!
                        _sharedFileIndexCache.withLock { $0[storePath] = idx }
                    }
                }
            }
        }
    }

    private func buildFileIndex() -> [String: UnitInfo] {
        _cbStore = store
        _cbFileIndex = [:]

        let cb: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?, Int) -> Bool = {
            _, data, len in
            guard let data, let s = _cbStore else { return true }
            let unitName = String(decoding: UnsafeRawBufferPointer(start: data, count: len), as: UTF8.self)
            guard let reader = unitName.withCString({ indexstore_unit_reader_create(s, $0, nil) }) else { return true }
            defer { indexstore_unit_reader_dispose(reader) }

            let mainFile = str(indexstore_unit_reader_get_main_file(reader))
            guard !mainFile.isEmpty, mainFile.hasSuffix(".swift") else { return true }
            guard !mainFile.contains("/.build/") else { return true }

            let mod = str(indexstore_unit_reader_get_module_name(reader))

            // Collect record name while reader is already open (avoids reopening on every extractFile call)
            _cbRecordName = nil
            _cbImports = []
            _ = indexstore_unit_reader_dependencies_apply_f(reader, nil, _collectRecordName)

            _cbFileIndex[mainFile] = UnitInfo(
                unitName: unitName,
                moduleName: mod.isEmpty ? nil : mod,
                recordName: _cbRecordName,
                imports: _cbImports
            )
            return true
        }

        _ = indexstore_store_units_apply_f(store, 0, nil, cb)
        return _cbFileIndex
    }

    private func findByFileName(_ fileName: String) -> UnitInfo? {
        fileIndex?.first(where: { $0.key.hasSuffix("/" + fileName) })?.value
    }

    // MARK: - Occurrence Reading

    private func readOccurrences(
        recordName: String,
        fileName: String,
        moduleName: String?
    ) -> OccCollector {
        let collector = OccCollector(fileName: fileName, moduleName: moduleName)

        guard let reader = recordName.withCString({
            indexstore_record_reader_create(store, $0, nil)
        }) else {
            return collector
        }
        defer { indexstore_record_reader_dispose(reader) }

        let ctx = ExtractionContext(collector: collector)
        let ctxPtr = Unmanaged.passUnretained(ctx).toOpaque()

        let cb: @convention(c) (UnsafeMutableRawPointer?, indexstore_occurrence_t?) -> Bool = {
            rawCtx, occ in
            guard let rawCtx, let occ else { return true }
            let ctx = Unmanaged<ExtractionContext>.fromOpaque(rawCtx).takeUnretainedValue()
            processOccurrence(ctx: ctx, occ: occ)
            return true
        }

        _ = indexstore_record_reader_occurrences_apply_f(reader, ctxPtr, cb)
        return collector
    }
}

// MARK: - Occurrence Processing

private func processOccurrence(ctx: ExtractionContext, occ: indexstore_occurrence_t) {
    let c = ctx.collector
    let symbol = indexstore_occurrence_get_symbol(occ)!
    let roles = indexstore_occurrence_get_roles(occ)
    let usr = str(indexstore_symbol_get_usr(symbol))
    guard !usr.isEmpty else { return }

    let name = str(indexstore_symbol_get_name(symbol))
    let kindRaw = indexstore_symbol_get_kind(symbol)

    var line: UInt32 = 0
    var col: UInt32 = 0
    indexstore_occurrence_get_line_col(occ, &line, &col)
    let (endLine, endCol) = resolvedEndPosition(startLine: line, startCol: col, exactEnd: nil)

    // Record definitions/declarations as nodes
    let isDefOrDecl = (roles & Roles.definition) != 0 || (roles & Roles.declaration) != 0
    if isDefOrDecl, let kind = mapSymbolKind(kindRaw) {
        c.nodes[usr] = ExtractedNode(
            id: usr, kind: kind, name: name, file: c.fileName,
            line: line,
            col: col,
            endLine: endLine,
            endCol: endCol,
            visibility: 0,
            module: c.moduleName
        )
    }

    // Extract edges from relations — pass context through ctx pointer
    ctx.relSymbolUSR = usr
    ctx.relRoles = roles
    let ctxPtr = Unmanaged.passUnretained(ctx).toOpaque()

    let cb: @convention(c) (UnsafeMutableRawPointer?, indexstore_symbol_relation_t?) -> Bool = {
        rawCtx, rel in
        guard let rawCtx, let rel else { return true }
        let ctx = Unmanaged<ExtractionContext>.fromOpaque(rawCtx).takeUnretainedValue()
        let c = ctx.collector
        let relSym = indexstore_symbol_relation_get_symbol(rel)!
        let relUSR = str(indexstore_symbol_get_usr(relSym))
        guard !relUSR.isEmpty else { return true }

        let relRoles = indexstore_symbol_relation_get_roles(rel)
        let combinedRoles = ctx.relRoles | relRoles

        if (combinedRoles & Roles.call) != 0 {
            c.edges.insert(ExtractedEdge(
                source: relUSR, target: ctx.relSymbolUSR,
                kind: BinaryEdgeKind.calls, confidencePct: 100
            ))
        } else if (combinedRoles & Roles.containedBy) != 0 {
            c.edges.insert(ExtractedEdge(
                source: relUSR, target: ctx.relSymbolUSR,
                kind: BinaryEdgeKind.contains, confidencePct: 100
            ))
        }

        if (combinedRoles & Roles.baseOf) != 0 {
            c.edges.insert(ExtractedEdge(
                source: ctx.relSymbolUSR, target: relUSR,
                kind: BinaryEdgeKind.inherits, confidencePct: 100
            ))
        }

        if (combinedRoles & Roles.conformsTo) != 0 {
            c.edges.insert(ExtractedEdge(
                source: ctx.relSymbolUSR, target: relUSR,
                kind: BinaryEdgeKind.implements, confidencePct: 100
            ))
        }

        if (combinedRoles & Roles.overrideOf) != 0 {
            c.edges.insert(ExtractedEdge(
                source: ctx.relSymbolUSR, target: relUSR,
                kind: BinaryEdgeKind.implements, confidencePct: 90
            ))
        }

        if (combinedRoles & Roles.reference) != 0
            && (combinedRoles & Roles.call) == 0
            && (combinedRoles & Roles.containedBy) == 0
            && (combinedRoles & Roles.baseOf) == 0
            && (combinedRoles & Roles.conformsTo) == 0
        {
            c.edges.insert(ExtractedEdge(
                source: ctx.relSymbolUSR, target: relUSR,
                kind: BinaryEdgeKind.typeRef, confidencePct: 90
            ))
        }

        return true
    }

    _ = indexstore_occurrence_relations_apply_f(occ, ctxPtr, cb)
}

// MARK: - Symbol Kind Mapping

private func mapSymbolKind(_ raw: UInt64) -> UInt8? {
    switch raw {
    case 5:  return 2   // enum
    case 6:  return 1   // struct
    case 7:  return 9   // class
    case 8:  return 3   // protocol
    case 9:  return 4   // extension
    case 11: return 5   // type_alias
    case 12: return 0   // Function
    case 13: return 6   // Variable → property
    case 14: return 7   // field
    case 15: return 8   // variant
    case 16: return 0   // InstanceMethod
    case 17: return 0   // ClassMethod
    case 18: return 0   // StaticMethod
    case 19: return 6   // InstanceProperty
    case 20: return 6   // ClassProperty
    case 21: return 6   // StaticProperty
    case 22: return 0   // Constructor
    case 23: return 0   // Destructor
    default: return nil
    }
}

private struct BinaryEdgeKind {
    static let calls: UInt8 = 0
    static let contains: UInt8 = 1
    static let inherits: UInt8 = 2
    static let implements: UInt8 = 3
    static let typeRef: UInt8 = 4
}

private struct BinaryImportKind {
    static let module: UInt8 = 2
}

// MARK: - Helpers

private func resolvePath(_ path: String) -> String {
    if path.hasPrefix("/") { return path }
    // Avoid Foundation URL allocation for relative paths
    if let cwd = ProcessInfo.processInfo.environment["PWD"] {
        return cwd + "/" + path
    }
    return URL(fileURLWithPath: path).standardized.path
}

private func resolvedEndPosition(
    startLine: UInt32,
    startCol: UInt32,
    exactEnd: (UInt32, UInt32)?
) -> (UInt32, UInt32) {
    exactEnd ?? (startLine, startCol)
}

func resolvedEndPositionForTests(
    startLine: UInt32,
    startCol: UInt32,
    exactEnd: (UInt32, UInt32)?
) -> (UInt32, UInt32) {
    resolvedEndPosition(startLine: startLine, startCol: startCol, exactEnd: exactEnd)
}

private let BINARY_MAGIC: UInt32 = 0x47524148
private let BINARY_VERSION: UInt8 = 2
private let HEADER_SIZE = 24
private let PACKED_NODE_SIZE = 52
private let PACKED_EDGE_SIZE = 20
private let PACKED_IMPORT_SIZE = 12
private let NO_MODULE: UInt32 = 0xFFFFFFFF

func makeBinaryFixtureForTests() -> (UnsafeMutableRawPointer, UInt32) {
    let node = ExtractedNode(
        id: "usr://demo",
        kind: 0,
        name: "Demo",
        file: "File.swift",
        line: 4,
        col: 2,
        endLine: 4,
        endCol: 15,
        visibility: 0,
        module: nil
    )
    let imports = [ExtractedImport(path: "Foundation", kind: BinaryImportKind.module)]
    let nodes = Dictionary(uniqueKeysWithValues: [(node.id, node)])
    return buildBinaryBuffer(nodes: nodes.values, edges: [], imports: imports)
}

private func buildBinaryBuffer(
    nodes: Dictionary<String, ExtractedNode>.Values,
    edges: Set<ExtractedEdge>,
    imports: [ExtractedImport]
) -> (UnsafeMutableRawPointer, UInt32) {
    // Phase 1: build string table with deduplication
    let nodeList = Array(nodes)
    let edgeList = Array(edges)
    let estimatedStrings = nodeList.count * 3 + edgeList.count * 2 + imports.count
    var stringTable = Data()
    stringTable.reserveCapacity(estimatedStrings * 32) // avg ~32 bytes per string
    var stringIndex: [String: (UInt32, UInt32)] = Dictionary(minimumCapacity: estimatedStrings)

    func intern(_ s: String) -> (UInt32, UInt32) {
        if let existing = stringIndex[s] { return existing }
        let offset = UInt32(stringTable.count)
        // Use contiguous UTF-8 buffer directly — avoids copy for native Swift strings (Swift 5+)
        let len: Int = s.utf8.withContiguousStorageIfAvailable { buf in
            stringTable.append(buf)
            return buf.count
        } ?? {
            // Fallback: copy via withUTF8
            var count = 0
            var copy = s
            copy.withUTF8 { buf in
                stringTable.append(buf)
                count = buf.count
            }
            return count
        }()
        let entry = (offset, UInt32(len))
        stringIndex[s] = entry
        return entry
    }

    // Pre-intern all strings (nodes first, then edges reuse via dedup)
    var nodeRefs: [(id: (UInt32, UInt32), name: (UInt32, UInt32), file: (UInt32, UInt32), module: (UInt32, UInt32)?, line: UInt32, col: UInt32, endLine: UInt32, endCol: UInt32, kind: UInt8, visibility: UInt8)] = []
    nodeRefs.reserveCapacity(nodeList.count)
    for n in nodeList {
        let idRef = intern(n.id)
        let nameRef = intern(n.name)
        let fileRef = intern(n.file)
        let modRef = n.module.map { intern($0) }
        nodeRefs.append((idRef, nameRef, fileRef, modRef, n.line, n.col, n.endLine, n.endCol, n.kind, n.visibility))
    }

    var edgeRefs: [(source: (UInt32, UInt32), target: (UInt32, UInt32), kind: UInt8, confidencePct: UInt8)] = []
    edgeRefs.reserveCapacity(edgeList.count)
    for e in edgeList {
        let srcRef = intern(e.source)
        let tgtRef = intern(e.target)
        edgeRefs.append((srcRef, tgtRef, e.kind, e.confidencePct))
    }

    var importRefs: [(path: (UInt32, UInt32), kind: UInt8)] = []
    importRefs.reserveCapacity(imports.count)
    for entry in imports {
        importRefs.append((intern(entry.path), entry.kind))
    }

    // Phase 2: allocate and write buffer
    let nodeCount = UInt32(nodeRefs.count)
    let edgeCount = UInt32(edgeRefs.count)
    let importCount = UInt32(importRefs.count)
    let strTableOffset = UInt32(
        HEADER_SIZE + nodeRefs.count * PACKED_NODE_SIZE + edgeRefs.count * PACKED_EDGE_SIZE
            + importRefs.count * PACKED_IMPORT_SIZE
    )
    let totalSize = Int(strTableOffset) + stringTable.count

    let buf = malloc(totalSize)!  // must use malloc — freed via free() across FFI
    var pos = 0

    func writeU32(_ val: UInt32) {
        buf.storeBytes(of: val.littleEndian, toByteOffset: pos, as: UInt32.self)
        pos += 4
    }
    func writeU8(_ val: UInt8) {
        buf.storeBytes(of: val, toByteOffset: pos, as: UInt8.self)
        pos += 1
    }
    func pad(_ count: Int) {
        memset(buf.advanced(by: pos), 0, count)
        pos += count
    }

    // Header
    writeU32(BINARY_MAGIC)
    writeU8(BINARY_VERSION)
    pad(3)
    writeU32(nodeCount)
    writeU32(edgeCount)
    writeU32(importCount)
    writeU32(strTableOffset)

    // Nodes
    for node in nodeRefs {
        writeU32(node.id.0); writeU32(node.id.1)
        writeU32(node.name.0); writeU32(node.name.1)
        writeU32(node.file.0); writeU32(node.file.1)
        if let m = node.module {
            writeU32(m.0); writeU32(m.1)
        } else {
            writeU32(NO_MODULE); writeU32(0)
        }
        writeU32(node.line)
        writeU32(node.col)
        writeU32(node.endLine)
        writeU32(node.endCol)
        writeU8(node.kind)
        writeU8(node.visibility)
        pad(2)
    }

    // Edges
    for ref in edgeRefs {
        writeU32(ref.source.0); writeU32(ref.source.1)
        writeU32(ref.target.0); writeU32(ref.target.1)
        writeU8(ref.kind)
        writeU8(ref.confidencePct)
        pad(2)
    }

    for entry in importRefs {
        writeU32(entry.path.0); writeU32(entry.path.1)
        writeU8(entry.kind)
        pad(3)
    }

    // String table
    stringTable.withUnsafeBytes { rawBuf in
        buf.advanced(by: Int(strTableOffset)).copyMemory(
            from: rawBuf.baseAddress!,
            byteCount: stringTable.count
        )
    }

    return (buf, UInt32(totalSize))
}
