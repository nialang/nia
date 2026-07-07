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

use crate::program_index::ProgramIndex;
use nia_backend_ir::{
    BackendModule, BackendParam, BackendProgram, BackendTraitObjectVtableFunction,
};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{FunctionBody, FunctionLocalKind};
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_layout::TypeLayout;
use nia_mangle::mangle_symbol_id;
use nia_symbol::SymbolId;
use nia_ty::{ConstGenericArg, PrimitiveTy};

type FunctionInstanceKey = (
    GlobalDefId,
    ModuleId,
    Option<InternedTyId>,
    Vec<InternedTyId>,
    Vec<ConstGenericArg>,
);
type AggregateInstanceKey = (GlobalDefId, Vec<InternedTyId>, Vec<ConstGenericArg>);
type AggregateFieldsLookup = RefCell<HashMap<AggregateInstanceKey, Option<Vec<InternedTyId>>>>;

pub(super) fn backend_symbol_debug_name(name: SymbolId) -> String {
    mangle_symbol_id(name)
}

pub(super) fn validate_backend_program(
    program: &BackendProgram,
    index: &ProgramIndex<'_>,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator {
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
    };
    for module in &program.modules {
        validator.validate_module(module);
    }
    validator.diagnostics
}

pub(super) struct BackendValidator<'a> {
    index: &'a ProgramIndex<'a>,
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
    fn validate_module(&mut self, module: &BackendModule) {
        for function in &module.functions {
            if function.generics.is_empty() {
                self.current_item = Some(format!(
                    "function {} in {}::{:?}",
                    backend_symbol_debug_name(function.name),
                    module.name,
                    function.def_id
                ));
                self.validate_type(function.return_type, function.span);
                for param in &function.params {
                    self.current_subject = Some("param passing_ty");
                    self.validate_runtime_type(param.passing_ty, param.span);
                    self.current_subject = Some("param local_ty");
                    self.validate_runtime_type(param.local_ty, param.span);
                    self.current_subject = None;
                }
                if let Some(body) = &function.function_body {
                    self.validate_function_param_locals(&function.params, body);
                    self.validate_function_body(body);
                }
                self.current_item = None;
            }
        }
        for function in &module.function_instances {
            self.current_item = Some(format!(
                "function instance {} in {}::{:?}::{:?}",
                backend_symbol_debug_name(function.name),
                module.name,
                function.def_id,
                function.args
            ));
            self.validate_type(function.return_type, function.span);
            for param in &function.params {
                self.current_subject = Some("param passing_ty");
                self.validate_runtime_type(param.passing_ty, param.span);
                self.current_subject = Some("param local_ty");
                self.validate_runtime_type(param.local_ty, param.span);
                self.current_subject = None;
            }
            if let Some(body) = &function.function_body {
                self.validate_function_param_locals(&function.params, body);
                self.validate_function_body(body);
            }
            self.current_item = None;
        }
        for global in &module.globals {
            self.current_item = Some(format!("global {}", backend_symbol_debug_name(global.name)));
            self.validate_runtime_type(global.ty, global.span);
            if let Some(init) = &global.init {
                self.validate_static_init(global.ty, init, global.span);
            }
            self.current_item = None;
        }
        for item in &module.structs {
            if item.generics.is_empty() {
                self.current_item =
                    Some(format!("struct {}", backend_symbol_debug_name(item.name)));
                for field in &item.fields {
                    self.validate_runtime_type(field.ty, field.span);
                }
                self.current_item = None;
            }
        }
        for item in &module.struct_instances {
            self.current_item = Some(format!(
                "struct instance {}::{:?}",
                backend_symbol_debug_name(item.name),
                item.args
            ));
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
            self.current_item = None;
        }
        for item in &module.unions {
            if item.generics.is_empty() {
                self.current_item = Some(format!("union {}", backend_symbol_debug_name(item.name)));
                for field in &item.fields {
                    self.validate_runtime_type(field.ty, field.span);
                }
                self.current_item = None;
            }
        }
        for item in &module.union_instances {
            self.current_item = Some(format!(
                "union instance {}::{:?}",
                backend_symbol_debug_name(item.name),
                item.args
            ));
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
            self.current_item = None;
        }
        for item in &module.enums {
            self.current_item = Some(format!("enum {}", backend_symbol_debug_name(item.name)));
            self.validate_runtime_type(item.backing_type, item.span);
            self.current_item = None;
        }
        for vtable in &module.trait_object_vtables {
            self.current_item = Some(format!("trait object vtable {:?}", vtable.key));
            self.validate_trait_object_self_type(vtable.key.self_ty, vtable.span);
            self.validate_runtime_type(vtable.key.object_ty, vtable.span);
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
                            *def_id,
                            *arg_module_id,
                            *self_arg,
                            args,
                            const_args,
                            vtable.span,
                            "backend IR vtable references missing function instance",
                        );
                    }
                }
            }
            self.current_item = None;
        }
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
        PrimitiveTy::Void | PrimitiveTy::Never => (0, 1),
    };
    TypeLayout { size, align }
}

pub(super) fn align_to(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    value.div_ceil(align) * align
}
