// SPDX-License-Identifier: GPL-3.0-or-later
use nia_span::Span;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub span: Span,
    pub message: String,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            span,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => f.write_str("error"),
            Severity::Warning => f.write_str("warning"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineInfo {
    number: usize,
    start: usize,
    end: usize,
    column: usize,
}

pub fn render_diagnostic(path: &str, source: &str, diagnostic: &Diagnostic) -> String {
    let line = line_info(source, diagnostic.span.start);
    let line_text = &source[line.start..line.end];
    let line_no = line.number.to_string();
    let gutter_width = line_no.len();
    let underline_start = line.column.saturating_sub(1);
    let underline_width = underline_width(source, diagnostic.span, &line);

    let mut output = String::new();
    output.push_str(&format!(
        "{}: {}\n",
        diagnostic.severity, diagnostic.message
    ));
    output.push_str(&format!(
        "{:>width$} --> {}:{}:{}\n",
        "",
        path,
        line.number,
        line.column,
        width = gutter_width
    ));
    output.push_str(&format!("{:>width$} |\n", "", width = gutter_width));
    output.push_str(&format!(
        "{line_no:>width$} | {line_text}\n",
        width = gutter_width
    ));
    output.push_str(&format!(
        "{:>width$} | {}{}\n",
        "",
        " ".repeat(underline_start),
        "^".repeat(underline_width),
        width = gutter_width
    ));
    output
}

fn line_info(source: &str, offset: usize) -> LineInfo {
    let offset = clamp_to_char_boundary(source, offset.min(source.len()));
    let mut line_number = 1;
    let mut line_start = 0;
    for (index, ch) in source.char_indices() {
        if index >= offset {
            break;
        }
        if ch == '\n' {
            line_number += 1;
            line_start = index + ch.len_utf8();
        }
    }

    let mut line_end = source.len();
    for (relative, ch) in source[line_start..].char_indices() {
        if ch == '\n' {
            line_end = line_start + relative;
            break;
        }
    }
    if line_end > line_start && source.as_bytes()[line_end - 1] == b'\r' {
        line_end -= 1;
    }

    LineInfo {
        number: line_number,
        start: line_start,
        end: line_end,
        column: source[line_start..offset].chars().count() + 1,
    }
}

fn underline_width(source: &str, span: Span, line: &LineInfo) -> usize {
    let start = clamp_to_char_boundary(source, span.start.min(source.len()));
    let end = clamp_to_char_boundary(source, span.end.min(source.len()));
    let line_relative_start = source[line.start..start.min(line.end)].chars().count();
    let line_relative_end = source[line.start..end.min(line.end)].chars().count();
    line_relative_end.saturating_sub(line_relative_start).max(1)
}

fn clamp_to_char_boundary(source: &str, mut offset: usize) -> usize {
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_line_column_and_caret() {
        let diagnostic = Diagnostic::error(Span::new(11, 16), "expected `;` after binding");
        let rendered = render_diagnostic("main.nia", "var x = 1;\nprint(x);\n", &diagnostic);
        assert!(rendered.contains("error: expected `;` after binding"));
        assert!(rendered.contains("--> main.nia:2:1"));
        assert!(rendered.contains("2 | print(x);"));
        assert!(rendered.contains("| ^^^^^"));
    }

    #[test]
    fn renders_empty_spans_with_one_caret() {
        let diagnostic = Diagnostic::error(Span::new(1, 1), "empty");
        let rendered = render_diagnostic("main.nia", "abc", &diagnostic);
        assert!(rendered.contains("--> main.nia:1:2"));
        assert!(rendered.contains("|  ^"));
    }

    #[test]
    fn aligns_gutter_bars() {
        let diagnostic = Diagnostic::error(Span::new(0, 1), "aligned");
        let rendered = render_diagnostic("main.nia", "abc", &diagnostic);
        let bar_columns = rendered
            .lines()
            .filter_map(|line| line.find('|'))
            .collect::<Vec<_>>();
        assert_eq!(bar_columns, vec![2, 2, 2]);
    }
}
