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

// MARK: - Extracted Data

private struct ExtractedNode {
    let id: String
    let kind: String
    let name: String
    let file: String
    let line: UInt32
    let col: UInt32
    let visibility: String
    let module: String?
}

private struct ExtractedEdge: Hashable {
    let source: String
    let target: String
    let kind: String
    let confidence: Double

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

private final class UnitCollector: @unchecked Sendable {
    var names: [String] = []
}

private final class DepCollector: @unchecked Sendable {
    var recordName: String?
}

private final class OccCollector: @unchecked Sendable {
    var nodes: [String: ExtractedNode] = [:]
    var edges: [ExtractedEdge] = []
    let fileName: String
    let moduleName: String?

    init(fileName: String, moduleName: String?) {
        self.fileName = fileName
        self.moduleName = moduleName
    }
}

private final class RelCollector: @unchecked Sendable {
    let symbolUSR: String
    let roles: UInt64
    var edges: [ExtractedEdge] = []

    init(symbolUSR: String, roles: UInt64) {
        self.symbolUSR = symbolUSR
        self.roles = roles
    }
}

// MARK: - IndexStoreReader


// MARK: - Callback State (file-level to avoid captures in @convention(c) callbacks)
// These are used as temporary storage during synchronous _apply_f iterations.
// Protected by _cbLock for thread safety.
nonisolated(unsafe) private var _cbLock = NSLock()
nonisolated(unsafe) private var _cbStore: indexstore_t? = nil
nonisolated(unsafe) private var _cbFileIndex: [String: UnitInfo] = [:]
nonisolated(unsafe) private var _cbSearchPath: String = ""
nonisolated(unsafe) private var _cbSearchFileName: String = ""
nonisolated(unsafe) private var _cbMatchedUnit: String? = nil
nonisolated(unsafe) private var _cbMatchedModule: String? = nil
nonisolated(unsafe) private var _cbRecordName: String? = nil
nonisolated(unsafe) private var _cbCollector: OccCollector? = nil
nonisolated(unsafe) private var _cbRelEdges: [ExtractedEdge] = []
nonisolated(unsafe) private var _cbRelSymbolUSR: String = ""
nonisolated(unsafe) private var _cbRelRoles: UInt64 = 0

/// Pre-built lookup: mainFile path → (unitName, moduleName)
private struct UnitInfo {
    let unitName: String
    let moduleName: String?
}

final class IndexStoreReader: @unchecked Sendable {
    private let store: indexstore_t
    /// Lazy file→unit index, built on first access
    private var fileIndex: [String: UnitInfo]?

    init?(storePath: String) {
        var err: indexstore_error_t?
        guard let store = storePath.withCString({ indexstore_store_create($0, &err) }) else {
            return nil
        }
        self.store = store
    }

    deinit {
        indexstore_store_dispose(store)
    }

    // MARK: - Public

    func extractFile(_ filePath: String) -> String? {
        _cbLock.lock()
        defer { _cbLock.unlock() }

        // Build the file index on first call (scans all units once)
        if fileIndex == nil {
            fileIndex = buildFileIndex()
        }

        let resolved = resolvePath(filePath)
        let fileName = (filePath as NSString).lastPathComponent

        // O(1) lookup instead of scanning 9,458 units
        let unitInfo = fileIndex?[resolved]
            ?? fileIndex?.values.first(where: { _ in
                // Fallback: check by filename suffix
                fileIndex?.keys.first(where: { $0.hasSuffix("/" + fileName) }).flatMap { fileIndex?[$0] } != nil
            })
            ?? findByFileName(fileName)

        guard let unitInfo else { return nil }
        guard let recordName = findRecordName(inUnit: unitInfo.unitName) else { return nil }

        let collector = readOccurrences(
            recordName: recordName,
            fileName: fileName,
            moduleName: unitInfo.moduleName
        )

        return buildJSON(nodes: Array(collector.nodes.values), edges: collector.edges)
    }

    // MARK: - File Index (built once)

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
            _cbFileIndex[mainFile] = UnitInfo(unitName: unitName, moduleName: mod.isEmpty ? nil : mod)
            return true
        }

        _ = indexstore_store_units_apply_f(store, 0, nil, cb)
        return _cbFileIndex
    }

    private func findByFileName(_ fileName: String) -> UnitInfo? {
        fileIndex?.first(where: { $0.key.hasSuffix("/" + fileName) })?.value
    }
    // MARK: - Record Discovery

    private func findRecordName(inUnit unitName: String) -> String? {
        guard let reader = unitName.withCString({
            indexstore_unit_reader_create(store, $0, nil)
        }) else {
            return nil
        }
        defer { indexstore_unit_reader_dispose(reader) }

        _cbRecordName = nil
        
        let cb: @convention(c) (UnsafeMutableRawPointer?, indexstore_unit_dependency_t?) -> Bool = {
            _, dep in
            guard let dep else { return true }
            if indexstore_unit_dependency_get_kind(dep) == 2 {
                let name = str(indexstore_unit_dependency_get_name(dep))
                if !name.isEmpty {
                    _cbRecordName = name
                }
            }
            return true
        }

        _ = indexstore_unit_reader_dependencies_apply_f(reader, nil, cb)
        return _cbRecordName
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

        _cbCollector = collector

        let cb: @convention(c) (UnsafeMutableRawPointer?, indexstore_occurrence_t?) -> Bool = {
            _, occ in
            guard let occ, let c = _cbCollector else { return true }
            processOccurrence(collector: c, occ: occ)
            return true
        }

        _ = indexstore_record_reader_occurrences_apply_f(reader, nil, cb)
        _cbCollector = nil
        return collector
    }
}

// MARK: - Occurrence Processing

private func processOccurrence(collector c: OccCollector, occ: indexstore_occurrence_t) {
    let symbol = indexstore_occurrence_get_symbol(occ)!
    let roles = indexstore_occurrence_get_roles(occ)
    let usr = str(indexstore_symbol_get_usr(symbol))
    guard !usr.isEmpty else { return }

    let name = str(indexstore_symbol_get_name(symbol))
    let kindRaw = indexstore_symbol_get_kind(symbol)

    var line: UInt32 = 0
    var col: UInt32 = 0
    indexstore_occurrence_get_line_col(occ, &line, &col)

    // Record definitions/declarations as nodes
    let isDefOrDecl = (roles & Roles.definition) != 0 || (roles & Roles.declaration) != 0
    if isDefOrDecl, let kind = mapSymbolKind(kindRaw) {
        c.nodes[usr] = ExtractedNode(
            id: usr, kind: kind, name: name, file: c.fileName,
            line: line, col: col, visibility: "public", module: c.moduleName
        )
    }

    // Extract edges from relations
    extractRelationEdges(collector: c, occ: occ, symbolUSR: usr, roles: roles)
}

private func extractRelationEdges(
    collector c: OccCollector,
    occ: indexstore_occurrence_t,
    symbolUSR: String,
    roles: UInt64
) {
    _cbRelEdges = []
    _cbRelSymbolUSR = symbolUSR
    _cbRelRoles = roles

    let cb: @convention(c) (UnsafeMutableRawPointer?, indexstore_symbol_relation_t?) -> Bool = {
        _, rel in
        guard let rel else { return true }
        let relSym = indexstore_symbol_relation_get_symbol(rel)!
        let relUSR = str(indexstore_symbol_get_usr(relSym))
        guard !relUSR.isEmpty else { return true }

        let relRoles = indexstore_symbol_relation_get_roles(rel)
        let combinedRoles = _cbRelRoles | relRoles

        if (combinedRoles & Roles.call) != 0 {
            _cbRelEdges.append(ExtractedEdge(
                source: relUSR, target: _cbRelSymbolUSR,
                kind: "calls", confidence: 1.0
            ))
        } else if (combinedRoles & Roles.containedBy) != 0 {
            _cbRelEdges.append(ExtractedEdge(
                source: relUSR, target: _cbRelSymbolUSR,
                kind: "contains", confidence: 1.0
            ))
        }

        if (combinedRoles & Roles.baseOf) != 0 {
            _cbRelEdges.append(ExtractedEdge(
                source: _cbRelSymbolUSR, target: relUSR,
                kind: "inherits", confidence: 1.0
            ))
        }

        if (combinedRoles & Roles.conformsTo) != 0 {
            _cbRelEdges.append(ExtractedEdge(
                source: _cbRelSymbolUSR, target: relUSR,
                kind: "implements", confidence: 1.0
            ))
        }

        if (combinedRoles & Roles.overrideOf) != 0 {
            _cbRelEdges.append(ExtractedEdge(
                source: _cbRelSymbolUSR, target: relUSR,
                kind: "implements", confidence: 0.9
            ))
        }

        if (combinedRoles & Roles.reference) != 0
            && (combinedRoles & Roles.call) == 0
            && (combinedRoles & Roles.containedBy) == 0
            && (combinedRoles & Roles.baseOf) == 0
            && (combinedRoles & Roles.conformsTo) == 0
        {
            _cbRelEdges.append(ExtractedEdge(
                source: _cbRelSymbolUSR, target: relUSR,
                kind: "type_ref", confidence: 0.9
            ))
        }

        return true
    }

    _ = indexstore_occurrence_relations_apply_f(occ, nil, cb)
    c.edges.append(contentsOf: _cbRelEdges)
}

// MARK: - Symbol Kind Mapping

private func mapSymbolKind(_ raw: UInt64) -> String? {
    // Values from LLVM IndexStore SymbolKind enum:
    // 0=Unknown 1=Module 2=Namespace 3=NamespaceAlias 4=Macro
    // 5=Enum 6=Struct 7=Class 8=Protocol 9=Extension 10=Union 11=TypeAlias
    // 12=Function 13=Variable 14=Field 15=EnumConstant
    // 16=InstanceMethod 17=ClassMethod 18=StaticMethod
    // 19=InstanceProperty 20=ClassProperty 21=StaticProperty
    // 22=Constructor 23=Destructor
    switch raw {
    case 5:  return "enum"
    case 6:  return "struct"
    case 7:  return "struct"     // Class → struct in grapha
    case 8:  return "protocol"
    case 9:  return "extension"
    case 11: return "type_alias"
    case 12: return "function"
    case 13: return "property"   // Variable → property
    case 14: return "field"
    case 15: return "variant"
    case 16: return "function"   // InstanceMethod
    case 17: return "function"   // ClassMethod
    case 18: return "function"   // StaticMethod
    case 19: return "property"   // InstanceProperty
    case 20: return "property"   // ClassProperty
    case 21: return "property"   // StaticProperty
    case 22: return "function"   // Constructor
    case 23: return "function"   // Destructor
    default: return nil
    }
}

// MARK: - Helpers

private func resolvePath(_ path: String) -> String {
    if path.hasPrefix("/") { return path }
    return URL(fileURLWithPath: path).standardized.path
}

private func buildJSON(nodes: [ExtractedNode], edges: [ExtractedEdge]) -> String {
    var nodeEntries: [String] = []
    for n in nodes {
        var e = "{"
        e += "\"id\":\(esc(n.id)),"
        e += "\"kind\":\(esc(n.kind)),"
        e += "\"name\":\(esc(n.name)),"
        e += "\"file\":\(esc(n.file)),"
        e += "\"span\":{\"start\":[\(n.line),\(n.col)],\"end\":[\(n.line),\(n.col)]},"
        e += "\"visibility\":\(esc(n.visibility)),"
        e += "\"metadata\":{}"
        if let m = n.module { e += ",\"module\":\(esc(m))" }
        e += "}"
        nodeEntries.append(e)
    }

    // Deduplicate edges
    var seen = Set<ExtractedEdge>()
    var edgeEntries: [String] = []
    for edge in edges {
        guard seen.insert(edge).inserted else { continue }
        var e = "{"
        e += "\"source\":\(esc(edge.source)),"
        e += "\"target\":\(esc(edge.target)),"
        e += "\"kind\":\(esc(edge.kind)),"
        e += "\"confidence\":\(edge.confidence)"
        e += "}"
        edgeEntries.append(e)
    }

    return "{\"nodes\":[\(nodeEntries.joined(separator: ","))],\"edges\":[\(edgeEntries.joined(separator: ","))],\"imports\":[]}"
}

private func esc(_ s: String) -> String {
    let escaped = s
        .replacingOccurrences(of: "\\", with: "\\\\")
        .replacingOccurrences(of: "\"", with: "\\\"")
        .replacingOccurrences(of: "\n", with: "\\n")
        .replacingOccurrences(of: "\r", with: "\\r")
        .replacingOccurrences(of: "\t", with: "\\t")
    return "\"\(escaped)\""
}
