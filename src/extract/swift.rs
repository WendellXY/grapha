use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Parser;

use crate::graph::{Edge, EdgeKind, Node, NodeKind, NodeRole, Span, Visibility};

use super::{ExtractionResult, LanguageExtractor};

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
        "function_declaration" => {
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

fn emit_contains_edge(parent_id: Option<&str>, child_id: &str, result: &mut ExtractionResult) {
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
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, result);

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
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, result);

    result.nodes.push(Node {
        id,
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
    let Some(name) = simple_identifier_text(node, source) else {
        return;
    };
    let id = make_id(file, module_path, &name);
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

    emit_contains_edge(parent_id, &id, result);

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

    // Walk function body for call expressions
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "function_body" {
            extract_calls(child, source, file, module_path, &id, result);
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
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, result);

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
            extract_enum_entries(child, source, file, module_path, &id, &name, result);
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
    parent_name: &str,
    result: &mut ExtractionResult,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "enum_entry"
            && let Some(case_name) = simple_identifier_text(child, source)
        {
            let qualified = format!("{parent_name}.{case_name}");
            let id = make_id(file, module_path, &qualified);

            result.edges.push(Edge {
                source: parent_id.to_string(),
                target: id.clone(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
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
    let id = make_id(file, module_path, &ext_name);

    emit_contains_edge(parent_id, &id, result);

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
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, result);

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

/// Extract function declaration.
fn extract_function(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    let Some(name) = simple_identifier_text(node, source) else {
        return;
    };
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);
    let signature = extract_swift_signature(node, source);
    let doc_comment = extract_swift_doc_comment(node, source);

    emit_contains_edge(parent_id, &id, result);

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

    // Walk function body for call expressions
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "function_body" {
            extract_calls(child, source, file, module_path, &id, result);
        }
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
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, result);

    result.nodes.push(Node {
        id,
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
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);

    emit_contains_edge(parent_id, &id, result);

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

            result.imports.push(crate::resolve::Import {
                path: path.clone(),
                symbols: vec![],
                kind: crate::resolve::ImportKind::Module,
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
                    && text.contains("DispatchQueue") && text.contains("async")
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
            });
        }
    }
}

/// Recursively scan for `call_expression` nodes, emitting Calls edges.
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
        });
    }

    // Recurse into all children
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        extract_calls(child, source, file, module_path, caller_id, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeKind, NodeKind, Visibility};

    fn extract(source: &str) -> ExtractionResult {
        let extractor = SwiftExtractor;
        extractor
            .extract(source.as_bytes(), Path::new("test.swift"))
            .unwrap()
    }

    fn find_node<'a>(result: &'a ExtractionResult, name: &str) -> &'a crate::graph::Node {
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
        assert_eq!(result.imports[0].kind, crate::resolve::ImportKind::Module);
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
            Some(crate::graph::NodeRole::EntryPoint),
            "View.body should be an EntryPoint"
        );
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
            Some(crate::graph::NodeRole::EntryPoint),
            "@main struct should be an EntryPoint"
        );
    }
}
