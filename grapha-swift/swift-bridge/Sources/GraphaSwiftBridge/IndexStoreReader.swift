import Foundation

// MARK: - IndexStore C API Types & Role Constants

private typealias IndexStoreError = OpaquePointer

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

// MARK: - Function Pointer Types (loaded via dlsym)

private typealias StoreCreateFn = @convention(c) (
    UnsafePointer<CChar>, UnsafeMutablePointer<IndexStoreError?>?
) -> OpaquePointer?

private typealias StoreDisposeFn = @convention(c) (OpaquePointer) -> Void

private typealias RecordReaderCreateFn = @convention(c) (
    OpaquePointer, UnsafePointer<CChar>, UnsafeMutablePointer<IndexStoreError?>?
) -> OpaquePointer?

private typealias HandleDisposeFn = @convention(c) (OpaquePointer) -> Void

private typealias UnitReaderCreateFn = @convention(c) (
    OpaquePointer, UnsafePointer<CChar>, UnsafeMutablePointer<IndexStoreError?>?
) -> OpaquePointer?

private typealias UnitGetStringFn = @convention(c) (OpaquePointer) -> UnsafePointer<CChar>

private typealias DepGetKindFn = @convention(c) (OpaquePointer) -> Int32
private typealias DepGetStringFn = @convention(c) (OpaquePointer) -> UnsafePointer<CChar>

private typealias OccGetSymbolFn = @convention(c) (OpaquePointer) -> OpaquePointer
private typealias OccGetRolesFn = @convention(c) (OpaquePointer) -> UInt64
private typealias OccGetLineColFn = @convention(c) (
    OpaquePointer, UnsafeMutablePointer<UInt32>, UnsafeMutablePointer<UInt32>
) -> Void

private typealias SymGetStringFn = @convention(c) (OpaquePointer) -> UnsafePointer<CChar>
private typealias SymGetKindFn = @convention(c) (OpaquePointer) -> UInt32

// apply_f callbacks: (handle, ctx, fn(ctx, item) -> Bool) -> Bool
private typealias UnitsApplyFn = @convention(c) (
    OpaquePointer, Int32, UnsafeMutableRawPointer?,
    @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>, Int) -> Bool
) -> Bool

private typealias DepsApplyFn = @convention(c) (
    OpaquePointer, UnsafeMutableRawPointer?,
    @convention(c) (UnsafeMutableRawPointer?, OpaquePointer) -> Bool
) -> Bool

private typealias OccApplyFn = @convention(c) (
    OpaquePointer, UnsafeMutableRawPointer?,
    @convention(c) (UnsafeMutableRawPointer?, OpaquePointer) -> Bool
) -> Bool

private typealias RelApplyFn = @convention(c) (
    OpaquePointer, UnsafeMutableRawPointer?,
    @convention(c) (UnsafeMutableRawPointer?, OpaquePointer) -> Bool
) -> Bool

// MARK: - Function Table

private struct FnTable: @unchecked Sendable {
    let storeCreate: StoreCreateFn
    let storeDispose: StoreDisposeFn
    let recordCreate: RecordReaderCreateFn
    let recordDispose: HandleDisposeFn
    let unitCreate: UnitReaderCreateFn
    let unitDispose: HandleDisposeFn
    let unitGetMainFile: UnitGetStringFn
    let unitGetModule: UnitGetStringFn
    let depGetKind: DepGetKindFn
    let depGetName: DepGetStringFn
    let occGetSymbol: OccGetSymbolFn
    let occGetRoles: OccGetRolesFn
    let occGetLineCol: OccGetLineColFn
    let symGetName: SymGetStringFn
    let symGetUSR: SymGetStringFn
    let symGetKind: SymGetKindFn
    let relGetSymbol: OccGetSymbolFn
    let relGetRoles: OccGetRolesFn
    // apply_f variants loaded as raw pointers
    let unitsApply: UnitsApplyFn
    let depsApply: DepsApplyFn
    let occApply: OccApplyFn
    let relApply: RelApplyFn
}

private func loadFnTable(from lib: UnsafeMutableRawPointer) -> FnTable? {
    func sym<T>(_ name: String) -> T? {
        dlsym(lib, name).map { unsafeBitCast($0, to: T.self) }
    }

    guard
        let storeCreate: StoreCreateFn = sym("indexstore_store_create"),
        let storeDispose: StoreDisposeFn = sym("indexstore_store_dispose"),
        let recordCreate: RecordReaderCreateFn = sym("indexstore_record_reader_create"),
        let recordDispose: HandleDisposeFn = sym("indexstore_record_reader_dispose"),
        let unitCreate: UnitReaderCreateFn = sym("indexstore_unit_reader_create"),
        let unitDispose: HandleDisposeFn = sym("indexstore_unit_reader_dispose"),
        let unitGetMainFile: UnitGetStringFn = sym("indexstore_unit_reader_get_main_file"),
        let unitGetModule: UnitGetStringFn = sym("indexstore_unit_reader_get_module_name"),
        let depGetKind: DepGetKindFn = sym("indexstore_unit_dependency_get_kind"),
        let depGetName: DepGetStringFn = sym("indexstore_unit_dependency_get_name"),
        let occGetSymbol: OccGetSymbolFn = sym("indexstore_occurrence_get_symbol"),
        let occGetRoles: OccGetRolesFn = sym("indexstore_occurrence_get_roles"),
        let occGetLineCol: OccGetLineColFn = sym("indexstore_occurrence_get_line_col"),
        let symGetName: SymGetStringFn = sym("indexstore_symbol_get_name"),
        let symGetUSR: SymGetStringFn = sym("indexstore_symbol_get_usr"),
        let symGetKind: SymGetKindFn = sym("indexstore_symbol_get_kind"),
        let relGetSymbol: OccGetSymbolFn = sym("indexstore_symbol_relation_get_symbol"),
        let relGetRoles: OccGetRolesFn = sym("indexstore_symbol_relation_get_roles"),
        let unitsApply: UnitsApplyFn = sym("indexstore_store_units_apply_f"),
        let depsApply: DepsApplyFn = sym("indexstore_unit_reader_dependencies_apply_f"),
        let occApply: OccApplyFn = sym("indexstore_record_reader_occurrences_apply_f"),
        let relApply: RelApplyFn = sym("indexstore_occurrence_relations_apply_f")
    else {
        return nil
    }

    return FnTable(
        storeCreate: storeCreate, storeDispose: storeDispose,
        recordCreate: recordCreate, recordDispose: recordDispose,
        unitCreate: unitCreate, unitDispose: unitDispose,
        unitGetMainFile: unitGetMainFile, unitGetModule: unitGetModule,
        depGetKind: depGetKind, depGetName: depGetName,
        occGetSymbol: occGetSymbol, occGetRoles: occGetRoles,
        occGetLineCol: occGetLineCol,
        symGetName: symGetName, symGetUSR: symGetUSR, symGetKind: symGetKind,
        relGetSymbol: relGetSymbol, relGetRoles: relGetRoles,
        unitsApply: unitsApply, depsApply: depsApply,
        occApply: occApply, relApply: relApply
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

// MARK: - Occurrence Collector (passed as context to C callbacks)

private final class OccCollector {
    let fn: FnTable
    var nodes: [String: ExtractedNode] = [:]
    var edges: [ExtractedEdge] = []
    let fileName: String
    let moduleName: String?

    init(fn: FnTable, fileName: String, moduleName: String?) {
        self.fn = fn
        self.fileName = fileName
        self.moduleName = moduleName
    }
}

// MARK: - IndexStoreReader

final class IndexStoreReader: @unchecked Sendable {
    private let fn: FnTable
    private let store: OpaquePointer
    private let lib: UnsafeMutableRawPointer

    init?(storePath: String) {
        let dylibPath = "/Applications/Xcode.app/Contents/Developer/Toolchains/"
            + "XcodeDefault.xctoolchain/usr/lib/libIndexStore.dylib"
        guard let lib = dlopen(dylibPath, RTLD_LAZY) else {
            let err = String(cString: dlerror())
            return nil
        }
        guard let fn = loadFnTable(from: lib) else {
            dlclose(lib)
            return nil
        }
        var errPtr: OpaquePointer?
        guard let store = storePath.withCString({ fn.storeCreate($0, &errPtr) }) else {
            if let errPtr {
                // Try to get error description if we have the function
                if let errDescSym = dlsym(lib, "indexstore_error_get_description") {
                    typealias ErrDescFn = @convention(c) (OpaquePointer) -> UnsafePointer<CChar>
                    let getDesc = unsafeBitCast(errDescSym, to: ErrDescFn.self)
                    let desc = String(cString: getDesc(errPtr))
                }
            } else {
            }
            dlclose(lib)
            return nil
        }

        self.lib = lib
        self.fn = fn
        self.store = store
    }

    deinit {
        fn.storeDispose(store)
        dlclose(lib)
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
        final class Ctx {
            var names: [String] = []
        }

        let ctx = Ctx()
        let ptr = Unmanaged.passUnretained(ctx).toOpaque()

        let cb: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>, Int) -> Bool = {
            raw, data, len in
            guard let raw else { return true }
            let c = Unmanaged<Ctx>.fromOpaque(raw).takeUnretainedValue()
            let buf = UnsafeRawBufferPointer(start: data, count: len)
            if let s = String(bytes: buf, encoding: .utf8) {
                c.names.append(s)
            }
            return true
        }

        _ = fn.unitsApply(store, 0, ptr, cb)

        let fileName = (path as NSString).lastPathComponent

        for name in ctx.names {
            guard let reader = name.withCString({ fn.unitCreate(store, $0, nil) }) else { continue }
            defer { fn.unitDispose(reader) }

            let mainFile = String(cString: fn.unitGetMainFile(reader))
            if mainFile == path || mainFile.hasSuffix("/" + fileName) {
                let mod = String(cString: fn.unitGetModule(reader))
                return (name, mod.isEmpty ? nil : mod)
            }
        }

        return nil
    }

    // MARK: - Record Discovery

    private func findRecordName(inUnit unitName: String) -> String? {
        guard let reader = unitName.withCString({ fn.unitCreate(store, $0, nil) }) else {
            return nil
        }
        defer { fn.unitDispose(reader) }

        final class Ctx {
            let depGetKind: DepGetKindFn
            let depGetName: DepGetStringFn
            var recordName: String?

            init(depGetKind: DepGetKindFn, depGetName: DepGetStringFn) {
                self.depGetKind = depGetKind
                self.depGetName = depGetName
            }
        }

        let ctx = Ctx(depGetKind: fn.depGetKind, depGetName: fn.depGetName)
        let ptr = Unmanaged.passUnretained(ctx).toOpaque()

        let cb: @convention(c) (UnsafeMutableRawPointer?, OpaquePointer) -> Bool = { raw, dep in
            guard let raw else { return true }
            let c = Unmanaged<Ctx>.fromOpaque(raw).takeUnretainedValue()
            // kind 1 = record dependency
            if c.depGetKind(dep) == 1 {
                let name = String(cString: c.depGetName(dep))
                if !name.isEmpty {
                    c.recordName = name
                    return false // found it, stop
                }
            }
            return true
        }

        _ = fn.depsApply(reader, ptr, cb)
        return ctx.recordName
    }

    // MARK: - Occurrence Reading

    private func readOccurrences(
        recordName: String,
        fileName: String,
        moduleName: String?
    ) -> OccCollector {
        let collector = OccCollector(fn: fn, fileName: fileName, moduleName: moduleName)

        guard let reader = recordName.withCString({ fn.recordCreate(store, $0, nil) }) else {
            return collector
        }
        defer { fn.recordDispose(reader) }

        let ptr = Unmanaged.passUnretained(collector).toOpaque()

        let cb: @convention(c) (UnsafeMutableRawPointer?, OpaquePointer) -> Bool = { raw, occ in
            guard let raw else { return true }
            let c = Unmanaged<OccCollector>.fromOpaque(raw).takeUnretainedValue()
            processOccurrence(collector: c, occ: occ)
            return true
        }

        _ = fn.occApply(reader, ptr, cb)
        return collector
    }
}

// MARK: - Occurrence Processing

private func processOccurrence(collector c: OccCollector, occ: OpaquePointer) {
    let symbol = c.fn.occGetSymbol(occ)
    let roles = c.fn.occGetRoles(occ)
    let usr = String(cString: c.fn.symGetUSR(symbol))
    guard !usr.isEmpty else { return }

    let name = String(cString: c.fn.symGetName(symbol))
    let kindRaw = c.fn.symGetKind(symbol)

    var line: UInt32 = 0
    var col: UInt32 = 0
    c.fn.occGetLineCol(occ, &line, &col)

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
    occ: OpaquePointer,
    symbolUSR: String,
    roles: UInt64
) {
    final class RelCtx {
        let fn: FnTable
        let symbolUSR: String
        let roles: UInt64
        var edges: [ExtractedEdge] = []

        init(fn: FnTable, symbolUSR: String, roles: UInt64) {
            self.fn = fn
            self.symbolUSR = symbolUSR
            self.roles = roles
        }
    }

    let ctx = RelCtx(fn: c.fn, symbolUSR: symbolUSR, roles: roles)
    let ptr = Unmanaged.passUnretained(ctx).toOpaque()

    let cb: @convention(c) (UnsafeMutableRawPointer?, OpaquePointer) -> Bool = { raw, rel in
        guard let raw else { return true }
        let ctx = Unmanaged<RelCtx>.fromOpaque(raw).takeUnretainedValue()
        let relSym = ctx.fn.relGetSymbol(rel)
        let relUSR = String(cString: ctx.fn.symGetUSR(relSym))
        guard !relUSR.isEmpty else { return true }

        let relRoles = ctx.fn.relGetRoles(rel)
        let combinedRoles = ctx.roles | relRoles

        if (combinedRoles & Roles.call) != 0 {
            // For call occurrences, the relation symbol is the caller (container)
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

    _ = c.fn.relApply(occ, ptr, cb)
    c.edges.append(contentsOf: ctx.edges)
}

// MARK: - Symbol Kind Mapping

private func mapSymbolKind(_ raw: UInt32) -> String? {
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
