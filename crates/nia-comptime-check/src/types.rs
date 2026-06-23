use std::{collections::HashMap, fmt};

use nia_ast::Expr;
use nia_comptime_ir::{
    ResolvedComptimeModule, ResolvedComptimePattern, ResolvedComptimePatternKind,
};
use nia_defs::{DefCollection, DefId};
use nia_diagnostic::Diagnostic;
use nia_ids::{GlobalConstExprId, GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_item_signatures::{ItemSignatures, ProgramTraitImplSignature};
use nia_item_tree::ActiveModuleItemTree;
use nia_local_resolve::LocalResolution;
use nia_sema_ir::SemanticUseTable;
use nia_source::SourcePath;
use nia_target_config::TargetConfig;
use nia_ty::{TyInterner, import_type_into};
use nia_type_lower::TypeLowering;
use nia_value_resolve::ValueResolution;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComptimeCheck {
    pub interner: TyInterner,
    pub values: HashMap<ComptimeKey, ComptimeValue>,
    pub typed_values: HashMap<ComptimeKey, TypedComptimeValue>,
    pub enum_values: HashMap<DefId, ComptimeValue>,
    pub typed_enum_values: HashMap<DefId, TypedComptimeValue>,
    pub array_lengths: HashMap<GlobalConstExprId, u64>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedComptimeValue {
    pub value: ComptimeValue,
    pub ty: ComptimeValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComptimeValueType {
    Runtime(InternedTyId),
    Int,
    Bool,
    String,
    Array {
        elem: Box<ComptimeValueType>,
        len: Option<u64>,
    },
    Struct(Vec<ComptimeValueFieldType>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeValueFieldType {
    pub name: String,
    pub ty: ComptimeValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComptimeArmType {
    Value(ComptimeValueType),
    ControlFlow,
}

impl ComptimeValueType {
    pub fn runtime(&self) -> Option<InternedTyId> {
        match self {
            Self::Runtime(ty) => Some(*ty),
            Self::Int | Self::Bool | Self::String | Self::Array { .. } | Self::Struct(_) => None,
        }
    }

    pub fn structural_field(&self, name: &str) -> Option<&ComptimeValueType> {
        let Self::Struct(fields) = self else {
            return None;
        };
        fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.ty)
    }

    pub fn array_elem(&self) -> Option<(&ComptimeValueType, Option<u64>)> {
        let Self::Array { elem, len } = self else {
            return None;
        };
        Some((elem, *len))
    }
}

pub(crate) fn resolved_pattern_local_id(pattern: &ResolvedComptimePattern) -> Option<LocalId> {
    match pattern.kind() {
        ResolvedComptimePatternKind::Bind { local_id, .. } => Some(*local_id),
        ResolvedComptimePatternKind::OptionalSome { pattern, .. }
        | ResolvedComptimePatternKind::ErrorOk { pattern, .. }
        | ResolvedComptimePatternKind::ErrorErr { pattern, .. } => {
            resolved_pattern_local_id(pattern)
        }
        ResolvedComptimePatternKind::Wildcard { .. }
        | ResolvedComptimePatternKind::OptionalNull { .. }
        | ResolvedComptimePatternKind::Expr(_)
        | ResolvedComptimePatternKind::Range { .. } => None,
    }
}

pub fn import_comptime_value_type(
    source: &TyInterner,
    target: &mut TyInterner,
    ty: ComptimeValueType,
) -> Option<ComptimeValueType> {
    match ty {
        ComptimeValueType::Runtime(ty) => Some(ComptimeValueType::Runtime(import_type_into(
            target, source, ty,
        ))),
        ComptimeValueType::Array { elem, len } => Some(ComptimeValueType::Array {
            elem: Box::new(import_comptime_value_type(source, target, *elem)?),
            len,
        }),
        ComptimeValueType::Struct(fields) => fields
            .into_iter()
            .map(|field| {
                Some(ComptimeValueFieldType {
                    name: field.name,
                    ty: import_comptime_value_type(source, target, field.ty)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .map(ComptimeValueType::Struct),
        ComptimeValueType::Int => Some(ComptimeValueType::Int),
        ComptimeValueType::Bool => Some(ComptimeValueType::Bool),
        ComptimeValueType::String => Some(ComptimeValueType::String),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComptimeKey {
    Global(GlobalDefId),
    Local(LocalId),
}

pub use nia_comptime_engine::ComptimeValue;

#[derive(Debug, Clone, Copy)]
pub struct ComptimeInput<'a> {
    pub module: &'a ResolvedComptimeModule,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub signatures: &'a ItemSignatures,
    pub interner: &'a TyInterner,
    pub normalized: &'a HashMap<nia_ids::InternedTyId, nia_ids::InternedTyId>,
    pub target: &'a TargetConfig,
    pub source_path: &'a SourcePath,
    pub program: ComptimeProgramContext<'a>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComptimeModuleLowering {
    pub module: ResolvedComptimeModule,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub struct ComptimeModuleInput<'a> {
    pub active_item_tree: &'a ActiveModuleItemTree,
    pub defs: &'a DefCollection,
    pub values: &'a ValueResolution,
    pub locals: &'a LocalResolution,
    pub semantic_uses: &'a SemanticUseTable,
    pub const_exprs: &'a HashMap<GlobalConstExprId, Expr>,
    pub source_path: &'a SourcePath,
}

#[derive(Clone, Copy)]
pub struct ComptimeProgramContext<'a> {
    pub module: Option<&'a dyn Fn(ModuleId) -> Option<ResolvedComptimeModule>>,
    pub source_path: Option<&'a dyn Fn(ModuleId) -> Option<SourcePath>>,
    pub defs: Option<&'a dyn Fn(ModuleId) -> Option<DefCollection>>,
    pub type_lowerings: Option<&'a HashMap<ModuleId, TypeLowering>>,
    pub type_normalizations: Option<&'a HashMap<ModuleId, nia_type_normalize::TypeNormalization>>,
    pub signatures: Option<&'a HashMap<ModuleId, ItemSignatures>>,
    pub trait_impls: &'a [ProgramTraitImplSignature],
}

impl fmt::Debug for ComptimeProgramContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComptimeProgramContext")
            .field("module", &self.module.is_some())
            .field("source_path", &self.source_path.is_some())
            .field("defs", &self.defs.is_some())
            .field("type_lowerings", &self.type_lowerings.is_some())
            .field("type_normalizations", &self.type_normalizations.is_some())
            .field("signatures", &self.signatures.is_some())
            .field("trait_impls", &self.trait_impls.len())
            .finish()
    }
}

impl<'a> ComptimeProgramContext<'a> {
    pub fn empty() -> Self {
        Self {
            module: None,
            source_path: None,
            defs: None,
            type_lowerings: None,
            type_normalizations: None,
            signatures: None,
            trait_impls: &[],
        }
    }
}
