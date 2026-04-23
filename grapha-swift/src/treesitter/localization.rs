use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use tree_sitter::Tree;

use grapha_core::ExtractionResult;
use grapha_core::graph::{Edge, EdgeKind, EdgeProvenance, Node, NodeKind, Span, Visibility};

use super::common::*;
#[cfg(test)]
use super::extract::parse_swift;
use super::swiftui::builtin_view_accepts_localized_title;

#[derive(Debug, Clone)]
struct LocalizationWrapperMetadata {
    table: String,
    key: String,
    fallback: Option<String>,
    arg_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct LocalizationReferenceData {
    pub(super) ref_kind: &'static str,
    pub(super) wrapper_name: Option<String>,
    pub(super) wrapper_base: Option<String>,
    pub(super) arg_count: usize,
    pub(super) literal: Option<String>,
}

type LocalizationBindings = HashMap<String, Vec<LocalizationReferenceData>>;

static L10N_TR_METADATA_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)L10n\.tr\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*"((?:[^"\\]|\\.)*)".*?fallback:\s*"((?:[^"\\]|\\.)*)""#,
    )
    .expect("L10n.tr metadata regex should compile")
});

static L10N_RESOURCE_METADATA_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)L10nResource\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*table:\s*"((?:[^"\\]|\\.)*)".*?fallback:\s*"((?:[^"\\]|\\.)*)""#,
    )
    .expect("L10nResource metadata regex should compile")
});

static LOCALIZATION_WRAPPER_EXPR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)^\s*(?:(?:([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*)|\.)([A-Za-z_][A-Za-z0-9_]*)\s*(?:\((.*)\))?\s*$"#,
    )
    .expect("localization wrapper expression regex should compile")
});

static LOCALIZED_TEXT_WRAPPER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)^\s*Text\s*\(\s*((?:i18n)\s*:\s*)?(?:(?:([A-Za-z_][A-Za-z0-9_]*)\s*\.\s*)|\.)([A-Za-z_][A-Za-z0-9_]*)\s*(?:\((.*)\))?\s*(?:\)|,)"#,
    )
    .expect("localized text wrapper regex should compile")
});

static LOCALIZED_TEXT_LITERAL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)^\s*Text\s*\(\s*"((?:[^"\\]|\\.)*)"(?:\s*,\s*bundle\s*:\s*[^,)]+)?\s*(?:\)|,)"#,
    )
    .expect("localized text literal regex should compile")
});

fn decode_swift_string_literal(raw: &str) -> String {
    raw.replace(r#"\""#, "\"")
        .replace(r"\n", "\n")
        .replace(r"\t", "\t")
}

fn count_top_level_call_args(input: &str) -> usize {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return 0;
    }

    let mut count = 1usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for ch in trimmed.chars() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => count += 1,
            _ => {}
        }
    }

    count
}

fn function_parameter_count(node: tree_sitter::Node) -> usize {
    fn count_parameters(node: tree_sitter::Node) -> usize {
        let mut count = usize::from(node.kind() == "parameter");
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            count += count_parameters(child);
        }
        count
    }

    count_parameters(node)
}

fn parse_l10n_tr_metadata(text: &str, arg_count: usize) -> Option<LocalizationWrapperMetadata> {
    let captures = L10N_TR_METADATA_RE.captures(text)?;
    Some(LocalizationWrapperMetadata {
        table: decode_swift_string_literal(captures.get(1)?.as_str()),
        key: decode_swift_string_literal(captures.get(2)?.as_str()),
        fallback: Some(decode_swift_string_literal(captures.get(3)?.as_str())),
        arg_count,
    })
}

fn parse_l10n_resource_metadata(
    text: &str,
    arg_count: usize,
) -> Option<LocalizationWrapperMetadata> {
    let captures = L10N_RESOURCE_METADATA_RE.captures(text)?;
    Some(LocalizationWrapperMetadata {
        table: decode_swift_string_literal(captures.get(2)?.as_str()),
        key: decode_swift_string_literal(captures.get(1)?.as_str()),
        fallback: Some(decode_swift_string_literal(captures.get(3)?.as_str())),
        arg_count,
    })
}

fn extract_wrapper_metadata(
    node: tree_sitter::Node,
    source: &[u8],
) -> Option<LocalizationWrapperMetadata> {
    let text = node.utf8_text(source).ok()?;
    let arg_count = if node.kind() == "function_declaration" {
        function_parameter_count(node)
    } else {
        0
    };

    parse_l10n_tr_metadata(text, arg_count)
        .or_else(|| parse_l10n_resource_metadata(text, arg_count))
}

fn collect_localizable_wrapper_nodes<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if matches!(node.kind(), "property_declaration" | "function_declaration")
        && extract_wrapper_metadata(node, source).is_some()
    {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_localizable_wrapper_nodes(child, source, out);
    }
}

fn apply_wrapper_metadata(
    context: &mut EnrichmentContext<'_>,
    node_id: &str,
    metadata: &LocalizationWrapperMetadata,
) {
    let Some(node) = context.node_mut(node_id) else {
        return;
    };
    node.metadata
        .insert("l10n.wrapper.table".to_string(), metadata.table.clone());
    node.metadata
        .insert("l10n.wrapper.key".to_string(), metadata.key.clone());
    if let Some(fallback) = &metadata.fallback {
        node.metadata
            .insert("l10n.wrapper.fallback".to_string(), fallback.clone());
    }
    node.metadata.insert(
        "l10n.wrapper.arg_count".to_string(),
        metadata.arg_count.to_string(),
    );
}

fn apply_localization_reference(
    context: &mut EnrichmentContext<'_>,
    file_path: &Path,
    file: &str,
    usage_id: &str,
    usage_span: &Span,
    reference: &LocalizationReferenceData,
    argument_label: Option<&str>,
) {
    {
        let Some(node) = context.node_mut(usage_id) else {
            return;
        };
        node.metadata
            .insert("l10n.ref_kind".to_string(), reference.ref_kind.to_string());
        node.metadata.insert(
            "l10n.arg_count".to_string(),
            reference.arg_count.to_string(),
        );
        if let Some(wrapper_name) = &reference.wrapper_name {
            node.metadata
                .insert("l10n.wrapper_name".to_string(), wrapper_name.clone());
        }
        if let Some(wrapper_base) = &reference.wrapper_base {
            node.metadata
                .insert("l10n.wrapper_base".to_string(), wrapper_base.clone());
        }
        if let Some(literal) = &reference.literal {
            node.metadata
                .insert("l10n.literal".to_string(), literal.clone());
        }
        if let Some(label) = argument_label {
            node.metadata
                .insert("l10n.argument_label".to_string(), label.to_string());
        }
    }

    if let Some(wrapper_name) = &reference.wrapper_name
        && let Some(wrapper_id) = context.local_wrapper_id(file_path, wrapper_name)
    {
        context.emit_edge(Edge {
            source: usage_id.to_string(),
            target: wrapper_id,
            kind: EdgeKind::TypeRef,
            confidence: 0.85,
            direction: None,
            operation: reference.wrapper_base.clone(),
            condition: None,
            async_boundary: None,
            provenance: vec![EdgeProvenance {
                file: file.into(),
                span: usage_span.clone(),
                symbol_id: usage_id.to_string(),
            }],
            repo: None,
        });
    }
}

fn apply_localization_reference_to_span(
    context: &mut EnrichmentContext<'_>,
    file_path: &Path,
    file: &str,
    usage_id: &str,
    usage_span: &Span,
    reference: &LocalizationReferenceData,
    argument_label: Option<&str>,
) {
    apply_localization_reference(
        context,
        file_path,
        file,
        usage_id,
        usage_span,
        reference,
        argument_label,
    );
}

fn emit_localization_usage_node(
    context: &mut EnrichmentContext<'_>,
    parent_id: &str,
    file: &str,
    label: &str,
    span: &Span,
    unique_suffix: Option<&str>,
) -> String {
    let suffix = unique_suffix
        .map(sanitize_id_component)
        .filter(|value| !value.is_empty())
        .map(|value| format!(":{value}"))
        .unwrap_or_default();
    let id = format!(
        "{parent_id}::l10n:{}{}@{}:{}:{}:{}",
        sanitize_id_component(label),
        suffix,
        span.start[0],
        span.start[1],
        span.end[0],
        span.end[1]
    );
    context.push_node(Node {
        id: id.clone(),
        kind: NodeKind::Property,
        name: label.to_string(),
        file: file.into(),
        span: span.clone(),
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
        provenance: vec![EdgeProvenance {
            file: file.into(),
            span: span.clone(),
            symbol_id: parent_id.to_string(),
        }],
        repo: None,
    });
    id
}

/// Extract the first argument of a built-in SwiftUI view as a localization reference.
/// Handles `Button("Title") { }`, `Label("Title", systemImage: "star")`, etc.
fn builtin_view_title_reference(
    text: &str,
    view_name: &str,
    bindings: Option<&LocalizationBindings>,
) -> Option<LocalizationReferenceData> {
    let trimmed = text.trim();
    let prefix = format!("{view_name}(");
    if !trimmed.starts_with(&prefix) {
        return None;
    }
    let args = call_argument_list(trimmed)?;
    let first_argument = split_top_level_arguments(args).into_iter().next()?;
    let (label, value) = split_argument_label(first_argument);
    // Skip labeled first arguments that aren't title-like (e.g. `Button(action:)`)
    if let Some(label) = &label
        && !is_text_like_argument_label(label)
        && label != "i18n"
    {
        return None;
    }
    // Try binding-based resolution first
    let binding_refs = simple_identifier_binding_references(value, bindings);
    if let Some(reference) = binding_refs.into_iter().next() {
        return Some(reference);
    }
    localized_reference_for_expression_text(value)
}

pub(super) fn localized_reference_for_expression_text(
    text: &str,
) -> Option<LocalizationReferenceData> {
    let trimmed = text.trim();
    if let Some(captures) = LOCALIZED_TEXT_LITERAL_RE.captures(&format!("Text({trimmed})")) {
        return Some(LocalizationReferenceData {
            ref_kind: "literal",
            wrapper_name: None,
            wrapper_base: None,
            arg_count: 0,
            literal: Some(decode_swift_string_literal(captures.get(1)?.as_str())),
        });
    }

    if let Some(reference) = localized_i18n_call_reference(trimmed, "Text") {
        return Some(reference);
    }

    if let Some(reference) = localized_i18n_call_reference(trimmed, "String") {
        return Some(reference);
    }

    let captures = LOCALIZATION_WRAPPER_EXPR_RE.captures(trimmed)?;
    let wrapper_base = captures.get(1).map(|value| value.as_str().to_string());
    if !looks_like_localized_wrapper_expression(trimmed, wrapper_base.as_deref()) {
        return None;
    }
    let args = captures.get(3).map(|value| value.as_str()).unwrap_or("");
    Some(LocalizationReferenceData {
        ref_kind: "wrapper",
        wrapper_name: Some(captures.get(2)?.as_str().to_string()),
        wrapper_base,
        arg_count: count_top_level_call_args(args),
        literal: None,
    })
}

fn looks_like_localized_wrapper_expression(text: &str, wrapper_base: Option<&str>) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('.')
        || trimmed.contains("i18n:")
        || trimmed.contains("i10n:")
        || matches_localization_wrapper_base(wrapper_base)
}

fn matches_localization_wrapper_base(wrapper_base: Option<&str>) -> bool {
    match wrapper_base.map(str::trim) {
        None => false,
        Some("") => false,
        Some(base) => {
            let normalized = base.rsplit('.').next().unwrap_or(base);
            matches!(
                normalized,
                "L10n" | "L10nResource" | "l10n" | "i18n" | "i10n"
            ) || normalized.ends_with("L10n")
                || normalized.ends_with("Resource")
                || normalized.ends_with("Strings")
                || normalized
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
        }
    }
}

fn expression_maybe_string_text(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed != "true"
        && trimmed != "false"
        && !trimmed.starts_with('{')
        && !trimmed.starts_with('[')
        && !trimmed.starts_with('#')
        && trimmed.parse::<f64>().is_err()
}

fn is_text_like_argument_label(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    [
        "text",
        "title",
        "subtitle",
        "message",
        "label",
        "placeholder",
        "caption",
        "hint",
        "prompt",
        "description",
        "header",
        "footer",
    ]
    .iter()
    .any(|needle| lower == *needle || lower.ends_with(needle))
}

fn localized_i18n_call_reference(text: &str, callee: &str) -> Option<LocalizationReferenceData> {
    let trimmed = text.trim();
    let prefix = format!("{callee}(");
    if !trimmed.starts_with(&prefix) {
        return None;
    }

    let args = call_argument_list(trimmed)?;
    for segment in split_top_level_arguments(args) {
        let (label, value) = split_argument_label(segment);
        if !matches!(label.as_deref(), Some("i18n" | "i10n")) {
            continue;
        }

        if let Some(mut reference) = localized_reference_for_expression_text(value) {
            if reference.wrapper_base.is_none() {
                reference.wrapper_base = Some("L10nResource".to_string());
            }
            return Some(reference);
        }

        return Some(LocalizationReferenceData {
            ref_kind: "possible_wrapper",
            wrapper_name: None,
            wrapper_base: Some("L10nResource".to_string()),
            arg_count: 0,
            literal: None,
        });
    }

    None
}

pub(super) fn localized_text_references_from_text(
    text: &str,
    bindings: Option<&LocalizationBindings>,
) -> Vec<LocalizationReferenceData> {
    let trimmed = text.trim();
    if trimmed.starts_with("Text(verbatim:") {
        return Vec::new();
    }

    let binding_references = if let Some(args) = call_argument_list(trimmed) {
        split_top_level_arguments(args)
            .into_iter()
            .next()
            .map(|first_argument| {
                let (_, value) = split_argument_label(first_argument);
                simple_identifier_binding_references(value, bindings)
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if !binding_references.is_empty() {
        return binding_references;
    }

    if let Some(reference) = localized_i18n_call_reference(trimmed, "Text") {
        return vec![reference];
    }

    if let Some(captures) = LOCALIZED_TEXT_WRAPPER_RE.captures(trimmed) {
        let Some(wrapper_name) = captures.get(3).map(|value| value.as_str().to_string()) else {
            return Vec::new();
        };
        let uses_i18n_label = captures.get(1).is_some();
        let args = captures.get(4).map(|value| value.as_str()).unwrap_or("");
        return vec![LocalizationReferenceData {
            ref_kind: "wrapper",
            wrapper_name: Some(wrapper_name),
            wrapper_base: captures
                .get(2)
                .map(|value| value.as_str().to_string())
                .or_else(|| uses_i18n_label.then_some("L10nResource".to_string())),
            arg_count: count_top_level_call_args(args),
            literal: None,
        }];
    }

    if let Some(captures) = LOCALIZED_TEXT_LITERAL_RE.captures(trimmed) {
        let Some(literal) = captures
            .get(1)
            .map(|value| decode_swift_string_literal(value.as_str()))
        else {
            return Vec::new();
        };
        return vec![LocalizationReferenceData {
            ref_kind: "literal",
            wrapper_name: None,
            wrapper_base: None,
            arg_count: 0,
            literal: Some(literal),
        }];
    }

    if trimmed.starts_with("Text(") && trimmed.ends_with(')') {
        if let Some(args) = call_argument_list(trimmed)
            && let Some(first_argument) = split_top_level_arguments(args).into_iter().next()
        {
            let (label, value) = split_argument_label(first_argument);
            if label.as_deref() == Some("i18n") && expression_maybe_string_text(value) {
                return vec![LocalizationReferenceData {
                    ref_kind: "possible_wrapper",
                    wrapper_name: None,
                    wrapper_base: Some("L10nResource".to_string()),
                    arg_count: 0,
                    literal: None,
                }];
            }
        }

        return vec![LocalizationReferenceData {
            ref_kind: "possible_string",
            wrapper_name: None,
            wrapper_base: None,
            arg_count: 0,
            literal: None,
        }];
    }

    Vec::new()
}

fn call_argument_list(text: &str) -> Option<&str> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    (close > open).then_some(&text[open + 1..close])
}

fn split_top_level_arguments(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in args.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                parts.push(args[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }

    let tail = args[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }

    parts
}

fn split_argument_label(segment: &str) -> (Option<String>, &str) {
    let trimmed = segment.trim();
    let Some(colon) = trimmed.find(':') else {
        return (None, trimmed);
    };
    let label = trimmed[..colon].trim();
    if label.is_empty()
        || !label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return (None, trimmed);
    }
    (Some(label.to_string()), trimmed[colon + 1..].trim())
}

fn simple_identifier_binding_references(
    text: &str,
    bindings: Option<&LocalizationBindings>,
) -> Vec<LocalizationReferenceData> {
    let Some(bindings) = bindings else {
        return Vec::new();
    };

    let trimmed = text.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Vec::new();
    }

    bindings.get(trimmed).cloned().unwrap_or_default()
}

fn push_unique_localization_reference(
    references: &mut Vec<LocalizationReferenceData>,
    reference: LocalizationReferenceData,
) {
    if !references.contains(&reference) {
        references.push(reference);
    }
}

fn localization_reference_suffix(reference: &LocalizationReferenceData, index: usize) -> String {
    reference
        .wrapper_name
        .clone()
        .or_else(|| reference.literal.clone())
        .or_else(|| reference.wrapper_base.clone())
        .unwrap_or_else(|| format!("ref{index}"))
}

fn collect_localization_references_in_subtree(
    node: tree_sitter::Node,
    source: &[u8],
    references: &mut Vec<LocalizationReferenceData>,
) {
    match node.kind() {
        "call_expression" | "navigation_expression" => {
            if let Ok(text) = node.utf8_text(source)
                && let Some(reference) = localized_reference_for_expression_text(text)
            {
                push_unique_localization_reference(references, reference);
                return;
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_localization_references_in_subtree(child, source, references);
    }
}

fn collect_localization_bindings(
    node: tree_sitter::Node,
    source: &[u8],
    bindings: &mut LocalizationBindings,
) {
    match node.kind() {
        "class_declaration"
        | "protocol_declaration"
        | "function_declaration"
        | "protocol_function_declaration"
        | "init_declaration"
        | "deinit_declaration"
        | "lambda_literal" => return,
        "property_declaration" => {
            if let Some(name) = find_pattern_name(node, source) {
                let mut references = Vec::new();
                collect_localization_references_in_subtree(node, source, &mut references);
                if !references.is_empty() {
                    let entry = bindings.entry(name).or_default();
                    for reference in references {
                        push_unique_localization_reference(entry, reference);
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_localization_bindings(child, source, bindings);
    }
}

fn localization_bindings_for_declaration(
    node: tree_sitter::Node,
    source: &[u8],
) -> LocalizationBindings {
    let mut bindings = LocalizationBindings::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "computed_property" | "function_body") {
            collect_localization_bindings(child, source, &mut bindings);
        }
    }
    bindings
}

fn custom_view_localization_arguments_from_text(
    text: &str,
    bindings: Option<&LocalizationBindings>,
) -> Vec<(Option<String>, LocalizationReferenceData)> {
    let Some(args) = call_argument_list(text) else {
        return Vec::new();
    };

    let mut localized_args = Vec::new();
    for segment in split_top_level_arguments(args) {
        let (label, value) = split_argument_label(segment);
        let accepts_reference = label
            .as_deref()
            .is_some_and(|label| label == "i18n" || is_text_like_argument_label(label));
        let binding_references = simple_identifier_binding_references(value, bindings);
        if accepts_reference && !binding_references.is_empty() {
            localized_args.extend(
                binding_references
                    .into_iter()
                    .map(|reference| (label.clone(), reference)),
            );
            continue;
        }

        let reference = if let Some(reference) = localized_reference_for_expression_text(value) {
            reference
        } else if label.as_deref() == Some("i18n") && expression_maybe_string_text(value) {
            LocalizationReferenceData {
                ref_kind: "possible_wrapper",
                wrapper_name: None,
                wrapper_base: Some("L10nResource".to_string()),
                arg_count: 0,
                literal: None,
            }
        } else if label.as_deref().is_some_and(is_text_like_argument_label)
            && expression_maybe_string_text(value)
        {
            LocalizationReferenceData {
                ref_kind: "possible_string",
                wrapper_name: None,
                wrapper_base: None,
                arg_count: 0,
                literal: None,
            }
        } else {
            continue;
        };
        if !accepts_reference {
            continue;
        }
        localized_args.push((label, reference));
    }

    localized_args
}

fn looks_like_constructor_style_call(text: &str) -> bool {
    let Some(open_paren) = text.find('(') else {
        return false;
    };
    let callee = text[..open_paren].trim();
    let tail = callee.rsplit('.').next().unwrap_or(callee);
    tail.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn collect_call_expression_nodes<'a>(
    node: tree_sitter::Node<'a>,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if matches!(node.kind(), "call_expression" | "navigation_expression") {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_call_expression_nodes(child, out);
    }
}

fn collect_localization_owner_nodes<'a>(
    node: tree_sitter::Node<'a>,
    out: &mut Vec<tree_sitter::Node<'a>>,
) {
    if matches!(node.kind(), "property_declaration" | "function_declaration") {
        out.push(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_localization_owner_nodes(child, out);
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub fn enrich_localization_metadata(
    source: &[u8],
    file_path: &Path,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let tree = parse_swift(source)?;
    enrich_localization_metadata_with_tree(source, file_path, &tree, result)
}

pub fn enrich_localization_metadata_with_tree(
    source: &[u8],
    file_path: &Path,
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let mut context = EnrichmentContext::new(result);
    let file_str = file_path.to_string_lossy().to_string();
    let source_spans = SourceIndex::new(source);

    let mut wrapper_nodes = Vec::new();
    collect_localizable_wrapper_nodes(tree.root_node(), source, &mut wrapper_nodes);
    for wrapper_node in wrapper_nodes {
        let Some(wrapper_metadata) = extract_wrapper_metadata(wrapper_node, source) else {
            continue;
        };
        let Some(node_id) =
            matching_swiftui_declaration_id(&context, file_path, wrapper_node, source)
        else {
            continue;
        };
        apply_wrapper_metadata(&mut context, &node_id, &wrapper_metadata);
    }

    let mut declaration_nodes = Vec::new();
    collect_localization_owner_nodes(tree.root_node(), &mut declaration_nodes);
    let mut localization_bindings_by_owner: HashMap<String, LocalizationBindings> = HashMap::new();
    for declaration_node in declaration_nodes {
        let Some(decl_id) =
            matching_swiftui_declaration_id(&context, file_path, declaration_node, source)
        else {
            continue;
        };
        let bindings = localization_bindings_for_declaration(declaration_node, source);
        if !bindings.is_empty() {
            localization_bindings_by_owner.insert(decl_id, bindings);
        }
    }

    let mut non_view_declaration_nodes = Vec::new();
    collect_localization_owner_nodes(tree.root_node(), &mut non_view_declaration_nodes);
    for declaration_node in non_view_declaration_nodes {
        if declaration_returns_swiftui_view(declaration_node, source) {
            continue;
        }

        let Some(decl_id) =
            matching_swiftui_declaration_id(&context, file_path, declaration_node, source)
        else {
            continue;
        };
        let bindings = localization_bindings_by_owner.get(&decl_id);
        let mut call_nodes = Vec::new();
        collect_call_expression_nodes(declaration_node, &mut call_nodes);
        for call_node in call_nodes {
            let Ok(text) = call_node.utf8_text(source) else {
                continue;
            };
            if !looks_like_constructor_style_call(text) {
                continue;
            }

            for (index, (label, reference)) in
                custom_view_localization_arguments_from_text(text, bindings)
                    .into_iter()
                    .enumerate()
            {
                let call_span = make_span(call_node);
                let usage_id = emit_localization_usage_node(
                    &mut context,
                    &decl_id,
                    &file_str,
                    label.as_deref().unwrap_or("text"),
                    &call_span,
                    Some(&localization_reference_suffix(&reference, index)),
                );
                apply_localization_reference_to_span(
                    &mut context,
                    file_path,
                    &file_str,
                    &usage_id,
                    &call_span,
                    &reference,
                    label.as_deref(),
                );
            }
        }
    }

    let view_nodes = context.node_snapshots.clone();
    for view_node in view_nodes {
        if view_node.kind != NodeKind::View || !file_matches(&view_node.file, file_path) {
            continue;
        }
        let Some(text) = source_spans.text_for_span(source, &view_node.span) else {
            continue;
        };
        let bindings = swiftui_owner_id(&view_node.id)
            .and_then(|owner_id| localization_bindings_by_owner.get(owner_id));

        if view_node.name == "Text" {
            let references = localized_text_references_from_text(text, bindings);
            if let Some(reference) = references.first() {
                apply_localization_reference_to_span(
                    &mut context,
                    file_path,
                    &file_str,
                    &view_node.id,
                    &view_node.span,
                    reference,
                    None,
                );
            }
            for (index, reference) in references.iter().enumerate().skip(1) {
                let usage_id = emit_localization_usage_node(
                    &mut context,
                    &view_node.id,
                    &file_str,
                    "text",
                    &view_node.span,
                    Some(&localization_reference_suffix(reference, index)),
                );
                apply_localization_reference_to_span(
                    &mut context,
                    file_path,
                    &file_str,
                    &usage_id,
                    &view_node.span,
                    reference,
                    None,
                );
            }
            continue;
        }

        if builtin_view_accepts_localized_title(&view_node.name) {
            if let Some(reference) = builtin_view_title_reference(text, &view_node.name, bindings) {
                let usage_id = emit_localization_usage_node(
                    &mut context,
                    &view_node.id,
                    &file_str,
                    "title",
                    &view_node.span,
                    Some(&localization_reference_suffix(&reference, 0)),
                );
                apply_localization_reference_to_span(
                    &mut context,
                    file_path,
                    &file_str,
                    &usage_id,
                    &view_node.span,
                    &reference,
                    Some("title"),
                );
            }
            continue;
        }

        if is_builtin_swiftui_view(&view_node.name) {
            continue;
        }

        for (index, (label, reference)) in
            custom_view_localization_arguments_from_text(text, bindings)
                .into_iter()
                .enumerate()
        {
            let usage_id = emit_localization_usage_node(
                &mut context,
                &view_node.id,
                &file_str,
                label.as_deref().unwrap_or("text"),
                &view_node.span,
                Some(&localization_reference_suffix(&reference, index)),
            );
            apply_localization_reference_to_span(
                &mut context,
                file_path,
                &file_str,
                &usage_id,
                &view_node.span,
                &reference,
                label.as_deref(),
            );
        }
    }

    Ok(())
}
