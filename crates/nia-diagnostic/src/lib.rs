// SPDX-License-Identifier: GPL-3.0-or-later
use nia_span::Span;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub category: DiagnosticCategory,
    pub code: DiagnosticCode,
    pub summary: String,
    pub labels: Box<Vec<DiagnosticLabel>>,
    pub notes: Box<Vec<String>>,
    pub help: Box<Vec<String>>,
    pub related: Box<Vec<RelatedDiagnostic>>,
    pub debug: Box<Vec<DebugField>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCategory {
    User,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiagnosticCode(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticLabel {
    pub span: Span,
    pub style: LabelStyle,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    Primary,
    Secondary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedDiagnostic {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugField {
    pub key: String,
    pub value: String,
}

pub struct DiagnosticBuilder {
    diagnostic: Diagnostic,
}

impl Diagnostic {
    pub fn build(
        severity: Severity,
        category: DiagnosticCategory,
        code: impl Into<DiagnosticCode>,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        DiagnosticBuilder {
            diagnostic: Diagnostic {
                severity,
                category,
                code: code.into(),
                summary: summary.into(),
                labels: Box::new(Vec::new()),
                notes: Box::new(Vec::new()),
                help: Box::new(Vec::new()),
                related: Box::new(Vec::new()),
                debug: Box::new(Vec::new()),
            },
        }
    }

    pub fn user_error(
        code: impl Into<DiagnosticCode>,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        Self::build(Severity::Error, DiagnosticCategory::User, code, summary)
    }

    pub fn user_error_at(
        code: impl Into<DiagnosticCode>,
        span: Span,
        summary: impl Into<String>,
    ) -> Diagnostic {
        let summary = summary.into();
        Self::user_error(code, summary.clone())
            .primary(span, summary)
            .finish()
    }

    pub fn internal_error(
        code: impl Into<DiagnosticCode>,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        Self::build(Severity::Error, DiagnosticCategory::Internal, code, summary)
    }

    pub fn internal_error_at(
        code: impl Into<DiagnosticCode>,
        span: Span,
        summary: impl Into<String>,
    ) -> Diagnostic {
        let summary = summary.into();
        Self::internal_error(code, summary.clone())
            .primary(span, summary)
            .finish()
    }

    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|label| label.style == LabelStyle::Primary)
            .map(|label| label.span)
            .or_else(|| self.labels.first().map(|label| label.span))
    }

    pub fn primary_message(&self) -> Option<&str> {
        self.labels
            .iter()
            .find(|label| label.style == LabelStyle::Primary)
            .and_then(|label| label.message.as_deref())
    }
}

impl DiagnosticBuilder {
    pub fn primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            style: LabelStyle::Primary,
            message: Some(message.into()),
        });
        self
    }

    pub fn primary_span(mut self, span: Span) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            style: LabelStyle::Primary,
            message: None,
        });
        self
    }

    pub fn secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            style: LabelStyle::Secondary,
            message: Some(message.into()),
        });
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.diagnostic.notes.push(note.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.diagnostic.help.push(help.into());
        self
    }

    pub fn related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.related.push(RelatedDiagnostic {
            span,
            message: message.into(),
        });
        self
    }

    pub fn debug(mut self, key: impl Into<String>, value: impl fmt::Debug) -> Self {
        self.diagnostic.debug.push(DebugField {
            key: key.into(),
            value: format!("{value:?}"),
        });
        self
    }

    pub fn finish(self) -> Diagnostic {
        self.diagnostic
    }
}

impl DiagnosticCode {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for DiagnosticCode {
    fn from(value: &'static str) -> Self {
        Self::new(value)
    }
}

impl From<String> for DiagnosticCode {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
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
    let mut output = String::new();
    let category = match diagnostic.category {
        DiagnosticCategory::User => "",
        DiagnosticCategory::Internal => " internal",
    };
    output.push_str(&format!(
        "{}{category}[{}]: {}\n",
        diagnostic.severity, diagnostic.code, diagnostic.summary
    ));

    if diagnostic.labels.is_empty() {
        output.push_str(&format!("  --> {path}:1:1\n"));
    } else {
        for (index, label) in diagnostic.labels.iter().enumerate() {
            render_label(path, source, label, index == 0, &mut output);
        }
    }

    for note in diagnostic.notes.iter() {
        output.push_str(&format!("note: {note}\n"));
    }
    for help in diagnostic.help.iter() {
        output.push_str(&format!("help: {help}\n"));
    }
    for related in diagnostic.related.iter() {
        let line = line_info(source, related.span.start);
        output.push_str(&format!(
            "related: {}:{}:{}: {}\n",
            path, line.number, line.column, related.message
        ));
    }
    if diagnostic.category == DiagnosticCategory::Internal {
        for field in diagnostic.debug.iter() {
            output.push_str(&format!("debug: {} = {}\n", field.key, field.value));
        }
    }
    output
}

fn render_label(
    path: &str,
    source: &str,
    label: &DiagnosticLabel,
    first: bool,
    output: &mut String,
) {
    let line = line_info(source, label.span.start);
    let line_text = &source[line.start..line.end];
    let line_no = line.number.to_string();
    let gutter_width = line_no.len();
    let underline_start = line.column.saturating_sub(1);
    let underline_width = underline_width(source, label.span, &line);
    let marker = match label.style {
        LabelStyle::Primary => '^',
        LabelStyle::Secondary => '-',
    };

    if first {
        output.push_str(&format!(
            "{:>width$} --> {}:{}:{}\n",
            "",
            path,
            line.number,
            line.column,
            width = gutter_width
        ));
    } else {
        output.push_str(&format!(
            "{:>width$} ::: {}:{}:{}\n",
            "",
            path,
            line.number,
            line.column,
            width = gutter_width
        ));
    }
    output.push_str(&format!("{:>width$} |\n", "", width = gutter_width));
    output.push_str(&format!(
        "{line_no:>width$} | {line_text}\n",
        width = gutter_width
    ));
    output.push_str(&format!(
        "{:>width$} | {}{}",
        "",
        " ".repeat(underline_start),
        marker.to_string().repeat(underline_width),
        width = gutter_width
    ));
    if let Some(message) = &label.message {
        output.push_str(&format!(" {message}"));
    }
    output.push('\n');
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
    fn renders_code_labels_notes_and_help() {
        let diagnostic = Diagnostic::user_error("E0001", "expected `;` after binding")
            .primary(Span::new(11, 16), "statement starts here")
            .note("parser was recovering after a missing token")
            .help("insert `;` before this statement")
            .finish();
        let rendered = render_diagnostic("main.nia", "var x = 1;\nprint(x);\n", &diagnostic);
        assert!(rendered.contains("error[E0001]: expected `;` after binding"));
        assert!(rendered.contains("--> main.nia:2:1"));
        assert!(rendered.contains("2 | print(x);"));
        assert!(rendered.contains("| ^^^^^ statement starts here"));
        assert!(rendered.contains("note: parser was recovering after a missing token"));
        assert!(rendered.contains("help: insert `;` before this statement"));
    }

    #[test]
    fn renders_empty_spans_with_one_caret() {
        let diagnostic = Diagnostic::user_error("E0001", "empty")
            .primary_span(Span::new(1, 1))
            .finish();
        let rendered = render_diagnostic("main.nia", "abc", &diagnostic);
        assert!(rendered.contains("--> main.nia:1:2"));
        assert!(rendered.contains("|  ^"));
    }

    #[test]
    fn renders_internal_debug_payload() {
        let diagnostic = Diagnostic::internal_error("I0001", "missing definition")
            .primary(Span::new(0, 1), "while collecting item signature")
            .debug("node_key", "n1")
            .finish();
        let rendered = render_diagnostic("main.nia", "abc", &diagnostic);
        assert!(rendered.contains("error internal[I0001]: missing definition"));
        assert!(rendered.contains("debug: node_key = \"n1\""));
    }
}
