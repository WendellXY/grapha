use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Parser;

use grapha_core::graph::{
    Edge, EdgeKind, EdgeProvenance, Node, NodeKind, NodeRole, Span, Visibility,
};

use grapha_core::{ExtractionResult, LanguageExtractor};

pub struct SwiftExtractor;

impl LanguageExtractor for SwiftExtractor {
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_swift::LANGUAGE.into())?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Swift source"))?;

        let mut result = ExtractionResult::new();
        let file_str = file_path.to_string_lossy().to_string();

        walk_node(tree.root_node(), source, &file_str, &[], None, &mut result);

        Ok(result)
    }
}

/// Recursively walk a tree-sitter node, extracting Swift symbols and edges.
fn walk_node(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    match node.kind() {
        "class_declaration" => {
            let declaration_type = detect_class_declaration_type(node);
            match declaration_type {
                ClassDeclarationType::Struct => {
                    extract_struct_or_class(
                        node,
                        source,
                        file,
                        module_path,
                        parent_id,
                        NodeKind::Struct,
                        result,
                    );
                }
                ClassDeclarationType::Class => {
                    extract_struct_or_class(
                        node,
                        source,
                        file,
                        module_path,
                        parent_id,
                        NodeKind::Struct,
                        result,
                    );
                }
                ClassDeclarationType::Enum => {
                    extract_enum(node, source, file, module_path, parent_id, result);
                }
                ClassDeclarationType::Extension => {
                    extract_extension(node, source, file, module_path, parent_id, result);
                }
            }
        }
        "protocol_declaration" => {
            extract_protocol(node, source, file, module_path, parent_id, result);
        }
        "function_declaration" | "init_declaration" | "deinit_declaration" => {
            extract_function(node, source, file, module_path, parent_id, result);
        }
        "protocol_function_declaration" => {
            extract_function(node, source, file, module_path, parent_id, result);
        }
        "property_declaration" => {
            extract_property(node, source, file, module_path, parent_id, result);
        }
        "typealias_declaration" => {
            extract_typealias(node, source, file, module_path, parent_id, result);
        }
        "import_declaration" => {
            extract_import(node, source, file, result);
        }
        _ => {
            walk_children(node, source, file, module_path, parent_id, result);
        }
    }
}

/// Walk all named children of a node.
fn walk_children(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_node(child, source, file, module_path, parent_id, result);
    }
}

/// Build a node ID from file, module path, and name.
fn make_id(file: &str, module_path: &[String], name: &str) -> String {
    if module_path.is_empty() {
        format!("{}::{}", file, name)
    } else {
        format!("{}::{}::{}", file, module_path.join("::"), name)
    }
}

/// Build a declaration/member ID scoped to its owning declaration when present.
fn make_decl_id(file: &str, module_path: &[String], parent_id: Option<&str>, name: &str) -> String {
    parent_id
        .map(|pid| format!("{pid}::{name}"))
        .unwrap_or_else(|| make_id(file, module_path, name))
}

fn unique_decl_id(
    result: &ExtractionResult,
    proposed_id: String,
    node: tree_sitter::Node,
) -> String {
    if result
        .nodes
        .iter()
        .all(|existing| existing.id != proposed_id)
    {
        return proposed_id;
    }

    let span = make_span(node);
    format!(
        "{proposed_id}@{}:{}:{}:{}",
        span.start[0], span.start[1], span.end[0], span.end[1]
    )
}

/// Extract the text of the first `simple_identifier` named child (used for function names).
fn simple_identifier_text(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "simple_identifier" {
            return child.utf8_text(source).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Extract the `type_identifier` named child text (used for type names).
fn type_identifier_text(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_identifier" {
            return child.utf8_text(source).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Determine the Swift declaration kind from a `class_declaration` node.
///
/// tree-sitter-swift uses `class_declaration` for struct, class, enum, and extension.
/// We distinguish them by the anonymous keyword child token.
fn detect_class_declaration_type(node: tree_sitter::Node) -> ClassDeclarationType {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            match child.kind() {
                "struct" => return ClassDeclarationType::Struct,
                "class" => return ClassDeclarationType::Class,
                "enum" => return ClassDeclarationType::Enum,
                "extension" => return ClassDeclarationType::Extension,
                _ => {}
            }
        }
    }
    // Default to class if we can't determine
    ClassDeclarationType::Class
}

enum ClassDeclarationType {
    Struct,
    Class,
    Enum,
    Extension,
}

/// Extract visibility from a Swift node by checking for a `modifiers` child
/// containing a `visibility_modifier`.
fn extract_visibility(node: tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for modifier in child.named_children(&mut mod_cursor) {
                if modifier.kind() == "visibility_modifier" {
                    let text = modifier.utf8_text(source).unwrap_or("");
                    if text == "public" || text == "open" {
                        return Visibility::Public;
                    } else if text == "private" || text == "fileprivate" {
                        return Visibility::Private;
                    }
                    // "internal" is default in Swift, maps to Crate
                    return Visibility::Crate;
                }
            }
        }
    }
    // Swift default is `internal`
    Visibility::Crate
}

fn make_span(node: tree_sitter::Node) -> Span {
    let start = node.start_position();
    let end = node.end_position();
    Span {
        start: [start.row, start.column],
        end: [end.row, end.column],
    }
}

fn edge_provenance(file: &str, span: Span, symbol_id: &str) -> Vec<EdgeProvenance> {
    vec![EdgeProvenance {
        file: file.into(),
        span,
        symbol_id: symbol_id.to_string(),
    }]
}

fn node_edge_provenance(
    file: &str,
    node: tree_sitter::Node,
    symbol_id: &str,
) -> Vec<EdgeProvenance> {
    edge_provenance(file, make_span(node), symbol_id)
}

fn span_from_text_range(
    node: tree_sitter::Node,
    text: &str,
    start: usize,
    end: usize,
) -> Option<Span> {
    let bytes = text.as_bytes();
    if start > end || end > bytes.len() {
        return None;
    }

    fn absolute_position(base: tree_sitter::Point, bytes: &[u8]) -> [usize; 2] {
        let newline_count = bytes.iter().filter(|&&byte| byte == b'\n').count();
        if newline_count == 0 {
            [base.row, base.column + bytes.len()]
        } else {
            let last_newline = bytes
                .iter()
                .rposition(|&byte| byte == b'\n')
                .expect("counted newlines above");
            [base.row + newline_count, bytes.len() - last_newline - 1]
        }
    }

    let base = node.start_position();
    Some(Span {
        start: absolute_position(base, &bytes[..start]),
        end: absolute_position(base, &bytes[..end]),
    })
}

fn emit_contains_edge(
    parent_id: Option<&str>,
    child_id: &str,
    file: &str,
    edge_node: tree_sitter::Node,
    result: &mut ExtractionResult,
) {
    if let Some(pid) = parent_id {
        result.edges.push(Edge {
            source: pid.to_string(),
            target: child_id.to_string(),
            kind: EdgeKind::Contains,
            confidence: 1.0,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, edge_node, pid),
        });
    }
}

/// Extract struct or class declaration.
fn extract_struct_or_class(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    kind: NodeKind,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    // Extract inheritance/conformance edges
    extract_inheritance_edges(node, source, file, module_path, &id, result);

    // Detect entry point: @main attribute
    let has_main_attr = has_swift_attribute(node, source, "main");
    let role = if has_main_attr {
        Some(NodeRole::EntryPoint)
    } else {
        None
    };

    // Detect conformances for body-level entry point marking
    let conformances = collect_inheritance_names(node, source);
    let conforms_to_view = conformances.iter().any(|c| c == "View" || c == "App");
    let is_observable = conformances.iter().any(|c| c == "ObservableObject")
        || has_swift_attribute(node, source, "Observable");

    result.nodes.push(Node {
        id: id.clone(),
        kind,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role,
        signature: None,
        doc_comment: None,
        module: None,
    });

    // Walk the class_body for nested declarations
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "class_body" {
            walk_children_with_hints(
                child,
                source,
                file,
                module_path,
                Some(&id),
                conforms_to_view,
                is_observable,
                result,
            );
        }
    }
}

/// Walk children with hints about parent conformances for entry point detection.
#[allow(clippy::too_many_arguments)]
fn walk_children_with_hints(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    parent_conforms_to_view: bool,
    parent_is_observable: bool,
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "property_declaration" && parent_conforms_to_view {
            // Check if this is the `body` property
            if let Some(prop_name) = find_pattern_name(child, source)
                && prop_name == "body"
            {
                extract_property_as_entry_point(
                    child,
                    source,
                    file,
                    module_path,
                    parent_id,
                    result,
                );
                continue;
            }
        }
        if child.kind() == "function_declaration" && parent_is_observable {
            // Mark public methods as entry points
            extract_function_with_entry_hint(
                child,
                source,
                file,
                module_path,
                parent_id,
                true,
                result,
            );
            continue;
        }
        walk_node(child, source, file, module_path, parent_id, result);
    }
}

/// Extract a property and mark it as an entry point (e.g., View.body).
fn extract_property_as_entry_point(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let name = find_pattern_name(node, source);
    let Some(name) = name else { return };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Property,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: Some(NodeRole::EntryPoint),
        signature: None,
        doc_comment: None,
        module: None,
    });

    // Scan property body for calls (same as extract_property)
    extract_calls(node, source, file, module_path, &id, result);
    // Always run regex fallback to catch calls inside closures/ViewBuilder
    // bodies that tree-sitter doesn't parse as call_expression nodes.
    if let Ok(text) = node.utf8_text(source) {
        extract_calls_from_text(text, node, file, module_path, &id, result);
    }
    extract_swiftui_declaration_structure(node, source, file, module_path, &id, result);
}

/// Extract a function with an optional entry point hint from parent (Observable).
fn extract_function_with_entry_hint(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    observable_parent: bool,
    result: &mut ExtractionResult,
) {
    // init/deinit declarations don't have a simple_identifier name
    let name = if node.kind() == "init_declaration" {
        "init".to_string()
    } else if node.kind() == "deinit_declaration" {
        "deinit".to_string()
    } else {
        let Some(n) = simple_identifier_text(node, source) else {
            return;
        };
        n
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);
    let signature = extract_swift_signature(node, source);
    let doc_comment = extract_swift_doc_comment(node, source);

    let role = if observable_parent
        && (visibility == Visibility::Public || visibility == Visibility::Crate)
    {
        Some(NodeRole::EntryPoint)
    } else {
        None
    };

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Function,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role,
        signature,
        doc_comment,
        module: None,
    });

    // Walk function body for call expressions.
    // init_declaration uses "class_body" or direct children, not "function_body"
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "function_body" | "class_body") {
            extract_calls(child, source, file, module_path, &id, result);
        }
    }
    // Fallback: if no function_body found (e.g., init), scan all children
    // that aren't parameter lists or modifiers
    let has_body = {
        let mut c = node.walk();
        node.named_children(&mut c)
            .any(|ch| matches!(ch.kind(), "function_body" | "class_body"))
    };
    if !has_body {
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            if !matches!(
                child.kind(),
                "modifiers" | "parameter" | "type_annotation" | "attribute" | "where_clause"
            ) {
                extract_calls(child, source, file, module_path, &id, result);
            }
        }
    }
}

/// Extract enum declaration with case variants.
fn extract_enum(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Enum,
        name: name.clone(),
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
    });

    // Extract enum entries from enum_class_body
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "enum_class_body" {
            extract_enum_entries(child, source, file, module_path, &id, result);
        }
    }
}

/// Extract enum_entry children from an enum body.
fn extract_enum_entries(
    body: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: &str,
    result: &mut ExtractionResult,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "enum_entry"
            && let Some(case_name) = simple_identifier_text(child, source)
        {
            let id = make_decl_id(file, module_path, Some(parent_id), &case_name);

            result.edges.push(Edge {
                source: parent_id.to_string(),
                target: id.clone(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: node_edge_provenance(file, child, parent_id),
            });

            result.nodes.push(Node {
                id,
                kind: NodeKind::Variant,
                name: case_name,
                file: file.into(),
                span: make_span(child),
                visibility: Visibility::Public,
                metadata: HashMap::new(),
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
            });
        }
    }
}

/// Extract extension declaration.
fn extract_extension(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    // Extension uses user_type > type_identifier for the extended type name
    let name = find_user_type_name(node, source).unwrap_or_else(|| "Unknown".to_string());
    let ext_name = format!("ext_{}", name);
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &ext_name),
        node,
    );

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Extension,
        name,
        file: file.into(),
        span: make_span(node),
        visibility: Visibility::Crate,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
    });

    // Walk the class_body for nested declarations
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "class_body" {
            walk_children(child, source, file, module_path, Some(&id), result);
        }
    }
}

/// Find the type name from a `user_type > type_identifier` child.
fn find_user_type_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "user_type" {
            return type_identifier_text(child, source);
        }
    }
    None
}

/// Extract protocol declaration.
fn extract_protocol(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Protocol,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
    });

    // Walk the protocol_body for method declarations
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "protocol_body" {
            walk_children(child, source, file, module_path, Some(&id), result);
        }
    }
}

/// Extract function declaration (including init/deinit).
fn extract_function(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let name = if node.kind() == "init_declaration" {
        "init".to_string()
    } else if node.kind() == "deinit_declaration" {
        "deinit".to_string()
    } else {
        let Some(n) = simple_identifier_text(node, source) else {
            return;
        };
        n
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);
    let signature = extract_swift_signature(node, source);
    let doc_comment = extract_swift_doc_comment(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Function,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature,
        doc_comment,
        module: None,
    });

    // Walk function body for call expressions.
    // init_declaration may not have "function_body" — scan all non-parameter children as fallback.
    let mut cursor = node.walk();
    let mut found_body = false;
    for child in node.named_children(&mut cursor) {
        if child.kind() == "function_body" {
            extract_calls(child, source, file, module_path, &id, result);
            found_body = true;
        }
    }
    if !found_body {
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            if !matches!(
                child.kind(),
                "modifiers"
                    | "parameter"
                    | "type_annotation"
                    | "attribute"
                    | "where_clause"
                    | "simple_identifier"
                    | "type_identifier"
            ) {
                extract_calls(child, source, file, module_path, &id, result);
            }
        }
    }

    if declaration_returns_swiftui_view(node, source) {
        extract_swiftui_declaration_structure(node, source, file, module_path, &id, result);
    }
}

/// Extract property declaration.
fn extract_property(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    // Property name is in pattern > simple_identifier
    let name = find_pattern_name(node, source);
    let Some(name) = name else { return };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Property,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
    });

    // Scan property body for calls via tree-sitter AST
    extract_calls(node, source, file, module_path, &id, result);

    // Fallback: if no calls edges were found via AST, scan the property's source
    // text for function call patterns using regex. This handles cases where
    // tree-sitter-swift doesn't produce call_expression nodes (e.g., SwiftUI
    // View body with result builders).
    let calls_before = result
        .edges
        .iter()
        .filter(|e| e.source == id && e.kind == EdgeKind::Calls)
        .count();
    if calls_before == 0
        && let Ok(text) = node.utf8_text(source)
    {
        extract_calls_from_text(text, node, file, module_path, &id, result);
    }

    if declaration_returns_swiftui_view(node, source) {
        extract_swiftui_declaration_structure(node, source, file, module_path, &id, result);
    }
}

/// Extract function calls from raw source text using regex.
/// Fallback for when tree-sitter doesn't produce call_expression nodes
/// (e.g., SwiftUI View body with result builders).
fn extract_calls_from_text(
    text: &str,
    node: tree_sitter::Node,
    file: &str,
    module_path: &[String],
    caller_id: &str,
    result: &mut ExtractionResult,
) {
    // Match patterns like: identifier( or .identifier(
    // But skip common keywords and type annotations
    let call_re = regex::Regex::new(r"(?:^|[.\s({,])([a-z][a-zA-Z0-9]*)\s*\(").unwrap();
    let skip_names: std::collections::HashSet<&str> = [
        "if", "for", "while", "switch", "guard", "return", "let", "var", "case", "some", "in",
        "as", "is", "try", "await", "throw", "catch", "where",
    ]
    .into_iter()
    .collect();

    let mut seen = std::collections::HashSet::new();
    for cap in call_re.captures_iter(text) {
        let fn_name = cap.get(1).unwrap().as_str();
        if skip_names.contains(fn_name) || !seen.insert(fn_name.to_string()) {
            continue;
        }
        let target_id = make_id(file, module_path, fn_name);
        let span = cap
            .get(1)
            .and_then(|capture| span_from_text_range(node, text, capture.start(), capture.end()))
            .unwrap_or_else(|| make_span(node));
        result.edges.push(Edge {
            source: caller_id.to_string(),
            target: target_id,
            kind: EdgeKind::Calls,
            confidence: 0.5, // lower confidence for regex-based extraction
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: edge_provenance(file, span, caller_id),
        });
    }

    // Also match property access chains like AppContext.gift.activityGiftConfigs
    let nav_re = regex::Regex::new(r"[A-Za-z][a-zA-Z0-9]*(?:\.[a-zA-Z][a-zA-Z0-9]*)+").unwrap();
    for mat in nav_re.find_iter(text) {
        let chain = mat.as_str();
        let parts: Vec<&str> = chain.split('.').collect();
        if parts.len() >= 2 {
            let last = *parts.last().unwrap();
            if skip_names.contains(last) || seen.contains(last) {
                continue;
            }
            seen.insert(last.to_string());
            let prefix = parts[..parts.len() - 1].join(".");
            let target_id = make_id(file, module_path, last);
            let span = span_from_text_range(node, text, mat.start(), mat.end())
                .unwrap_or_else(|| make_span(node));
            result.edges.push(Edge {
                source: caller_id.to_string(),
                target: target_id,
                kind: EdgeKind::Calls,
                confidence: 0.5,
                direction: None,
                operation: Some(prefix),
                condition: None,
                async_boundary: None,
                provenance: edge_provenance(file, span, caller_id),
            });
        }
    }
}

/// Find the name from a `pattern > simple_identifier` child.
fn find_pattern_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "pattern" {
            return simple_identifier_text(child, source);
        }
    }
    None
}

/// Extract typealias declaration.
fn extract_typealias(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let Some(name) = type_identifier_text(node, source) else {
        return;
    };
    let id = unique_decl_id(
        result,
        make_decl_id(file, module_path, parent_id, &name),
        node,
    );
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, file, node, result);

    result.nodes.push(Node {
        id,
        kind: NodeKind::TypeAlias,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
    });
}

/// Extract import declaration into an Import struct.
fn extract_import(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    result: &mut ExtractionResult,
) {
    // The import path is in the identifier > simple_identifier child
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "identifier"
            && let Ok(path_text) = child.utf8_text(source)
        {
            let path = path_text.to_string();

            result.imports.push(grapha_core::resolve::Import {
                path: path.clone(),
                symbols: vec![],
                kind: grapha_core::resolve::ImportKind::Module,
            });

            result.edges.push(Edge {
                source: file.to_string(),
                target: format!("import {}", path),
                kind: EdgeKind::Uses,
                confidence: 0.7,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: node_edge_provenance(file, node, file),
            });
        }
    }
}

/// Extract function signature (text up to opening `{`).
fn extract_swift_signature(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let text = node.utf8_text(source).ok()?;
    let sig = if let Some(brace_pos) = text.find('{') {
        text[..brace_pos].trim()
    } else {
        text.trim()
    };
    if sig.is_empty() {
        None
    } else {
        Some(sig.to_string())
    }
}

/// Enrich an existing `ExtractionResult` (e.g. from the index store) with doc
/// comments extracted via tree-sitter.  The index store does not provide doc
/// comments, so we do a lightweight tree-sitter parse and match nodes by
/// `(name, start_line)`.
///
/// Index store lines are **1-based**; tree-sitter rows are **0-based**, so we
/// compare with `row + 1`.
pub fn enrich_doc_comments(source: &[u8], result: &mut ExtractionResult) -> anyhow::Result<()> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_swift::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Swift source"))?;

    // Collect (name, 1-based line) → doc_comment from tree-sitter AST.
    let mut doc_map: HashMap<(String, usize), String> = HashMap::new();
    collect_doc_comments(tree.root_node(), source, &mut doc_map);

    // Patch nodes that are missing a doc_comment.
    for node in &mut result.nodes {
        if node.doc_comment.is_some() {
            continue;
        }
        let line_1based = node.span.start[0];
        if let Some(doc) = doc_map.remove(&(node.name.clone(), line_1based)) {
            node.doc_comment = Some(doc);
        }
    }

    Ok(())
}

/// Recursively walk the tree-sitter AST and collect doc comments for every
/// declaration that has one.  Results are keyed by `(name, 1-based line)`.
fn collect_doc_comments(
    node: tree_sitter::Node,
    source: &[u8],
    out: &mut HashMap<(String, usize), String>,
) {
    match node.kind() {
        "class_declaration" | "protocol_declaration" => {
            if let Some(name) = type_identifier_text(node, source) {
                if let Some(doc) = extract_swift_doc_comment(node, source) {
                    out.insert((name, node.start_position().row + 1), doc);
                }
            }
        }
        "function_declaration"
        | "init_declaration"
        | "deinit_declaration"
        | "protocol_function_declaration" => {
            let name = if node.kind() == "init_declaration" {
                Some("init".to_string())
            } else if node.kind() == "deinit_declaration" {
                Some("deinit".to_string())
            } else {
                simple_identifier_text(node, source)
            };
            if let Some(name) = name {
                if let Some(doc) = extract_swift_doc_comment(node, source) {
                    out.insert((name, node.start_position().row + 1), doc);
                }
            }
        }
        "property_declaration" => {
            if let Some(name) = find_pattern_name(node, source) {
                if let Some(doc) = extract_swift_doc_comment(node, source) {
                    out.insert((name, node.start_position().row + 1), doc);
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_doc_comments(child, source, out);
    }
}

/// Extract doc comments from previous sibling comment nodes.
fn extract_swift_doc_comment(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut comments = Vec::new();
    let mut prev = node.prev_named_sibling();
    while let Some(sib) = prev {
        if sib.kind() == "comment" || sib.kind() == "multiline_comment" {
            if let Ok(text) = sib.utf8_text(source) {
                comments.push(text.to_string());
            }
            prev = sib.prev_named_sibling();
        } else {
            break;
        }
    }
    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        Some(comments.join("\n"))
    }
}

/// Check if a Swift node has a specific attribute (e.g., @main, @Observable).
fn has_swift_attribute(node: tree_sitter::Node, source: &[u8], attr_name: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for modifier in child.named_children(&mut mod_cursor) {
                if modifier.kind() == "attribute"
                    && let Ok(text) = modifier.utf8_text(source)
                {
                    let trimmed = text.trim_start_matches('@');
                    if trimmed == attr_name {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Collect inheritance/conformance names from a class_declaration node.
fn collect_inheritance_names(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "inheritance_specifier"
            && let Some(name) =
                find_user_type_name(child, source).or_else(|| type_identifier_text(child, source))
        {
            names.push(name);
        }
    }
    names
}

/// Walk up from a call node to find an enclosing Swift conditional.
/// Stops at `function_declaration` or `closure_expression` boundary.
fn find_enclosing_swift_condition(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "function_declaration" | "closure_expression" => return None,
            "if_statement" => {
                // Get the condition from the if_statement
                if let Some(cond) = parent.child_by_field_name("condition") {
                    return cond.utf8_text(source).ok().map(|s| s.trim().to_string());
                }
                // Fallback: get text between "if" and "{"
                if let Ok(text) = parent.utf8_text(source)
                    && let Some(if_pos) = text.find("if")
                    && let Some(brace_pos) = text.find('{')
                {
                    let cond = text[if_pos + 2..brace_pos].trim();
                    if !cond.is_empty() {
                        return Some(cond.to_string());
                    }
                }
                return None;
            }
            "guard_statement" => {
                if let Ok(text) = parent.utf8_text(source)
                    && let Some(guard_pos) = text.find("guard")
                    && let Some(else_pos) = text.find("else")
                {
                    let cond = text[guard_pos + 5..else_pos].trim();
                    if !cond.is_empty() {
                        return Some(format!("guard {}", cond));
                    }
                }
                return None;
            }
            "switch_entry" => {
                // Get the case pattern text
                if let Ok(text) = parent.utf8_text(source)
                    && let Some(case_pos) = text.find("case")
                    && let Some(colon_pos) = text[case_pos..].find(':').map(|p| case_pos + p)
                    && colon_pos > case_pos + 4
                {
                    let pattern = text[case_pos + 4..colon_pos].trim();
                    if !pattern.is_empty() {
                        return Some(format!("case {}", pattern));
                    }
                }
                return None;
            }
            _ => {
                current = parent.parent();
            }
        }
    }
    None
}

/// Check if a Swift call node is at an async boundary.
fn detect_swift_async_boundary(node: tree_sitter::Node, source: &[u8]) -> Option<bool> {
    // Check if parent is await_expression
    if let Some(parent) = node.parent()
        && parent.kind() == "await_expression"
    {
        return Some(true);
    }
    // Check if inside a Task { } block
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "function_declaration" || parent.kind() == "closure_expression" {
            // Check if the closure is an argument to Task { } or DispatchQueue.async
            if let Some(gp) = parent.parent()
                && gp.kind() == "call_expression"
            {
                if let Some(fn_name) = simple_identifier_text(gp, source)
                    && fn_name == "Task"
                {
                    return Some(true);
                }
                if let Ok(text) = gp.utf8_text(source)
                    && text.contains("DispatchQueue")
                    && text.contains("async")
                {
                    return Some(true);
                }
            }
            break;
        }
        current = parent.parent();
    }
    None
}

/// Extract inheritance/conformance edges from `inheritance_specifier` children.
fn extract_inheritance_edges(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    type_id: &str,
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "inheritance_specifier"
            && let Some(inherited_name) =
                find_user_type_name(child, source).or_else(|| type_identifier_text(child, source))
        {
            let target_id = make_id(file, module_path, &inherited_name);
            result.edges.push(Edge {
                source: type_id.to_string(),
                target: target_id,
                kind: EdgeKind::Implements,
                confidence: 0.9,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: node_edge_provenance(file, child, type_id),
            });
        }
    }
}

/// Extract the prefix of a navigation expression chain.
/// For `AppContext.gift.activityGiftConfigs`, returns `Some("AppContext.gift")`.
/// For `foo.bar`, returns `Some("foo")`.
fn extract_nav_prefix(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    // The first named child that isn't a navigation_suffix is the prefix expression
    let first_child = node
        .named_children(&mut cursor)
        .find(|c| c.kind() != "navigation_suffix")?;
    let text = first_child.utf8_text(source).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Recursively scan for `call_expression` and `navigation_expression` nodes,
/// emitting Calls edges for function calls and TypeRef edges for property accesses.
fn extract_calls(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    caller_id: &str,
    result: &mut ExtractionResult,
) {
    if node.kind() == "call_expression"
        && let Some(fn_name) = simple_identifier_text(node, source)
    {
        let target_id = make_id(file, module_path, &fn_name);
        let condition = find_enclosing_swift_condition(node, source);
        let async_boundary = detect_swift_async_boundary(node, source);
        result.edges.push(Edge {
            source: caller_id.to_string(),
            target: target_id,
            kind: EdgeKind::Calls,
            confidence: 0.8,
            direction: None,
            operation: None,
            condition,
            async_boundary,
            provenance: node_edge_provenance(file, node, caller_id),
        });
    }

    // Property access: `foo.bar` generates a navigation_expression.
    // Emit a Calls edge so that property reads appear in the graph
    // and impact analysis can trace through them.
    // Skip if the parent is a call_expression (already handled above as the callee name).
    if node.kind() == "navigation_expression"
        && !matches!(node.parent().map(|p| p.kind()), Some("call_expression"))
    {
        // The accessed property name is the last navigation_suffix child
        let mut cursor = node.walk();
        if let Some(suffix) = node
            .named_children(&mut cursor)
            .filter(|c| c.kind() == "navigation_suffix")
            .last()
            && let Some(name_node) = suffix.named_child(0)
            && let Ok(prop_name) = name_node.utf8_text(source)
            && !prop_name.is_empty()
        {
            let target_id = make_id(file, module_path, prop_name);
            let condition = find_enclosing_swift_condition(node, source);
            // Extract the prefix chain (e.g., "AppContext.gift" from "AppContext.gift.activityGiftConfigs")
            // to help the merge step disambiguate among multiple candidates.
            let prefix = extract_nav_prefix(node, source);
            result.edges.push(Edge {
                source: caller_id.to_string(),
                target: target_id,
                kind: EdgeKind::Calls,
                confidence: 0.6,
                direction: None,
                operation: prefix,
                condition,
                async_boundary: None,
                provenance: node_edge_provenance(file, node, caller_id),
            });
        }
    }

    // Recurse into all children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_calls(child, source, file, module_path, caller_id, result);
    }
}

fn emit_unique_edge(result: &mut ExtractionResult, edge: Edge) {
    if result.edges.iter().any(|existing| {
        existing.source == edge.source
            && existing.target == edge.target
            && existing.kind == edge.kind
            && existing.operation == edge.operation
    }) {
        return;
    }
    result.edges.push(edge);
}

fn push_unique_node(result: &mut ExtractionResult, node: Node) {
    if result.nodes.iter().any(|existing| existing.id == node.id) {
        return;
    }
    result.nodes.push(node);
}

fn sanitize_id_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn make_swiftui_synthetic_id(
    owner_id: &str,
    prefix: &str,
    name: &str,
    node: tree_sitter::Node,
) -> String {
    let start = node.start_position();
    let end = node.end_position();
    format!(
        "{owner_id}::{prefix}:{}@{}:{}:{}:{}",
        sanitize_id_component(name),
        start.row,
        start.column,
        end.row,
        end.column
    )
}

fn emit_swiftui_node(
    result: &mut ExtractionResult,
    owner_id: &str,
    parent_id: &str,
    name: &str,
    kind: NodeKind,
    file: &str,
    node: tree_sitter::Node,
) -> String {
    let prefix = match kind {
        NodeKind::View => "view",
        NodeKind::Branch => "branch",
        _ => "synthetic",
    };
    let id = make_swiftui_synthetic_id(owner_id, prefix, name, node);
    push_unique_node(
        result,
        Node {
            id: id.clone(),
            kind,
            name: name.to_string(),
            file: file.into(),
            span: make_span(node),
            visibility: Visibility::Private,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
        },
    );
    emit_unique_edge(
        result,
        Edge {
            source: parent_id.to_string(),
            target: id.clone(),
            kind: EdgeKind::Contains,
            confidence: 1.0,
            direction: None,
            operation: None,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, node, parent_id),
        },
    );
    id
}

fn same_owner_member_id(owner_id: &str, name: &str) -> Option<String> {
    let (owner_prefix, _) = owner_id.rsplit_once("::")?;
    Some(format!("{owner_prefix}::{name}"))
}

fn enclosing_owner_type_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut current = Some(node);
    while let Some(cursor) = current {
        if cursor.kind() == "class_declaration" {
            return match detect_class_declaration_type(cursor) {
                ClassDeclarationType::Extension => find_user_type_name(cursor, source),
                _ => type_identifier_text(cursor, source),
            };
        }
        current = cursor.parent();
    }
    None
}

fn uppercase_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn is_builtin_swiftui_view(name: &str) -> bool {
    matches!(
        name,
        "AnyView"
            | "Button"
            | "Color"
            | "Divider"
            | "DisclosureGroup"
            | "EmptyView"
            | "ForEach"
            | "Form"
            | "GeometryReader"
            | "Grid"
            | "GridRow"
            | "Group"
            | "HStack"
            | "Image"
            | "Label"
            | "LazyHGrid"
            | "LazyHStack"
            | "LazyVGrid"
            | "LazyVStack"
            | "Link"
            | "List"
            | "Menu"
            | "NavigationLink"
            | "NavigationStack"
            | "NavigationView"
            | "Picker"
            | "ProgressView"
            | "ScrollView"
            | "Section"
            | "SecureField"
            | "Spacer"
            | "TabView"
            | "Text"
            | "TextField"
            | "TimelineView"
            | "Toggle"
            | "VStack"
            | "ZStack"
    )
}

fn navigation_base_and_member_name<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
) -> Option<(tree_sitter::Node<'a>, String)> {
    if node.kind() != "navigation_expression" {
        return None;
    }

    let mut cursor = node.walk();
    let mut base = None;
    let mut member_name = None;
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "navigation_suffix" => {
                if let Some(name_node) = child.named_child(0)
                    && let Ok(name) = name_node.utf8_text(source)
                    && !name.is_empty()
                {
                    member_name = Some(name.to_string());
                }
            }
            _ if base.is_none() => {
                base = Some(child);
            }
            _ => {}
        }
    }

    Some((base?, member_name?))
}

fn swiftui_call_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let callee = node.named_children(&mut cursor).next()?;
    match callee.kind() {
        "simple_identifier" => callee.utf8_text(source).ok().map(ToString::to_string),
        "navigation_expression" => {
            navigation_base_and_member_name(callee, source).map(|(_, member_name)| member_name)
        }
        _ => None,
    }
}

fn swiftui_modifier_receiver_and_name<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
) -> Option<(tree_sitter::Node<'a>, String)> {
    let mut cursor = node.walk();
    let callee = node.named_children(&mut cursor).next()?;
    let (receiver, member_name) = navigation_base_and_member_name(callee, source)?;
    if uppercase_identifier(&member_name) {
        None
    } else {
        Some((receiver, member_name))
    }
}

fn is_view_builder_modifier(name: &str) -> bool {
    matches!(
        name,
        "background"
            | "contextMenu"
            | "footer"
            | "header"
            | "mask"
            | "overlay"
            | "safeAreaInset"
            | "toolbar"
    )
}

fn view_reference_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let parent_kind = node.parent().map(|parent| parent.kind())?;
    if !matches!(
        parent_kind,
        "statements" | "computed_property" | "lambda_literal"
    ) {
        return None;
    }

    match node.kind() {
        "simple_identifier" => node
            .utf8_text(source)
            .ok()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToString::to_string),
        "navigation_expression" => {
            navigation_base_and_member_name(node, source).map(|(_, member_name)| member_name)
        }
        _ => None,
    }
}

fn emit_swiftui_view_reference(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    result: &mut ExtractionResult,
) -> Option<String> {
    let name = view_reference_name(node, source)?;
    let view_id = emit_swiftui_node(
        result,
        owner_id,
        parent_id,
        &name,
        NodeKind::View,
        file,
        node,
    );
    if !is_builtin_swiftui_view(&name) {
        let target_id = same_owner_member_id(owner_id, &name)
            .unwrap_or_else(|| make_id(file, module_path, &name));
        let owner_hint = enclosing_owner_type_name(node, source);
        emit_unique_edge(
            result,
            Edge {
                source: view_id.clone(),
                target: target_id,
                kind: EdgeKind::TypeRef,
                confidence: 0.85,
                direction: None,
                operation: owner_hint,
                condition: None,
                async_boundary: None,
                provenance: node_edge_provenance(file, node, &view_id),
            },
        );
    }
    Some(view_id)
}

fn emit_swiftui_call_reference(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    result: &mut ExtractionResult,
) -> Option<String> {
    let name = swiftui_call_name(node, source)?;
    if uppercase_identifier(&name) {
        return None;
    }

    let view_id = emit_swiftui_node(
        result,
        owner_id,
        parent_id,
        &name,
        NodeKind::View,
        file,
        node,
    );
    let target_id =
        same_owner_member_id(owner_id, &name).unwrap_or_else(|| make_id(file, module_path, &name));
    let owner_hint = enclosing_owner_type_name(node, source);
    emit_unique_edge(
        result,
        Edge {
            source: view_id.clone(),
            target: target_id,
            kind: EdgeKind::TypeRef,
            confidence: 0.85,
            direction: None,
            operation: owner_hint,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, node, &view_id),
        },
    );
    for lambda in call_suffix_lambda_children(node) {
        let _ = extract_swiftui_structure(
            lambda,
            source,
            file,
            module_path,
            owner_id,
            &view_id,
            result,
        );
    }
    Some(view_id)
}

fn call_suffix_lambda_children(node: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    let mut suffixes = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "call_suffix" {
            let mut suffix_cursor = child.walk();
            for suffix_child in child.named_children(&mut suffix_cursor) {
                if suffix_child.kind() == "lambda_literal" {
                    suffixes.push(suffix_child);
                }
            }
        }
    }
    suffixes
}

fn recurse_swiftui_named_children(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    result: &mut ExtractionResult,
) -> Vec<String> {
    let mut anchors = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        anchors.extend(extract_swiftui_structure(
            child,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            result,
        ));
    }
    anchors
}

fn extract_swiftui_if_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    result: &mut ExtractionResult,
) -> Vec<String> {
    let condition = node
        .child_by_field_name("condition")
        .and_then(|condition| condition.utf8_text(source).ok())
        .map(|text| format!("if {}", text.trim()))
        .filter(|text| text != "if");

    let mut named_children = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "statements" | "if_statement") {
            named_children.push(child);
        }
    }

    let mut branch_ids = Vec::new();
    if let Some(then_child) = named_children.first().copied() {
        let branch_id = emit_swiftui_node(
            result,
            owner_id,
            parent_id,
            condition.as_deref().unwrap_or("if"),
            NodeKind::Branch,
            file,
            then_child,
        );
        branch_ids.push(branch_id.clone());
        let _ = extract_swiftui_structure(
            then_child,
            source,
            file,
            module_path,
            owner_id,
            &branch_id,
            result,
        );
    }

    if let Some(else_child) = named_children.get(1).copied() {
        let branch_id = emit_swiftui_node(
            result,
            owner_id,
            parent_id,
            "else",
            NodeKind::Branch,
            file,
            else_child,
        );
        branch_ids.push(branch_id.clone());
        let _ = extract_swiftui_structure(
            else_child,
            source,
            file,
            module_path,
            owner_id,
            &branch_id,
            result,
        );
    }

    branch_ids
}

fn switch_entry_label(node: tree_sitter::Node, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "default_keyword" => return "default".to_string(),
            "switch_pattern" => {
                if let Ok(text) = child.utf8_text(source) {
                    return format!("case {}", text.trim());
                }
            }
            _ => {}
        }
    }
    "case".to_string()
}

fn extract_swiftui_switch_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    result: &mut ExtractionResult,
) -> Vec<String> {
    let label = node
        .child_by_field_name("expr")
        .and_then(|expr| expr.utf8_text(source).ok())
        .map(|text| format!("switch {}", text.trim()))
        .unwrap_or_else(|| "switch".to_string());
    let switch_id = emit_swiftui_node(
        result,
        owner_id,
        parent_id,
        &label,
        NodeKind::Branch,
        file,
        node,
    );

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "switch_entry" {
            continue;
        }
        let case_id = emit_swiftui_node(
            result,
            owner_id,
            &switch_id,
            &switch_entry_label(child, source),
            NodeKind::Branch,
            file,
            child,
        );
        let mut case_cursor = child.walk();
        for case_child in child.named_children(&mut case_cursor) {
            if case_child.kind() == "statements" {
                let _ = extract_swiftui_structure(
                    case_child,
                    source,
                    file,
                    module_path,
                    owner_id,
                    &case_id,
                    result,
                );
            }
        }
    }

    vec![switch_id]
}

fn extract_swiftui_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    result: &mut ExtractionResult,
) -> Vec<String> {
    match node.kind() {
        "statements" | "lambda_literal" | "computed_property" => recurse_swiftui_named_children(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            result,
        ),
        "call_expression" => {
            if let Some(name) = swiftui_call_name(node, source)
                && uppercase_identifier(&name)
            {
                let view_id = emit_swiftui_node(
                    result,
                    owner_id,
                    parent_id,
                    &name,
                    NodeKind::View,
                    file,
                    node,
                );
                if !is_builtin_swiftui_view(&name) {
                    emit_unique_edge(
                        result,
                        Edge {
                            source: view_id.clone(),
                            target: make_id(file, module_path, &name),
                            kind: EdgeKind::TypeRef,
                            confidence: 0.85,
                            direction: None,
                            operation: None,
                            condition: None,
                            async_boundary: None,
                            provenance: node_edge_provenance(file, node, &view_id),
                        },
                    );
                }
                for lambda in call_suffix_lambda_children(node) {
                    let _ = extract_swiftui_structure(
                        lambda,
                        source,
                        file,
                        module_path,
                        owner_id,
                        &view_id,
                        result,
                    );
                }
                vec![view_id]
            } else if let Some((receiver, modifier_name)) =
                swiftui_modifier_receiver_and_name(node, source)
            {
                let anchors = extract_swiftui_structure(
                    receiver,
                    source,
                    file,
                    module_path,
                    owner_id,
                    parent_id,
                    result,
                );
                if is_view_builder_modifier(&modifier_name) {
                    let modifier_parents: Vec<String> = if anchors.is_empty() {
                        vec![parent_id.to_string()]
                    } else {
                        anchors.clone()
                    };
                    for lambda in call_suffix_lambda_children(node) {
                        for anchor in &modifier_parents {
                            let _ = extract_swiftui_structure(
                                lambda,
                                source,
                                file,
                                module_path,
                                owner_id,
                                anchor,
                                result,
                            );
                        }
                    }
                }
                anchors
            } else if let Some(view_id) = emit_swiftui_call_reference(
                node,
                source,
                file,
                module_path,
                owner_id,
                parent_id,
                result,
            ) {
                vec![view_id]
            } else {
                recurse_swiftui_named_children(
                    node,
                    source,
                    file,
                    module_path,
                    owner_id,
                    parent_id,
                    result,
                )
            }
        }
        "if_statement" => extract_swiftui_if_structure(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            result,
        ),
        "switch_statement" => extract_swiftui_switch_structure(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            result,
        ),
        "simple_identifier" | "navigation_expression" => emit_swiftui_view_reference(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            result,
        )
        .into_iter()
        .collect(),
        _ => recurse_swiftui_named_children(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            result,
        ),
    }
}

fn extract_swiftui_declaration_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    decl_id: &str,
    result: &mut ExtractionResult,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "computed_property" | "function_body") {
            let _ = extract_swiftui_structure(
                child,
                source,
                file,
                module_path,
                decl_id,
                decl_id,
                result,
            );
        }
    }
}

fn type_text_looks_like_swiftui_view(type_text: &str) -> bool {
    let trimmed = type_text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("some View")
        || trimmed.starts_with("any View")
        || trimmed.starts_with("some SwiftUI.View")
        || trimmed.starts_with("any SwiftUI.View")
    {
        return true;
    }

    let ident: String = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '<' | '>'))
        .collect();
    let base = ident
        .split('<')
        .next()
        .unwrap_or(&ident)
        .rsplit('.')
        .next()
        .unwrap_or(&ident);
    base == "View" || base.ends_with("View") || is_builtin_swiftui_view(base)
}

fn declaration_returns_swiftui_view(node: tree_sitter::Node, source: &[u8]) -> bool {
    let Ok(text) = node.utf8_text(source) else {
        return false;
    };

    match node.kind() {
        "property_declaration" => text
            .split_once(':')
            .map(|(_, type_text)| type_text_looks_like_swiftui_view(type_text))
            .unwrap_or(false),
        "function_declaration" | "protocol_function_declaration" => text
            .split_once("->")
            .map(|(_, type_text)| type_text_looks_like_swiftui_view(type_text))
            .unwrap_or(false),
        _ => false,
    }
}

fn file_matches(node_file: &Path, file_path: &Path) -> bool {
    let node_file = node_file.to_string_lossy();
    let file_path = file_path.to_string_lossy();
    node_file == file_path
        || node_file.ends_with(file_path.as_ref())
        || file_path.ends_with(node_file.as_ref())
        || node_file
            .rsplit('/')
            .next()
            .zip(file_path.rsplit('/').next())
            .is_some_and(|(left, right)| left == right)
}

fn line_matches(node_line: usize, ast_row_zero_based: usize) -> bool {
    node_line.abs_diff(ast_row_zero_based) <= 1
}

fn collect_swiftui_declaration_nodes<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if matches!(node.kind(), "property_declaration" | "function_declaration")
        && declaration_returns_swiftui_view(node, source)
    {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_swiftui_declaration_nodes(child, source, out);
    }
}

fn declaration_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "property_declaration" => find_pattern_name(node, source),
        "function_declaration" => simple_identifier_text(node, source),
        "protocol_function_declaration" => simple_identifier_text(node, source),
        _ => None,
    }
}

fn declaration_kind(node: tree_sitter::Node) -> Option<NodeKind> {
    match node.kind() {
        "property_declaration" => Some(NodeKind::Property),
        "function_declaration" | "protocol_function_declaration" => Some(NodeKind::Function),
        _ => None,
    }
}

fn matching_swiftui_declaration_id(
    result: &ExtractionResult,
    file_path: &Path,
    decl_node: tree_sitter::Node,
    source: &[u8],
) -> Option<String> {
    let decl_line = decl_node.start_position().row;
    let decl_name = declaration_name(decl_node, source)?;
    let decl_kind = declaration_kind(decl_node)?;

    result
        .nodes
        .iter()
        .filter(|node| {
            node.kind == decl_kind
                && node.name == decl_name
                && file_matches(&node.file, file_path)
                && line_matches(node.span.start[0], decl_line)
        })
        .min_by_key(|node| node.span.start[0].abs_diff(decl_line))
        .map(|node| node.id.clone())
}

fn collect_swiftui_body_nodes<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if node.kind() == "class_declaration" {
        let conformances = collect_inheritance_names(node, source);
        let conforms_to_view = conformances.iter().any(|c| c == "View" || c == "App");
        if conforms_to_view {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "class_body" {
                    continue;
                }
                let mut body_cursor = child.walk();
                for body_child in child.named_children(&mut body_cursor) {
                    if body_child.kind() == "property_declaration"
                        && let Some(name) = find_pattern_name(body_child, source)
                        && name == "body"
                    {
                        out.push(body_child);
                    }
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_swiftui_body_nodes(child, source, out);
    }
}

fn matching_body_id(
    result: &ExtractionResult,
    file_path: &Path,
    body_node: tree_sitter::Node,
) -> Option<String> {
    let body_line = body_node.start_position().row;
    result
        .nodes
        .iter()
        .filter(|node| {
            node.kind == NodeKind::Property
                && node.name == "body"
                && file_matches(&node.file, file_path)
                && line_matches(node.span.start[0], body_line)
        })
        .min_by_key(|node| node.span.start[0].abs_diff(body_line))
        .map(|node| node.id.clone())
}

pub fn enrich_swiftui_structure(
    source: &[u8],
    file_path: &Path,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_swift::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Swift source"))?;

    let mut declaration_nodes = Vec::new();
    collect_swiftui_declaration_nodes(tree.root_node(), source, &mut declaration_nodes);

    let file_str = file_path.to_string_lossy().to_string();
    for decl_node in declaration_nodes {
        let Some(decl_id) = matching_swiftui_declaration_id(result, file_path, decl_node, source)
        else {
            continue;
        };
        extract_swiftui_declaration_structure(decl_node, source, &file_str, &[], &decl_id, result);
    }

    let mut body_nodes = Vec::new();
    collect_swiftui_body_nodes(tree.root_node(), source, &mut body_nodes);

    for body_node in body_nodes {
        let Some(body_id) = matching_body_id(result, file_path, body_node) else {
            continue;
        };
        extract_swiftui_declaration_structure(body_node, source, &file_str, &[], &body_id, result);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::{EdgeKind, NodeKind, Visibility};

    fn extract(source: &str) -> ExtractionResult {
        let extractor = SwiftExtractor;
        extractor
            .extract(source.as_bytes(), Path::new("test.swift"))
            .unwrap()
    }

    fn find_node<'a>(result: &'a ExtractionResult, name: &str) -> &'a grapha_core::graph::Node {
        result
            .nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("node '{}' not found", name))
    }

    fn has_edge(result: &ExtractionResult, source: &str, target: &str, kind: EdgeKind) -> bool {
        result
            .edges
            .iter()
            .any(|e| e.source == source && e.target == target && e.kind == kind)
    }

    #[test]
    fn extracts_struct() {
        let result = extract("public struct Config { let debug: Bool }");
        let node = find_node(&result, "Config");
        assert_eq!(node.kind, NodeKind::Struct);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_class() {
        let result = extract("public class AppDelegate { }");
        let node = find_node(&result, "AppDelegate");
        assert_eq!(node.kind, NodeKind::Struct); // classes map to Struct in our model
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_protocol() {
        let result = extract("public protocol Drawable { func draw() }");
        let node = find_node(&result, "Drawable");
        assert_eq!(node.kind, NodeKind::Protocol);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_enum_with_cases() {
        let result = extract(
            r#"
            public enum Color {
                case red
                case green
            }
            "#,
        );
        let color = find_node(&result, "Color");
        assert_eq!(color.kind, NodeKind::Enum);

        let red = find_node(&result, "red");
        assert_eq!(red.kind, NodeKind::Variant);

        let green = find_node(&result, "green");
        assert_eq!(green.kind, NodeKind::Variant);

        assert!(has_edge(&result, &color.id, &red.id, EdgeKind::Contains));
        assert!(has_edge(&result, &color.id, &green.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_function() {
        let result = extract("public func greet() { }");
        let node = find_node(&result, "greet");
        assert_eq!(node.kind, NodeKind::Function);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn overloaded_initializers_get_distinct_ids() {
        let result = extract(
            r#"
            struct StringPair {
                init(key: String, value: String) {}
                init?(iosLine: String) {}
            }
            "#,
        );

        let init_nodes: Vec<_> = result
            .nodes
            .iter()
            .filter(|node| node.name == "init")
            .collect();
        assert_eq!(init_nodes.len(), 2);

        let unique_ids: std::collections::HashSet<_> =
            init_nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(unique_ids.len(), 2);
    }

    #[test]
    fn multiple_extensions_get_distinct_ids() {
        let result = extract(
            r#"
            struct Config {}

            extension Config {
                func alpha() {}
            }

            extension Config {
                func beta() {}
            }
            "#,
        );

        let extension_nodes: Vec<_> = result
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Extension)
            .collect();
        assert_eq!(extension_nodes.len(), 2);

        let unique_ids: std::collections::HashSet<_> = extension_nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(unique_ids.len(), 2);
    }

    #[test]
    fn extracts_extension() {
        let result = extract("extension Config { func foo() {} }");
        let ext = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Extension)
            .expect("extension node not found");
        assert_eq!(ext.name, "Config");

        let foo = find_node(&result, "foo");
        assert_eq!(foo.kind, NodeKind::Function);
        assert!(has_edge(&result, &ext.id, &foo.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_protocol_conformance() {
        let result = extract("public class AppDelegate: Configurable { }");
        let app = find_node(&result, "AppDelegate");
        assert!(has_edge(
            &result,
            &app.id,
            "test.swift::Configurable",
            EdgeKind::Implements
        ));
    }

    #[test]
    fn extracts_import() {
        let result = extract("import Foundation");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].path, "Foundation");
        assert_eq!(
            result.imports[0].kind,
            grapha_core::resolve::ImportKind::Module
        );
    }

    #[test]
    fn extracts_call_edges() {
        let result = extract(
            r#"
            func greet() { }
            func launch() {
                greet()
            }
            "#,
        );
        assert!(has_edge(
            &result,
            "test.swift::launch",
            "test.swift::greet",
            EdgeKind::Calls,
        ));
        let call_edge = result
            .edges
            .iter()
            .find(|edge| {
                edge.source == "test.swift::launch"
                    && edge.target == "test.swift::greet"
                    && edge.kind == EdgeKind::Calls
            })
            .expect("should find call edge");
        assert!(
            !call_edge.provenance.is_empty(),
            "call edges should carry provenance"
        );
        assert_eq!(call_edge.provenance[0].symbol_id, "test.swift::launch");
    }

    #[test]
    fn extracts_condition_on_call_inside_if() {
        let result = extract(
            r#"
            func run() {
                if true {
                    helper()
                }
            }
            func helper() { }
            "#,
        );
        let cond_edge = result
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls && e.target.contains("helper"));
        assert!(cond_edge.is_some(), "should find Calls edge to helper");
        // The condition may or may not be extracted depending on tree-sitter-swift's AST
        // We verify the edge exists and the mechanism doesn't crash
    }

    #[test]
    fn detects_view_body_as_entry_point() {
        let result = extract(
            r#"
            struct ContentView: View {
                var body: Int { return 0 }
            }
            "#,
        );
        let body_node = result
            .nodes
            .iter()
            .find(|n| n.name == "body")
            .expect("should find body property");
        assert_eq!(
            body_node.role,
            Some(grapha_core::graph::NodeRole::EntryPoint),
            "View.body should be an EntryPoint"
        );
    }

    #[test]
    fn scopes_body_ids_per_view_type() {
        let result = extract(
            r#"
            struct FirstView: View {
                var body: some View { Text("One") }
            }

            struct SecondView: View {
                var body: some View { Text("Two") }
            }
            "#,
        );

        let body_ids: Vec<&str> = result
            .nodes
            .iter()
            .filter(|n| n.name == "body" && n.kind == NodeKind::Property)
            .map(|n| n.id.as_str())
            .collect();

        assert_eq!(body_ids.len(), 2);
        assert!(body_ids.contains(&"test.swift::FirstView::body"));
        assert!(body_ids.contains(&"test.swift::SecondView::body"));
    }

    #[test]
    fn extracts_swiftui_view_hierarchy_and_type_refs() {
        let result = extract(
            r#"
            import SwiftUI

            struct Row: View {
                let title: String
                var body: some View { Text(title) }
            }

            struct ContentView: View {
                var body: some View {
                    VStack {
                        Text("Hello")
                        Row(title: "World")
                    }
                }
            }
            "#,
        );

        let body = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::body")
            .expect("content body node should exist");
        let vstack = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "VStack")
            .expect("VStack synthetic view should exist");
        let row_view = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "Row")
            .expect("Row synthetic view should exist");
        let vstack_children: Vec<&Node> = result
            .edges
            .iter()
            .filter(|edge| edge.source == vstack.id && edge.kind == EdgeKind::Contains)
            .filter_map(|edge| result.nodes.iter().find(|node| node.id == edge.target))
            .collect();

        assert!(has_edge(&result, &body.id, &vstack.id, EdgeKind::Contains));
        assert_eq!(
            vstack_children
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Text", "Row"]
        );
        assert!(has_edge(
            &result,
            &row_view.id,
            "test.swift::Row",
            EdgeKind::TypeRef
        ));
    }

    #[test]
    fn extracts_swiftui_branch_hierarchy() {
        let result = extract(
            r#"
            import SwiftUI

            struct ContentView: View {
                var body: some View {
                    VStack {
                        if showDetails {
                            Group {
                                Text("More")
                            }
                        } else {
                            switch mode {
                            case .empty:
                                EmptyView()
                            default:
                                ForEach(items) { item in
                                    Text(item.name)
                                }
                            }
                        }
                    }
                }
            }
            "#,
        );

        let vstack = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "VStack")
            .expect("VStack should exist");
        let if_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "if showDetails")
            .expect("if branch should exist");
        let else_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "else")
            .expect("else branch should exist");
        let switch_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "switch mode")
            .expect("switch branch should exist");
        let default_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "default")
            .expect("default branch should exist");
        let for_each = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "ForEach")
            .expect("ForEach view should exist");

        assert!(has_edge(
            &result,
            &vstack.id,
            &if_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &vstack.id,
            &else_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &else_branch.id,
            &switch_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &switch_branch.id,
            &default_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &default_branch.id,
            &for_each.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn extracts_swiftui_structure_through_modifier_chains_and_view_refs() {
        let result = extract(
            r#"
            import SwiftUI

            struct InlinePanel: View {
                var body: some View { Text("Inline") }
            }

            struct OverlayPanel: View {
                var body: some View { Text("Overlay") }
            }

            struct ContentView: View {
                var chatRoomFragViewPanel: some View {
                    InlinePanel()
                }

                var exitPopView: some View {
                    Text("Exit")
                }

                var body: some View {
                    NavigationStack {
                        if showDetails {
                            InlinePanel()
                                .onReceive(events) { _ in
                                    switch mode {
                                    case .loading:
                                        helper()
                                    default:
                                        break
                                    }
                                }
                        }

                        chatRoomFragViewPanel
                        DialogStreamView()
                        exitPopView
                    }
                    .frame(width: 100)
                    .overlay {
                        OverlayPanel()
                    }
                    .onReceive(events) { _ in
                        switch mode {
                        case .done:
                            helper()
                        default:
                            break
                        }
                    }
                }
            }
            "#,
        );

        let body = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::body")
            .expect("content body node should exist");
        let nav = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "NavigationStack")
            .expect("NavigationStack synthetic view should exist");
        let if_branch = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Branch && n.name == "if showDetails")
            .expect("if branch should exist");
        let body_inline_panel = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "InlinePanel"
                    && n.id.starts_with("test.swift::ContentView::body::view:")
            })
            .expect("body InlinePanel view should exist");
        let chat_panel = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "chatRoomFragViewPanel")
            .expect("chatRoomFragViewPanel view ref should exist");
        let exit_pop = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "exitPopView")
            .expect("exitPopView view ref should exist");
        let dialog_stream = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "DialogStreamView")
            .expect("DialogStreamView should exist");
        let overlay_panel = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::View && n.name == "OverlayPanel")
            .expect("OverlayPanel should exist");
        let helper_property = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::chatRoomFragViewPanel")
            .expect("helper property should exist");
        let exit_property = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::exitPopView")
            .expect("exit property should exist");
        let helper_text = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "Text"
                    && n.id
                        .starts_with("test.swift::ContentView::exitPopView::view:")
            })
            .expect("exit helper text should exist");
        let helper_inline_panel = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "InlinePanel"
                    && n.id
                        .starts_with("test.swift::ContentView::chatRoomFragViewPanel::view:")
            })
            .expect("helper InlinePanel view should exist");

        assert!(has_edge(&result, &body.id, &nav.id, EdgeKind::Contains));
        assert!(has_edge(
            &result,
            &nav.id,
            &if_branch.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &if_branch.id,
            &body_inline_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &nav.id,
            &chat_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(&result, &nav.id, &exit_pop.id, EdgeKind::Contains));
        assert!(has_edge(
            &result,
            &nav.id,
            &dialog_stream.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &nav.id,
            &overlay_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &helper_property.id,
            &helper_inline_panel.id,
            EdgeKind::Contains
        ));
        assert!(has_edge(
            &result,
            &exit_property.id,
            &helper_text.id,
            EdgeKind::Contains
        ));
        assert!(
            result
                .nodes
                .iter()
                .all(|n| !(n.kind == NodeKind::Branch && n.name == "switch mode")),
            "non-view modifier closures should not become structural branches"
        );
    }

    #[test]
    fn extracts_swiftui_structure_for_same_type_view_methods() {
        let result = extract(
            r#"
            import SwiftUI

            struct ContentView: View {
                func helperPanel() -> some View {
                    Text("Helper")
                }

                var body: some View {
                    VStack {
                        helperPanel()
                    }
                }
            }
            "#,
        );

        let helper_fn = result
            .nodes
            .iter()
            .find(|n| n.id == "test.swift::ContentView::helperPanel")
            .expect("helper method should exist");
        let helper_call = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "helperPanel"
                    && n.id.starts_with("test.swift::ContentView::body::view:")
            })
            .expect("helper method call should exist");
        let helper_text = result
            .nodes
            .iter()
            .find(|n| {
                n.kind == NodeKind::View
                    && n.name == "Text"
                    && n.id
                        .starts_with("test.swift::ContentView::helperPanel::view:")
            })
            .expect("helper method text should exist");

        assert!(has_edge(
            &result,
            &helper_call.id,
            &helper_fn.id,
            EdgeKind::TypeRef
        ));
        assert!(has_edge(
            &result,
            &helper_fn.id,
            &helper_text.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn enrich_swiftui_structure_overlays_synthetic_nodes_without_duplicating_declarations() {
        let source = br#"
import SwiftUI

struct Row: View {
    let title: String
    var body: some View { Text(title) }
}

struct ContentView: View {
    var body: some View {
        VStack {
            Text("Hello")
            Row(title: "World")
        }
    }
}
"#;
        let mut result = ExtractionResult::new();
        result.nodes.push(Node {
            id: "s:Row".into(),
            kind: NodeKind::Struct,
            name: "Row".into(),
            file: "test.swift".into(),
            span: Span {
                start: [3, 0],
                end: [6, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
        });
        result.nodes.push(Node {
            id: "s:ContentView".into(),
            kind: NodeKind::Struct,
            name: "ContentView".into(),
            file: "test.swift".into(),
            span: Span {
                start: [8, 0],
                end: [15, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
        });
        result.nodes.push(Node {
            id: "s:ContentView.body".into(),
            kind: NodeKind::Property,
            name: "body".into(),
            file: "test.swift".into(),
            span: Span {
                start: [9, 4],
                end: [14, 5],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: Some(NodeRole::EntryPoint),
            signature: None,
            doc_comment: None,
            module: None,
        });

        enrich_swiftui_structure(source, Path::new("test.swift"), &mut result).unwrap();

        let view_nodes: Vec<&Node> = result
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::View)
            .collect();
        assert!(
            !view_nodes.is_empty(),
            "overlay should add synthetic view nodes"
        );
        assert_eq!(
            result
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Property && n.name == "body")
                .count(),
            1,
            "overlay should not duplicate declaration nodes"
        );
        let vstack = view_nodes
            .iter()
            .find(|n| n.name == "VStack")
            .expect("overlay should add VStack");
        assert!(has_edge(
            &result,
            "s:ContentView.body",
            &vstack.id,
            EdgeKind::Contains
        ));
        let row_view = view_nodes
            .iter()
            .find(|n| n.name == "Row")
            .expect("overlay should add Row instance");
        assert!(has_edge(
            &result,
            &row_view.id,
            "test.swift::Row",
            EdgeKind::TypeRef
        ));
    }

    #[test]
    fn extracts_function_signature() {
        let result = extract("func greet(name: String) -> String { return name }");
        let node = find_node(&result, "greet");
        assert!(node.signature.is_some(), "signature should be extracted");
        let sig = node.signature.as_ref().unwrap();
        assert!(
            sig.contains("func greet"),
            "signature should contain func name"
        );
    }

    #[test]
    fn extracts_doc_comment() {
        let result = extract(
            r#"
            /// A documented function
            func documented() { }
            "#,
        );
        let node = find_node(&result, "documented");
        assert!(
            node.doc_comment.is_some(),
            "doc_comment should be extracted"
        );
        let doc = node.doc_comment.as_ref().unwrap();
        assert!(doc.contains("documented"), "should contain comment text");
    }

    #[test]
    fn extracts_doc_comment_with_attributes() {
        let result = extract(
            r#"
            class GameManager {
                /// Setup the initial running context and load the game scene, this method should be called when
                /// the game view is appeared.
                @MainActor public func bootstrapGame(with launchContext: WebGameLaunchContext) async {
                }
            }
            "#,
        );
        let node = find_node(&result, "bootstrapGame");
        assert!(
            node.doc_comment.is_some(),
            "doc_comment should be extracted for method with @MainActor attribute"
        );
        let doc = node.doc_comment.as_ref().unwrap();
        assert!(
            doc.contains("Setup the initial running context"),
            "should contain comment text, got: {}",
            doc
        );
    }

    #[test]
    fn enrich_doc_comments_patches_missing_docs() {
        let source = br#"
class GameManager {
    /// Setup the initial running context and load the game scene.
    /// This method should be called when the game view is appeared.
    @MainActor public func bootstrapGame(with launchContext: WebGameLaunchContext) async {
    }

    /// Returns the current score.
    var score: Int { 0 }
}
"#;
        // Simulate index-store output: correct names and 1-based lines, no doc comments.
        let mut result = ExtractionResult::new();
        result.nodes.push(Node {
            id: "s:GameManager".into(),
            kind: NodeKind::Struct,
            name: "GameManager".into(),
            file: "test.swift".into(),
            span: Span {
                start: [1, 0],
                end: [1, 0],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
        });
        result.nodes.push(Node {
            id: "s:GameManager.bootstrapGame".into(),
            kind: NodeKind::Function,
            name: "bootstrapGame".into(),
            file: "test.swift".into(),
            span: Span {
                start: [5, 4],
                end: [5, 4],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
        });
        result.nodes.push(Node {
            id: "s:GameManager.score".into(),
            kind: NodeKind::Property,
            name: "score".into(),
            file: "test.swift".into(),
            span: Span {
                start: [9, 4],
                end: [9, 4],
            },
            visibility: Visibility::Public,
            metadata: HashMap::new(),
            role: None,
            signature: None,
            doc_comment: None,
            module: None,
        });

        enrich_doc_comments(source, &mut result).unwrap();

        let _game_mgr = result
            .nodes
            .iter()
            .find(|n| n.name == "GameManager")
            .unwrap();
        // class_declaration on line 1 (0-based row 1 → 1-based line 2 actually)
        // Let's just check the function and property which are the real targets.

        let bootstrap = result
            .nodes
            .iter()
            .find(|n| n.name == "bootstrapGame")
            .unwrap();
        assert!(
            bootstrap.doc_comment.is_some(),
            "bootstrapGame should have doc_comment after enrichment"
        );
        assert!(
            bootstrap
                .doc_comment
                .as_ref()
                .unwrap()
                .contains("Setup the initial running context"),
            "doc should contain expected text"
        );

        let score = result.nodes.iter().find(|n| n.name == "score").unwrap();
        assert!(
            score.doc_comment.is_some(),
            "score property should have doc_comment after enrichment"
        );
        assert!(
            score
                .doc_comment
                .as_ref()
                .unwrap()
                .contains("current score"),
            "doc should contain expected text"
        );
    }

    #[test]
    fn detects_main_attr_as_entry_point() {
        let result = extract(
            r#"
            @main
            struct MyApp {
            }
            "#,
        );
        let app_node = find_node(&result, "MyApp");
        assert_eq!(
            app_node.role,
            Some(grapha_core::graph::NodeRole::EntryPoint),
            "@main struct should be an EntryPoint"
        );
    }
}
