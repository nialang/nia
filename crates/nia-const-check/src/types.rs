use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use nia_ast::Expr;
use nia_const_ir::{
    ResolvedConstExpr, ResolvedConstModule, ResolvedConstPattern, ResolvedConstPatternKind,
};
use nia_defs::{DefCollection, DefId, VisibleExtensionMethods};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_item_signatures::{ItemSignatures, ProgramTraitImplSignature};
use nia_item_tree::ActiveModuleItemTree;
use nia_local_resolve::LocalResolution;
use nia_provider_summary::ProviderDemand;
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_symbol_table::SymbolTable;
use nia_target::TargetConfig;
use nia_type_lower::TypeLowering;
use nia_value_resolve::ValueResolution;

#[derive(Debug, Clone, PartialEq, Default)]
/// Complete compile-time result for one module.
///
/// The maps are shared because later compiler queries frequently need only one
/// view and should not clone all evaluated aggregate values.
pub struct ConstCheck {
    /// Evaluated module and local initializer values.
    pub values: Arc<HashMap<ConstKey, ConstValue>>,
    /// Scalar local constants whose value is supplied by each generic instance.
    pub symbolic_locals: Arc<HashMap<LocalId, nia_ty::ConstGenericArg>>,
    /// Initializers whose evaluation failed. Each failure was reported once,
    /// at its root; a use of a failed constant reports nothing further.
    pub failed: Arc<HashSet<ConstKey>>,
    /// Initializer values paired with inferred runtime types.
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    /// Evaluated enum discriminants.
    pub enum_values: Arc<HashMap<DefId, ConstValue>>,
    /// Enum discriminants paired with inferred runtime types.
    pub typed_enum_values: Arc<HashMap<DefId, TypedConstValue>>,
    /// Array lengths required by lowered types.
    pub array_lengths: Arc<HashMap<GlobalConstExprId, u64>>,
    /// Const expressions written in types whose evaluation failed; each
    /// failure was reported at its root.
    pub failed_const_exprs: Arc<HashSet<GlobalConstExprId>>,
    /// Terms of the generic const expressions written in types, for
    /// check-time type normalization.
    pub symbolic_const_exprs: Arc<HashMap<GlobalConstExprId, nia_ty::ConstGenericArg>>,
    /// Cross-module semantic facts required to complete const analysis.
    pub provider_demands: Arc<HashSet<ProviderDemand>>,
    /// Diagnostics accumulated by all phases.
    pub diagnostics: ConstDiagnostics,
}

impl ConstCheck {
    /// What const analysis found for the array length `id`, for layout.
    pub fn array_length(&self, id: &GlobalConstExprId) -> Option<nia_layout::ArrayLengthValue> {
        array_length_value(&self.array_lengths, &self.failed_const_exprs, id)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached output of the array-length phase.
pub struct ConstArrayLengths {
    /// Computed array lengths keyed by resolved expression identity.
    pub values: Arc<HashMap<GlobalConstExprId, u64>>,
    /// Const expressions written in types whose evaluation failed; each
    /// failure was reported at its root.
    pub failed: Arc<HashSet<GlobalConstExprId>>,
    /// Const expressions written in types that depend on generic parameters,
    /// array lengths and nominal const arguments alike, as the terms each
    /// instance evaluates.
    pub symbolic: Arc<HashMap<GlobalConstExprId, nia_ty::ConstGenericArg>>,
    /// Cross-module semantic facts required by this phase.
    pub provider_demands: Arc<HashSet<ProviderDemand>>,
    /// Diagnostics emitted while computing lengths.
    pub diagnostics: ConstDiagnostics,
}

impl ConstArrayLengths {
    /// The value of a const expression written in a type, for type
    /// normalization: a closed length, or the term of one that depends on
    /// generic parameters.
    pub fn resolve(&self, id: &GlobalConstExprId) -> Option<nia_ty::ConstGenericValue> {
        resolve_type_const_expr(&self.values, &self.symbolic, id)
    }

    /// What const analysis found for the array length `id`, for layout.
    pub fn length(&self, id: &GlobalConstExprId) -> Option<nia_layout::ArrayLengthValue> {
        array_length_value(&self.values, &self.failed, id)
    }
}

/// What const analysis found for an array length written as a const
/// expression, from the tables it publishes. Every phase that computes
/// layouts reads lengths through this, so a length whose failure was
/// reported is never reported again as unevaluated.
pub fn array_length_value(
    values: &HashMap<GlobalConstExprId, u64>,
    failed: &HashSet<GlobalConstExprId>,
    id: &GlobalConstExprId,
) -> Option<nia_layout::ArrayLengthValue> {
    match values.get(id) {
        Some(value) => Some(nia_layout::ArrayLengthValue::Known(*value)),
        None => failed
            .contains(id)
            .then_some(nia_layout::ArrayLengthValue::Failed),
    }
}

/// The value of a const expression written in a type, from the two tables
/// const analysis publishes. Every phase that normalizes types reads sites
/// through this.
pub fn resolve_type_const_expr(
    values: &HashMap<GlobalConstExprId, u64>,
    symbolic: &HashMap<GlobalConstExprId, nia_ty::ConstGenericArg>,
    id: &GlobalConstExprId,
) -> Option<nia_ty::ConstGenericValue> {
    values
        .get(id)
        .map(|value| nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned((*value).into())))
        .or_else(|| symbolic.get(id).map(|arg| arg.value.clone()))
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached output of enum discriminant evaluation.
pub struct ConstEnumValues {
    /// Computed enum discriminants keyed by definition identity.
    pub values: Arc<HashMap<DefId, ConstValue>>,
    /// Discriminants paired with inferred runtime types.
    pub typed_values: Arc<HashMap<DefId, TypedConstValue>>,
    /// Cross-module semantic facts required through this phase.
    pub provider_demands: Arc<HashSet<ProviderDemand>>,
    /// Diagnostics emitted by this and earlier phases.
    pub diagnostics: ConstDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached values produced by initializer evaluation.
pub struct ConstValues {
    /// Computed initializer values keyed by global or local identity.
    pub values: Arc<HashMap<ConstKey, ConstValue>>,
    /// Scalar local constants deferred until their generic instance is known.
    pub symbolic_locals: Arc<HashMap<LocalId, nia_ty::ConstGenericArg>>,
    /// Initializers whose evaluation failed and was reported.
    pub failed: Arc<HashSet<ConstKey>>,
    /// Initializer values paired with inferred runtime types.
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    /// Cross-module semantic facts required through this phase.
    pub provider_demands: Arc<HashSet<ProviderDemand>>,
    /// Diagnostics emitted by this and earlier phases.
    pub diagnostics: ConstDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Runtime type facts attached to evaluated const values.
pub struct ConstTypedFacts {
    /// Runtime type facts keyed by initializer identity.
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    /// Cross-module semantic facts required through this phase.
    pub provider_demands: Arc<HashSet<ProviderDemand>>,
    /// Diagnostics emitted by this and earlier phases.
    pub diagnostics: ConstDiagnostics,
}

/// Diagnostics of const analysis in the order found, each filed under what
/// it was found checking.
///
/// Body checking owns the static rules of every initializer and every
/// function body it checks, and const analysis runs before it. When body
/// checking rejects an initializer, const analysis checking that initializer
/// statically only reports the same error again; what evaluating it finds,
/// such as a division by zero, is independent and still reported. When body
/// checking rejects a function body, executing it has no meaning, so
/// nothing found executing it is reported. A caller with body checking's
/// verdict reports [`ConstDiagnostics::reported`]. Every phase carries the
/// filing, so no diagnostic is lost or reported against the verdict.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConstDiagnostics {
    entries: Vec<(Filing, Diagnostic)>,
}

/// Where a const analysis diagnostic belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filing {
    /// Not yet filed: found outside any initializer, or by an initializer
    /// check still in progress.
    Open,
    /// Found statically checking this initializer.
    Initializer(ConstKey),
    /// Found evaluating an initializer.
    Evaluation,
    /// Found executing this function's body.
    Function(GlobalDefId),
}

/// What a static const error belongs to: an initializer or a function body
/// that body checking also checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticOwner {
    Initializer(ConstKey),
    Function(GlobalDefId),
}

/// The initializers and function bodies body checking rejected: it
/// reported their static errors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodyCheckRejections {
    /// Constant initializers whose checking reported an error.
    pub initializers: HashSet<ConstKey>,
    /// Functions whose body checking reported an error.
    pub functions: HashSet<GlobalDefId>,
}

impl BodyCheckRejections {
    /// Adds the rejections of another body check.
    pub fn extend(&mut self, other: &Self) {
        self.initializers.extend(other.initializers.iter().copied());
        self.functions.extend(other.functions.iter().copied());
    }
}

impl ConstDiagnostics {
    /// Adds a diagnostic, filed by the initializer check in progress if any.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.entries.push((Filing::Open, diagnostic));
    }

    /// Adds a diagnostic found executing `function`'s body.
    pub(crate) fn push_for_function(&mut self, diagnostic: Diagnostic, function: GlobalDefId) {
        self.entries.push((Filing::Function(function), diagnostic));
    }

    /// Adds a static error of `owner`, found lowering it.
    pub(crate) fn push_owned(&mut self, diagnostic: Diagnostic, owner: DiagnosticOwner) {
        let filing = match owner {
            DiagnosticOwner::Initializer(key) => Filing::Initializer(key),
            DiagnosticOwner::Function(function) => Filing::Function(function),
        };
        self.entries.push((filing, diagnostic));
    }

    /// Adds `other`'s diagnostics, keeping their filing.
    pub fn append_from(&mut self, other: &Self) {
        self.entries.extend(other.entries.iter().cloned());
    }

    /// Number of diagnostics.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no diagnostics.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every diagnostic, whatever body checking's verdict. For callers
    /// without that verdict, such as tests of const analysis alone.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.entries.iter().map(|(_, diagnostic)| diagnostic)
    }

    /// The diagnostics to report given what body checking `rejected`.
    pub fn reported<'a>(
        &'a self,
        rejected: &'a BodyCheckRejections,
    ) -> impl Iterator<Item = &'a Diagnostic> + 'a {
        self.entries
            .iter()
            .filter(|(filing, _)| match filing {
                Filing::Initializer(key) => !rejected.initializers.contains(key),
                Filing::Function(function) => !rejected.functions.contains(function),
                Filing::Open | Filing::Evaluation => true,
            })
            .map(|(_, diagnostic)| diagnostic)
    }

    /// The modules defining a function whose execution found a diagnostic.
    /// Their body checks' verdicts decide whether those are reported.
    pub fn function_modules(&self) -> HashSet<ModuleId> {
        self.entries
            .iter()
            .filter_map(|(filing, _)| match filing {
                Filing::Function(function) => Some(function.module_id),
                _ => None,
            })
            .collect()
    }

    /// Whether executing `function`'s body found a diagnostic.
    pub fn found_in_function(&self, function: GlobalDefId) -> bool {
        self.entries
            .iter()
            .any(|(filing, _)| *filing == Filing::Function(function))
    }

    /// Files the open diagnostics from `start` on as found executing
    /// `function`'s body.
    pub(crate) fn file_function(&mut self, start: usize, function: GlobalDefId) {
        self.file_open(start, Filing::Function(function));
    }

    /// Files the open diagnostics from `start` on as found statically
    /// checking `key`'s initializer.
    pub(crate) fn file_static(&mut self, start: usize, key: ConstKey) {
        self.file_open(start, Filing::Initializer(key));
    }

    /// Files the open diagnostics from `start` on as found evaluating an
    /// initializer.
    pub(crate) fn file_evaluation(&mut self, start: usize) {
        self.file_open(start, Filing::Evaluation);
    }

    /// Files the open diagnostics from `start` on; those a nested
    /// initializer check filed keep their filing.
    fn file_open(&mut self, start: usize, filing: Filing) {
        // A probe may have discarded diagnostics below `start`.
        let start = start.min(self.entries.len());
        for (entry, _) in &mut self.entries[start..] {
            if *entry == Filing::Open {
                *entry = filing;
            }
        }
    }

    /// Gives the diagnostics from `start` on that name no source file the
    /// file `path`, where they were found.
    pub(crate) fn locate_unlocated_since(&mut self, start: usize, path: &str) {
        let start = start.min(self.entries.len());
        for (_, diagnostic) in &mut self.entries[start..] {
            if diagnostic.source_path.is_none() {
                diagnostic.source_path = Some(path.to_owned());
            }
        }
    }

    /// Whether an error was found from `start` on.
    pub(crate) fn has_errors_since(&self, start: usize) -> bool {
        self.entries[start..]
            .iter()
            .any(|(_, diagnostic)| diagnostic.severity() == nia_diagnostic::Severity::Error)
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.entries.truncate(len);
    }

    pub(crate) fn split_off(&mut self, at: usize) -> Self {
        Self {
            entries: self.entries.split_off(at),
        }
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        self.entries.append(&mut other.entries);
    }

    /// Every diagnostic, as [`ConstDiagnostics::iter`] gives them.
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.entries
            .into_iter()
            .map(|(_, diagnostic)| diagnostic)
            .collect()
    }
}

impl Extend<Diagnostic> for ConstDiagnostics {
    fn extend<I: IntoIterator<Item = Diagnostic>>(&mut self, diagnostics: I) {
        self.entries.extend(
            diagnostics
                .into_iter()
                .map(|diagnostic| (Filing::Open, diagnostic)),
        );
    }
}

impl<'a> Extend<&'a Diagnostic> for ConstDiagnostics {
    fn extend<I: IntoIterator<Item = &'a Diagnostic>>(&mut self, diagnostics: I) {
        self.extend(diagnostics.into_iter().cloned());
    }
}

#[derive(Debug, Clone, PartialEq)]
/// An evaluated value paired with the type governing its runtime layout.
pub struct TypedConstValue {
    /// Evaluated compile-time value.
    pub value: ConstValue,
    /// Type information governing its runtime representation.
    pub ty: ConstValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Type information available while checking a compile-time value.
///
/// `Runtime` is a fully interned Nia type. The remaining variants represent
/// literals and aggregates whose runtime representation is still driven by an
/// expected type; preserving that distinction prevents premature defaulting.
pub enum ConstValueType {
    /// A fully lowered runtime type.
    Runtime(InternedTyId),
    /// An integer literal whose width is supplied by an expected type.
    Int,
    /// A boolean literal.
    Bool,
    /// A string literal awaiting its expected representation.
    String,
    /// An aggregate with an optional statically known length.
    Array {
        /// Element type inferred for the aggregate.
        elem: Box<ConstValueType>,
        /// Known element count, when it is available.
        len: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConstArmType {
    Value(ConstValueType),
    ControlFlow,
}

impl ConstValueType {
    /// Returns the interned runtime type, if this value has one.
    pub fn runtime(&self) -> Option<InternedTyId> {
        match self {
            Self::Runtime(ty) => Some(*ty),
            Self::Int | Self::Bool | Self::String | Self::Array { .. } => None,
        }
    }

    /// Returns the element type and known length for an array value.
    pub fn array_elem(&self) -> Option<(&ConstValueType, Option<u64>)> {
        let Self::Array { elem, len } = self else {
            return None;
        };
        Some((elem, *len))
    }
}

pub(crate) fn resolved_pattern_local_id(pattern: &ResolvedConstPattern) -> Option<LocalId> {
    match pattern.kind() {
        ResolvedConstPatternKind::Bind { local_id, .. } => Some(*local_id),
        ResolvedConstPatternKind::Pointer { pattern, .. }
        | ResolvedConstPatternKind::MutPointer { pattern, .. }
        | ResolvedConstPatternKind::OptionalSome { pattern, .. }
        | ResolvedConstPatternKind::ErrorOk { pattern, .. }
        | ResolvedConstPatternKind::ErrorErr { pattern, .. } => resolved_pattern_local_id(pattern),
        ResolvedConstPatternKind::Tuple { patterns, .. } => {
            patterns.iter().find_map(resolved_pattern_local_id)
        }
        ResolvedConstPatternKind::EnumVariant { fields, .. } => match fields {
            nia_const_ir::ConstEnumPatternFields::Tuple(fields) => {
                fields.iter().find_map(resolved_pattern_local_id)
            }
            nia_const_ir::ConstEnumPatternFields::Named { fields, .. } => fields
                .iter()
                .find_map(|field| resolved_pattern_local_id(&field.pattern)),
        },
        ResolvedConstPatternKind::Struct { fields, .. } => fields
            .iter()
            .find_map(|field| resolved_pattern_local_id(&field.pattern)),
        ResolvedConstPatternKind::Wildcard { .. }
        | ResolvedConstPatternKind::OptionalNull { .. }
        | ResolvedConstPatternKind::Expr(_)
        | ResolvedConstPatternKind::Range { .. } => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Identity of a module-level or local const initializer.
pub enum ConstKey {
    /// Module-level definition identity.
    Global(GlobalDefId),
    /// Function-local binding identity.
    Local(LocalId),
}

pub use nia_const_eval::{ConstEnumPayload, ConstPointerValue, ConstValue};

#[derive(Debug, Clone, Copy)]
/// Borrowed semantic databases required for whole-module const analysis.
pub struct ConstInput<'a> {
    /// Resolved const IR for the active module.
    pub module: &'a ResolvedConstModule,
    /// Definition identities used by analysis.
    pub defs: &'a DefCollection,
    /// Resolved value identities.
    pub values: &'a ValueResolution,
    /// Resolved local identities.
    pub locals: &'a LocalResolution,
    /// Semantic use table for resolved references.
    pub semantic_uses: &'a SemanticUseTable,
    /// Symbol table used for diagnostics and names.
    pub symbols: &'a SymbolTable,
    /// Lowered runtime type information.
    pub lowered: &'a TypeLowering,
    /// Item and function signatures.
    pub signatures: &'a ItemSignatures,
    /// Interned type store.
    pub type_store: &'a nia_ty::TypeStore,
    /// Normalized type relations.
    pub normalization: &'a nia_type_normalize::TypeNormalization,
    /// Target layout configuration.
    pub target: &'a TargetConfig,
    /// Source path for diagnostics.
    pub source_path: &'a SourcePath,
    /// Exact source text used to materialize source coordinates.
    pub source_text: &'a str,
    /// Optional cross-module providers.
    pub program: ConstProgramContext<'a>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Resolved const IR and diagnostics produced by const lowering.
pub struct ConstModuleLowering {
    /// Lowered resolved const module.
    pub module: Arc<ResolvedConstModule>,
    /// Diagnostics emitted during lowering, each lowering error filed under
    /// the initializer or function it belongs to.
    pub diagnostics: ConstDiagnostics,
}

#[derive(Clone, Copy)]
/// Inputs needed to lower active module items into resolved const IR.
pub struct ConstModuleInput<'a> {
    /// Session type store used to recover nominal identities while propagating
    /// expected types through const-function locals and patterns.
    pub type_store: &'a nia_ty::TypeStore,
    /// Lazily loads definition scopes for enums owned by imported modules.
    pub defs_for_module:
        Option<&'a dyn Fn(nia_ids::ModuleId) -> Option<std::sync::Arc<DefCollection>>>,
    /// Lazily loads item signatures for definitions owned by imported modules.
    ///
    /// Const lowering must use the same declaration facts as ordinary body
    /// checking when contextualizing omitted aggregate constructors. Keeping
    /// this as a provider avoids copying foreign signatures into the active
    /// module and preserves the program's separated compilation boundary.
    pub signatures_for_module:
        Option<&'a dyn Fn(nia_ids::ModuleId) -> Option<std::sync::Arc<ItemSignatures>>>,
    /// Lazily loads visible extension methods for the active module.
    pub visible_extensions_for_module:
        Option<&'a dyn Fn(nia_ids::ModuleId) -> Option<std::sync::Arc<VisibleExtensionMethods>>>,
    /// Active item tree selecting the module's current revision.
    pub active_item_tree: &'a ActiveModuleItemTree,
    /// Definition identities used during lowering.
    pub defs: &'a DefCollection,
    /// Item and function signatures.
    pub signatures: &'a ItemSignatures,
    /// Resolved value identities.
    pub values: &'a ValueResolution,
    /// Resolved local identities.
    pub locals: &'a LocalResolution,
    /// Semantic use table for lowered references.
    pub semantic_uses: &'a SemanticUseTable,
    /// Symbol table used by lowering diagnostics.
    pub symbols: &'a SymbolTable,
    /// Parsed const expressions keyed by global expression identity.
    pub const_exprs: &'a HashMap<GlobalConstExprId, Expr>,
    /// Source path for lowering diagnostics.
    pub source_path: &'a SourcePath,
}

#[derive(Clone, Copy)]
/// Lazy cross-module queries available during const analysis.
///
/// Keeping these providers optional lets isolated module tests and early
/// compiler stages use the same analyzer. Missing providers yield ordinary
/// unavailable-context results; they must never cause lookup to fall back to a
/// different module's identities.
pub struct ConstProgramContext<'a> {
    /// Lazily loads another module's resolved const IR.
    pub module: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ResolvedConstModule>>>,
    /// Lazily loads another module's source path.
    pub source_path: Option<&'a dyn Fn(ModuleId) -> Option<SourcePath>>,
    /// Resolves another module's stable package identity.
    pub module_path: Option<&'a dyn Fn(ModuleId) -> Option<nia_imports::ModulePath>>,
    /// Lazily loads another module's exact source text.
    pub source_text: Option<&'a dyn Fn(ModuleId) -> Option<Arc<str>>>,
    /// Lazily loads another module's definitions.
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
    /// Lazily loads another module's type normalization.
    pub type_normalizations:
        Option<&'a dyn Fn(ModuleId) -> Option<Arc<nia_type_normalize::TypeNormalization>>>,
    /// Lazily loads another module's signatures.
    pub signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    /// Resolves callable declarations by global identity, including methods.
    /// This must use the same declaration lookup as ordinary body checking;
    /// a type-only or ordinary-function-only module product is insufficient.
    pub function_signature:
        Option<&'a dyn Fn(GlobalDefId) -> Option<nia_item_signatures::FunctionSignature>>,
    /// Lazily loads value signatures for cross-module access.
    pub value_signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    /// Lazily loads cached const values for another module.
    pub const_values: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>>,
    /// Resolves a global initializer by stable identity.
    pub global_initializer: Option<&'a dyn Fn(GlobalDefId) -> Option<ResolvedConstExpr>>,
    /// Identifies enum definitions for value interpretation.
    pub program_fieldless_enum: Option<&'a dyn Fn(GlobalDefId) -> bool>,
    /// Lazily loads the complete trait witness scope for an execution module.
    /// These facts are already filtered by trait visibility and provider
    /// closure; consumers must not intersect them with a method index.
    pub trait_impls_for_module:
        Option<&'a dyn Fn(ModuleId) -> Option<Vec<ProgramTraitImplSignature>>>,
    /// Lazily loads visible extension methods for another module.
    pub visible_extensions: Option<&'a dyn Fn(ModuleId) -> Option<Arc<VisibleExtensionMethods>>>,
}

impl fmt::Debug for ConstProgramContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConstProgramContext")
            .field("module", &self.module.is_some())
            .field("source_path", &self.source_path.is_some())
            .field("module_path", &self.module_path.is_some())
            .field("source_text", &self.source_text.is_some())
            .field("defs", &self.defs.is_some())
            .field("type_normalizations", &self.type_normalizations.is_some())
            .field("signatures", &self.signatures.is_some())
            .field("function_signature", &self.function_signature.is_some())
            .field("value_signatures", &self.value_signatures.is_some())
            .field("const_values", &self.const_values.is_some())
            .field("global_initializer", &self.global_initializer.is_some())
            .field(
                "program_fieldless_enum",
                &self.program_fieldless_enum.is_some(),
            )
            .field(
                "trait_impls_for_module",
                &self.trait_impls_for_module.is_some(),
            )
            .finish()
    }
}

impl<'a> ConstProgramContext<'a> {
    /// Creates a context with no cross-module providers.
    pub fn empty() -> Self {
        Self {
            module: None,
            source_path: None,
            module_path: None,
            source_text: None,
            defs: None,
            type_normalizations: None,
            signatures: None,
            function_signature: None,
            value_signatures: None,
            const_values: None,
            global_initializer: None,
            program_fieldless_enum: None,
            trait_impls_for_module: None,
            visible_extensions: None,
        }
    }
}
