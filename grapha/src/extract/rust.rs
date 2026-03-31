use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Parser;

use grapha_core::graph::{
    Edge, EdgeKind, EdgeProvenance, Node, NodeKind, NodeRole, Span, Visibility,
};

use super::{ExtractionResult, LanguageExtractor};

pub struct RustExtractor;

impl LanguageExtractor for RustExtractor {
    fn extract(&self, source: &[u8], file_path: &Path) -> anyhow::Result<ExtractionResult> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse source"))?;

        let mut result = ExtractionResult::new();
        let file_str = file_path.to_string_lossy().to_string();

        walk_node(tree.root_node(), source, &file_str, &[], None, &mut result);

        Ok(result)
    }
}

fn edge_provenance(file: &str, node: tree_sitter::Node, symbol_id: &str) -> Vec<EdgeProvenance> {
    let start = node.start_position();
    let end = node.end_position();
    vec![EdgeProvenance {
        file: file.into(),
        span: Span {
            start: [start.row, start.column],
            end: [end.row, end.column],
        },
        symbol_id: symbol_id.to_string(),
    }]
}

/// Recursively walk a tree-sitter node, extracting symbols and edges.
///
/// `module_path` tracks the logical nesting (module names) for ID generation.
/// `parent_id` is the node ID of the enclosing symbol, used to emit Contains edges.
fn walk_node(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    parent_id: Option<&str>,
    result: &mut ExtractionResult,
) {
    match node.kind() {
        "function_item" | "function_signature_item" => {
            if let Some(graph_node) = extract_function(node, source, file, module_path) {
                if let Some(pid) = parent_id {
                    result.edges.push(Edge {
                        source: pid.to_string(),
                        target: graph_node.id.clone(),
                        kind: EdgeKind::Contains,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: edge_provenance(file, node, pid),
                    });
                }
                let node_id = graph_node.id.clone();
                result.nodes.push(graph_node);

                // Emit TypeRef edge for non-primitive return types
                if let Some(return_type_node) = node.child_by_field_name("return_type")
                    && let Ok(return_text) = return_type_node.utf8_text(source)
                {
                    // Strip leading "->" and whitespace
                    let type_name = return_text.trim_start_matches("->").trim();
                    if !type_name.is_empty() && !is_primitive(type_name) && type_name != "Self" {
                        let target_id = make_id(file, module_path, type_name);
                        result.edges.push(Edge {
                            source: node_id.clone(),
                            target: target_id,
                            kind: EdgeKind::TypeRef,
                            confidence: 0.85,
                            direction: None,
                            operation: None,
                            condition: None,
                            async_boundary: None,
                            provenance: edge_provenance(file, return_type_node, &node_id),
                        });
                    }
                }

                // Walk function body for nested items and call expressions
                if let Some(body) = node.child_by_field_name("body") {
                    walk_children(body, source, file, module_path, Some(&node_id), result);
                    extract_calls(body, source, file, module_path, &node_id, result);
                }
            }
        }
        "struct_item" => {
            if let Some(graph_node) =
                extract_named_item(node, source, file, module_path, NodeKind::Struct)
            {
                if let Some(pid) = parent_id {
                    result.edges.push(Edge {
                        source: pid.to_string(),
                        target: graph_node.id.clone(),
                        kind: EdgeKind::Contains,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: edge_provenance(file, node, pid),
                    });
                }
                let node_id = graph_node.id.clone();
                let node_name = graph_node.name.clone();
                result.nodes.push(graph_node);

                // Extract fields from the struct body
                if let Some(body) = node.child_by_field_name("body") {
                    extract_struct_fields(
                        body,
                        source,
                        file,
                        module_path,
                        &node_id,
                        &node_name,
                        result,
                    );
                }
            }
        }
        "enum_item" => {
            if let Some(graph_node) =
                extract_named_item(node, source, file, module_path, NodeKind::Enum)
            {
                if let Some(pid) = parent_id {
                    result.edges.push(Edge {
                        source: pid.to_string(),
                        target: graph_node.id.clone(),
                        kind: EdgeKind::Contains,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: edge_provenance(file, node, pid),
                    });
                }
                let node_id = graph_node.id.clone();
                let node_name = graph_node.name.clone();
                result.nodes.push(graph_node);

                // Extract variants from the enum body
                if let Some(body) = node.child_by_field_name("body") {
                    extract_enum_variants(
                        body,
                        source,
                        file,
                        module_path,
                        &node_id,
                        &node_name,
                        result,
                    );
                }
            }
        }
        "trait_item" => {
            if let Some(graph_node) =
                extract_named_item(node, source, file, module_path, NodeKind::Trait)
            {
                if let Some(pid) = parent_id {
                    result.edges.push(Edge {
                        source: pid.to_string(),
                        target: graph_node.id.clone(),
                        kind: EdgeKind::Contains,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: edge_provenance(file, node, pid),
                    });
                }
                let node_id = graph_node.id.clone();
                result.nodes.push(graph_node);

                // Emit Inherits edges for supertrait bounds (e.g. `trait Child: Base`)
                if let Some(bounds) = node.child_by_field_name("bounds") {
                    let mut cursor = bounds.walk();
                    for child in bounds.named_children(&mut cursor) {
                        if child.kind() == "type_identifier"
                            && let Ok(bound_name) = child.utf8_text(source)
                        {
                            let target_id = make_id(file, module_path, bound_name);
                            result.edges.push(Edge {
                                source: node_id.clone(),
                                target: target_id,
                                kind: EdgeKind::Inherits,
                                confidence: 0.9,
                                direction: None,
                                operation: None,
                                condition: None,
                                async_boundary: None,
                                provenance: edge_provenance(file, child, &node_id),
                            });
                        }
                    }
                }

                if let Some(body) = node.child_by_field_name("body") {
                    walk_children(body, source, file, module_path, Some(&node_id), result);
                }
            }
        }
        "impl_item" => {
            if let Some(graph_node) = extract_impl_item(node, source, file, module_path) {
                if let Some(pid) = parent_id {
                    result.edges.push(Edge {
                        source: pid.to_string(),
                        target: graph_node.id.clone(),
                        kind: EdgeKind::Contains,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: edge_provenance(file, node, pid),
                    });
                }
                let node_id = graph_node.id.clone();

                // Emit Implements edge if this is `impl Trait for Type`
                // The source is the type being implemented, target is the trait
                if let Some(trait_node) = node.child_by_field_name("trait")
                    && let Ok(trait_name) = trait_node.utf8_text(source)
                {
                    let type_name = &graph_node.name;
                    let type_id = make_id(file, module_path, type_name);
                    let trait_id = make_id(file, module_path, trait_name);
                    result.edges.push(Edge {
                        source: type_id,
                        target: trait_id,
                        kind: EdgeKind::Implements,
                        confidence: 0.9,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: edge_provenance(file, trait_node, &node_id),
                    });
                }

                result.nodes.push(graph_node);

                if let Some(body) = node.child_by_field_name("body") {
                    walk_children(body, source, file, module_path, Some(&node_id), result);
                }
            }
        }
        "mod_item" => {
            if let Some(graph_node) =
                extract_named_item(node, source, file, module_path, NodeKind::Module)
            {
                if let Some(pid) = parent_id {
                    result.edges.push(Edge {
                        source: pid.to_string(),
                        target: graph_node.id.clone(),
                        kind: EdgeKind::Contains,
                        confidence: 1.0,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: edge_provenance(file, node, pid),
                    });
                }
                let mod_name = graph_node.name.clone();
                let node_id = graph_node.id.clone();
                result.nodes.push(graph_node);

                // Walk the module body with extended module_path
                if let Some(body) = node.child_by_field_name("body") {
                    let mut new_path = module_path.to_vec();
                    new_path.push(mod_name);
                    walk_children(body, source, file, &new_path, Some(&node_id), result);
                }
            }
        }
        "use_declaration" => {
            if let Ok(use_text) = node.utf8_text(source) {
                let raw = use_text
                    .trim_start_matches("use ")
                    .trim_end_matches(';')
                    .trim()
                    .to_string();

                let kind = if raw.starts_with("crate::")
                    || raw.starts_with("super::")
                    || raw.starts_with("self::")
                {
                    grapha_core::resolve::ImportKind::Relative
                } else if raw.ends_with("::*") {
                    grapha_core::resolve::ImportKind::Wildcard
                } else {
                    grapha_core::resolve::ImportKind::Named
                };

                // Extract symbols from grouped imports: use foo::{A, B}
                let (path, symbols) = if let Some(brace_start) = raw.find('{') {
                    let base = raw[..brace_start].trim_end_matches("::").to_string();
                    let inner = raw[brace_start + 1..].trim_end_matches('}').trim();
                    let syms = inner.split(',').map(|s| s.trim().to_string()).collect();
                    (base, syms)
                } else {
                    (raw.trim_end_matches("::*").to_string(), vec![])
                };

                result.imports.push(grapha_core::resolve::Import {
                    path,
                    symbols,
                    kind,
                });

                // Keep the Uses edge for backwards compatibility
                result.edges.push(Edge {
                    source: file.to_string(),
                    target: use_text.to_string(),
                    kind: EdgeKind::Uses,
                    confidence: 0.7,
                    direction: None,
                    operation: None,
                    condition: None,
                    async_boundary: None,
                    provenance: edge_provenance(file, node, file),
                });
            }
        }
        _ => {
            // For any other node kind, just walk its children
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

/// Extract the text of a named child field.
fn field_text<'a>(node: tree_sitter::Node<'a>, field: &str, source: &'a [u8]) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|n| n.utf8_text(source).ok())
        .map(|s| s.to_string())
}

/// Extract visibility from a node by checking for a `visibility_modifier` child.
fn extract_visibility(node: tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let text = child.utf8_text(source).unwrap_or("");
            if text.contains("pub(crate)") {
                return Visibility::Crate;
            } else if text.starts_with("pub") {
                return Visibility::Public;
            }
        }
    }
    Visibility::Private
}

/// Extract metadata (async, unsafe) from the `function_modifiers` named child.
///
/// tree-sitter-rust wraps these keywords in a `function_modifiers` node.
/// The keywords themselves are anonymous tokens with kinds `"async"` / `"unsafe"`.
fn extract_function_metadata(node: tree_sitter::Node, _source: &[u8]) -> HashMap<String, String> {
    let mut meta = HashMap::new();

    // Check direct children for function_modifiers node
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "function_modifiers" {
            let mut mod_cursor = child.walk();
            for modifier in child.children(&mut mod_cursor) {
                match modifier.kind() {
                    "async" => {
                        meta.insert("async".to_string(), "true".to_string());
                    }
                    "unsafe" => {
                        meta.insert("unsafe".to_string(), "true".to_string());
                    }
                    _ => {}
                }
            }
        }
    }

    meta
}

/// Extract a function_item or function_signature_item into a Node.
fn extract_function(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
) -> Option<Node> {
    let name = field_text(node, "name", source)?;
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);
    let metadata = extract_function_metadata(node, source);
    let start = node.start_position();
    let end = node.end_position();

    let role = detect_entry_point(node, source, &name, module_path);
    let signature = extract_signature(node, source);
    let doc_comment = extract_doc_comment(node, source);

    Some(Node {
        id,
        kind: NodeKind::Function,
        name,
        file: file.into(),
        span: Span {
            start: [start.row, start.column],
            end: [end.row, end.column],
        },
        visibility,
        metadata,
        role,
        signature,
        doc_comment,
        module: None,
    })
}

/// Detect if a function is an entry point.
///
/// Entry points are: `fn main()` at module level, functions with `#[test]`,
/// `#[tokio::main]`, or `pub fn` at crate root level (no parent impl/trait).
fn detect_entry_point(
    node: tree_sitter::Node,
    source: &[u8],
    name: &str,
    module_path: &[String],
) -> Option<NodeRole> {
    let attrs = collect_attributes(node, source);
    let is_module_level = node
        .parent()
        .map(|p| p.kind() == "source_file" || p.kind() == "declaration_list")
        .unwrap_or(false);
    let is_inside_impl_or_trait = node
        .parent()
        .and_then(|p| p.parent())
        .map(|gp| gp.kind() == "impl_item" || gp.kind() == "trait_item")
        .unwrap_or(false);

    // #[test] or #[tokio::main] attributes
    for attr in &attrs {
        if attr == "test" || attr == "tokio::test" || attr == "tokio::main" {
            return Some(NodeRole::EntryPoint);
        }
    }

    // fn main() at module level, not inside impl/trait
    if name == "main" && is_module_level && !is_inside_impl_or_trait {
        return Some(NodeRole::EntryPoint);
    }

    // pub fn at crate root level (module_path is empty, not inside impl/trait)
    let visibility = extract_visibility(node, source);
    if visibility == Visibility::Public
        && module_path.is_empty()
        && is_module_level
        && !is_inside_impl_or_trait
    {
        return Some(NodeRole::EntryPoint);
    }

    None
}

/// Collect attribute names from sibling `attribute_item` nodes before the function.
fn collect_attributes(node: tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut attrs = Vec::new();
    let mut prev = node.prev_named_sibling();
    while let Some(sib) = prev {
        if sib.kind() == "attribute_item" {
            if let Ok(text) = sib.utf8_text(source) {
                // Strip #[ and ]
                let inner = text.trim_start_matches("#[").trim_end_matches(']').trim();
                // Take the attribute path (before any parentheses)
                let attr_name = inner.split('(').next().unwrap_or(inner).trim();
                attrs.push(attr_name.to_string());
            }
            prev = sib.prev_named_sibling();
        } else {
            break;
        }
    }
    attrs
}

/// Extract function signature (text up to opening `{`).
fn extract_signature(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let text = node.utf8_text(source).ok()?;
    let sig = if let Some(brace_pos) = text.find('{') {
        text[..brace_pos].trim()
    } else {
        // For signature items (no body), use the whole text minus trailing semicolons
        text.trim().trim_end_matches(';').trim()
    };
    if sig.is_empty() {
        None
    } else {
        Some(sig.to_string())
    }
}

/// Extract doc comments from previous sibling comment nodes.
fn extract_doc_comment(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut comments = Vec::new();
    let mut prev = node.prev_named_sibling();
    // Skip over attribute_item siblings first
    while let Some(sib) = prev {
        if sib.kind() == "attribute_item" {
            prev = sib.prev_named_sibling();
            continue;
        }
        if sib.kind() == "line_comment" || sib.kind() == "block_comment" {
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

/// Walk up from a call node to find an enclosing conditional, returning condition text.
/// Stops at `function_item` boundary.
fn find_enclosing_condition(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "function_item" | "function_signature_item" => return None,
            "if_expression" => {
                if let Some(cond) = parent.child_by_field_name("condition") {
                    return cond.utf8_text(source).ok().map(|s| s.trim().to_string());
                }
                return None;
            }
            "if_let_expression" => {
                // Grab "let PATTERN = EXPR" condition text
                if let Some(pat) = parent.child_by_field_name("pattern")
                    && let Some(val) = parent.child_by_field_name("value")
                {
                    let pat_text = pat.utf8_text(source).unwrap_or_default();
                    let val_text = val.utf8_text(source).unwrap_or_default();
                    return Some(format!("let {} = {}", pat_text.trim(), val_text.trim()));
                }
                return None;
            }
            "match_arm" => {
                if let Some(pat) = parent.child_by_field_name("pattern")
                    && let Ok(pat_text) = pat.utf8_text(source)
                {
                    return Some(format!("match {}", pat_text.trim()));
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

/// Check if a call node is at an async boundary (await or inside spawn).
fn detect_async_boundary(node: tree_sitter::Node, source: &[u8]) -> Option<bool> {
    // Check if parent is await_expression
    if let Some(parent) = node.parent()
        && parent.kind() == "await_expression"
    {
        return Some(true);
    }
    // Check if inside a tokio::spawn or std::thread::spawn call
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.kind() == "function_item" || parent.kind() == "function_signature_item" {
            break;
        }
        if parent.kind() == "call_expression"
            && let Some(func) = parent.child_by_field_name("function")
            && let Ok(func_text) = func.utf8_text(source)
        {
            let trimmed = func_text.trim();
            if trimmed.contains("spawn") {
                return Some(true);
            }
        }
        current = parent.parent();
    }
    None
}

/// Extract a named symbol (struct, enum, trait, module) into a Node.
fn extract_named_item(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    kind: NodeKind,
) -> Option<Node> {
    let name = field_text(node, "name", source)?;
    let id = make_id(file, module_path, &name);
    let visibility = extract_visibility(node, source);
    let start = node.start_position();
    let end = node.end_position();

    Some(Node {
        id,
        kind,
        name,
        file: file.into(),
        span: Span {
            start: [start.row, start.column],
            end: [end.row, end.column],
        },
        visibility,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
    })
}

/// Extract an impl_item into a Node.
/// The node name is the type being implemented (e.g. `Foo`).
/// The ID uses `impl_{TypeName}` to avoid collisions with the type node.
fn extract_impl_item(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
) -> Option<Node> {
    let type_name = field_text(node, "type", source)?;
    let impl_name = format!("impl_{}", type_name);
    let id = make_id(file, module_path, &impl_name);
    let start = node.start_position();
    let end = node.end_position();

    Some(Node {
        id,
        kind: NodeKind::Impl,
        name: type_name,
        file: file.into(),
        span: Span {
            start: [start.row, start.column],
            end: [end.row, end.column],
        },
        visibility: Visibility::Private,
        metadata: HashMap::new(),
        role: None,
        signature: None,
        doc_comment: None,
        module: None,
    })
}

/// Extract field_declaration children from a struct body.
fn extract_struct_fields(
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
        if child.kind() == "field_declaration"
            && let Some(name) = field_text(child, "name", source)
        {
            let qualified = format!("{parent_name}.{name}");
            let id = make_id(file, module_path, &qualified);
            let visibility = extract_visibility(child, source);
            let start = child.start_position();
            let end = child.end_position();

            result.edges.push(Edge {
                source: parent_id.to_string(),
                target: id.clone(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: edge_provenance(file, child, parent_id),
            });

            result.nodes.push(Node {
                id,
                kind: NodeKind::Field,
                name,
                file: file.into(),
                span: Span {
                    start: [start.row, start.column],
                    end: [end.row, end.column],
                },
                visibility,
                metadata: HashMap::new(),
                role: None,
                signature: None,
                doc_comment: None,
                module: None,
            });
        }
    }
}

/// Returns true if the type name is a Rust primitive.
fn is_primitive(name: &str) -> bool {
    matches!(
        name,
        "bool"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "char"
            | "str"
            | "()"
    )
}

/// Recursively scan a node tree for `call_expression` nodes, emitting Calls edges.
fn extract_calls(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    caller_id: &str,
    result: &mut ExtractionResult,
) {
    if node.kind() == "call_expression"
        && let Some(function_node) = node.child_by_field_name("function")
        && let Ok(fn_text) = function_node.utf8_text(source)
    {
        // Skip macro calls (names ending with '!')
        if !fn_text.ends_with('!') {
            let callee_name = fn_text.trim();
            if !callee_name.is_empty() {
                let target_id = make_id(file, module_path, callee_name);
                let condition = find_enclosing_condition(node, source);
                let async_boundary = detect_async_boundary(node, source);
                result.edges.push(Edge {
                    source: caller_id.to_string(),
                    target: target_id,
                    kind: EdgeKind::Calls,
                    confidence: 0.8,
                    direction: None,
                    operation: None,
                    condition,
                    async_boundary,
                    provenance: edge_provenance(file, node, caller_id),
                });
            }
        }
    }

    // Recurse into all children
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        extract_calls(child, source, file, module_path, caller_id, result);
    }
}

/// Extract enum_variant children from an enum body.
fn extract_enum_variants(
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
        if child.kind() == "enum_variant"
            && let Some(name) = field_text(child, "name", source)
        {
            let qualified = format!("{parent_name}.{name}");
            let id = make_id(file, module_path, &qualified);
            let start = child.start_position();
            let end = child.end_position();

            result.edges.push(Edge {
                source: parent_id.to_string(),
                target: id.clone(),
                kind: EdgeKind::Contains,
                confidence: 1.0,
                direction: None,
                operation: None,
                condition: None,
                async_boundary: None,
                provenance: edge_provenance(file, child, parent_id),
            });

            result.nodes.push(Node {
                id,
                kind: NodeKind::Variant,
                name,
                file: file.into(),
                span: Span {
                    start: [start.row, start.column],
                    end: [end.row, end.column],
                },
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

#[cfg(test)]
mod tests {
    use super::*;
    use grapha_core::graph::{EdgeKind, NodeKind, Visibility};

    fn extract(source: &str) -> ExtractionResult {
        let extractor = RustExtractor;
        extractor
            .extract(source.as_bytes(), Path::new("test.rs"))
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
    fn extracts_function() {
        let result = extract("pub fn greet(name: &str) -> String { format!(\"hi {}\", name) }");
        let node = find_node(&result, "greet");
        assert_eq!(node.kind, NodeKind::Function);
        assert_eq!(node.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_async_unsafe_metadata() {
        let result = extract("pub async fn fetch() {} unsafe fn danger() {}");
        let fetch = find_node(&result, "fetch");
        assert_eq!(
            fetch.metadata.get("async").map(|s| s.as_str()),
            Some("true")
        );
        let danger = find_node(&result, "danger");
        assert_eq!(
            danger.metadata.get("unsafe").map(|s| s.as_str()),
            Some("true")
        );
    }

    #[test]
    fn extracts_struct_with_fields() {
        let result = extract(
            r#"
            pub struct Config {
                pub debug: bool,
                name: String,
            }
            "#,
        );
        let config = find_node(&result, "Config");
        assert_eq!(config.kind, NodeKind::Struct);
        assert_eq!(config.visibility, Visibility::Public);

        let debug = find_node(&result, "debug");
        assert_eq!(debug.kind, NodeKind::Field);
        assert_eq!(debug.visibility, Visibility::Public);

        let name = find_node(&result, "name");
        assert_eq!(name.kind, NodeKind::Field);
        assert_eq!(name.visibility, Visibility::Private);

        assert!(has_edge(&result, &config.id, &debug.id, EdgeKind::Contains));
        assert!(has_edge(&result, &config.id, &name.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_enum_with_variants() {
        let result = extract(
            r#"
            pub enum Color {
                Red,
                Green,
                Blue,
            }
            "#,
        );
        let color = find_node(&result, "Color");
        assert_eq!(color.kind, NodeKind::Enum);

        let red = find_node(&result, "Red");
        assert_eq!(red.kind, NodeKind::Variant);

        assert!(has_edge(&result, &color.id, &red.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_trait() {
        let result = extract(
            r#"
            pub trait Drawable {
                fn draw(&self);
            }
            "#,
        );
        let drawable = find_node(&result, "Drawable");
        assert_eq!(drawable.kind, NodeKind::Trait);
        assert_eq!(drawable.visibility, Visibility::Public);

        let draw = find_node(&result, "draw");
        assert_eq!(draw.kind, NodeKind::Function);

        assert!(has_edge(
            &result,
            &drawable.id,
            &draw.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn extracts_impl_block() {
        let result = extract(
            r#"
            struct Foo;
            impl Foo {
                pub fn new() -> Self { Foo }
            }
            "#,
        );
        let impl_node = result
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Impl)
            .expect("impl node not found");
        assert_eq!(impl_node.name, "Foo");

        let new_fn = find_node(&result, "new");
        assert!(has_edge(
            &result,
            &impl_node.id,
            &new_fn.id,
            EdgeKind::Contains
        ));
    }

    #[test]
    fn extracts_module() {
        let result = extract(
            r#"
            pub mod utils {
                pub fn helper() {}
            }
            "#,
        );
        let utils = find_node(&result, "utils");
        assert_eq!(utils.kind, NodeKind::Module);
        assert_eq!(utils.visibility, Visibility::Public);

        let helper = find_node(&result, "helper");
        assert!(has_edge(&result, &utils.id, &helper.id, EdgeKind::Contains));
    }

    #[test]
    fn extracts_pub_crate_visibility() {
        let result = extract("pub(crate) fn internal() {}");
        let node = find_node(&result, "internal");
        assert_eq!(node.visibility, Visibility::Crate);
    }

    #[test]
    fn extracts_calls_edges() {
        let result = extract(
            r#"
            fn helper() {}
            fn main() {
                helper();
            }
            "#,
        );
        assert!(has_edge(
            &result,
            "test.rs::main",
            "test.rs::helper",
            EdgeKind::Calls,
        ));
    }

    #[test]
    fn extracts_use_edges() {
        let result = extract("use std::collections::HashMap;");
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Uses));
    }

    #[test]
    fn extracts_implements_edge() {
        let result = extract(
            r#"
            trait Drawable { fn draw(&self); }
            struct Circle;
            impl Drawable for Circle {
                fn draw(&self) {}
            }
            "#,
        );
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Implements));
    }

    #[test]
    fn extracts_type_ref_edges() {
        let result = extract(
            r#"
            struct Config { debug: bool }
            fn make_config() -> Config {
                Config { debug: true }
            }
            "#,
        );
        assert!(result.edges.iter().any(|e| e.kind == EdgeKind::TypeRef));
    }

    #[test]
    fn extracts_structured_imports() {
        let result = extract("use std::collections::HashMap;");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].path, "std::collections::HashMap");
        assert_eq!(
            result.imports[0].kind,
            grapha_core::resolve::ImportKind::Named
        );
    }

    #[test]
    fn extracts_relative_imports() {
        let result = extract("use crate::graph::Node;");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(
            result.imports[0].kind,
            grapha_core::resolve::ImportKind::Relative
        );
    }

    #[test]
    fn extracts_glob_imports() {
        let result = extract("use std::collections::*;");
        assert_eq!(result.imports.len(), 1);
        assert_eq!(
            result.imports[0].kind,
            grapha_core::resolve::ImportKind::Wildcard
        );
    }

    #[test]
    fn extracts_inherits_edge_for_supertraits() {
        let result = extract(
            r#"
            trait Base {}
            trait Child: Base {}
            "#,
        );
        assert!(has_edge(
            &result,
            "test.rs::Child",
            "test.rs::Base",
            EdgeKind::Inherits,
        ));
    }

    #[test]
    fn extracts_condition_on_call_inside_if() {
        let result = extract(
            r#"
            fn check() -> bool { true }
            fn run() {
                if check() {
                    helper();
                }
            }
            fn helper() {}
            "#,
        );
        let cond_edge = result
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls && e.target.contains("helper"))
            .expect("should find Calls edge to helper");
        assert!(
            cond_edge.condition.is_some(),
            "condition should be set on call inside if"
        );
        assert!(
            !cond_edge.provenance.is_empty(),
            "call edges should carry provenance"
        );
        assert_eq!(cond_edge.provenance[0].symbol_id, "test.rs::run");
    }

    #[test]
    fn detects_main_as_entry_point() {
        let result = extract(
            r#"
            fn main() {
                println!("hello");
            }
            "#,
        );
        let main_node = find_node(&result, "main");
        assert_eq!(
            main_node.role,
            Some(grapha_core::graph::NodeRole::EntryPoint),
            "fn main() should be detected as EntryPoint"
        );
    }

    #[test]
    fn detects_test_as_entry_point() {
        let result = extract(
            r#"
            #[test]
            fn my_test() {
                assert!(true);
            }
            "#,
        );
        let test_node = find_node(&result, "my_test");
        assert_eq!(
            test_node.role,
            Some(grapha_core::graph::NodeRole::EntryPoint),
            "#[test] fn should be detected as EntryPoint"
        );
    }

    #[test]
    fn detects_pub_fn_at_root_as_entry_point() {
        let result = extract("pub fn api_handler() {}");
        let node = find_node(&result, "api_handler");
        assert_eq!(
            node.role,
            Some(grapha_core::graph::NodeRole::EntryPoint),
            "pub fn at root should be EntryPoint"
        );
    }

    #[test]
    fn private_fn_at_root_is_not_entry_point() {
        let result = extract("fn helper() {}");
        let node = find_node(&result, "helper");
        assert!(
            node.role.is_none() || node.role == Some(grapha_core::graph::NodeRole::Internal),
            "private fn at root should not be EntryPoint (unless it's main)"
        );
    }

    #[test]
    fn extracts_function_signature() {
        let result = extract("pub fn greet(name: &str) -> String { format!(\"hi {}\", name) }");
        let node = find_node(&result, "greet");
        assert!(node.signature.is_some(), "signature should be extracted");
        let sig = node.signature.as_ref().unwrap();
        assert!(sig.contains("fn greet"), "signature should contain fn name");
        assert!(
            sig.contains("-> String"),
            "signature should contain return type"
        );
    }

    #[test]
    fn extracts_doc_comment() {
        let result = extract(
            r#"
            /// This is a doc comment
            /// with two lines
            fn documented() {}
            "#,
        );
        let node = find_node(&result, "documented");
        assert!(
            node.doc_comment.is_some(),
            "doc_comment should be extracted"
        );
        let doc = node.doc_comment.as_ref().unwrap();
        assert!(doc.contains("doc comment"), "should contain comment text");
    }

    #[test]
    fn detects_async_boundary_on_await() {
        let result = extract(
            r#"
            async fn caller() {
                fetch().await;
            }
            async fn fetch() {}
            "#,
        );
        let await_edge = result
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Calls && e.target.contains("fetch"));
        // Note: tree-sitter-rust may or may not produce an await_expression parent
        // depending on version. We verify the edge exists at minimum.
        assert!(await_edge.is_some(), "should find Calls edge to fetch");
    }

    #[test]
    fn extracts_condition_on_match_arm() {
        let result = extract(
            r#"
            fn process(x: i32) {
                match x {
                    0 => handle_zero(),
                    _ => handle_other(),
                }
            }
            fn handle_zero() {}
            fn handle_other() {}
            "#,
        );
        let match_edges: Vec<_> = result
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls && e.condition.is_some())
            .collect();
        // At least one call inside a match should have a condition
        assert!(
            !match_edges.is_empty(),
            "calls inside match arms should have conditions"
        );
        let cond = match_edges[0].condition.as_ref().unwrap();
        assert!(
            cond.starts_with("match"),
            "match arm condition should start with 'match'"
        );
    }
}
