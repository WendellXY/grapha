use grapha_core::graph::{NodeKind, Span};

pub fn should_extract_snippet(kind: NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::Field | NodeKind::Variant | NodeKind::View | NodeKind::Branch
    )
}

/// Pre-computed line index for a source file. Build once, query many times.
pub struct LineIndex<'a> {
    source: &'a str,
    /// Byte offsets of the start of each line.
    line_starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0usize];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' && i + 1 < source.len() {
                line_starts.push(i + 1);
            }
        }
        Self {
            source,
            line_starts,
        }
    }

    fn normalize_line(&self, line: usize, one_based: bool) -> Option<usize> {
        if one_based {
            line.checked_sub(1)
        } else {
            Some(line)
        }
    }

    fn byte_offset(&self, line: usize, column: usize) -> Option<usize> {
        let line_start = *self.line_starts.get(line)?;
        let line_limit = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.source.len());
        Some((line_start + column).min(line_limit))
    }

    fn extract_exact_span_with_base(&self, span: &Span, one_based_lines: bool) -> Option<String> {
        let start_line = self.normalize_line(span.start[0], one_based_lines)?;
        let end_line = self.normalize_line(span.end[0], one_based_lines)?;
        let byte_start = self.byte_offset(start_line, span.start[1])?;
        let byte_end = self.byte_offset(end_line, span.end[1])?;
        if byte_start > byte_end || byte_end > self.source.len() {
            return None;
        }

        let slice = &self.source.as_bytes()[byte_start..byte_end];
        let snippet = String::from_utf8_lossy(slice)
            .trim_end_matches(['\n', '\r'])
            .to_string();
        if snippet.is_empty() { None } else { Some(snippet) }
    }

    fn extract_full_lines_with_base(&self, span: &Span, one_based_lines: bool) -> Option<String> {
        let start_line = self.normalize_line(span.start[0], one_based_lines)?;
        let end_line = self.normalize_line(span.end[0], one_based_lines)?;

        if start_line >= self.line_starts.len() {
            return None;
        }

        let end_line = end_line.min(self.line_starts.len().saturating_sub(1));
        let byte_start = self.line_starts[start_line];
        let byte_end = if end_line + 1 < self.line_starts.len() {
            self.line_starts[end_line + 1].saturating_sub(1)
        } else {
            self.source.len()
        };

        let slice = &self.source.as_bytes()[byte_start..byte_end];
        Some(
            String::from_utf8_lossy(slice)
                .trim_end_matches(['\n', '\r'])
                .to_string(),
        )
    }

    pub fn extract_full_snippet(&self, span: &Span) -> Option<String> {
        self.extract_exact_span_with_base(span, false)
            .or_else(|| self.extract_exact_span_with_base(span, true))
            .or_else(|| self.extract_full_lines_with_base(span, false))
            .or_else(|| self.extract_full_lines_with_base(span, true))
    }

    #[allow(dead_code)]
    pub fn extract_snippet(&self, span: &Span, max_len: usize) -> Option<String> {
        let full = self.extract_full_snippet(span)?;

        if full.len() <= max_len {
            return Some(full);
        }

        let mut truncate_at = max_len;
        while !full.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }

        let truncated = &full[..truncate_at];
        match truncated.rfind('\n') {
            Some(pos) if pos > 0 => Some(truncated[..pos].to_string()),
            _ => Some(truncated.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LineIndex;
    use grapha_core::graph::Span;

    #[test]
    fn extract_snippet_truncates_single_line_at_utf8_boundary() {
        let source = "abc中def";
        let index = LineIndex::new(source);
        let span = Span {
            start: [0, 0],
            end: [0, 0],
        };

        assert_eq!(index.extract_snippet(&span, 4), Some("abc".to_string()));
    }

    #[test]
    fn extract_snippet_truncates_multiline_at_newline_before_utf8_cutoff() {
        let source = "alpha\n中文beta";
        let index = LineIndex::new(source);
        let span = Span {
            start: [0, 0],
            end: [1, 0],
        };

        assert_eq!(index.extract_snippet(&span, 8), Some("alpha".to_string()));
    }

    #[test]
    fn extract_full_snippet_uses_exact_columns() {
        let source = "let before = 1; fn hello() {\n    println!(\"hi\");\n} let after = 2;";
        let index = LineIndex::new(source);
        let span = Span {
            start: [0, 16],
            end: [2, 1],
        };

        assert_eq!(
            index.extract_full_snippet(&span),
            Some("fn hello() {\n    println!(\"hi\");\n}".to_string())
        );
    }

    #[test]
    fn extract_full_snippet_accepts_one_based_lines_as_fallback() {
        let source = "struct A {}\nfn greet() {\n    println!(\"hi\");\n}\n";
        let index = LineIndex::new(source);
        let span = Span {
            start: [2, 0],
            end: [4, 1],
        };

        assert_eq!(
            index.extract_full_snippet(&span),
            Some("fn greet() {\n    println!(\"hi\");\n}".to_string())
        );
    }
}
