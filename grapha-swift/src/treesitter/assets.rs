use std::path::Path;

use tree_sitter::Tree;

use grapha_core::ExtractionResult;

use super::common::SourceIndex;

const META_ASSET_REF_KIND: &str = "asset.ref_kind";
const META_ASSET_NAME: &str = "asset.name";

/// Fast byte-level check — skip tree-sitter parsing when no asset markers found.
pub fn source_contains_image_asset_markers(source: &[u8]) -> bool {
    fn bytes_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
    bytes_contains(source, b"Image(") || bytes_contains(source, b"UIImage(")
}

pub fn enrich_asset_references_with_tree(
    source: &[u8],
    _file_path: &Path,
    tree: &Tree,
    result: &mut ExtractionResult,
) -> anyhow::Result<()> {
    let line_index = SourceIndex::new(source);
    let mut calls = Vec::new();
    collect_image_asset_calls(tree.root_node(), source, &mut calls);

    for call in calls {
        let call_line = line_index.line_at_byte(call.byte_offset);
        let Some(owner_idx) = find_enclosing_node_index(result, call_line) else {
            continue;
        };
        // Only set if not already tagged (first call wins for that node)
        if !result.nodes[owner_idx]
            .metadata
            .contains_key(META_ASSET_REF_KIND)
        {
            result.nodes[owner_idx]
                .metadata
                .insert(META_ASSET_REF_KIND.to_string(), "image".to_string());
            result.nodes[owner_idx]
                .metadata
                .insert(META_ASSET_NAME.to_string(), call.asset_name.clone());
        }
    }

    Ok(())
}

struct ImageAssetCall {
    asset_name: String,
    byte_offset: usize,
}

fn collect_image_asset_calls<'a>(
    node: tree_sitter::Node<'a>,
    source: &[u8],
    calls: &mut Vec<ImageAssetCall>,
) {
    if node.kind() == "call_expression"
        && let Some(asset_call) = try_parse_image_call(node, source)
    {
        calls.push(asset_call);
    }

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_image_asset_calls(child, source, calls);
        }
    }
}

fn try_parse_image_call(node: tree_sitter::Node, source: &[u8]) -> Option<ImageAssetCall> {
    // A call_expression has a callee (first child) and call_suffix child containing
    // value_arguments. The callee text should be "Image" or "UIImage".
    let callee = node.child(0)?;
    let callee_text = callee.utf8_text(source).ok()?;
    if callee_text != "Image" && callee_text != "UIImage" {
        return None;
    }

    // Find call_suffix -> value_arguments
    let call_suffix = find_child_by_kind(node, "call_suffix")?;
    let value_arguments = find_child_by_kind(call_suffix, "value_arguments")?;

    // Look at value_argument children
    let arg_count = value_arguments.child_count();
    for i in 0..arg_count {
        let Some(arg) = value_arguments.child(i) else {
            continue;
        };
        if arg.kind() != "value_argument" {
            continue;
        }

        // Check for label
        let (label, value_node) = extract_argument_label_and_value(arg, source);
        let label = label.as_deref();

        // Only process relevant arguments
        match label {
            // Image("icon") or Image(named: "icon") or UIImage(named: "icon")
            // Image(asset: .X.Y) or Image(resource: .X.Y)
            None | Some("named") | Some("asset") | Some("resource") => {}
            // Image(systemName: ...) — skip SF Symbols
            Some("systemName") | Some("systemimage") => return None,
            // Image(decorative: ...) — treat as asset reference
            Some("decorative") => {}
            _ => continue,
        }

        let Some(value_node) = value_node else {
            continue;
        };

        // Try string literal
        if let Some(name) = extract_string_literal(value_node, source) {
            return Some(ImageAssetCall {
                asset_name: name,
                byte_offset: node.start_byte(),
            });
        }

        // Try dot expression: .Room.voiceWave → "Room/voiceWave"
        if let Some(name) = extract_dot_expression_asset_name(value_node, source) {
            return Some(ImageAssetCall {
                asset_name: name,
                byte_offset: node.start_byte(),
            });
        }
    }

    None
}

fn find_child_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i)
            && child.kind() == kind
        {
            return Some(child);
        }
    }
    None
}

fn extract_argument_label_and_value<'a>(
    arg: tree_sitter::Node<'a>,
    source: &[u8],
) -> (Option<String>, Option<tree_sitter::Node<'a>>) {
    // value_argument may have: simple_identifier ":" value
    // or just: value
    let child_count = arg.child_count();
    if child_count == 0 {
        return (None, None);
    }

    // Check if first child is a simple_identifier (label)
    if let Some(first) = arg.child(0)
        && first.kind() == "simple_identifier"
    {
        let label = first.utf8_text(source).ok().map(|s| s.to_string());
        // Value is the last non-colon child
        let value = (0..child_count)
            .rev()
            .filter_map(|i| arg.child(i))
            .find(|c| c.kind() != "simple_identifier" && c.kind() != ":");
        return (label, value);
    }

    // No label — value is the first meaningful child
    let value = (0..child_count)
        .filter_map(|i| arg.child(i))
        .find(|c| c.kind() != "," && c.kind() != "(" && c.kind() != ")");
    (None, value)
}

fn extract_string_literal(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    // line_string_literal containing string_content
    if node.kind() == "line_string_literal" {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i)
                && child.kind() == "line_str_text"
            {
                return child.utf8_text(source).ok().map(|s| s.to_string());
            }
        }
    }
    None
}

fn extract_dot_expression_asset_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    // Dot expressions like .Room.voiceWave are navigation_expression nodes
    // We collect all member segments
    let text = node.utf8_text(source).ok()?;
    if !text.starts_with('.') {
        return None;
    }
    // .Room.voiceWave → "Room/voiceWave"
    let trimmed = text.trim_start_matches('.');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.replace('.', "/"))
}

/// Find the graph node whose span contains the given 0-based line number,
/// preferring the tightest (smallest) enclosing span.
fn find_enclosing_node_index(result: &ExtractionResult, line: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (index, span_size)

    for (idx, node) in result.nodes.iter().enumerate() {
        let start_line = node.span.start[0];
        let end_line = node.span.end[0];

        if line >= start_line && line <= end_line {
            let span_size = end_line - start_line;
            if best.is_none() || span_size < best.unwrap().1 {
                best = Some((idx, span_size));
            }
        }
    }

    best.map(|(idx, _)| idx)
}
