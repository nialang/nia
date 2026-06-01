// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use crate::program_index::ProgramIndex;
use nia_backend_ir::{BackendModule, BackendProgram};
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_layout::TypeLayout;
use nia_span::Span;
use nia_ty::{ArrayLenTy, LayoutBuiltin, PrimitiveTy, RangeTyKind, TyKind};

pub(super) fn validate_backend_program(
    program: &BackendProgram,
    index: &ProgramIndex<'_>,
) -> Vec<Diagnostic> {
    let mut validator = BackendValidator {
        index,
        diagnostics: Vec::new(),
        seen_types: HashSet::new(),
    };
    for module in &program.modules {
        validator.validate_module(module);
    }
    validator.diagnostics
}

struct BackendValidator<'a> {
    index: &'a ProgramIndex<'a>,
    diagnostics: Vec<Diagnostic>,
    seen_types: HashSet<InternedTyId>,
}

impl BackendValidator<'_> {
    fn validate_module(&mut self, module: &BackendModule) {
        for function in &module.functions {
            self.validate_type(function.return_type, function.span);
            for param in &function.params {
                self.validate_runtime_type(param.ty, param.span);
            }
        }
        for function in &module.function_instances {
            self.validate_type(function.return_type, function.span);
            for param in &function.params {
                self.validate_runtime_type(param.ty, param.span);
            }
        }
        for global in &module.globals {
            self.validate_runtime_type(global.ty, global.span);
        }
        for item in &module.structs {
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
        }
        for item in &module.struct_instances {
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
            }
        }
        for item in &module.unions {
            for field in &item.fields {
                self.validate_runtime_type(field.ty, field.span);
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
    }

    fn validate_runtime_type(&mut self, ty: InternedTyId, span: Span) {
        self.validate_type(ty, span);
        if self.layout_of(ty).is_none() {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("backend IR type {ty:?} has no ABI layout before LLVM codegen"),
            ));
        }
    }

    fn validate_type(&mut self, ty: InternedTyId, span: Span) {
        if !self.seen_types.insert(ty) {
            return;
        }
        let Some(module) = self.index.module(ty.interner_id) else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!(
                    "backend IR type {ty:?} belongs to missing module {:?}",
                    ty.interner_id
                ),
            ));
            return;
        };
        let Some(kind) = module.interner.get(ty).cloned() else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("backend IR type {ty:?} is missing from its owner interner"),
            ));
            return;
        };
        match kind {
            TyKind::Pointer { elem, .. } | TyKind::Slice { elem, .. } => {
                self.validate_type(elem, span);
            }
            TyKind::Array { len, elem } => {
                self.validate_array_len(&len, span);
                self.validate_runtime_type(elem, span);
            }
            TyKind::Range { bound, .. } => {
                if let Some(bound) = bound {
                    self.validate_runtime_type(bound, span);
                }
            }
            TyKind::FunctionPointer {
                params,
                return_type,
                ..
            } => {
                for param in params {
                    self.validate_runtime_type(param, span);
                }
                self.validate_type(return_type, span);
            }
            TyKind::Nominal { def_id, args } => {
                if self.index.module(def_id.module_id).is_none() {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "backend IR nominal type {def_id:?} belongs to missing module {:?}",
                            def_id.module_id
                        ),
                    ));
                }
                for arg in args {
                    self.validate_type(arg, span);
                }
            }
            TyKind::TraitObject {
                trait_args,
                associated_type_bindings,
                ..
            } => {
                for arg in trait_args {
                    self.validate_type(arg, span);
                }
                for (_, ty) in associated_type_bindings {
                    self.validate_type(ty, span);
                }
            }
            TyKind::Projection {
                self_ty,
                trait_args,
                ..
            } => {
                self.validate_type(self_ty, span);
                for arg in trait_args {
                    self.validate_type(arg, span);
                }
            }
            TyKind::BuiltinTrait { args, .. } => {
                for arg in args {
                    self.validate_type(arg, span);
                }
            }
            TyKind::Primitive(_) | TyKind::GenericParam(_) | TyKind::Error => {}
        }
    }

    fn validate_array_len(&mut self, len: &ArrayLenTy, span: Span) {
        match len {
            ArrayLenTy::ConstValue(_) => {}
            ArrayLenTy::ConstExpr(id) => {
                let Some(module) = self.index.module(id.module_id) else {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "backend IR array length {id:?} belongs to missing module {:?}",
                            id.module_id
                        ),
                    ));
                    return;
                };
                if !module.comptime.array_lengths.contains_key(id) {
                    self.diagnostics.push(Diagnostic::error(
                        span,
                        format!(
                            "backend IR array length {id:?} was not evaluated before LLVM codegen"
                        ),
                    ));
                }
            }
            ArrayLenTy::Builtin { ty, .. } => {
                self.validate_runtime_type(*ty, span);
            }
            ArrayLenTy::Infer => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "backend IR array length inference reached LLVM codegen",
                ));
            }
        }
    }

    fn layout_of(&self, ty: InternedTyId) -> Option<TypeLayout> {
        self.layout_of_with_active(ty, &mut HashSet::new())
    }

    fn layout_of_with_active(
        &self,
        ty: InternedTyId,
        active: &mut HashSet<InternedTyId>,
    ) -> Option<TypeLayout> {
        if !active.insert(ty) {
            return None;
        }
        let layout = self.layout_of_inner(ty, active);
        active.remove(&ty);
        layout
    }

    fn layout_of_inner(
        &self,
        ty: InternedTyId,
        active: &mut HashSet<InternedTyId>,
    ) -> Option<TypeLayout> {
        let owner = self.index.module(ty.interner_id)?;
        if let Some(layout) = owner
            .layouts
            .types
            .iter()
            .find_map(|(candidate, layout)| (*candidate == ty).then_some(layout.clone()))
        {
            return Some(layout);
        }
        match owner.interner.get(ty)? {
            TyKind::Primitive(primitive) => Some(primitive_layout(*primitive)),
            TyKind::Pointer { .. } | TyKind::FunctionPointer { .. } => {
                Some(TypeLayout { size: 8, align: 8 })
            }
            TyKind::Slice { .. } | TyKind::TraitObject { .. } => {
                Some(TypeLayout { size: 16, align: 8 })
            }
            TyKind::Range { bound: None, .. } => Some(TypeLayout { size: 0, align: 1 }),
            TyKind::Range {
                kind,
                bound: Some(bound),
            } => {
                let field_count = match kind {
                    RangeTyKind::Exclusive | RangeTyKind::Inclusive => 2,
                    RangeTyKind::From | RangeTyKind::To | RangeTyKind::ToInclusive => 1,
                    RangeTyKind::Full => 0,
                };
                let bound_layout = self.layout_of_with_active(*bound, active)?;
                Some(TypeLayout {
                    size: align_to(
                        bound_layout.size.saturating_mul(field_count),
                        bound_layout.align,
                    ),
                    align: bound_layout.align,
                })
            }
            TyKind::Array { len, elem } => {
                let len = self.array_len_value(len)?;
                let elem_layout = self.layout_of_with_active(*elem, active)?;
                Some(TypeLayout {
                    size: elem_layout.size.saturating_mul(len),
                    align: elem_layout.align,
                })
            }
            TyKind::Nominal { def_id, args } => {
                let def_owner = self.index.module(def_id.module_id)?;
                if args.is_empty() {
                    def_owner
                        .layouts
                        .structs
                        .iter()
                        .find_map(|(candidate, layout)| {
                            (*candidate == *def_id).then_some(layout.layout.clone())
                        })
                        .or_else(|| {
                            def_owner
                                .layouts
                                .unions
                                .iter()
                                .find_map(|(candidate, layout)| {
                                    (*candidate == *def_id).then_some(layout.layout.clone())
                                })
                        })
                } else {
                    def_owner
                        .layouts
                        .struct_instances
                        .iter()
                        .find_map(|(key, layout)| {
                            (key.def_id == *def_id && key.args == *args)
                                .then_some(layout.layout.clone())
                        })
                        .or_else(|| {
                            def_owner
                                .layouts
                                .union_instances
                                .iter()
                                .find_map(|(key, layout)| {
                                    (key.def_id == *def_id && key.args == *args)
                                        .then_some(layout.layout.clone())
                                })
                        })
                }
            }
            TyKind::BuiltinTrait { .. } => Some(TypeLayout { size: 0, align: 1 }),
            TyKind::Projection { .. } | TyKind::GenericParam(_) | TyKind::Error => None,
        }
    }

    fn array_len_value(&self, len: &ArrayLenTy) -> Option<u64> {
        match len {
            ArrayLenTy::ConstValue(value) => Some(*value),
            ArrayLenTy::ConstExpr(id) => self
                .index
                .module(id.module_id)
                .and_then(|module| module.comptime.array_lengths.get(id).copied()),
            ArrayLenTy::Builtin { builtin, ty } => {
                let layout = self.layout_of(*ty)?;
                match builtin {
                    LayoutBuiltin::Size => Some(layout.size),
                    LayoutBuiltin::Align => Some(layout.align),
                }
            }
            ArrayLenTy::Infer => None,
        }
    }
}

fn primitive_layout(primitive: PrimitiveTy) -> TypeLayout {
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

fn align_to(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    value.div_ceil(align) * align
}
