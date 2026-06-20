// SPDX-License-Identifier: GPL-3.0-or-later
use nia_span::Span;
use std::cmp::Ordering;
use std::fmt;

pub mod codes {
    use super::{DiagnosticCategory, DiagnosticCode, Severity};

    pub const ICE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0001",
        DiagnosticStage::Compiler,
        "ice",
        "compiler panic or explicit internal compiler error",
    );
    pub const INTERNAL_RESOLUTION: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0100",
        DiagnosticStage::Resolution,
        "internal-resolution",
        "resolver invariant failure",
    );
    pub const ITEM_SIGNATURE_DEF_NODE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0101",
        DiagnosticStage::ItemSignature,
        "item-signature-def-node",
        "item signature definition node invariant failure",
    );
    pub const ITEM_SIGNATURE_DEF_MAP: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0102",
        DiagnosticStage::ItemSignature,
        "item-signature-def-map",
        "item signature definition map invariant failure",
    );
    pub const ITEM_SIGNATURE_DEF_KIND: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0103",
        DiagnosticStage::ItemSignature,
        "item-signature-def-kind",
        "item signature definition kind invariant failure",
    );
    pub const ITEM_SIGNATURE_LOWERED_TYPE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0104",
        DiagnosticStage::ItemSignature,
        "item-signature-lowered-type",
        "item signature lowered type invariant failure",
    );
    pub const LOCAL_RESOLVER_SCOPE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0105",
        DiagnosticStage::LocalResolution,
        "local-resolver-scope",
        "local resolver scope stack invariant failure",
    );
    pub const METHOD_RESOLUTION_INVARIANT: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0106",
        DiagnosticStage::TypeCheck,
        "method-resolution-invariant",
        "method resolution invariant failure",
    );
    pub const MODULE_GRAPH_LOOKUP: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0107",
        DiagnosticStage::Load,
        "module-graph-lookup",
        "module graph lookup invariant failure",
    );
    pub const MODULE_GRAPH_RECORDING: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0108",
        DiagnosticStage::Load,
        "module-graph-recording",
        "module graph recording invariant failure",
    );
    pub const MODULE_GRAPH_CHILD: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0109",
        DiagnosticStage::Load,
        "module-graph-child",
        "module graph child invariant failure",
    );
    pub const QUERY_ENGINE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0110",
        DiagnosticStage::Compiler,
        "query-engine",
        "query engine invariant failure",
    );
    pub const INTERNAL_LLVM_API: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0200",
        DiagnosticStage::Llvm,
        "llvm-api",
        "LLVM API returned an unexpected failure",
    );
    pub const INVALID_FUNCTION_IR: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0201",
        DiagnosticStage::FunctionIr,
        "invalid-function-ir",
        "function IR invariant failure",
    );
    pub const INVALID_BACKEND_IR: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0300",
        DiagnosticStage::BackendIr,
        "invalid-backend-ir",
        "backend IR invariant failure",
    );
    pub const INVALID_BODY_IR: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0301",
        DiagnosticStage::BodyIr,
        "invalid-body-ir",
        "body IR invariant failure",
    );

    pub const PARSE: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0101",
        DiagnosticStage::Parse,
        "parse",
        "source could not be parsed into valid Nia syntax",
    );
    pub const LOAD: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0102",
        DiagnosticStage::Load,
        "load",
        "module loading or import graph failed",
    );
    pub const TARGET_CONFIG: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0103",
        DiagnosticStage::TargetConfig,
        "target-config",
        "target configuration is invalid",
    );
    pub const NAME_RESOLUTION: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0201",
        DiagnosticStage::Resolution,
        "name-resolution",
        "name or item resolution failed",
    );
    pub const TYPE_NORMALIZATION: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0202",
        DiagnosticStage::TypeNormalization,
        "type-normalization",
        "type normalization failed",
    );
    pub const ITEM_SIGNATURE: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0203",
        DiagnosticStage::ItemSignature,
        "item-signature",
        "item signature collection failed",
    );
    pub const TYPE_CHECK: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0301",
        DiagnosticStage::TypeCheck,
        "type-check",
        "expression, statement, or body type checking failed",
    );
    pub const LOCAL_RESOLUTION: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0302",
        DiagnosticStage::LocalResolution,
        "local-resolution",
        "local binding resolution failed",
    );
    pub const COMPTIME: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0401",
        DiagnosticStage::Comptime,
        "comptime",
        "comptime evaluation failed",
    );
    pub const STATIC_CHECK: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0501",
        DiagnosticStage::StaticCheck,
        "static-check",
        "static layout, ABI, or initializer check failed",
    );
    pub const LLVM_CODEGEN: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0601",
        DiagnosticStage::Llvm,
        "llvm-codegen",
        "LLVM code generation failed",
    );

    pub const ALL: &[DiagnosticCodeDef] = &[
        ICE,
        INTERNAL_RESOLUTION,
        ITEM_SIGNATURE_DEF_NODE,
        ITEM_SIGNATURE_DEF_MAP,
        ITEM_SIGNATURE_DEF_KIND,
        ITEM_SIGNATURE_LOWERED_TYPE,
        LOCAL_RESOLVER_SCOPE,
        METHOD_RESOLUTION_INVARIANT,
        MODULE_GRAPH_LOOKUP,
        MODULE_GRAPH_RECORDING,
        MODULE_GRAPH_CHILD,
        QUERY_ENGINE,
        INTERNAL_LLVM_API,
        INVALID_FUNCTION_IR,
        INVALID_BACKEND_IR,
        INVALID_BODY_IR,
        PARSE,
        LOAD,
        TARGET_CONFIG,
        NAME_RESOLUTION,
        TYPE_NORMALIZATION,
        ITEM_SIGNATURE,
        TYPE_CHECK,
        LOCAL_RESOLUTION,
        COMPTIME,
        STATIC_CHECK,
        LLVM_CODEGEN,
    ];

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct DiagnosticCodeDef {
        pub code: &'static str,
        pub severity: Severity,
        pub category: DiagnosticCategory,
        pub stage: DiagnosticStage,
        pub name: &'static str,
        pub description: &'static str,
    }

    impl DiagnosticCodeDef {
        pub const fn user(
            code: &'static str,
            stage: DiagnosticStage,
            name: &'static str,
            description: &'static str,
        ) -> Self {
            Self {
                code,
                severity: Severity::Error,
                category: DiagnosticCategory::User,
                stage,
                name,
                description,
            }
        }

        pub const fn internal(
            code: &'static str,
            stage: DiagnosticStage,
            name: &'static str,
            description: &'static str,
        ) -> Self {
            Self {
                code,
                severity: Severity::Error,
                category: DiagnosticCategory::Internal,
                stage,
                name,
                description,
            }
        }

        pub const fn as_str(self) -> &'static str {
            self.code
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum DiagnosticStage {
        Compiler,
        Parse,
        Load,
        TargetConfig,
        Resolution,
        TypeNormalization,
        ItemSignature,
        LocalResolution,
        TypeCheck,
        BodyIr,
        Comptime,
        StaticCheck,
        FunctionIr,
        BackendIr,
        Llvm,
    }

    impl From<DiagnosticCodeDef> for DiagnosticCode {
        fn from(value: DiagnosticCodeDef) -> Self {
            Self::registered(value)
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCategory {
    User,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiagnosticCode {
    code: String,
    registered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticLabel {
    pub span: Span,
    pub span_source: SpanSource,
    pub style: LabelStyle,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanSource {
    Source,
    Fallback,
    Generated,
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
pub struct DiagnosticSinkConfig {
    pub allow_user_fallback_spans: bool,
}

impl Default for DiagnosticSinkConfig {
    fn default() -> Self {
        Self {
            allow_user_fallback_spans: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
    config: DiagnosticSinkConfig,
}

impl DiagnosticSink {
    pub fn new(config: DiagnosticSinkConfig) -> Self {
        Self {
            diagnostics: Vec::new(),
            config,
        }
    }

    pub fn emit(&mut self, diagnostic: Diagnostic) {
        self.validate_emit_contract(&diagnostic);
        self.diagnostics.push(diagnostic);
    }

    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        for diagnostic in diagnostics {
            self.emit(diagnostic);
        }
    }

    pub fn finish(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn validate_emit_contract(&self, diagnostic: &Diagnostic) {
        if diagnostic.category == DiagnosticCategory::User
            && !self.config.allow_user_fallback_spans
            && diagnostic
                .labels
                .iter()
                .any(|label| label.span_source == SpanSource::Fallback)
        {
            panic!("Nia ICE: user diagnostic emitted with fallback span");
        }
    }
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
    fn build(code: codes::DiagnosticCodeDef, summary: impl Into<String>) -> DiagnosticBuilder {
        DiagnosticBuilder {
            diagnostic: Diagnostic {
                severity: code.severity,
                category: code.category,
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
        code: codes::DiagnosticCodeDef,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        assert_eq!(code.category, DiagnosticCategory::User);
        Self::build(code, summary)
    }

    #[cfg(test)]
    fn user_error_unregistered_for_test(
        code: impl Into<String>,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        DiagnosticBuilder {
            diagnostic: Diagnostic {
                severity: Severity::Error,
                category: DiagnosticCategory::User,
                code: DiagnosticCode::unregistered_for_test(code),
                summary: summary.into(),
                labels: Box::new(Vec::new()),
                notes: Box::new(Vec::new()),
                help: Box::new(Vec::new()),
                related: Box::new(Vec::new()),
                debug: Box::new(Vec::new()),
            },
        }
    }

    pub fn user_error_at(
        code: codes::DiagnosticCodeDef,
        span: Span,
        summary: impl Into<String>,
    ) -> Diagnostic {
        let summary = summary.into();
        Self::user_error(code, summary.clone())
            .primary(span, summary)
            .finish()
    }

    pub fn internal_error(
        code: codes::DiagnosticCodeDef,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        assert_eq!(code.category, DiagnosticCategory::Internal);
        Self::build(code, summary)
    }

    pub fn internal_error_at(
        code: codes::DiagnosticCodeDef,
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

    pub fn primary_label(&self) -> Option<&DiagnosticLabel> {
        self.labels
            .iter()
            .find(|label| label.style == LabelStyle::Primary)
            .or_else(|| self.labels.first())
    }

    pub fn primary_span_source(&self) -> Option<SpanSource> {
        self.primary_label().map(|label| label.span_source)
    }

    pub fn uses_unregistered_code(&self) -> bool {
        !self.code.is_registered()
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
            span_source: SpanSource::Source,
            style: LabelStyle::Primary,
            message: Some(message.into()),
        });
        self
    }

    pub fn primary_span(mut self, span: Span) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Source,
            style: LabelStyle::Primary,
            message: None,
        });
        self
    }

    pub fn primary_fallback(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Fallback,
            style: LabelStyle::Primary,
            message: Some(message.into()),
        });
        self
    }

    pub fn secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Source,
            style: LabelStyle::Secondary,
            message: Some(message.into()),
        });
        self
    }

    pub fn secondary_fallback(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Fallback,
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
                .uses_unregistered_code()
                .cmp(&right_diagnostic.uses_unregistered_code())
        })
        .then_with(|| span_source_rank(left_diagnostic).cmp(&span_source_rank(right_diagnostic)))
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

fn span_source_rank(diagnostic: &Diagnostic) -> u8 {
    match diagnostic.primary_span_source() {
        Some(SpanSource::Source) => 0,
        Some(SpanSource::Generated) => 1,
        Some(SpanSource::Fallback) => 2,
        None => 3,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiagnosticDedupeKey {
    path: Option<String>,
    category: DiagnosticCategory,
    severity: Severity,
    code: String,
    registered: bool,
    span: Option<Span>,
    span_source: Option<SpanSource>,
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
            registered: diagnostic.code.is_registered(),
            span: diagnostic.primary_span(),
            span_source: diagnostic.primary_span_source(),
            summary: diagnostic.summary.clone(),
        }
    }
}

impl DiagnosticCode {
    #[cfg(test)]
    fn unregistered_for_test(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            registered: false,
        }
    }

    pub fn registered(code: codes::DiagnosticCodeDef) -> Self {
        Self {
            code: code.code.to_string(),
            registered: true,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.code
    }

    pub fn is_registered(&self) -> bool {
        self.registered
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.code)
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
        let diagnostic = Diagnostic::user_error(codes::PARSE, "expected `;` after binding")
            .primary(Span::new(11, 16), "statement starts here")
            .note("parser was recovering after a missing token")
            .help("insert `;` before this statement")
            .finish();
        let rendered = render_diagnostic("main.nia", "var x = 1;\nprint(x);\n", &diagnostic);
        assert!(rendered.contains("error[E0101]: expected `;` after binding"));
        assert!(rendered.contains("--> main.nia:2:1"));
        assert!(rendered.contains("2 | print(x);"));
        assert!(rendered.contains("| ^^^^^ statement starts here"));
        assert!(rendered.contains("note: parser was recovering after a missing token"));
        assert!(rendered.contains("help: insert `;` before this statement"));
    }

    #[test]
    fn renders_empty_spans_with_one_caret() {
        let diagnostic = Diagnostic::user_error(codes::PARSE, "empty")
            .primary_span(Span::new(1, 1))
            .finish();
        let rendered = render_diagnostic("main.nia", "abc", &diagnostic);
        assert!(rendered.contains("--> main.nia:1:2"));
        assert!(rendered.contains("|  ^"));
    }

    #[test]
    fn renders_internal_debug_payload() {
        let diagnostic = Diagnostic::internal_error(codes::ICE, "missing definition")
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
            Diagnostic::user_error_at(codes::PARSE, Span::new(10, 11), "user error"),
            Diagnostic::internal_error_at(codes::ICE, Span::new(20, 21), "internal error"),
            Diagnostic::user_error_at(codes::LOAD, Span::new(30, 31), "second user error"),
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
            Diagnostic::user_error_at(codes::PARSE, Span::new(10, 11), "same error"),
            Diagnostic::user_error_at(codes::PARSE, Span::new(10, 11), "same error"),
        ];

        let report = build_diagnostic_report(&diagnostics, DiagnosticReportConfig::default());

        assert_eq!(report.entries().len(), 1);
        assert_eq!(report.suppressed_duplicates(), 1);
    }

    #[test]
    fn registered_codes_are_unique_and_well_formed() {
        let mut seen = Vec::new();
        for code in codes::ALL {
            assert_eq!(code.code.len(), 5, "{code:?}");
            let mut chars = code.code.chars();
            let prefix = chars.next().expect("code prefix");
            assert!(matches!(prefix, 'E' | 'I'), "{code:?}");
            assert!(chars.all(|ch| ch.is_ascii_digit()), "{code:?}");
            assert_eq!(
                matches!(prefix, 'I'),
                code.category == DiagnosticCategory::Internal,
                "{code:?}"
            );
            assert!(
                seen.iter().all(|seen_code| seen_code != &code.code),
                "duplicate diagnostic code {}",
                code.code
            );
            seen.push(code.code);
        }
    }

    #[test]
    fn diagnostics_remember_whether_code_is_registered() {
        let registered = Diagnostic::user_error(codes::PARSE, "registered").finish();
        let raw = Diagnostic::user_error_unregistered_for_test("E9999", "raw").finish();

        assert!(!registered.uses_unregistered_code());
        assert!(raw.uses_unregistered_code());
    }

    #[test]
    fn labels_track_span_source() {
        let diagnostic = Diagnostic::internal_error(codes::ICE, "fallback")
            .primary_fallback(Span::default(), "fallback span")
            .finish();

        assert_eq!(diagnostic.primary_span_source(), Some(SpanSource::Fallback));
    }

    #[test]
    fn diagnostic_sink_accepts_internal_fallback_spans() {
        let diagnostic = Diagnostic::internal_error(codes::ICE, "fallback")
            .primary_fallback(Span::default(), "fallback span")
            .finish();
        let mut sink = DiagnosticSink::new(DiagnosticSinkConfig::default());

        sink.emit(diagnostic);

        assert_eq!(sink.diagnostics().len(), 1);
    }

    #[test]
    #[should_panic(expected = "user diagnostic emitted with fallback span")]
    fn diagnostic_sink_rejects_user_fallback_spans() {
        let diagnostic = Diagnostic::user_error(codes::PARSE, "fallback")
            .primary_fallback(Span::default(), "fallback span")
            .finish();
        let mut sink = DiagnosticSink::new(DiagnosticSinkConfig::default());

        sink.emit(diagnostic);
    }
}
