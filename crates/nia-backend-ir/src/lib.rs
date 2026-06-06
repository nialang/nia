// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::ReceiverKind;
use nia_comptime_check::ComptimeCheck;
use nia_function_ir::FunctionBody;
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_layout::{Layouts, StructLayout, StructLayoutKey, TypeLayout};
use nia_span::Span;
use nia_static_ir::StaticInit;
use nia_ty::{TraitId, TyInterner};

#[derive(Debug, Clone, PartialEq)]
pub struct BackendProgram {
    pub modules: Vec<BackendModule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendModule {
    pub id: ModuleId,
    pub name: String,
    pub interner: TyInterner,
    pub comptime: ComptimeCheck,
    pub layouts: BackendLayouts,
    pub structs: Vec<BackendStruct>,
    pub unions: Vec<BackendUnion>,
    pub struct_instances: Vec<BackendStructInstance>,
    pub union_instances: Vec<BackendUnionInstance>,
    pub enums: Vec<BackendEnum>,
    pub globals: Vec<BackendGlobal>,
    pub functions: Vec<BackendFunction>,
    pub function_instances: Vec<BackendFunctionInstance>,
    pub trait_object_vtables: Vec<BackendTraitObjectVtable>,
    pub generic_instantiations: Vec<BackendGenericInstantiation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendLayouts {
    pub target: nia_layout::TargetDataLayout,
    pub types: Vec<(InternedTyId, TypeLayout)>,
    pub structs: Vec<(GlobalDefId, StructLayout)>,
    pub unions: Vec<(GlobalDefId, StructLayout)>,
    pub struct_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
    pub union_instances: Vec<(BackendStructInstanceKey, StructLayout)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendStructInstanceKey {
    pub def_id: GlobalDefId,
    pub args: Vec<InternedTyId>,
}

impl BackendLayouts {
    pub fn from_module_layouts(module_id: ModuleId, layouts: &Layouts) -> Self {
        Self {
            target: layouts.target,
            types: layouts
                .types
                .iter()
                .map(|(ty, layout)| (*ty, layout.clone()))
                .collect(),
            structs: layouts
                .structs
                .iter()
                .map(|(def_id, layout)| {
                    (
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                        layout.clone(),
                    )
                })
                .collect(),
            unions: layouts
                .unions
                .iter()
                .map(|(def_id, layout)| {
                    (
                        GlobalDefId {
                            module_id,
                            def_id: *def_id,
                        },
                        layout.clone(),
                    )
                })
                .collect(),
            struct_instances: layouts
                .struct_instances
                .iter()
                .map(|(key, layout)| {
                    (
                        BackendStructInstanceKey::from_module_key(module_id, key),
                        layout.clone(),
                    )
                })
                .collect(),
            union_instances: layouts
                .union_instances
                .iter()
                .map(|(key, layout)| {
                    (
                        BackendStructInstanceKey::from_module_key(module_id, key),
                        layout.clone(),
                    )
                })
                .collect(),
        }
    }
}

impl BackendStructInstanceKey {
    pub fn from_module_key(module_id: ModuleId, key: &StructLayoutKey) -> Self {
        Self {
            def_id: GlobalDefId {
                module_id,
                def_id: key.def_id,
            },
            args: key.args.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendStruct {
    pub def_id: GlobalDefId,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnion {
    pub def_id: GlobalDefId,
    pub name: String,
    pub generics: Vec<String>,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendStructInstance {
    pub def_id: GlobalDefId,
    pub name: String,
    pub args: Vec<InternedTyId>,
    pub symbol: String,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendUnionInstance {
    pub def_id: GlobalDefId,
    pub name: String,
    pub args: Vec<InternedTyId>,
    pub symbol: String,
    pub fields: Vec<BackendField>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendField {
    pub def_id: GlobalDefId,
    pub name: String,
    pub ty: InternedTyId,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnum {
    pub def_id: GlobalDefId,
    pub name: String,
    pub backing_type: InternedTyId,
    pub variants: Vec<BackendEnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendEnumVariant {
    pub def_id: GlobalDefId,
    pub name: String,
    pub value: Option<i128>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendGlobal {
    pub def_id: GlobalDefId,
    pub name: String,
    pub ty: InternedTyId,
    pub is_let: bool,
    pub is_extern: bool,
    pub init: Option<StaticInit>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunction {
    pub def_id: GlobalDefId,
    pub name: String,
    pub generics: Vec<String>,
    pub params: Vec<BackendParam>,
    pub return_type: InternedTyId,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub attributes: Vec<BackendFunctionAttribute>,
    pub function_body: Option<FunctionBody>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendFunctionAttribute {
    Naked,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendFunctionInstance {
    pub def_id: GlobalDefId,
    pub name: String,
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
    pub symbol: String,
    pub params: Vec<BackendParam>,
    pub return_type: InternedTyId,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub attributes: Vec<BackendFunctionAttribute>,
    pub function_body: Option<FunctionBody>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendTraitObjectVtableKey {
    pub self_ty: InternedTyId,
    pub object_ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendTraitObjectVtable {
    pub key: BackendTraitObjectVtableKey,
    pub trait_id: TraitId,
    pub trait_args: Vec<InternedTyId>,
    pub entries: Vec<BackendTraitObjectVtableEntry>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendTraitObjectVtableEntry {
    pub trait_id: TraitId,
    pub method_id: GlobalDefId,
    pub method_name: String,
    pub slot: usize,
    pub function: BackendTraitObjectVtableFunction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendTraitObjectVtableFunction {
    Function(GlobalDefId),
    FunctionInstance {
        def_id: GlobalDefId,
        arg_module_id: ModuleId,
        args: Vec<InternedTyId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendGenericInstantiation {
    pub def_id: GlobalDefId,
    pub arg_module_id: ModuleId,
    pub args: Vec<InternedTyId>,
    pub span: Span,
    pub source_def_id: Option<GlobalDefId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackendParam {
    pub local_id: Option<LocalId>,
    pub name: Option<String>,
    pub receiver: Option<ReceiverKind>,
    pub passing_ty: InternedTyId,
    pub local_ty: InternedTyId,
    pub span: Span,
}
