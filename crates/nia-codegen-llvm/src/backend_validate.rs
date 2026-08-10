// SPDX-License-Identifier: GPL-3.0-or-later
mod aggregates;
mod function_ir;
mod refs;
mod static_init;
mod types;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use crate::{declaration_membership::CodegenDeclarationMembership, program_index::ProgramIndex};
use nia_backend_ir::{
    BackendClosureEntry, BackendFunction, BackendFunctionInstance, BackendGlobal,
    BackendGlobalInstance, BackendModule, BackendParam, BackendStruct, BackendStructInstance,
    BackendTraitObjectVtable, BackendTraitObjectVtableFunction, BackendUnion, BackendUnionInstance,
    CodegenPartition,
};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{FunctionBody, FunctionLocalKind};
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_layout::TypeLayout;
use nia_mangle::mangle_symbol_id;
use nia_symbol::SymbolId;
use nia_ty::{ConstGenericArg, PrimitiveTy, TyKind};

type FunctionInstanceKey = (
    GlobalDefId,
    ModuleId,
    Option<InternedTyId>,
    Vec<InternedTyId>,
    Vec<ConstGenericArg>,
);

#[derive(Clone, Copy)]
struct FunctionInstanceRef<'a> {
    def_id: GlobalDefId,
    arg_module_id: ModuleId,
    self_arg: Option<InternedTyId>,
    args: &'a [InternedTyId],
    const_args: &'a [ConstGenericArg],
}
type AggregateInstanceKey = (GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>);
type AggregateFieldsLookup = RefCell<HashMap<AggregateInstanceKey, Option<Vec<InternedTyId>>>>;

pub(super) fn backend_symbol_debug_name(name: SymbolId) -> String {
    mangle_symbol_id(name)
}

pub(super) fn validate_backend_partition_definitions(
    partition: &CodegenPartition,
    index: &ProgramIndex,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator::new(index);
    let module = index.module_for_partition(partition);
    for &position in partition.function_definitions() {
        validator.validate_function(&module.name, &module.functions[position], true);
    }
    for &position in partition.function_instance_definitions() {
        validator.validate_function_instance(
            &module.name,
            &module.function_instances[position],
            true,
        );
    }
    for &position in partition.closure_entry_definitions() {
        validator.validate_closure_entry(&module.name, &module.closure_entries[position], true);
    }
    for &position in partition.global_definitions() {
        validator.validate_global(&module.globals[position], true);
    }
    for &position in partition.global_instance_definitions() {
        validator.validate_global_instance(&module.global_instances[position], true);
    }
    for &position in partition.vtable_definitions() {
        validator.validate_vtable(&module.trait_object_vtables[position], true);
    }
    validator.diagnostics
}

pub(super) fn validate_backend_partition_declarations(
    declarations: &CodegenDeclarationMembership,
    index: &ProgramIndex,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator::new(index);
    for &def_id in &declarations.functions {
        let item = index
            .function(def_id)
            .expect("declaration membership contains indexed function");
        let owner = index
            .function_owner(def_id)
            .and_then(|owner| index.module(owner))
            .expect("indexed function owner");
        validator.validate_function(&owner.name, item, false);
    }
    for key in &declarations.function_instances {
        let item = index
            .function_instance(
                key.def_id,
                key.arg_module_id,
                key.self_arg,
                &key.args,
                &key.const_args,
            )
            .expect("declaration membership contains indexed function instance");
        let owner = index
            .function_instance_owner(
                key.def_id,
                key.arg_module_id,
                key.self_arg,
                &key.args,
                &key.const_args,
            )
            .and_then(|owner| index.module(owner))
            .expect("indexed function instance owner");
        validator.validate_function_instance(&owner.name, item, false);
    }
    for &def_id in &declarations.globals {
        validator.validate_global(
            index
                .global(def_id)
                .expect("declaration membership contains indexed global"),
            false,
        );
    }
    for key in &declarations.global_instances {
        validator.validate_global_instance(
            index
                .global_instance(key.def_id, key.arg_module_id, &key.args, &key.const_args)
                .expect("declaration membership contains indexed global instance"),
            false,
        );
    }
    for &def_id in &declarations.structs {
        validator.validate_struct(
            index
                .struct_item(def_id)
                .expect("declaration membership contains indexed struct"),
        );
    }
    for key in &declarations.struct_instances {
        validator.validate_struct_instance(
            index
                .struct_instance(key.def_id, &key.args, &key.const_args)
                .expect("declaration membership contains indexed struct instance"),
        );
    }
    for &def_id in &declarations.unions {
        validator.validate_union(
            index
                .union_item(def_id)
                .expect("declaration membership contains indexed union"),
        );
    }
    for key in &declarations.union_instances {
        validator.validate_union_instance(
            index
                .union_instance(key.def_id, &key.args, &key.const_args)
                .expect("declaration membership contains indexed union instance"),
        );
    }
    for key in &declarations.vtables {
        validator.validate_vtable(
            index
                .trait_object_vtable(key)
                .expect("declaration membership contains indexed vtable"),
            false,
        );
    }
    validator.diagnostics
}

pub(super) fn validate_backend_declaration_module(
    module: &BackendModule,
    index: &ProgramIndex,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator::new(index);
    for function in &module.functions {
        validator.validate_function(&module.name, function, false);
    }
    for function in &module.function_instances {
        validator.validate_function_instance(&module.name, function, false);
    }
    for entry in &module.closure_entries {
        validator.validate_closure_entry(&module.name, entry, false);
    }
    for global in &module.globals {
        validator.validate_global(global, false);
    }
    for global in &module.global_instances {
        validator.validate_global_instance(global, false);
    }
    for item in &module.structs {
        validator.validate_struct(item);
    }
    for item in &module.struct_instances {
        validator.validate_struct_instance(item);
    }
    for item in &module.unions {
        validator.validate_union(item);
    }
    for item in &module.union_instances {
        validator.validate_union_instance(item);
    }
    for item in &module.enums {
        validator.current_item = Some(format!("enum {}", backend_symbol_debug_name(item.name)));
        validator.validate_runtime_type(item.backing_type, item.span);
        for variant in &item.variants {
            match &variant.payload {
                nia_backend_ir::BackendEnumVariantPayload::Unit => {}
                nia_backend_ir::BackendEnumVariantPayload::Tuple(fields) => {
                    for field in fields {
                        validator.validate_runtime_type(*field, variant.span);
                    }
                }
                nia_backend_ir::BackendEnumVariantPayload::Named(fields) => {
                    for field in fields {
                        validator.validate_runtime_type(field.ty, field.span);
                    }
                }
            }
        }
        validator.current_item = None;
    }
    for vtable in &module.trait_object_vtables {
        validator.validate_vtable(vtable, false);
    }
    validator.diagnostics
}

impl<'a> BackendValidator<'a> {
    fn new(index: &'a ProgramIndex) -> Self {
        Self {
            index,
            diagnostics: Vec::new(),
            seen_types: HashSet::new(),
            layout_cache: RefCell::new(HashMap::new()),
            same_type_cache: RefCell::new(HashMap::new()),
            function_instance_ref_cache: RefCell::new(HashMap::new()),
            struct_fields_lookup_cache: RefCell::new(HashMap::new()),
            union_fields_lookup_cache: RefCell::new(HashMap::new()),
            local_tys: Vec::new(),
            current_item: None,
            current_subject: None,
        }
    }
}

pub(super) struct BackendValidator<'a> {
    index: &'a ProgramIndex,
    diagnostics: Vec<Diagnostic>,
    seen_types: HashSet<InternedTyId>,
    layout_cache: RefCell<HashMap<InternedTyId, Option<TypeLayout>>>,
    same_type_cache: RefCell<HashMap<(InternedTyId, InternedTyId), bool>>,
    function_instance_ref_cache: RefCell<HashMap<FunctionInstanceKey, bool>>,
    struct_fields_lookup_cache: AggregateFieldsLookup,
    union_fields_lookup_cache: AggregateFieldsLookup,
    local_tys: Vec<HashMap<LocalId, InternedTyId>>,
    current_item: Option<String>,
    current_subject: Option<&'static str>,
}

impl BackendValidator<'_> {
    fn validate_function(&mut self, module_name: &str, function: &BackendFunction, body: bool) {
        if !function.generics.is_empty() {
            return;
        }
        self.current_item = Some(format!(
            "function {} in {}::{:?}",
            backend_symbol_debug_name(function.name),
            module_name,
            function.def_id
        ));
        self.validate_function_signature(&function.params, function.return_type, function.span);
        if body && let Some(body) = &function.function_body {
            self.validate_function_param_locals(&function.params, body);
            self.validate_function_body(body);
        }
        self.current_item = None;
    }

    fn validate_function_instance(
        &mut self,
        module_name: &str,
        function: &BackendFunctionInstance,
        body: bool,
    ) {
        self.current_item = Some(format!(
            "function instance {} in {}::{:?}::{:?}",
            backend_symbol_debug_name(function.name),
            module_name,
            function.def_id,
            function.args
        ));
        self.validate_function_signature(&function.params, function.return_type, function.span);
        if body && let Some(body) = &function.function_body {
            self.validate_function_param_locals(&function.params, body);
            self.validate_function_body(body);
        }
        self.current_item = None;
    }

    fn validate_closure_entry(
        &mut self,
        module_name: &str,
        entry: &BackendClosureEntry,
        body: bool,
    ) {
        self.current_item = Some(format!(
            "closure entry {} in {}::{:?}#{}",
            entry.symbol, module_name, entry.key.closure_id.owner, entry.key.closure_id.ordinal
        ));
        self.validate_runtime_type(entry.abi.state_type, entry.span);
        self.validate_runtime_type(entry.abi.state_pointer_type, entry.span);
        for param in &entry.abi.params {
            self.validate_runtime_type(*param, entry.span);
        }
        self.validate_type(entry.abi.return_type, entry.span);

        let state_pointer_matches = matches!(
            self.index.type_store().get(entry.abi.state_pointer_type),
            Some(TyKind::Pointer {
                is_readonly: true,
                elem,
            }) if *elem == entry.abi.state_type
        );
        if !state_pointer_matches {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry ABI state pointer must be a readonly pointer to its state type",
            ));
        }
        if entry.abi.params.len() != entry.params.len() {
            self.diagnostics.push(Diagnostic::internal_error_at(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                entry.span,
                "closure entry ABI parameter list does not match its body parameters",
            ));
        }
        if body {
            self.validate_closure_entry_param_locals(entry);
            self.validate_function_body(&entry.function_body);
            if !self.same_type(entry.function_body.ty, entry.abi.return_type) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.function_body.span,
                    "closure entry body type does not match its ABI return type",
                ));
            }
        }
        self.current_item = None;
    }

    fn validate_function_signature(
        &mut self,
        params: &[BackendParam],
        return_type: InternedTyId,
        span: nia_span::Span,
    ) {
        self.validate_type(return_type, span);
        for param in params {
            self.current_subject = Some("param passing_ty");
            self.validate_runtime_type(param.passing_ty, param.span);
            self.current_subject = Some("param local_ty");
            self.validate_runtime_type(param.local_ty, param.span);
            self.current_subject = None;
        }
    }

    fn validate_global(&mut self, global: &BackendGlobal, init: bool) {
        self.current_item = Some(format!("global {}", backend_symbol_debug_name(global.name)));
        self.validate_runtime_type(global.ty, global.span);
        if init && let Some(value) = &global.init {
            self.validate_static_init(global.ty, value, global.span);
        }
        self.current_item = None;
    }

    fn validate_global_instance(&mut self, global: &BackendGlobalInstance, init: bool) {
        self.current_item = Some(format!(
            "global instance {}::{:?}",
            backend_symbol_debug_name(global.name),
            global.args
        ));
        self.validate_runtime_type(global.ty, global.span);
        if init && let Some(value) = &global.init {
            self.validate_static_init(global.ty, value, global.span);
        }
        self.current_item = None;
    }

    fn validate_struct(&mut self, item: &BackendStruct) {
        if item.generics.is_empty() {
            self.current_item = Some(format!("struct {}", backend_symbol_debug_name(item.name)));
            self.validate_fields(&item.fields);
            self.current_item = None;
        }
    }

    fn validate_struct_instance(&mut self, item: &BackendStructInstance) {
        self.current_item = Some(format!(
            "struct instance {}::{:?}",
            backend_symbol_debug_name(item.name),
            item.args
        ));
        self.validate_fields(&item.fields);
        self.current_item = None;
    }

    fn validate_union(&mut self, item: &BackendUnion) {
        if item.generics.is_empty() {
            self.current_item = Some(format!("union {}", backend_symbol_debug_name(item.name)));
            self.validate_fields(&item.fields);
            self.current_item = None;
        }
    }

    fn validate_union_instance(&mut self, item: &BackendUnionInstance) {
        self.current_item = Some(format!(
            "union instance {}::{:?}",
            backend_symbol_debug_name(item.name),
            item.args
        ));
        self.validate_fields(&item.fields);
        self.current_item = None;
    }

    fn validate_fields(&mut self, fields: &[nia_backend_ir::BackendField]) {
        for field in fields {
            self.validate_runtime_type(field.ty, field.span);
        }
    }

    fn validate_vtable(&mut self, vtable: &BackendTraitObjectVtable, entries: bool) {
        self.current_item = Some(format!("trait object vtable {:?}", vtable.key));
        self.validate_trait_object_self_type(vtable.key.self_ty, vtable.span);
        self.validate_runtime_type(vtable.key.object_ty, vtable.span);
        if entries {
            for entry in &vtable.entries {
                match &entry.function {
                    BackendTraitObjectVtableFunction::Function(def_id) => {
                        self.validate_function_ref(
                            *def_id,
                            vtable.span,
                            "backend IR vtable references missing function",
                        );
                    }
                    BackendTraitObjectVtableFunction::FunctionInstance {
                        def_id,
                        arg_module_id,
                        self_arg,
                        args,
                        const_args,
                    } => {
                        self.validate_function_instance_ref(
                            FunctionInstanceRef {
                                def_id: *def_id,
                                arg_module_id: *arg_module_id,
                                self_arg: *self_arg,
                                args,
                                const_args,
                            },
                            vtable.span,
                            "backend IR vtable references missing function instance",
                        );
                    }
                }
            }
        }
        self.current_item = None;
    }

    fn validate_function_param_locals(&mut self, params: &[BackendParam], body: &FunctionBody) {
        let param_locals = body
            .locals
            .iter()
            .filter(|local| local.kind == FunctionLocalKind::Param)
            .map(|local| (local.id, local.ty))
            .collect::<HashMap<_, _>>();
        for param in params {
            let Some(local_id) = param.local_id else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    param.span,
                    "backend IR function parameter with a body is missing its local binding",
                ));
                continue;
            };
            let Some(local_ty) = param_locals.get(&local_id).copied() else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    param.span,
                    format!("backend IR function parameter references missing local {local_id:?}"),
                ));
                continue;
            };
            if !self.same_type(param.local_ty, local_ty) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    param.span,
                    "backend IR function parameter local type does not match its body local",
                ));
            }
        }
    }

    fn validate_closure_entry_param_locals(&mut self, entry: &BackendClosureEntry) {
        let locals = entry
            .function_body
            .locals
            .iter()
            .map(|local| (local.id, local.ty))
            .collect::<HashMap<_, _>>();
        let mut expected = Vec::with_capacity(entry.params.len() + 1);
        expected.push((entry.state_param, entry.abi.state_pointer_type));
        expected.extend(
            entry
                .params
                .iter()
                .copied()
                .zip(entry.abi.params.iter().copied()),
        );
        for (local_id, expected_ty) in expected {
            let Some(local_ty) = locals.get(&local_id).copied() else {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.span,
                    format!(
                        "closure entry ABI parameter references missing body local {local_id:?}"
                    ),
                ));
                continue;
            };
            if !self.same_type(expected_ty, local_ty) {
                self.diagnostics.push(Diagnostic::internal_error_at(
                    nia_diagnostic::codes::INVALID_BACKEND_IR,
                    entry.span,
                    format!("closure entry ABI parameter local {local_id:?} has a mismatched type"),
                ));
            }
        }
    }
}

pub(super) fn primitive_layout(primitive: PrimitiveTy) -> TypeLayout {
    let (size, align) = match primitive {
        PrimitiveTy::I8 | PrimitiveTy::U8 | PrimitiveTy::Bool => (1, 1),
        PrimitiveTy::I16 | PrimitiveTy::U16 => (2, 2),
        PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::F32 | PrimitiveTy::Char => (4, 4),
        PrimitiveTy::I64
        | PrimitiveTy::U64
        | PrimitiveTy::F64
        | PrimitiveTy::Isize
        | PrimitiveTy::Usize => (8, 8),
        PrimitiveTy::I128 | PrimitiveTy::U128 => (16, 16),
        PrimitiveTy::Never => (0, 1),
    };
    TypeLayout { size, align }
}

pub(super) fn align_to(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    value.div_ceil(align) * align
}
