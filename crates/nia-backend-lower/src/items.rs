// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ModuleLowerer;
use nia_ast::{BindingItem, FunctionItem};
use nia_backend_ir::{
    BackendEnum, BackendEnumVariant, BackendField, BackendFunction, BackendGlobal, BackendParam,
    BackendStruct, BackendUnion,
};
use nia_comptime_check::ComptimeValue;
use nia_defs::DefKind;
use nia_span::Span;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn lower_struct(
        &mut self,
        span: Span,
        item: &nia_ast::StructItem,
    ) -> Option<BackendStruct> {
        let def_id = self.def_id_for_span(span, DefKind::Struct)?;
        let signature = self.input.signatures.structs.get(&def_id)?;
        Some(BackendStruct {
            def_id: self.global_def_id(def_id),
            name: item.name.clone(),
            generics: signature.generics.clone(),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name.clone(),
                    ty: field.ty,
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span,
        })
    }

    pub(crate) fn lower_union(
        &mut self,
        span: Span,
        item: &nia_ast::UnionItem,
    ) -> Option<BackendUnion> {
        let def_id = self.def_id_for_span(span, DefKind::Union)?;
        let signature = self.input.signatures.unions.get(&def_id)?;
        Some(BackendUnion {
            def_id: self.global_def_id(def_id),
            name: item.name.clone(),
            generics: signature.generics.clone(),
            fields: signature
                .fields
                .iter()
                .map(|field| BackendField {
                    def_id: self.global_def_id(field.def_id),
                    name: field.name.clone(),
                    ty: field.ty,
                    span: field.span,
                })
                .collect(),
            is_extern: signature.is_extern,
            span,
        })
    }

    pub(crate) fn lower_enum(
        &mut self,
        span: Span,
        item: &nia_ast::EnumItem,
    ) -> Option<BackendEnum> {
        let def_id = self.def_id_for_span(span, DefKind::Enum)?;
        let signature = self.input.signatures.enums.get(&def_id)?;
        Some(BackendEnum {
            def_id: self.global_def_id(def_id),
            name: item.name.clone(),
            backing_type: signature.backing_type,
            variants: signature
                .variants
                .iter()
                .map(|variant| BackendEnumVariant {
                    def_id: self.global_def_id(variant.def_id),
                    name: variant.name.clone(),
                    value: self
                        .input
                        .comptime
                        .enum_values
                        .get(&variant.def_id)
                        .map(|value| match value {
                            ComptimeValue::Int(value) => *value,
                        }),
                    span: variant.span,
                })
                .collect(),
            span,
        })
    }

    pub(crate) fn lower_global(
        &mut self,
        span: Span,
        binding: &BindingItem,
    ) -> Option<BackendGlobal> {
        let def_id = self.def_id_for_span(span, DefKind::Global)?;
        let signature = self.input.signatures.globals.get(&def_id)?;
        let ty = signature
            .explicit_type
            .or_else(|| binding.value.as_ref().and_then(|value| self.expr_ty(value)))
            .unwrap_or_else(|| self.error_ty());
        let init = self
            .input
            .body_check
            .ir
            .global_inits
            .get(&self.global_def_id(def_id))
            .cloned();
        Some(BackendGlobal {
            def_id: self.global_def_id(def_id),
            name: binding.name.clone(),
            ty,
            is_const: signature.is_const,
            is_extern: signature.is_extern,
            init,
            span,
        })
    }

    pub(crate) fn lower_function(
        &mut self,
        span: Span,
        function: &FunctionItem,
    ) -> Option<BackendFunction> {
        let def_id = self.def_id_for_span_any_function(span)?;
        let signature = self.input.signatures.functions.get(&def_id)?;
        let body = self
            .input
            .body_check
            .ir
            .function_bodies
            .get(&self.global_def_id(def_id))
            .cloned();
        let control_body = body.as_ref().map(nia_control_ir::lower_control_body);
        Some(BackendFunction {
            def_id: self.global_def_id(def_id),
            name: function.name.clone(),
            generics: signature.generics.clone(),
            params: function
                .params
                .iter()
                .zip(signature.params.iter())
                .map(|(param, signature)| {
                    let local_id = self.input.locals.local_defs.get(&param.span).copied();
                    let ty = if signature.receiver.is_some() {
                        local_id
                            .and_then(|local_id| {
                                self.input.body_check.ir.local_types.get(&local_id).copied()
                            })
                            .unwrap_or(signature.ty)
                    } else {
                        signature.ty
                    };
                    BackendParam {
                        local_id,
                        name: param.name.clone(),
                        receiver: signature.receiver,
                        ty,
                        span: param.span,
                    }
                })
                .collect(),
            return_type: signature.return_type,
            is_extern: signature.is_extern,
            is_variadic: signature.is_variadic,
            body,
            control_body,
            span,
        })
    }
}
