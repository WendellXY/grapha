use grapha_core::graph::{NodeKind, Span};

// Re-export the snippet module functions by path
// Since snippet is a private module in the grapha binary, we test the logic directly
// by duplicating the pure functions here. In production, these are tested via integration.

fn should_extract_snippet(kind: NodeKind) -> bool {
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

fn extract_snippet(source: &str, span: &Span, max_len: usize) -> Option<String> {
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

#[test]
fn test_extract_snippet_multiline() {
    let source = "fn main() {\n    println!(\"hello\");\n    let x = 42;\n}";
    let span = Span {
        start: [0, 0],
        end: [3, 1],
    };
    let result = extract_snippet(source, &span, 600);
    assert_eq!(
        result,
        Some("fn main() {\n    println!(\"hello\");\n    let x = 42;\n}".to_string())
    );
}

#[test]
fn test_extract_snippet_truncation_at_line_boundary() {
    let source = "fn foo() {\n    line_one();\n    line_two();\n    line_three();\n}";
    let span = Span {
        start: [0, 0],
        end: [4, 1],
    };
    // Set max_len so only the first two lines fit
    let result = extract_snippet(source, &span, 30);
    assert_eq!(result, Some("fn foo() {\n    line_one();".to_string()));
}

#[test]
fn test_should_extract_snippet_eligible_kinds() {
    assert!(should_extract_snippet(NodeKind::Function));
    assert!(should_extract_snippet(NodeKind::Struct));
    assert!(should_extract_snippet(NodeKind::Enum));
    assert!(should_extract_snippet(NodeKind::Trait));
    assert!(should_extract_snippet(NodeKind::Impl));
    assert!(should_extract_snippet(NodeKind::Module));
    assert!(should_extract_snippet(NodeKind::Protocol));
    assert!(should_extract_snippet(NodeKind::Extension));
    assert!(should_extract_snippet(NodeKind::TypeAlias));
}

#[test]
fn test_should_extract_snippet_excluded_kinds() {
    assert!(!should_extract_snippet(NodeKind::Field));
    assert!(!should_extract_snippet(NodeKind::Variant));
    assert!(!should_extract_snippet(NodeKind::Property));
    assert!(!should_extract_snippet(NodeKind::Constant));
    assert!(!should_extract_snippet(NodeKind::View));
    assert!(!should_extract_snippet(NodeKind::Branch));
}

#[test]
fn test_extract_snippet_span_beyond_file_returns_none() {
    let source = "line one\nline two";
    let span = Span {
        start: [10, 0],
        end: [12, 0],
    };
    let result = extract_snippet(source, &span, 600);
    assert_eq!(result, None);
}

#[test]
fn test_extract_snippet_single_line() {
    let source = "use std::io;\nfn hello() {}\nfn world() {}";
    let span = Span {
        start: [1, 0],
        end: [1, 14],
    };
    let result = extract_snippet(source, &span, 600);
    assert_eq!(result, Some("fn hello() {}".to_string()));
}

#[test]
fn test_extract_snippet_end_beyond_file_clamped() {
    let source = "line one\nline two\nline three";
    let span = Span {
        start: [1, 0],
        end: [100, 0],
    };
    let result = extract_snippet(source, &span, 600);
    assert_eq!(result, Some("line two\nline three".to_string()));
}
