use std::{collections::HashMap, fmt, sync::Arc};

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
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_symbol_table::SymbolTable;
use nia_target_config::TargetConfig;
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
    /// Initializer values paired with inferred runtime types.
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    /// Evaluated enum discriminants.
    pub enum_values: Arc<HashMap<DefId, ConstValue>>,
    /// Enum discriminants paired with inferred runtime types.
    pub typed_enum_values: Arc<HashMap<DefId, TypedConstValue>>,
    /// Array lengths required by lowered types.
    pub array_lengths: Arc<HashMap<GlobalConstExprId, u64>>,
    /// Diagnostics accumulated by all phases.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached output of the array-length phase.
pub struct ConstArrayLengths {
    /// Computed array lengths keyed by resolved expression identity.
    pub values: Arc<HashMap<GlobalConstExprId, u64>>,
    /// Diagnostics emitted while computing lengths.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached output of enum discriminant evaluation.
pub struct ConstEnumValues {
    /// Computed enum discriminants keyed by definition identity.
    pub values: Arc<HashMap<DefId, ConstValue>>,
    /// Discriminants paired with inferred runtime types.
    pub typed_values: Arc<HashMap<DefId, TypedConstValue>>,
    /// Diagnostics emitted while evaluating discriminants.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached values produced by initializer evaluation.
pub struct ConstValues {
    /// Computed initializer values keyed by global or local identity.
    pub values: Arc<HashMap<ConstKey, ConstValue>>,
    /// Initializer values paired with inferred runtime types.
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    /// Diagnostics emitted while evaluating initializers.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Runtime type facts attached to evaluated const values.
pub struct ConstTypedFacts {
    /// Runtime type facts keyed by initializer identity.
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    /// Diagnostics emitted while deriving runtime type facts.
    pub diagnostics: Vec<Diagnostic>,
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

pub use nia_const_eval::{ConstPointerValue, ConstValue};

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
    /// Optional cross-module providers.
    pub program: ConstProgramContext<'a>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Resolved const IR and diagnostics produced by const lowering.
pub struct ConstModuleLowering {
    /// Lowered resolved const module.
    pub module: Arc<ResolvedConstModule>,
    /// Diagnostics emitted during lowering.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
/// Inputs needed to lower active module items into resolved const IR.
pub struct ConstModuleInput<'a> {
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
    /// Lazily loads another module's definitions.
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
    /// Lazily loads another module's type normalization.
    pub type_normalizations:
        Option<&'a dyn Fn(ModuleId) -> Option<Arc<nia_type_normalize::TypeNormalization>>>,
    /// Lazily loads another module's signatures.
    pub signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    /// Lazily loads function signatures for cross-module calls.
    pub function_signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    /// Lazily loads value signatures for cross-module access.
    pub value_signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    /// Lazily loads cached const values for another module.
    pub const_values: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>>,
    /// Resolves a global initializer by stable identity.
    pub global_initializer: Option<&'a dyn Fn(GlobalDefId) -> Option<ResolvedConstExpr>>,
    /// Identifies enum definitions for value interpretation.
    pub program_is_enum: Option<&'a dyn Fn(GlobalDefId) -> bool>,
    /// Lazily loads trait implementations visible in another module.
    pub trait_impls_for_module:
        Option<&'a dyn Fn(ModuleId) -> Option<Vec<ProgramTraitImplSignature>>>,
    /// Lazily loads visible extension methods for another module.
    pub visible_extensions: Option<&'a dyn Fn(ModuleId) -> Option<VisibleExtensionMethods>>,
}

impl fmt::Debug for ConstProgramContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConstProgramContext")
            .field("module", &self.module.is_some())
            .field("source_path", &self.source_path.is_some())
            .field("defs", &self.defs.is_some())
            .field("type_normalizations", &self.type_normalizations.is_some())
            .field("signatures", &self.signatures.is_some())
            .field("function_signatures", &self.function_signatures.is_some())
            .field("value_signatures", &self.value_signatures.is_some())
            .field("const_values", &self.const_values.is_some())
            .field("global_initializer", &self.global_initializer.is_some())
            .field("program_is_enum", &self.program_is_enum.is_some())
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
            defs: None,
            type_normalizations: None,
            signatures: None,
            function_signatures: None,
            value_signatures: None,
            const_values: None,
            global_initializer: None,
            program_is_enum: None,
            trait_impls_for_module: None,
            visible_extensions: None,
        }
    }
}
