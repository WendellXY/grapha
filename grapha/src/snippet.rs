use grapha_core::graph::{NodeKind, Span};

pub fn should_extract_snippet(kind: NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::Field
            | NodeKind::Variant
            | NodeKind::Property
            | NodeKind::Constant
            | NodeKind::View
            | NodeKind::Branch
    )
}

pub fn extract_snippet(source: &str, span: &Span, max_len: usize) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let start_line = span.start[0];
    let end_line = span.end[0];

    if start_line >= lines.len() {
        return None;
    }

    let end_line = end_line.min(lines.len().saturating_sub(1));
    let span_lines = &lines[start_line..=end_line];
    let full = span_lines.join("\n");

    if full.len() <= max_len {
        return Some(full);
    }

    // Truncate at a clean line boundary
    let mut truncated = String::new();
    for line in span_lines {
        if truncated.len() + line.len() + 1 > max_len {
            break;
        }
        if !truncated.is_empty() {
            truncated.push('\n');
        }
        truncated.push_str(line);
    }

    if truncated.is_empty() {
        Some(full[..max_len].to_string())
    } else {
        Some(truncated)
    }
}
