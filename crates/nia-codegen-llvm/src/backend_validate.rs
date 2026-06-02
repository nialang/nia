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
use nia_backend_ir::{BackendModule, BackendProgram, BackendTraitObjectVtableFunction};
use nia_diagnostic::Diagnostic;
use nia_ids::{InternedTyId, LocalId};
use nia_layout::TypeLayout;
use nia_ty::PrimitiveTy;

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
    function_instance_ref_cache: RefCell<HashMap<(nia_ids::GlobalDefId, Vec<InternedTyId>), bool>>,
    struct_fields_lookup_cache:
        RefCell<HashMap<(nia_ids::GlobalDefId, Vec<InternedTyId>), Option<Vec<InternedTyId>>>>,
    union_fields_lookup_cache:
        RefCell<HashMap<(nia_ids::GlobalDefId, Vec<InternedTyId>), Option<Vec<InternedTyId>>>>,
    local_tys: Vec<HashMap<LocalId, InternedTyId>>,
}

impl BackendValidator<'_> {
    fn validate_module(&mut self, module: &BackendModule) {
        for function in &module.functions {
            if function.generics.is_empty() {
                self.validate_type(function.return_type, function.span);
                for param in &function.params {
                    self.validate_runtime_type(param.ty, param.span);
                }
                if let Some(body) = &function.function_body {
                    self.validate_function_body(body);
                }
            }
        }
        for function in &module.function_instances {
            self.validate_type(function.return_type, function.span);
            for param in &function.params {
                self.validate_runtime_type(param.ty, param.span);
            }
            if let Some(body) = &function.function_body {
                self.validate_function_body(body);
            }
        }
        for global in &module.globals {
            self.validate_runtime_type(global.ty, global.span);
            if let Some(init) = &global.init {
                self.validate_static_init(global.ty, init, global.span);
            }
        }
        for item in &module.structs {
            if item.generics.is_empty() {
                for field in &item.fields {
                    self.validate_runtime_type(field.ty, field.span);
                }
            }
        }
        for item in &module.struct_instances {
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
        }
        for item in &module.unions {
            if item.generics.is_empty() {
                for field in &item.fields {
                    self.validate_runtime_type(field.ty, field.span);
                }
            }
        }
        for item in &module.union_instances {
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
        }
        for item in &module.enums {
            self.validate_runtime_type(item.backing_type, item.span);
        }
        for vtable in &module.trait_object_vtables {
            self.validate_runtime_type(vtable.key.self_ty, vtable.span);
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
                    BackendTraitObjectVtableFunction::FunctionInstance { def_id, args } => {
                        self.validate_function_instance_ref(
                            *def_id,
                            args,
                            vtable.span,
                            "backend IR vtable references missing function instance",
                        );
                    }
                }
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
