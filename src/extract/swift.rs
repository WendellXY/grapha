use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Parser;

use crate::graph::{Edge, EdgeKind, Node, NodeKind, Span, Visibility};

use super::{ExtractionResult, LanguageExtractor};

pub struct SwiftExtractor;

impl LanguageExtractor for SwiftExtractor {
    fn language(&self) -> &str {
        "swift"
    }

    fn file_extensions(&self) -> &[&str] {
        &["swift"]
    }

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

    result.nodes.push(Node {
        id: id.clone(),
        kind,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
    });

    // Walk the class_body for nested declarations
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "class_body" {
            walk_children(child, source, file, module_path, Some(&id), result);
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
            });

            result.nodes.push(Node {
                id,
                kind: NodeKind::Variant,
                name: case_name,
                file: file.into(),
                span: make_span(child),
                visibility: Visibility::Public,
                metadata: HashMap::new(),
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

    emit_contains_edge(parent_id, &id, result);

    result.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Function,
        name,
        file: file.into(),
        span: make_span(node),
        visibility,
        metadata: HashMap::new(),
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
            });
        }
    }
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
    if node.kind() == "call_expression" {
        // The first named child (simple_identifier) is the function name
        if let Some(fn_name) = simple_identifier_text(node, source) {
            let target_id = make_id(file, module_path, &fn_name);
            result.edges.push(Edge {
                source: caller_id.to_string(),
                target: target_id,
                kind: EdgeKind::Calls,
                confidence: 0.8,
            });
        }
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
}
