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
    pub values: Arc<HashMap<ConstKey, ConstValue>>,
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    pub enum_values: Arc<HashMap<DefId, ConstValue>>,
    pub typed_enum_values: Arc<HashMap<DefId, TypedConstValue>>,
    pub array_lengths: Arc<HashMap<GlobalConstExprId, u64>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached output of the array-length phase.
pub struct ConstArrayLengths {
    pub values: Arc<HashMap<GlobalConstExprId, u64>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached output of enum discriminant evaluation.
pub struct ConstEnumValues {
    pub values: Arc<HashMap<DefId, ConstValue>>,
    pub typed_values: Arc<HashMap<DefId, TypedConstValue>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Cached values produced by initializer evaluation.
pub struct ConstValues {
    pub values: Arc<HashMap<ConstKey, ConstValue>>,
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Runtime type facts attached to evaluated const values.
pub struct ConstTypedFacts {
    pub typed_values: Arc<HashMap<ConstKey, TypedConstValue>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
/// An evaluated value paired with the type governing its runtime layout.
pub struct TypedConstValue {
    pub value: ConstValue,
    pub ty: ConstValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Type information available while checking a compile-time value.
///
/// `Runtime` is a fully interned Nia type. The remaining variants represent
/// literals and aggregates whose runtime representation is still driven by an
/// expected type; preserving that distinction prevents premature defaulting.
pub enum ConstValueType {
    Runtime(InternedTyId),
    Int,
    Bool,
    String,
    Array {
        elem: Box<ConstValueType>,
        len: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConstArmType {
    Value(ConstValueType),
    ControlFlow,
}

impl ConstValueType {
    pub fn runtime(&self) -> Option<InternedTyId> {
        match self {
            Self::Runtime(ty) => Some(*ty),
            Self::Int | Self::Bool | Self::String | Self::Array { .. } => None,
        }
    }

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
    Global(GlobalDefId),
    Local(LocalId),
}

pub use nia_const_eval::{ConstPointerValue, ConstValue};

#[derive(Debug, Clone, Copy)]
/// Borrowed semantic databases required for whole-module const analysis.
pub struct ConstInput<'a> {
    pub module: &'a ResolvedConstModule,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub symbols: &'a SymbolTable,
    pub lowered: &'a TypeLowering,
    pub signatures: &'a ItemSignatures,
    pub type_store: &'a nia_ty::TypeStore,
    pub normalization: &'a nia_type_normalize::TypeNormalization,
    pub target: &'a TargetConfig,
    pub source_path: &'a SourcePath,
    pub program: ConstProgramContext<'a>,
}

#[derive(Debug, Clone, PartialEq, Default)]
/// Resolved const IR and diagnostics produced by const lowering.
pub struct ConstModuleLowering {
    pub module: Arc<ResolvedConstModule>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
/// Inputs needed to lower active module items into resolved const IR.
pub struct ConstModuleInput<'a> {
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub signatures: &'a ItemSignatures,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub symbols: &'a SymbolTable,
    pub const_exprs: &'a HashMap<GlobalConstExprId, Expr>,
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
    pub module: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ResolvedConstModule>>>,
    pub source_path: Option<&'a dyn Fn(ModuleId) -> Option<SourcePath>>,
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>>,
    pub type_normalizations:
        Option<&'a dyn Fn(ModuleId) -> Option<Arc<nia_type_normalize::TypeNormalization>>>,
    pub signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    pub function_signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    pub value_signatures: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ItemSignatures>>>,
    pub const_values: Option<&'a dyn Fn(ModuleId) -> Option<Arc<ConstValues>>>,
    pub global_initializer: Option<&'a dyn Fn(GlobalDefId) -> Option<ResolvedConstExpr>>,
    pub program_is_enum: Option<&'a dyn Fn(GlobalDefId) -> bool>,
    pub trait_impls_for_module:
        Option<&'a dyn Fn(ModuleId) -> Option<Vec<ProgramTraitImplSignature>>>,
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
