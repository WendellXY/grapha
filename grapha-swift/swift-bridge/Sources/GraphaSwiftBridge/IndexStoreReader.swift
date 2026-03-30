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

final class IndexStoreReader: @unchecked Sendable {
    private let store: UnsafeMutableRawPointer

    init?(storePath: String) {
        var err: UnsafeMutableRawPointer?
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
        let resolved = resolvePath(filePath)

        guard let (unitName, moduleName) = findUnit(forFile: resolved) else { return nil }
        guard let recordName = findRecordName(inUnit: unitName) else { return nil }

        let collector = readOccurrences(
            recordName: recordName,
            fileName: (filePath as NSString).lastPathComponent,
            moduleName: moduleName
        )

        return buildJSON(nodes: Array(collector.nodes.values), edges: collector.edges)
    }

    // MARK: - Unit Discovery

    private func findUnit(forFile path: String) -> (unitName: String, module: String?)? {
        let ctx = UnitCollector()
        let ptr = Unmanaged.passUnretained(ctx).toOpaque()

        let cb: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?, Int) -> Bool = {
            raw, data, len in
            guard let raw, let data else { return true }
            let c = Unmanaged<UnitCollector>.fromOpaque(raw).takeUnretainedValue()
            let buf = UnsafeRawBufferPointer(start: data, count: len)
            if let s = String(bytes: buf, encoding: .utf8) {
                c.names.append(s)
            }
            return true
        }

        _ = indexstore_store_units_apply_f(store, 0, ptr, cb)

        let fileName = (path as NSString).lastPathComponent

        for name in ctx.names {
            guard let reader = name.withCString({
                indexstore_unit_reader_create(store, $0, nil)
            }) else { continue }
            defer { indexstore_unit_reader_dispose(reader) }

            let mainFile = str(indexstore_unit_reader_get_main_file(reader))
            if mainFile == path || mainFile.hasSuffix("/" + fileName) {
                let mod = str(indexstore_unit_reader_get_module_name(reader))
                return (name, mod.isEmpty ? nil : mod)
            }
        }

        return nil
    }

    // MARK: - Record Discovery

    private func findRecordName(inUnit unitName: String) -> String? {
        guard let reader = unitName.withCString({
            indexstore_unit_reader_create(store, $0, nil)
        }) else {
            return nil
        }
        defer { indexstore_unit_reader_dispose(reader) }

        let ctx = DepCollector()
        let ptr = Unmanaged.passUnretained(ctx).toOpaque()

        let cb: @convention(c) (UnsafeMutableRawPointer?, UnsafeMutableRawPointer?) -> Bool = {
            raw, dep in
            guard let raw, let dep else { return true }
            let c = Unmanaged<DepCollector>.fromOpaque(raw).takeUnretainedValue()
            // kind 1 = record dependency
            if indexstore_unit_dependency_get_kind(dep) == 1 {
                let name = str(indexstore_unit_dependency_get_name(dep))
                if !name.isEmpty {
                    c.recordName = name
                    return false // found it, stop
                }
            }
            return true
        }

        _ = indexstore_unit_reader_dependencies_apply_f(reader, ptr, cb)
        return ctx.recordName
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

        let ptr = Unmanaged.passUnretained(collector).toOpaque()

        let cb: @convention(c) (UnsafeMutableRawPointer?, UnsafeMutableRawPointer?) -> Bool = {
            raw, occ in
            guard let raw, let occ else { return true }
            let c = Unmanaged<OccCollector>.fromOpaque(raw).takeUnretainedValue()
            processOccurrence(collector: c, occ: occ)
            return true
        }

        _ = indexstore_record_reader_occurrences_apply_f(reader, ptr, cb)
        return collector
    }
}

// MARK: - Occurrence Processing

private func processOccurrence(collector c: OccCollector, occ: UnsafeMutableRawPointer) {
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
    occ: UnsafeMutableRawPointer,
    symbolUSR: String,
    roles: UInt64
) {
    let ctx = RelCollector(symbolUSR: symbolUSR, roles: roles)
    let ptr = Unmanaged.passUnretained(ctx).toOpaque()

    let cb: @convention(c) (UnsafeMutableRawPointer?, UnsafeMutableRawPointer?) -> Bool = {
        raw, rel in
        guard let raw, let rel else { return true }
        let ctx = Unmanaged<RelCollector>.fromOpaque(raw).takeUnretainedValue()
        let relSym = indexstore_symbol_relation_get_symbol(rel)!
        let relUSR = str(indexstore_symbol_get_usr(relSym))
        guard !relUSR.isEmpty else { return true }

        let relRoles = indexstore_symbol_relation_get_roles(rel)
        let combinedRoles = ctx.roles | relRoles

        if (combinedRoles & Roles.call) != 0 {
            ctx.edges.append(ExtractedEdge(
                source: relUSR, target: ctx.symbolUSR,
                kind: "calls", confidence: 1.0
            ))
        } else if (combinedRoles & Roles.containedBy) != 0 {
            ctx.edges.append(ExtractedEdge(
                source: relUSR, target: ctx.symbolUSR,
                kind: "contains", confidence: 1.0
            ))
        }

        if (combinedRoles & Roles.baseOf) != 0 {
            ctx.edges.append(ExtractedEdge(
                source: ctx.symbolUSR, target: relUSR,
                kind: "inherits", confidence: 1.0
            ))
        }

        if (combinedRoles & Roles.conformsTo) != 0 {
            ctx.edges.append(ExtractedEdge(
                source: ctx.symbolUSR, target: relUSR,
                kind: "implements", confidence: 1.0
            ))
        }

        if (combinedRoles & Roles.overrideOf) != 0 {
            ctx.edges.append(ExtractedEdge(
                source: ctx.symbolUSR, target: relUSR,
                kind: "implements", confidence: 0.9
            ))
        }

        if (combinedRoles & Roles.reference) != 0
            && (combinedRoles & Roles.call) == 0
            && (combinedRoles & Roles.containedBy) == 0
            && (combinedRoles & Roles.baseOf) == 0
            && (combinedRoles & Roles.conformsTo) == 0
        {
            ctx.edges.append(ExtractedEdge(
                source: ctx.symbolUSR, target: relUSR,
                kind: "type_ref", confidence: 0.9
            ))
        }

        return true
    }

    _ = indexstore_occurrence_relations_apply_f(occ, ptr, cb)
    c.edges.append(contentsOf: ctx.edges)
}

// MARK: - Symbol Kind Mapping

private func mapSymbolKind(_ raw: UInt64) -> String? {
    switch raw {
    case 4: return "struct"      // class
    case 5: return "struct"      // struct
    case 6: return "enum"        // enum
    case 7: return "protocol"    // protocol
    case 8: return "extension"   // extension
    case 10: return "type_alias" // typealias
    case 11: return "function"   // free function
    case 13: return "field"      // field
    case 15: return "variant"    // enum constant
    case 17: return "function"   // instance method
    case 18: return "function"   // class method
    case 19: return "function"   // static method
    case 20: return "property"   // instance property
    case 21: return "property"   // class property
    case 22: return "property"   // static property
    case 25: return "function"   // constructor
    case 26: return "function"   // destructor
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
