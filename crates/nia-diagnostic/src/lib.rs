// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured compiler diagnostics, stable persistence, and report rendering.
//!
//! Producers emit registered codes with source, generated, or explicitly
//! marked fallback spans. Reports impose deterministic presentation order and
//! deduplicate only byte-for-byte equivalent diagnostic structure; secondary
//! labels, notes, help, related locations, and internal debug fields are part
//! of a diagnostic's identity.

use nia_span::Span;

mod stable_bundle;
mod store;

pub use stable_bundle::{
    StableDiagnosticBundleError, decode_stable_diagnostic_bundle, encode_stable_diagnostic_bundle,
};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;
pub use store::{DiagnosticBundle, DiagnosticBundleId, DiagnosticStore};

/// Registered diagnostic code definitions shared by all compiler stages.
pub mod codes {
    use super::{DiagnosticCategory, DiagnosticCode, Severity};

    /// Explicit internal compiler error.
    pub const ICE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0001",
        DiagnosticStage::Compiler,
        "ice",
        "compiler panic or explicit internal compiler error",
    );
    /// Internal name/value resolution invariant failure.
    pub const INTERNAL_RESOLUTION: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0100",
        DiagnosticStage::Resolution,
        "internal-resolution",
        "resolver invariant failure",
    );
    /// Internal item-signature definition-node failure.
    pub const ITEM_SIGNATURE_DEF_NODE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0101",
        DiagnosticStage::ItemSignature,
        "item-signature-def-node",
        "item signature definition node invariant failure",
    );
    /// Internal item-signature definition-map failure.
    pub const ITEM_SIGNATURE_DEF_MAP: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0102",
        DiagnosticStage::ItemSignature,
        "item-signature-def-map",
        "item signature definition map invariant failure",
    );
    /// Internal item-signature definition-kind failure.
    pub const ITEM_SIGNATURE_DEF_KIND: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0103",
        DiagnosticStage::ItemSignature,
        "item-signature-def-kind",
        "item signature definition kind invariant failure",
    );
    /// Internal lowered item-signature type failure.
    pub const ITEM_SIGNATURE_LOWERED_TYPE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0104",
        DiagnosticStage::ItemSignature,
        "item-signature-lowered-type",
        "item signature lowered type invariant failure",
    );
    /// Internal local-resolver scope-stack failure.
    pub const LOCAL_RESOLVER_SCOPE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0105",
        DiagnosticStage::LocalResolution,
        "local-resolver-scope",
        "local resolver scope stack invariant failure",
    );
    /// Internal method-resolution invariant failure.
    pub const METHOD_RESOLUTION_INVARIANT: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0106",
        DiagnosticStage::TypeCheck,
        "method-resolution-invariant",
        "method resolution invariant failure",
    );
    /// Internal module-graph lookup failure.
    pub const MODULE_GRAPH_LOOKUP: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0107",
        DiagnosticStage::Load,
        "module-graph-lookup",
        "module graph lookup invariant failure",
    );
    /// Internal module-graph recording failure.
    pub const MODULE_GRAPH_RECORDING: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0108",
        DiagnosticStage::Load,
        "module-graph-recording",
        "module graph recording invariant failure",
    );
    /// Internal module-graph child lookup failure.
    pub const MODULE_GRAPH_CHILD: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0109",
        DiagnosticStage::Load,
        "module-graph-child",
        "module graph child invariant failure",
    );
    /// Internal query-engine failure.
    pub const QUERY_ENGINE: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0110",
        DiagnosticStage::Compiler,
        "query-engine",
        "query engine invariant failure",
    );
    /// Internal LLVM API failure.
    pub const INTERNAL_LLVM_API: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0200",
        DiagnosticStage::Llvm,
        "llvm-api",
        "LLVM API returned an unexpected failure",
    );
    /// Invalid function IR product.
    pub const INVALID_FUNCTION_IR: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0201",
        DiagnosticStage::FunctionIr,
        "invalid-function-ir",
        "function IR invariant failure",
    );
    /// Invalid backend IR product.
    pub const INVALID_BACKEND_IR: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0300",
        DiagnosticStage::BackendIr,
        "invalid-backend-ir",
        "backend IR invariant failure",
    );
    /// Invalid body IR product.
    pub const INVALID_BODY_IR: DiagnosticCodeDef = DiagnosticCodeDef::internal(
        "I0301",
        DiagnosticStage::BodyIr,
        "invalid-body-ir",
        "body IR invariant failure",
    );

    /// User parse failure.
    pub const PARSE: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0101",
        DiagnosticStage::Parse,
        "parse",
        "source could not be parsed into valid Nia syntax",
    );
    /// User module-load or import failure.
    pub const LOAD: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0102",
        DiagnosticStage::Load,
        "load",
        "module loading or import graph failed",
    );
    /// Invalid target configuration.
    pub const TARGET_CONFIG: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0103",
        DiagnosticStage::TargetConfig,
        "target-config",
        "target configuration is invalid",
    );
    /// Name or item resolution failure.
    pub const NAME_RESOLUTION: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0201",
        DiagnosticStage::Resolution,
        "name-resolution",
        "name or item resolution failed",
    );
    /// Type normalization failure.
    pub const TYPE_NORMALIZATION: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0202",
        DiagnosticStage::TypeNormalization,
        "type-normalization",
        "type normalization failed",
    );
    /// Item signature collection failure.
    pub const ITEM_SIGNATURE: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0203",
        DiagnosticStage::ItemSignature,
        "item-signature",
        "item signature collection failed",
    );
    /// Expression, statement, or body type-check failure.
    pub const TYPE_CHECK: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0301",
        DiagnosticStage::TypeCheck,
        "type-check",
        "expression, statement, or body type checking failed",
    );
    /// Local binding resolution failure.
    pub const LOCAL_RESOLUTION: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0302",
        DiagnosticStage::LocalResolution,
        "local-resolution",
        "local binding resolution failed",
    );
    /// Compile-time evaluation failure.
    pub const CONST: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0401",
        DiagnosticStage::Const,
        "const",
        "const evaluation failed",
    );
    /// Static initializer or layout validation failure.
    pub const STATIC_CHECK: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0501",
        DiagnosticStage::StaticCheck,
        "static-check",
        "static layout, ABI, or initializer check failed",
    );
    /// LLVM code-generation failure.
    pub const LLVM_CODEGEN: DiagnosticCodeDef = DiagnosticCodeDef::user(
        "E0601",
        DiagnosticStage::Llvm,
        "llvm-codegen",
        "LLVM code generation failed",
    );
    /// Unused import warning.
    pub const UNUSED_IMPORT: DiagnosticCodeDef = DiagnosticCodeDef::user_warning(
        "W0201",
        DiagnosticStage::Load,
        "unused-import",
        "import binding is never used",
    );

    /// All registered diagnostic codes in stable registry order.
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
        CONST,
        STATIC_CHECK,
        LLVM_CODEGEN,
        UNUSED_IMPORT,
    ];

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    /// Metadata for one stable diagnostic code.
    pub struct DiagnosticCodeDef {
        /// Stable machine-readable code such as `E0301`.
        pub code: &'static str,
        /// Default report severity.
        pub severity: Severity,
        /// User-facing or internal category.
        pub category: DiagnosticCategory,
        /// Compiler stage that owns the code.
        pub stage: DiagnosticStage,
        /// Short registry name.
        pub name: &'static str,
        /// Stable description of the diagnostic class.
        pub description: &'static str,
    }

    impl DiagnosticCodeDef {
        /// Creates a user-facing error code definition.
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

        /// Creates an internal compiler error definition.
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

        /// Creates a user-facing warning definition.
        pub const fn user_warning(
            code: &'static str,
            stage: DiagnosticStage,
            name: &'static str,
            description: &'static str,
        ) -> Self {
            Self {
                code,
                severity: Severity::Warning,
                category: DiagnosticCategory::User,
                stage,
                name,
                description,
            }
        }

        /// Returns the stable code string.
        pub const fn as_str(self) -> &'static str {
            self.code
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    /// Compiler stage associated with a diagnostic code.
    pub enum DiagnosticStage {
        /// Cross-stage compiler infrastructure.
        Compiler,
        /// Lexing or parsing.
        Parse,
        /// Module loading and imports.
        Load,
        /// Target configuration.
        TargetConfig,
        /// Name and value resolution.
        Resolution,
        /// Type normalization.
        TypeNormalization,
        /// Item signature collection.
        ItemSignature,
        /// Local binding resolution.
        LocalResolution,
        /// Type checking.
        TypeCheck,
        /// Typed body IR construction.
        BodyIr,
        /// Compile-time evaluation.
        Const,
        /// Static initializer validation.
        StaticCheck,
        /// Function IR construction.
        FunctionIr,
        /// Backend IR validation.
        BackendIr,
        /// LLVM lowering and code generation.
        Llvm,
    }

    impl From<DiagnosticCodeDef> for DiagnosticCode {
        fn from(value: DiagnosticCodeDef) -> Self {
            Self::registered(value)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Structured diagnostic payload emitted by compiler stages.
pub struct Diagnostic {
    /// Report severity.
    pub severity: Severity,
    /// User-facing or internal category.
    pub category: DiagnosticCategory,
    /// Stable diagnostic code.
    pub code: DiagnosticCode,
    /// Primary summary shown to users.
    pub summary: String,
    /// Source and generated labels.
    pub labels: Box<Vec<DiagnosticLabel>>,
    /// Additional explanatory notes.
    pub notes: Box<Vec<String>>,
    /// Suggested remediation text.
    pub help: Box<Vec<String>>,
    /// Related source locations and messages.
    pub related: Box<Vec<RelatedDiagnostic>>,
    /// Internal debug fields retained for diagnostics tooling.
    pub debug: Box<Vec<DebugField>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Severity shown for a diagnostic.
pub enum Severity {
    /// Compilation-blocking error.
    Error,
    /// Non-blocking warning.
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Ownership category of a diagnostic.
pub enum DiagnosticCategory {
    /// User-facing source or configuration problem.
    User,
    /// Compiler invariant or infrastructure failure.
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Stable diagnostic code, optionally marked as registry-backed.
pub struct DiagnosticCode {
    code: String,
    registered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// One labeled source span attached to a diagnostic.
pub struct DiagnosticLabel {
    /// Labeled source span.
    pub span: Span,
    /// Provenance of the span.
    pub span_source: SpanSource,
    /// Primary or secondary presentation style.
    pub style: LabelStyle,
    /// Optional label text.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Provenance class for a diagnostic span.
pub enum SpanSource {
    /// Span came directly from source text.
    Source,
    /// Span is a deliberate fallback for missing source ownership.
    Fallback,
    /// Span was synthesized by a compiler transformation.
    Generated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Presentation style for a diagnostic label.
pub enum LabelStyle {
    /// Main location for the diagnostic.
    Primary,
    /// Supporting location.
    Secondary,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Related source location attached to a diagnostic.
pub struct RelatedDiagnostic {
    /// Related source span.
    pub span: Span,
    /// Explanation for the related location.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Internal key/value context attached to a diagnostic.
pub struct DebugField {
    /// Debug field name.
    pub key: String,
    /// Debug field value.
    pub value: String,
}

/// Fluent builder for structured diagnostics.
pub struct DiagnosticBuilder {
    diagnostic: Diagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
/// Validation policy for diagnostics emitted through a sink.
pub struct DiagnosticSinkConfig {
    /// Whether user diagnostics may use fallback spans.
    pub allow_user_fallback_spans: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Validates and collects diagnostics before publication.
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
    config: DiagnosticSinkConfig,
}

impl DiagnosticSink {
    /// Creates an empty sink with the supplied validation policy.
    pub fn new(config: DiagnosticSinkConfig) -> Self {
        Self {
            diagnostics: Vec::new(),
            config,
        }
    }

    /// Validates and appends one diagnostic.
    pub fn emit(&mut self, diagnostic: Diagnostic) {
        self.validate_emit_contract(&diagnostic);
        self.diagnostics.push(diagnostic);
    }

    /// Validates and appends all diagnostics from an iterator.
    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        for diagnostic in diagnostics {
            self.emit(diagnostic);
        }
    }

    /// Consumes the sink and returns its diagnostics in emission order.
    pub fn finish(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// Borrows diagnostics currently held by the sink.
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
/// Presentation limit applied when building a diagnostic report.
pub struct DiagnosticReportConfig {
    /// Maximum number of unique diagnostics retained.
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
/// Sorted, deduplicated diagnostic report with suppression counts.
pub struct DiagnosticReport<'a, T> {
    entries: Vec<&'a T>,
    suppressed_duplicates: usize,
    suppressed_by_limit: usize,
}

impl<'a, T> DiagnosticReport<'a, T> {
    /// Returns selected report entries in presentation order.
    pub fn entries(&self) -> &[&'a T] {
        &self.entries
    }

    /// Returns the number of exact duplicates removed.
    pub fn suppressed_duplicates(&self) -> usize {
        self.suppressed_duplicates
    }

    /// Returns the number omitted by the configured limit.
    pub fn suppressed_by_limit(&self) -> usize {
        self.suppressed_by_limit
    }

    /// Returns all suppressed entries.
    pub fn suppressed_total(&self) -> usize {
        self.suppressed_duplicates + self.suppressed_by_limit
    }
}

/// Supplies a diagnostic and optional source path to report ordering.
pub trait DiagnosticReportItem {
    /// Returns the structured diagnostic payload.
    fn report_diagnostic(&self) -> &Diagnostic;

    /// Returns an optional path used as a deterministic ordering key.
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

    /// Starts a user-facing error diagnostic builder.
    pub fn user_error(
        code: codes::DiagnosticCodeDef,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        assert_eq!(code.category, DiagnosticCategory::User);
        assert_eq!(code.severity, Severity::Error);
        Self::build(code, summary)
    }

    /// Starts a user-facing warning diagnostic builder.
    pub fn user_warning(
        code: codes::DiagnosticCodeDef,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        assert_eq!(code.category, DiagnosticCategory::User);
        assert_eq!(code.severity, Severity::Warning);
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

    /// Creates a user error with one primary source label.
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

    /// Creates a user warning with one primary source label.
    pub fn user_warning_at(
        code: codes::DiagnosticCodeDef,
        span: Span,
        summary: impl Into<String>,
    ) -> Diagnostic {
        let summary = summary.into();
        Self::user_warning(code, summary.clone())
            .primary(span, summary)
            .finish()
    }

    /// Starts an internal compiler error diagnostic builder.
    pub fn internal_error(
        code: codes::DiagnosticCodeDef,
        summary: impl Into<String>,
    ) -> DiagnosticBuilder {
        assert_eq!(code.category, DiagnosticCategory::Internal);
        Self::build(code, summary)
    }

    /// Creates an internal error with one primary source label.
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

    /// Returns the first primary span, falling back to the first label.
    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|label| label.style == LabelStyle::Primary)
            .map(|label| label.span)
            .or_else(|| self.labels.first().map(|label| label.span))
    }

    /// Returns the primary label's message, when present.
    pub fn primary_message(&self) -> Option<&str> {
        self.labels
            .iter()
            .find(|label| label.style == LabelStyle::Primary)
            .and_then(|label| label.message.as_deref())
    }

    /// Returns the primary label, or the first label when no primary exists.
    pub fn primary_label(&self) -> Option<&DiagnosticLabel> {
        self.labels
            .iter()
            .find(|label| label.style == LabelStyle::Primary)
            .or_else(|| self.labels.first())
    }

    /// Returns the provenance of the selected primary label.
    pub fn primary_span_source(&self) -> Option<SpanSource> {
        self.primary_label().map(|label| label.span_source)
    }

    /// Reports whether the diagnostic uses an unregistered code.
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
    /// Adds a source-backed primary label with a message.
    pub fn primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Source,
            style: LabelStyle::Primary,
            message: Some(message.into()),
        });
        self
    }

    /// Adds a source-backed primary label without a message.
    pub fn primary_span(mut self, span: Span) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Source,
            style: LabelStyle::Primary,
            message: None,
        });
        self
    }

    /// Adds a fallback primary label with a message.
    pub fn primary_fallback(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Fallback,
            style: LabelStyle::Primary,
            message: Some(message.into()),
        });
        self
    }

    /// Adds a source-backed secondary label.
    pub fn secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Source,
            style: LabelStyle::Secondary,
            message: Some(message.into()),
        });
        self
    }

    /// Adds a fallback secondary label.
    pub fn secondary_fallback(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.labels.push(DiagnosticLabel {
            span,
            span_source: SpanSource::Fallback,
            style: LabelStyle::Secondary,
            message: Some(message.into()),
        });
        self
    }

    /// Appends an explanatory note.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.diagnostic.notes.push(note.into());
        self
    }

    /// Appends suggested remediation text.
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.diagnostic.help.push(help.into());
        self
    }

    /// Appends a related source location.
    pub fn related(mut self, span: Span, message: impl Into<String>) -> Self {
        self.diagnostic.related.push(RelatedDiagnostic {
            span,
            message: message.into(),
        });
        self
    }

    /// Appends a formatted internal debug field.
    pub fn debug(mut self, key: impl Into<String>, value: impl fmt::Debug) -> Self {
        self.diagnostic.debug.push(DebugField {
            key: key.into(),
            value: format!("{value:?}"),
        });
        self
    }

    /// Finishes the builder and returns the immutable diagnostic.
    pub fn finish(self) -> Diagnostic {
        self.diagnostic
    }
}

/// Sorts diagnostics for presentation, removes exact duplicates, and applies
/// the configured display limit.
///
/// Deduplication happens before limiting so suppression counts remain stable.
/// The full diagnostic participates in equality: sharing a primary location
/// does not make two different candidate explanations interchangeable.
pub fn build_diagnostic_report<T: DiagnosticReportItem>(
    diagnostics: &[T],
    config: DiagnosticReportConfig,
) -> DiagnosticReport<'_, T> {
    let mut entries = diagnostics.iter().collect::<Vec<_>>();
    entries.sort_by(|left, right| compare_report_items(*left, *right));

    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let mut suppressed_duplicates = 0;
    let mut suppressed_by_limit = 0;
    for entry in entries {
        let key = DiagnosticDedupeKey::from_item(entry);
        if !seen.insert(key) {
            suppressed_duplicates += 1;
            continue;
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DiagnosticDedupeKey<'a> {
    path: Option<&'a str>,
    diagnostic: &'a Diagnostic,
}

impl<'a> DiagnosticDedupeKey<'a> {
    fn from_item<T: DiagnosticReportItem>(item: &'a T) -> Self {
        Self {
            path: item.report_path(),
            diagnostic: item.report_diagnostic(),
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

    /// Creates a code handle backed by the registered code definition.
    pub fn registered(code: codes::DiagnosticCodeDef) -> Self {
        Self {
            code: code.code.to_string(),
            registered: true,
        }
    }

    /// Returns the stable code text.
    pub fn as_str(&self) -> &str {
        &self.code
    }

    /// Reports whether this handle came from the registry.
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

/// Renders one diagnostic using source labels and explanatory sections.
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
        let source = "let mut x = 1;\nprint(x);\n";
        let print_start = source.find("print").expect("print statement");
        let diagnostic = Diagnostic::user_error(codes::PARSE, "expected `;` after binding")
            .primary(
                Span::new(print_start, print_start + "print".len()),
                "statement starts here",
            )
            .note("parser was recovering after a missing token")
            .help("insert `;` before this statement")
            .finish();
        let rendered = render_diagnostic("main.nia", source, &diagnostic);
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
    fn report_preserves_distinct_secondary_context() {
        let diagnostics = vec![
            Diagnostic::user_error(codes::TYPE_CHECK, "ambiguous candidate")
                .primary(Span::new(10, 11), "ambiguous here")
                .secondary(Span::new(20, 21), "first candidate")
                .finish(),
            Diagnostic::user_error(codes::TYPE_CHECK, "ambiguous candidate")
                .primary(Span::new(10, 11), "ambiguous here")
                .secondary(Span::new(30, 31), "second candidate")
                .finish(),
        ];

        let report = build_diagnostic_report(&diagnostics, DiagnosticReportConfig::default());

        assert_eq!(report.entries().len(), 2);
        assert_eq!(report.suppressed_duplicates(), 0);
    }

    #[test]
    fn registered_codes_are_unique_and_well_formed() {
        let mut seen = Vec::new();
        for code in codes::ALL {
            assert_eq!(code.code.len(), 5, "{code:?}");
            let mut chars = code.code.chars();
            let prefix = chars.next().expect("code prefix");
            assert!(matches!(prefix, 'E' | 'W' | 'I'), "{code:?}");
            assert!(chars.all(|ch| ch.is_ascii_digit()), "{code:?}");
            assert_eq!(
                prefix == 'W',
                code.severity == Severity::Warning,
                "{code:?}"
            );
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
