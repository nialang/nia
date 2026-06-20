// SPDX-License-Identifier: GPL-3.0-or-later
use nia_span::Span;
use std::cmp::Ordering;
use std::fmt;

pub mod codes {
    pub const ICE: &str = "I0001";
    pub const INTERNAL_RESOLUTION: &str = "I0100";
    pub const INTERNAL_LLVM_API: &str = "I0200";
    pub const INVALID_FUNCTION_IR: &str = "I0201";
    pub const INVALID_BACKEND_IR: &str = "I0300";
    pub const INVALID_BODY_IR: &str = "I0301";

    pub const PARSE: &str = "E0101";
    pub const LOAD: &str = "E0102";
    pub const TARGET_CONFIG: &str = "E0103";
    pub const NAME_RESOLUTION: &str = "E0201";
    pub const TYPE_NORMALIZATION: &str = "E0202";
    pub const ITEM_SIGNATURE: &str = "E0203";
    pub const TYPE_CHECK: &str = "E0301";
    pub const LOCAL_RESOLUTION: &str = "E0302";
    pub const COMPTIME: &str = "E0401";
    pub const STATIC_CHECK: &str = "E0501";
    pub const LLVM_CODEGEN: &str = "E0601";
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticReportConfig {
    pub max_diagnostics: usize,
}

impl Default for DiagnosticReportConfig {
    fn default() -> Self {
        Self {
            max_diagnostics: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticReport<'a, T> {
    entries: Vec<&'a T>,
    suppressed_duplicates: usize,
    suppressed_by_limit: usize,
}

impl<'a, T> DiagnosticReport<'a, T> {
    pub fn entries(&self) -> &[&'a T] {
        &self.entries
    }

    pub fn suppressed_duplicates(&self) -> usize {
        self.suppressed_duplicates
    }

    pub fn suppressed_by_limit(&self) -> usize {
        self.suppressed_by_limit
    }

    pub fn suppressed_total(&self) -> usize {
        self.suppressed_duplicates + self.suppressed_by_limit
    }
}

pub trait DiagnosticReportItem {
    fn report_diagnostic(&self) -> &Diagnostic;

    fn report_path(&self) -> Option<&str> {
        None
    }
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

impl DiagnosticReportItem for Diagnostic {
    fn report_diagnostic(&self) -> &Diagnostic {
        self
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

pub fn build_diagnostic_report<T: DiagnosticReportItem>(
    diagnostics: &[T],
    config: DiagnosticReportConfig,
) -> DiagnosticReport<'_, T> {
    let mut entries = diagnostics.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| compare_report_items(*left, *right));

    let mut selected = Vec::new();
    let mut seen = Vec::<DiagnosticDedupeKey>::new();
    let mut suppressed_duplicates = 0;
    let mut suppressed_by_limit = 0;
    for entry in entries {
        let key = DiagnosticDedupeKey::from_item(entry);
        if seen.iter().any(|seen| seen == &key) {
            suppressed_duplicates += 1;
            continue;
        }
        seen.push(key);
        if selected.len() >= config.max_diagnostics {
            suppressed_by_limit += 1;
            continue;
        }
        selected.push(entry);
    }

    DiagnosticReport {
        entries: selected,
        suppressed_duplicates,
        suppressed_by_limit,
    }
}

fn compare_report_items<T: DiagnosticReportItem>(left: &T, right: &T) -> Ordering {
    let left_diagnostic = left.report_diagnostic();
    let right_diagnostic = right.report_diagnostic();
    diagnostic_priority(left_diagnostic)
        .cmp(&diagnostic_priority(right_diagnostic))
        .then_with(|| left.report_path().cmp(&right.report_path()))
        .then_with(|| {
            left_diagnostic
                .primary_span()
                .unwrap_or_default()
                .start
                .cmp(&right_diagnostic.primary_span().unwrap_or_default().start)
        })
        .then_with(|| {
            left_diagnostic
                .primary_span()
                .unwrap_or_default()
                .end
                .cmp(&right_diagnostic.primary_span().unwrap_or_default().end)
        })
        .then_with(|| {
            left_diagnostic
                .code
                .as_str()
                .cmp(right_diagnostic.code.as_str())
        })
        .then_with(|| left_diagnostic.summary.cmp(&right_diagnostic.summary))
}

fn diagnostic_priority(diagnostic: &Diagnostic) -> (u8, u8) {
    let category = match diagnostic.category {
        DiagnosticCategory::Internal => 0,
        DiagnosticCategory::User => 1,
    };
    let severity = match diagnostic.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    };
    (category, severity)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiagnosticDedupeKey {
    path: Option<String>,
    category: DiagnosticCategory,
    severity: Severity,
    code: String,
    span: Option<Span>,
    summary: String,
}

impl DiagnosticDedupeKey {
    fn from_item<T: DiagnosticReportItem>(item: &T) -> Self {
        let diagnostic = item.report_diagnostic();
        Self {
            path: item.report_path().map(str::to_string),
            category: diagnostic.category,
            severity: diagnostic.severity,
            code: diagnostic.code.as_str().to_string(),
            span: diagnostic.primary_span(),
            summary: diagnostic.summary.clone(),
        }
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

    #[test]
    fn report_prioritizes_internal_diagnostics_and_limits_output() {
        let diagnostics = vec![
            Diagnostic::user_error_at("E0001", Span::new(10, 11), "user error"),
            Diagnostic::internal_error_at("I0001", Span::new(20, 21), "internal error"),
            Diagnostic::user_error_at("E0002", Span::new(30, 31), "second user error"),
        ];

        let report =
            build_diagnostic_report(&diagnostics, DiagnosticReportConfig { max_diagnostics: 2 });

        assert_eq!(report.entries().len(), 2);
        assert_eq!(report.entries()[0].code.as_str(), "I0001");
        assert_eq!(report.suppressed_by_limit(), 1);
    }

    #[test]
    fn report_deduplicates_same_diagnostic() {
        let diagnostics = vec![
            Diagnostic::user_error_at("E0001", Span::new(10, 11), "same error"),
            Diagnostic::user_error_at("E0001", Span::new(10, 11), "same error"),
        ];

        let report = build_diagnostic_report(&diagnostics, DiagnosticReportConfig::default());

        assert_eq!(report.entries().len(), 1);
        assert_eq!(report.suppressed_duplicates(), 1);
    }
}
