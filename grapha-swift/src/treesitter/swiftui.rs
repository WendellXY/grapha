use std::collections::HashMap;
use std::path::Path;

use tree_sitter::Tree;

use grapha_core::ExtractionResult;
use grapha_core::graph::{Edge, EdgeKind, Node, NodeKind, Visibility};

use super::common::*;
#[cfg(test)]
use super::extract::parse_swift;

struct SwiftUiDynamicPropertyMetadata {
    wrapper: String,
}

fn extract_swiftui_dynamic_property_metadata(
    node: tree_sitter::Node,
    source: &[u8],
) -> Option<SwiftUiDynamicPropertyMetadata> {
    let wrapper = collect_swift_attribute_names(node, source)
        .into_iter()
        .find_map(|name| {
            let normalized = match name.as_str() {
                "State" => "state",
                "StateObject" => "state_object",
                "ObservedObject" => "observed_object",
                "EnvironmentObject" => "environment_object",
                "Environment" => "environment",
                "AppStorage" => "app_storage",
                "SceneStorage" => "scene_storage",
                "Binding" => "binding",
                "GestureState" => "gesture_state",
                "FocusState" => "focus_state",
                "FocusedValue" => "focused_value",
                "FocusedBinding" => "focused_binding",
                "FetchRequest" => "fetch_request",
                "SectionedFetchRequest" => "sectioned_fetch_request",
                "Query" => "query",
                "Bindable" => "bindable",
                _ => return None,
            };
            Some(normalized.to_string())
        })?;

    Some(SwiftUiDynamicPropertyMetadata { wrapper })
}

fn apply_swiftui_dynamic_property_metadata(
    result: &mut ExtractionResult,
    node_id: &str,
    metadata: &SwiftUiDynamicPropertyMetadata,
) {
    let Some(node) = node_by_id_mut(result, node_id) else {
        return;
    };
    node.metadata
        .insert("swiftui.dynamic_property".to_string(), "true".to_string());
    node.metadata.insert(
        "swiftui.dynamic_property.wrapper".to_string(),
        metadata.wrapper.clone(),
    );
    node.metadata.insert(
        "swiftui.invalidation_source".to_string(),
        "true".to_string(),
    );
}

/// Collect inheritance/conformance names from a class_declaration node.
fn node_by_id_mut<'a>(result: &'a mut ExtractionResult, node_id: &str) -> Option<&'a mut Node> {
    result.nodes.iter_mut().find(|node| node.id == node_id)
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
    context: &mut EnrichmentContext<'_>,
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
    context.push_node(Node {
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
        snippet: None,
        repo: None,
    });
    context.emit_edge(Edge {
        source: parent_id.to_string(),
        target: id.clone(),
        kind: EdgeKind::Contains,
        confidence: 1.0,
        direction: None,
        operation: None,
        condition: None,
        async_boundary: None,
        provenance: node_edge_provenance(file, node, parent_id),
        repo: None,
    });
    id
}

fn same_owner_member_id(owner_id: &str, name: &str) -> Option<String> {
    let (owner_prefix, _) = owner_id.rsplit_once("::")?;
    Some(format!("{owner_prefix}::{name}"))
}

fn uppercase_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn navigation_base_is_owner(
    base: tree_sitter::Node,
    source: &[u8],
    owner_name: Option<&str>,
) -> bool {
    let Ok(text) = base.utf8_text(source) else {
        return false;
    };
    let trimmed = text.trim();
    trimmed == "self" || trimmed == "Self" || owner_name.is_some_and(|owner| trimmed == owner)
}

fn dependency_read_name(
    node: tree_sitter::Node,
    source: &[u8],
    owner_name: Option<&str>,
) -> Option<String> {
    if node.kind() != "simple_identifier" {
        return None;
    }

    if is_syntactic_argument_label(node, source) {
        return None;
    }

    let name = node.utf8_text(source).ok()?.trim().to_string();
    if name.is_empty() || name == "_" || uppercase_identifier(&name) {
        return None;
    }

    if let Some(parent) = node.parent()
        && parent.kind() == "navigation_suffix"
        && let Some(nav) = parent.parent()
        && nav.kind() == "navigation_expression"
        && let Some((base, _)) = navigation_base_and_member_name(nav, source)
        && !navigation_base_is_owner(base, source, owner_name)
    {
        return None;
    }

    Some(name)
}

fn is_syntactic_argument_label(node: tree_sitter::Node, source: &[u8]) -> bool {
    let mut cursor = node.parent();
    let mut inside_call_syntax = false;
    while let Some(parent) = cursor {
        if matches!(
            parent.kind(),
            "value_argument"
                | "value_arguments"
                | "call_suffix"
                | "call_expression"
                | "navigation_suffix"
                | "navigation_expression"
        ) {
            inside_call_syntax = true;
            break;
        }
        if matches!(
            parent.kind(),
            "function_declaration"
                | "protocol_function_declaration"
                | "init_declaration"
                | "closure_parameter"
                | "parameter"
        ) {
            break;
        }
        cursor = parent.parent();
    }

    if !inside_call_syntax {
        return false;
    }

    let remaining = &source[node.end_byte()..];
    let offset = remaining
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(remaining.len());

    remaining.get(offset).copied() == Some(b':')
}

fn emit_dependency_read(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    let owner_name = enclosing_owner_type_name(node, source);
    let Some(name) = dependency_read_name(node, source, owner_name.as_deref()) else {
        return;
    };

    // Scope read targets to the enclosing type when possible.
    // Without this, bare targets like "File.swift::viewModel" match ALL viewModel
    // properties across the codebase during merge, causing false positive reads.
    let target_id = same_owner_member_id(owner_id, &name).unwrap_or_else(|| {
        if let Some(ref owner) = owner_name {
            // Scope to the enclosing type: "File.swift::OwnerType::propertyName"
            make_id(file, std::slice::from_ref(owner), &name)
        } else {
            make_id(file, module_path, &name)
        }
    });
    if target_id == owner_id {
        return;
    }

    context.emit_edge(Edge {
        source: owner_id.to_string(),
        target: target_id,
        kind: EdgeKind::Reads,
        confidence: 0.85,
        direction: None,
        operation: owner_name,
        condition: find_enclosing_swift_condition(node, source),
        async_boundary: None,
        provenance: node_edge_provenance(file, node, owner_id),
        repo: None,
    });
}

fn extract_property_dependency_reads(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    emit_dependency_read(node, source, file, module_path, owner_id, context);

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_declaration"
                | "protocol_declaration"
                | "function_declaration"
                | "protocol_function_declaration"
                | "init_declaration"
                | "deinit_declaration"
        ) {
            continue;
        }
        extract_property_dependency_reads(child, source, file, module_path, owner_id, context);
    }
}

/// Built-in SwiftUI views whose first argument is a `LocalizedStringKey`.
pub(super) fn builtin_view_accepts_localized_title(name: &str) -> bool {
    matches!(
        name,
        "Button"
            | "DisclosureGroup"
            | "Label"
            | "Link"
            | "Menu"
            | "NavigationLink"
            | "Picker"
            | "Section"
            | "Toggle"
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
    context: &mut EnrichmentContext<'_>,
) -> Option<String> {
    let name = view_reference_name(node, source)?;
    let view_id = emit_swiftui_node(
        context,
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
        context.emit_edge(Edge {
            source: view_id.clone(),
            target: target_id,
            kind: EdgeKind::TypeRef,
            confidence: 0.85,
            direction: None,
            operation: owner_hint,
            condition: None,
            async_boundary: None,
            provenance: node_edge_provenance(file, node, &view_id),
            repo: None,
        });
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
    context: &mut EnrichmentContext<'_>,
) -> Option<String> {
    let name = swiftui_call_name(node, source)?;
    if uppercase_identifier(&name) {
        return None;
    }

    let view_id = emit_swiftui_node(
        context,
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
    context.emit_edge(Edge {
        source: view_id.clone(),
        target: target_id,
        kind: EdgeKind::TypeRef,
        confidence: 0.85,
        direction: None,
        operation: owner_hint,
        condition: None,
        async_boundary: None,
        provenance: node_edge_provenance(file, node, &view_id),
        repo: None,
    });
    for lambda in structural_call_suffix_lambda_children(node, source) {
        let _ = extract_swiftui_structure(
            lambda,
            source,
            file,
            module_path,
            owner_id,
            &view_id,
            context,
        );
    }
    Some(view_id)
}

#[derive(Clone)]
struct SwiftUiLambdaChild<'a> {
    node: tree_sitter::Node<'a>,
    label: Option<String>,
}

fn trailing_closure_label_before(source: &[u8], lambda_start_byte: usize) -> Option<String> {
    let window_start = lambda_start_byte.saturating_sub(128);
    let text = std::str::from_utf8(&source[window_start..lambda_start_byte])
        .ok()?
        .trim_end();
    let colon_index = text.rfind(':')?;
    if !text[colon_index + 1..].trim().is_empty() {
        return None;
    }

    let before_colon = text[..colon_index].trim_end();
    let label_start = before_colon
        .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .map(|index| index + 1)
        .unwrap_or(0);
    let label = before_colon[label_start..].trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

fn is_view_builder_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "label"
            | "header"
            | "footer"
            | "background"
            | "overlay"
            | "placeholder"
            | "leading"
            | "trailing"
            | "detail"
            | "sidebar"
            | "top"
            | "bottom"
    ) || lower.contains("content")
}

fn call_suffix_lambda_children<'a>(
    node: tree_sitter::Node<'a>,
    source: &'a [u8],
) -> Vec<SwiftUiLambdaChild<'a>> {
    let mut suffixes = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "call_suffix" {
            let mut suffix_cursor = child.walk();
            for suffix_child in child.named_children(&mut suffix_cursor) {
                if suffix_child.kind() == "lambda_literal" {
                    let label = trailing_closure_label_before(source, suffix_child.start_byte());
                    suffixes.push(SwiftUiLambdaChild {
                        node: suffix_child,
                        label,
                    });
                }
            }
        }
    }
    suffixes
}

fn structural_call_suffix_lambda_children<'a>(
    node: tree_sitter::Node<'a>,
    source: &'a [u8],
) -> Vec<tree_sitter::Node<'a>> {
    let lambdas = call_suffix_lambda_children(node, source);
    let has_labeled = lambdas.iter().any(|child| child.label.is_some());

    lambdas
        .into_iter()
        .filter(|child| match child.label.as_deref() {
            Some(label) => is_view_builder_label(label),
            None => !has_labeled,
        })
        .map(|child| child.node)
        .collect()
}

fn recurse_swiftui_named_children(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
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
            context,
        ));
    }
    anchors
}

fn extract_swiftui_dependency_reads(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    match node.kind() {
        "call_expression" => {
            let structural_lambdas = structural_call_suffix_lambda_children(node, source);

            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "call_suffix" {
                    let mut suffix_cursor = child.walk();
                    for suffix_child in child.named_children(&mut suffix_cursor) {
                        if suffix_child.kind() != "lambda_literal" {
                            extract_swiftui_dependency_reads(
                                suffix_child,
                                source,
                                file,
                                module_path,
                                owner_id,
                                context,
                            );
                        }
                    }
                    continue;
                }

                extract_swiftui_dependency_reads(
                    child,
                    source,
                    file,
                    module_path,
                    owner_id,
                    context,
                );
            }

            for lambda in structural_lambdas {
                extract_swiftui_dependency_reads(
                    lambda,
                    source,
                    file,
                    module_path,
                    owner_id,
                    context,
                );
            }
        }
        _ => {
            emit_dependency_read(node, source, file, module_path, owner_id, context);

            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                extract_swiftui_dependency_reads(
                    child,
                    source,
                    file,
                    module_path,
                    owner_id,
                    context,
                );
            }
        }
    }
}

fn extract_swiftui_if_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    owner_id: &str,
    parent_id: &str,
    context: &mut EnrichmentContext<'_>,
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
            context,
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
            context,
        );
    }

    if let Some(else_child) = named_children.get(1).copied() {
        let branch_id = emit_swiftui_node(
            context,
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
            context,
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
    context: &mut EnrichmentContext<'_>,
) -> Vec<String> {
    let label = node
        .child_by_field_name("expr")
        .and_then(|expr| expr.utf8_text(source).ok())
        .map(|text| format!("switch {}", text.trim()))
        .unwrap_or_else(|| "switch".to_string());
    let switch_id = emit_swiftui_node(
        context,
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
            context,
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
                    context,
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
    context: &mut EnrichmentContext<'_>,
) -> Vec<String> {
    match node.kind() {
        "statements" | "lambda_literal" | "computed_property" => recurse_swiftui_named_children(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        ),
        "call_expression" => {
            if let Some(name) = swiftui_call_name(node, source)
                && uppercase_identifier(&name)
            {
                let view_id = emit_swiftui_node(
                    context,
                    owner_id,
                    parent_id,
                    &name,
                    NodeKind::View,
                    file,
                    node,
                );
                if !is_builtin_swiftui_view(&name) {
                    context.emit_edge(Edge {
                        source: view_id.clone(),
                        target: make_id(file, module_path, &name),
                        kind: EdgeKind::TypeRef,
                        confidence: 0.85,
                        direction: None,
                        operation: None,
                        condition: None,
                        async_boundary: None,
                        provenance: node_edge_provenance(file, node, &view_id),
                        repo: None,
                    });
                }
                for lambda in structural_call_suffix_lambda_children(node, source) {
                    let _ = extract_swiftui_structure(
                        lambda,
                        source,
                        file,
                        module_path,
                        owner_id,
                        &view_id,
                        context,
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
                    context,
                );
                if is_view_builder_modifier(&modifier_name) {
                    let modifier_parents: Vec<String> = if anchors.is_empty() {
                        vec![parent_id.to_string()]
                    } else {
                        anchors.clone()
                    };
                    for lambda in structural_call_suffix_lambda_children(node, source) {
                        for anchor in &modifier_parents {
                            let _ = extract_swiftui_structure(
                                lambda,
                                source,
                                file,
                                module_path,
                                owner_id,
                                anchor,
                                context,
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
                context,
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
                    context,
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
            context,
        ),
        "switch_statement" => extract_swiftui_switch_structure(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
        ),
        "simple_identifier" | "navigation_expression" => emit_swiftui_view_reference(
            node,
            source,
            file,
            module_path,
            owner_id,
            parent_id,
            context,
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
            context,
        ),
    }
}

fn extract_swiftui_declaration_structure_with_context(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    decl_id: &str,
    context: &mut EnrichmentContext<'_>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "computed_property" | "function_body") {
            extract_swiftui_dependency_reads(child, source, file, module_path, decl_id, context);
            let _ = extract_swiftui_structure(
                child,
                source,
                file,
                module_path,
                decl_id,
                decl_id,
                context,
            );
        }
    }
}

pub(super) fn extract_swiftui_declaration_structure(
    node: tree_sitter::Node,
    source: &[u8],
    file: &str,
    module_path: &[String],
    decl_id: &str,
    result: &mut ExtractionResult,
) {
    let mut context = EnrichmentContext::new(result);
    extract_swiftui_declaration_structure_with_context(
        node,
        source,
        file,
        module_path,
        decl_id,
        &mut context,
    );
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

fn collect_property_declaration_nodes<'a>(
    node: tree_sitter::Node<'a>,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if node.kind() == "property_declaration" {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_property_declaration_nodes(child, out);
    }
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

#[cfg(test)]
pub fn enrich_swiftui_structure(
    source: &[u8],
    file_path: &Path,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let tree = parse_swift(source)?;
    enrich_swiftui_structure_with_tree(source, file_path, &tree, result)
}

pub fn enrich_swiftui_structure_with_tree(
    source: &[u8],
    file_path: &Path,
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let mut context = EnrichmentContext::new(result);
    let mut declaration_nodes = Vec::new();
    collect_swiftui_declaration_nodes(tree.root_node(), source, &mut declaration_nodes);

    let file_str = file_path.to_string_lossy().to_string();

    let mut property_nodes = Vec::new();
    collect_property_declaration_nodes(tree.root_node(), &mut property_nodes);
    for property_node in property_nodes {
        let Some(property_id) =
            matching_swiftui_declaration_id(&context, file_path, property_node, source)
        else {
            continue;
        };
        if let Some(metadata) = extract_swiftui_dynamic_property_metadata(property_node, source) {
            apply_swiftui_dynamic_property_metadata(context.result, &property_id, &metadata);
        }
        if declaration_returns_swiftui_view(property_node, source) {
            continue;
        }
        extract_property_dependency_reads(
            property_node,
            source,
            &file_str,
            &[],
            &property_id,
            &mut context,
        );
    }

    for decl_node in declaration_nodes {
        let Some(decl_id) = matching_swiftui_declaration_id(&context, file_path, decl_node, source)
        else {
            continue;
        };
        extract_swiftui_declaration_structure_with_context(
            decl_node,
            source,
            &file_str,
            &[],
            &decl_id,
            &mut context,
        );
    }

    let mut body_nodes = Vec::new();
    collect_swiftui_body_nodes(tree.root_node(), source, &mut body_nodes);

    for body_node in body_nodes {
        let Some(body_id) = matching_body_id(&context, file_path, body_node) else {
            continue;
        };
        extract_swiftui_declaration_structure_with_context(
            body_node,
            source,
            &file_str,
            &[],
            &body_id,
            &mut context,
        );
    }

    Ok(())
}
