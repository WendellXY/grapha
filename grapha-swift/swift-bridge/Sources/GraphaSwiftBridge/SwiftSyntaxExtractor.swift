import CodableKit
import Foundation
import SwiftParser
import SwiftSyntax

private let swiftSyntaxCallConfidence = 0.9
private let swiftSyntaxPropertyAccessConfidence = 0.6
private let swiftSyntaxInheritanceConfidence = 0.9
private let swiftSyntaxImportConfidence = 0.7

struct BridgeNodeRole: Encodable {
    let type: String

    static let entryPoint = BridgeNodeRole(type: "entry_point")
}

enum BridgeNodeKind: String, Encodable {
    case function
    case `struct`
    case `enum`
    case `protocol`
    case `extension`
    case field
    case variant
    case property
    case constant
    case typeAlias = "type_alias"
}

enum BridgeEdgeKind: String, Encodable {
    case calls
    case uses
    case implements
    case contains
    case typeRef = "type_ref"
    case inherits
}

enum BridgeVisibility: String, Encodable {
    case `public`
    case crate
    case `private`
}

enum BridgeImportKind: String, Encodable {
    case named
    case wildcard
    case module
    case relative
}

private struct BridgeEdgeKey: Hashable {
    let source: String
    let target: String
    let kind: BridgeEdgeKind
    let operation: String?
}

@Encodable
struct BridgeSpan {
    let start: [Int]
    let end: [Int]
}

@Encodable
struct BridgeEdgeProvenance {
    let file: String
    let span: BridgeSpan
    let symbol_id: String
}

@Encodable
struct BridgeNode {
    let id: String
    let kind: BridgeNodeKind
    let name: String
    let file: String
    let span: BridgeSpan
    let visibility: BridgeVisibility
    let metadata: [String: String]
    var role: BridgeNodeRole?
    var signature: String?
    var doc_comment: String?
    var module: String?
}

@Encodable
struct BridgeEdge {
    let source: String
    let target: String
    let kind: BridgeEdgeKind
    let confidence: Double
    var direction: String?
    var operation: String?
    var condition: String?
    var async_boundary: Bool?
    var provenance: [BridgeEdgeProvenance]
}

@Encodable
struct BridgeImport {
    let path: String
    let symbols: [String]
    let kind: BridgeImportKind
}

@Encodable
struct BridgeExtractionResult {
    var nodes: [BridgeNode] = []
    var edges: [BridgeEdge] = []
    var imports: [BridgeImport] = []
}

private final class CallCollector: SyntaxVisitor {
    private let filePath: String
    private let converter: SourceLocationConverter
    private let callerID: String
    private var seen = Set<BridgeEdgeKey>()
    private(set) var edges: [BridgeEdge] = []

    init(filePath: String, converter: SourceLocationConverter, callerID: String) {
        self.filePath = filePath
        self.converter = converter
        self.callerID = callerID
        super.init(viewMode: .sourceAccurate)
    }

    override func visit(_ node: FunctionCallExprSyntax) -> SyntaxVisitorContinueKind {
        guard let (targetName, operation) = callTarget(from: node.calledExpression) else {
            return .visitChildren
        }
        emit(
            target: "\(filePath)::\(targetName)",
            kind: .calls,
            confidence: swiftSyntaxCallConfidence,
            operation: operation,
            syntax: Syntax(node)
        )
        return .visitChildren
    }

    override func visit(_ node: MemberAccessExprSyntax) -> SyntaxVisitorContinueKind {
        if shouldSkipMemberAccess(node) {
            return .visitChildren
        }

        let targetName = normalizeIdentifier(node.declName.baseName.text)
        guard !targetName.isEmpty else {
            return .visitChildren
        }

        emit(
            target: "\(filePath)::\(targetName)",
            kind: .calls,
            confidence: swiftSyntaxPropertyAccessConfidence,
            operation: node.base.map { Syntax($0).trimmedDescription },
            syntax: Syntax(node)
        )
        return .visitChildren
    }

    private func emit(
        target: String,
        kind: BridgeEdgeKind,
        confidence: Double,
        operation: String?,
        syntax: Syntax
    ) {
        let key = BridgeEdgeKey(
            source: callerID,
            target: target,
            kind: kind,
            operation: operation
        )
        if !seen.insert(key).inserted {
            return
        }
        edges.append(
            BridgeEdge(
                source: callerID,
                target: target,
                kind: kind,
                confidence: confidence,
                direction: nil,
                operation: operation,
                condition: nil,
                async_boundary: nil,
                provenance: [
                    makeProvenance(
                        filePath: filePath,
                        converter: converter,
                        syntax: syntax,
                        symbolID: callerID
                    ),
                ]
            )
        )
    }

    private func shouldSkipMemberAccess(_ node: MemberAccessExprSyntax) -> Bool {
        if let parent = node.parent?.as(FunctionCallExprSyntax.self) {
            let calledExpression = Syntax(parent.calledExpression)
            let member = Syntax(node)
            if calledExpression.position == member.position
                && calledExpression.endPosition == member.endPosition
            {
                return true
            }
        }

        return node.parent?.is(MemberAccessExprSyntax.self) == true
    }

    private func callTarget(from expression: ExprSyntax) -> (String, String?)? {
        if let declRef = expression.as(DeclReferenceExprSyntax.self) {
            let name = normalizeIdentifier(declRef.baseName.text)
            return name.isEmpty ? nil : (name, nil)
        }

        if let memberAccess = expression.as(MemberAccessExprSyntax.self) {
            let name = normalizeIdentifier(memberAccess.declName.baseName.text)
            guard !name.isEmpty else {
                return nil
            }
            let operation = memberAccess.base.map { Syntax($0).trimmedDescription }
            return (name, operation)
        }

        return nil
    }
}

private final class SwiftSyntaxExtractor {
    private let filePath: String
    private let tree: SourceFileSyntax
    private let converter: SourceLocationConverter
    private var result = BridgeExtractionResult()
    private var existingNodeIDs = Set<String>()
    private var existingEdgeKeys = Set<BridgeEdgeKey>()

    init(source: String, filePath: String) {
        self.filePath = filePath
        self.tree = Parser.parse(source: source)
        self.converter = SourceLocationConverter(fileName: filePath, tree: tree)
    }

    func extract() -> BridgeExtractionResult {
        for statement in tree.statements {
            if case .decl(let decl) = statement.item {
                processDecl(decl, parentID: nil, parentConformsToView: false, parentIsObservable: false)
            }
        }
        return result
    }

    private func processDecl(
        _ decl: DeclSyntax,
        parentID: String?,
        parentConformsToView: Bool,
        parentIsObservable: Bool
    ) {
        if let node = decl.as(StructDeclSyntax.self) {
            processNominal(
                name: normalizeIdentifier(node.name.text),
                kind: .struct,
                syntax: Syntax(node),
                modifiers: node.modifiers,
                attributes: node.attributes,
                inheritanceClause: node.inheritanceClause,
                memberBlock: node.memberBlock,
                parentID: parentID,
                canUseSuperclassHeuristic: false
            )
            return
        }

        if let node = decl.as(ClassDeclSyntax.self) {
            processNominal(
                name: normalizeIdentifier(node.name.text),
                kind: .struct,
                syntax: Syntax(node),
                modifiers: node.modifiers,
                attributes: node.attributes,
                inheritanceClause: node.inheritanceClause,
                memberBlock: node.memberBlock,
                parentID: parentID,
                canUseSuperclassHeuristic: true
            )
            return
        }

        if let node = decl.as(EnumDeclSyntax.self) {
            processNominal(
                name: normalizeIdentifier(node.name.text),
                kind: .enum,
                syntax: Syntax(node),
                modifiers: node.modifiers,
                attributes: node.attributes,
                inheritanceClause: node.inheritanceClause,
                memberBlock: node.memberBlock,
                parentID: parentID,
                canUseSuperclassHeuristic: false
            )
            return
        }

        if let node = decl.as(ProtocolDeclSyntax.self) {
            processNominal(
                name: normalizeIdentifier(node.name.text),
                kind: .protocol,
                syntax: Syntax(node),
                modifiers: node.modifiers,
                attributes: node.attributes,
                inheritanceClause: node.inheritanceClause,
                memberBlock: node.memberBlock,
                parentID: parentID,
                canUseSuperclassHeuristic: false
            )
            return
        }

        if let node = decl.as(ExtensionDeclSyntax.self) {
            processExtension(node, parentID: parentID)
            return
        }

        if let node = decl.as(FunctionDeclSyntax.self) {
            processFunction(
                name: normalizeIdentifier(node.name.text),
                syntax: Syntax(node),
                modifiers: node.modifiers,
                body: node.body.map(Syntax.init),
                parentID: parentID,
                entryPointHint: parentIsObservable,
                signature: signatureText(for: Syntax(node))
            )
            return
        }

        if let node = decl.as(InitializerDeclSyntax.self) {
            processFunction(
                name: "init",
                syntax: Syntax(node),
                modifiers: node.modifiers,
                body: node.body.map(Syntax.init),
                parentID: parentID,
                entryPointHint: parentIsObservable,
                signature: signatureText(for: Syntax(node))
            )
            return
        }

        if let node = decl.as(DeinitializerDeclSyntax.self) {
            processFunction(
                name: "deinit",
                syntax: Syntax(node),
                modifiers: DeclModifierListSyntax([]),
                body: node.body.map(Syntax.init),
                parentID: parentID,
                entryPointHint: false,
                signature: signatureText(for: Syntax(node))
            )
            return
        }

        if let node = decl.as(VariableDeclSyntax.self) {
            processVariableDecl(
                node,
                parentID: parentID,
                asEntryPoint: parentConformsToView
            )
            return
        }

        if let node = decl.as(EnumCaseDeclSyntax.self) {
            processEnumCaseDecl(node, parentID: parentID)
            return
        }

        if let node = decl.as(TypeAliasDeclSyntax.self) {
            processTypeAlias(node, parentID: parentID)
            return
        }

        if let node = decl.as(ImportDeclSyntax.self) {
            processImport(node)
            return
        }
    }

    private func processNominal(
        name: String,
        kind: BridgeNodeKind,
        syntax: Syntax,
        modifiers: DeclModifierListSyntax,
        attributes: AttributeListSyntax,
        inheritanceClause: InheritanceClauseSyntax?,
        memberBlock: MemberBlockSyntax,
        parentID: String?,
        canUseSuperclassHeuristic: Bool
    ) {
        guard !name.isEmpty else {
            return
        }

        let id = uniqueDeclID(proposed: makeDeclID(parentID: parentID, name: name), syntax: syntax)
        let role = hasAttribute(attributes, named: "main") ? BridgeNodeRole.entryPoint : nil
        let conformances = inheritanceNames(inheritanceClause)
        let conformsToView = conformances.contains("View") || conformances.contains("App")
        let isObservable = conformances.contains("ObservableObject")
            || hasAttribute(attributes, named: "Observable")

        pushNode(
            BridgeNode(
                id: id,
                kind: kind,
                name: name,
                file: filePath,
                span: span(for: syntax),
                visibility: visibility(from: modifiers),
                metadata: [:],
                role: role,
                signature: nil,
                doc_comment: nil,
                module: nil
            )
        )

        emitContainsEdge(parentID: parentID, childID: id, syntax: syntax)
        emitInheritanceEdges(
            typeID: id,
            inheritanceClause: inheritanceClause,
            syntax: syntax,
            canUseSuperclassHeuristic: canUseSuperclassHeuristic
        )

        for member in memberBlock.members {
            let childParentConformsToView = conformsToView && kind == .struct
            processDecl(
                member.decl,
                parentID: id,
                parentConformsToView: childParentConformsToView,
                parentIsObservable: isObservable
            )
        }
    }

    private func processExtension(_ decl: ExtensionDeclSyntax, parentID: String?) {
        let typeName = normalizeIdentifier(decl.extendedType.trimmedDescription)
        guard !typeName.isEmpty else {
            return
        }

        let id = uniqueDeclID(
            proposed: makeDeclID(parentID: parentID, name: "ext_\(typeName)"),
            syntax: Syntax(decl)
        )

        pushNode(
            BridgeNode(
                id: id,
                kind: .extension,
                name: typeName,
                file: filePath,
                span: span(for: Syntax(decl)),
                visibility: .crate,
                metadata: [:],
                role: nil,
                signature: nil,
                doc_comment: nil,
                module: nil
            )
        )

        emitContainsEdge(parentID: parentID, childID: id, syntax: Syntax(decl))
        emitInheritanceEdges(
            typeID: id,
            inheritanceClause: decl.inheritanceClause,
            syntax: Syntax(decl),
            canUseSuperclassHeuristic: false
        )

        for member in decl.memberBlock.members {
            processDecl(
                member.decl,
                parentID: id,
                parentConformsToView: false,
                parentIsObservable: false
            )
        }
    }

    private func processFunction(
        name: String,
        syntax: Syntax,
        modifiers: DeclModifierListSyntax,
        body: Syntax?,
        parentID: String?,
        entryPointHint: Bool,
        signature: String?
    ) {
        guard !name.isEmpty else {
            return
        }

        let id = uniqueDeclID(proposed: makeDeclID(parentID: parentID, name: name), syntax: syntax)
        let visibility = visibility(from: modifiers)
        let role = entryPointHint && visibility != .private ? BridgeNodeRole.entryPoint : nil

        pushNode(
            BridgeNode(
                id: id,
                kind: .function,
                name: name,
                file: filePath,
                span: span(for: syntax),
                visibility: visibility,
                metadata: [:],
                role: role,
                signature: signature,
                doc_comment: nil,
                module: nil
            )
        )
        emitContainsEdge(parentID: parentID, childID: id, syntax: syntax)
        collectCalls(in: body, callerID: id)
    }

    private func processVariableDecl(
        _ decl: VariableDeclSyntax,
        parentID: String?,
        asEntryPoint: Bool
    ) {
        let visibility = visibility(from: decl.modifiers)

        for binding in decl.bindings {
            guard let identifier = binding.pattern.as(IdentifierPatternSyntax.self) else {
                continue
            }

            let name = normalizeIdentifier(identifier.identifier.text)
            guard !name.isEmpty else {
                continue
            }

            let entryPoint = asEntryPoint && name == "body"
            let id = uniqueDeclID(
                proposed: makeDeclID(parentID: parentID, name: name),
                syntax: Syntax(binding)
            )

            pushNode(
                BridgeNode(
                    id: id,
                    kind: .property,
                    name: name,
                    file: filePath,
                    span: span(for: Syntax(binding)),
                    visibility: visibility,
                    metadata: [:],
                    role: entryPoint ? BridgeNodeRole.entryPoint : nil,
                    signature: nil,
                    doc_comment: nil,
                    module: nil
                )
            )
            emitContainsEdge(parentID: parentID, childID: id, syntax: Syntax(binding))

            if let initializer = binding.initializer {
                collectCalls(in: Syntax(initializer.value), callerID: id)
            }
            if let accessorBlock = binding.accessorBlock {
                collectCalls(in: Syntax(accessorBlock), callerID: id)
            }
        }
    }

    private func processEnumCaseDecl(_ decl: EnumCaseDeclSyntax, parentID: String?) {
        guard let parentID else {
            return
        }

        for element in decl.elements {
            let name = normalizeIdentifier(element.name.text)
            guard !name.isEmpty else {
                continue
            }

            let id = makeDeclID(parentID: parentID, name: name)
            pushNode(
                BridgeNode(
                    id: id,
                    kind: .variant,
                    name: name,
                    file: filePath,
                    span: span(for: Syntax(element)),
                    visibility: .public,
                    metadata: [:],
                    role: nil,
                    signature: nil,
                    doc_comment: nil,
                    module: nil
                )
            )
            emitContainsEdge(parentID: parentID, childID: id, syntax: Syntax(element))
        }
    }

    private func processTypeAlias(_ decl: TypeAliasDeclSyntax, parentID: String?) {
        let name = normalizeIdentifier(decl.name.text)
        guard !name.isEmpty else {
            return
        }

        let id = uniqueDeclID(
            proposed: makeDeclID(parentID: parentID, name: name),
            syntax: Syntax(decl)
        )
        pushNode(
            BridgeNode(
                id: id,
                kind: .typeAlias,
                name: name,
                file: filePath,
                span: span(for: Syntax(decl)),
                visibility: visibility(from: decl.modifiers),
                metadata: [:],
                role: nil,
                signature: nil,
                doc_comment: nil,
                module: nil
            )
        )
        emitContainsEdge(parentID: parentID, childID: id, syntax: Syntax(decl))
    }

    private func processImport(_ decl: ImportDeclSyntax) {
        let path = decl.path.trimmedDescription
        guard !path.isEmpty else {
            return
        }

        result.imports.append(
            BridgeImport(path: path, symbols: [], kind: .module)
        )

        pushEdge(
            BridgeEdge(
                source: filePath,
                target: "import \(path)",
                kind: .uses,
                confidence: swiftSyntaxImportConfidence,
                direction: nil,
                operation: nil,
                condition: nil,
                async_boundary: nil,
                provenance: [
                    makeProvenance(
                        filePath: filePath,
                        converter: converter,
                        syntax: Syntax(decl),
                        symbolID: filePath
                    ),
                ]
            )
        )
    }

    private func collectCalls(in syntax: Syntax?, callerID: String) {
        guard let syntax else {
            return
        }
        let collector = CallCollector(filePath: filePath, converter: converter, callerID: callerID)
        collector.walk(syntax)
        for edge in collector.edges {
            pushEdge(edge)
        }
    }

    private func emitContainsEdge(parentID: String?, childID: String, syntax: Syntax) {
        guard let parentID else {
            return
        }

        pushEdge(
            BridgeEdge(
                source: parentID,
                target: childID,
                kind: .contains,
                confidence: 1.0,
                direction: nil,
                operation: nil,
                condition: nil,
                async_boundary: nil,
                provenance: [
                    makeProvenance(
                        filePath: filePath,
                        converter: converter,
                        syntax: syntax,
                        symbolID: parentID
                    ),
                ]
            )
        )
    }

    private func emitInheritanceEdges(
        typeID: String,
        inheritanceClause: InheritanceClauseSyntax?,
        syntax: Syntax,
        canUseSuperclassHeuristic: Bool
    ) {
        guard let inheritanceClause else {
            return
        }

        let inherited = inheritanceNames(inheritanceClause)
        if inherited.isEmpty {
            return
        }

        for (index, inheritedName) in inherited.enumerated() {
            let kind: BridgeEdgeKind
            if canUseSuperclassHeuristic && inherited.count > 1 && index == 0 {
                kind = .inherits
            } else {
                kind = .implements
            }

            pushEdge(
                BridgeEdge(
                    source: typeID,
                    target: "\(filePath)::\(inheritedName)",
                    kind: kind,
                    confidence: swiftSyntaxInheritanceConfidence,
                    direction: nil,
                    operation: nil,
                    condition: nil,
                    async_boundary: nil,
                    provenance: [
                        makeProvenance(
                            filePath: filePath,
                            converter: converter,
                            syntax: syntax,
                            symbolID: typeID
                        ),
                    ]
                )
            )
        }
    }

    private func uniqueDeclID(proposed: String, syntax: Syntax) -> String {
        if existingNodeIDs.insert(proposed).inserted {
            return proposed
        }

        let span = span(for: syntax)
        let unique = "\(proposed)@\(span.start[0]):\(span.start[1]):\(span.end[0]):\(span.end[1])"
        existingNodeIDs.insert(unique)
        return unique
    }

    private func pushNode(_ node: BridgeNode) {
        if result.nodes.contains(where: { $0.id == node.id }) {
            return
        }
        result.nodes.append(node)
    }

    private func pushEdge(_ edge: BridgeEdge) {
        let key = BridgeEdgeKey(
            source: edge.source,
            target: edge.target,
            kind: edge.kind,
            operation: edge.operation
        )
        if !existingEdgeKeys.insert(key).inserted {
            return
        }
        result.edges.append(edge)
    }

    private func span(for syntax: Syntax) -> BridgeSpan {
        let start = converter.location(for: syntax.positionAfterSkippingLeadingTrivia)
        let end = converter.location(for: syntax.endPositionBeforeTrailingTrivia)
        return BridgeSpan(
            start: [max(0, start.line - 1), max(0, start.column - 1)],
            end: [max(0, end.line - 1), max(0, end.column - 1)]
        )
    }

    private func visibility(from modifiers: DeclModifierListSyntax) -> BridgeVisibility {
        for modifier in modifiers {
            let text = modifier.name.text
            switch text {
            case "public", "open":
                return .public
            case "private", "fileprivate":
                return .private
            case "internal", "package":
                return .crate
            default:
                continue
            }
        }
        return .crate
    }

    private func hasAttribute(_ attributes: AttributeListSyntax, named name: String) -> Bool {
        for attribute in attributes {
            guard let attr = attribute.as(AttributeSyntax.self) else {
                continue
            }
            let attributeName = normalizeIdentifier(attr.attributeName.trimmedDescription)
            if attributeName == name {
                return true
            }
        }
        return false
    }

    private func inheritanceNames(_ clause: InheritanceClauseSyntax?) -> [String] {
        guard let clause else {
            return []
        }

        return clause.inheritedTypes.compactMap { inheritedType in
            let name = normalizeIdentifier(inheritedType.type.trimmedDescription)
            return name.isEmpty ? nil : name
        }
    }

    private func signatureText(for syntax: Syntax) -> String? {
        let text = syntax.trimmedDescription
        let signature = text.split(separator: "{", maxSplits: 1, omittingEmptySubsequences: false).first?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let signature, !signature.isEmpty else {
            return nil
        }
        return signature
    }

    private func makeDeclID(parentID: String?, name: String) -> String {
        if let parentID {
            return "\(parentID)::\(name)"
        }
        return "\(filePath)::\(name)"
    }
}

private func normalizeIdentifier(_ text: String) -> String {
    text
        .replacingOccurrences(of: "`", with: "")
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

private func makeProvenance(
    filePath: String,
    converter: SourceLocationConverter,
    syntax: Syntax,
    symbolID: String
) -> BridgeEdgeProvenance {
    let start = converter.location(for: syntax.positionAfterSkippingLeadingTrivia)
    let end = converter.location(for: syntax.endPositionBeforeTrailingTrivia)
    return BridgeEdgeProvenance(
        file: filePath,
        span: BridgeSpan(
            start: [max(0, start.line - 1), max(0, start.column - 1)],
            end: [max(0, end.line - 1), max(0, end.column - 1)]
        ),
        symbol_id: symbolID
    )
}

private func encodeCString<T: Encodable>(_ value: T) -> UnsafePointer<CChar>? {
    let encoder = JSONEncoder()
    guard let data = try? encoder.encode(value),
          let string = String(data: data, encoding: .utf8)
    else {
        return nil
    }
    return UnsafePointer(strdup(string))
}

func extractWithSwiftSyntax(
    source: UnsafePointer<CChar>,
    sourceLen: Int,
    filePath: UnsafePointer<CChar>
) -> UnsafePointer<CChar>? {
    guard sourceLen >= 0 else {
        return nil
    }

    let filePath = String(cString: filePath)
    let sourceBuffer = UnsafeBufferPointer(
        start: UnsafeRawPointer(source).assumingMemoryBound(to: UInt8.self),
        count: sourceLen
    )
    guard let sourceString = String(bytes: sourceBuffer, encoding: .utf8) else {
        return nil
    }

    let extractor = SwiftSyntaxExtractor(source: sourceString, filePath: filePath)
    return encodeCString(extractor.extract())
}
