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
use nia_static_ir::StaticInit;

pub(crate) const SIMPLIFY_STATIC_INIT_PASS: &str = "simplify-static-init";

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
        let global_def_id = self.global_def_id(def_id);
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
            .get(&global_def_id)
            .cloned()
            .map(|init| self.optimize_static_init(global_def_id, init));
        Some(BackendGlobal {
            def_id: global_def_id,
            name: binding.name.clone(),
            ty,
            is_const: signature.is_const,
            is_extern: signature.is_extern,
            init,
            span,
        })
    }

    fn optimize_static_init(
        &mut self,
        global: nia_ids::GlobalDefId,
        init: StaticInit,
    ) -> StaticInit {
        if !self.static_init_simplification_enabled() {
            return init;
        }
        let simplified = simplify_static_init(init.clone());
        if simplified != init {
            self.optimization_report.changed_passes.push(
                crate::BackendOptimizationChange::Global {
                    module_id: self.input.module_id,
                    global,
                    pass: SIMPLIFY_STATIC_INIT_PASS,
                },
            );
        }
        simplified
    }

    fn static_init_simplification_enabled(&self) -> bool {
        crate::static_init_simplification_enabled(&self.optimization)
    }

    pub(crate) fn lower_function(
        &mut self,
        span: Span,
        function: &FunctionItem,
    ) -> Option<BackendFunction> {
        let def_id = self.def_id_for_span_any_function(span)?;
        let signature = self.input.signatures.functions.get(&def_id)?;
        let global_def_id = self.global_def_id(def_id);
        let function_body = self
            .input
            .function_bodies
            .get(&global_def_id)
            .cloned()
            .map(|body| self.resolve_builtin_operator_calls_in_body(body))
            .map(|body| self.optimize_function_body(global_def_id, false, 0, body));
        Some(BackendFunction {
            def_id: global_def_id,
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
            function_body,
            span,
        })
    }
}

fn simplify_static_init(init: StaticInit) -> StaticInit {
    match init {
        StaticInit::Array(elems) => {
            let elems = elems
                .into_iter()
                .map(simplify_static_init)
                .collect::<Vec<_>>();
            if elems.iter().all(is_zero_static_init) {
                StaticInit::Zero
            } else {
                StaticInit::Array(elems)
            }
        }
        StaticInit::Repeat { value, count } => {
            let value = simplify_static_init(*value);
            if count == 0 || is_zero_static_init(&value) {
                StaticInit::Zero
            } else {
                StaticInit::Repeat {
                    value: Box::new(value),
                    count,
                }
            }
        }
        StaticInit::Struct(fields) => {
            let fields = fields
                .into_iter()
                .map(|mut field| {
                    field.value = simplify_static_init(field.value);
                    field
                })
                .collect::<Vec<_>>();
            if fields.iter().all(|field| is_zero_static_init(&field.value)) {
                StaticInit::Zero
            } else {
                StaticInit::Struct(fields)
            }
        }
        StaticInit::Chars(scalars) if scalars.iter().all(|scalar| *scalar == 0) => StaticInit::Zero,
        StaticInit::Bytes(bytes) if bytes.iter().all(|byte| *byte == 0) => StaticInit::Zero,
        StaticInit::Float(text) if is_zero_float_static_init(&text) => StaticInit::Zero,
        StaticInit::Int(0)
        | StaticInit::Bool(false)
        | StaticInit::Char(0)
        | StaticInit::Byte(0)
        | StaticInit::NullPtr
        | StaticInit::Zero => StaticInit::Zero,
        other => other,
    }
}

fn is_zero_static_init(init: &StaticInit) -> bool {
    match init {
        StaticInit::Zero
        | StaticInit::Int(0)
        | StaticInit::Bool(false)
        | StaticInit::Char(0)
        | StaticInit::Byte(0)
        | StaticInit::NullPtr => true,
        StaticInit::Float(text) => is_zero_float_static_init(text),
        StaticInit::Chars(scalars) => scalars.iter().all(|scalar| *scalar == 0),
        StaticInit::Bytes(bytes) => bytes.iter().all(|byte| *byte == 0),
        StaticInit::Array(elems) => elems.iter().all(is_zero_static_init),
        StaticInit::Repeat { value, count } => *count == 0 || is_zero_static_init(value),
        StaticInit::Struct(fields) => fields.iter().all(|field| is_zero_static_init(&field.value)),
        StaticInit::Int(_)
        | StaticInit::Bool(_)
        | StaticInit::Char(_)
        | StaticInit::Byte(_)
        | StaticInit::AddrOfGlobal { .. }
        | StaticInit::AddrOfFunction { .. } => false,
    }
}

fn is_zero_float_static_init(text: &str) -> bool {
    nia_comptime_engine::eval_float_literal(text) == Ok(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplifies_empty_repeat_static_init_to_zero() {
        let init = StaticInit::Repeat {
            value: Box::new(StaticInit::Int(1)),
            count: 0,
        };

        assert_eq!(simplify_static_init(init), StaticInit::Zero);
    }

    #[test]
    fn treats_empty_repeat_static_init_as_zero() {
        let init = StaticInit::Repeat {
            value: Box::new(StaticInit::AddrOfFunction {
                function: nia_ids::GlobalDefId {
                    module_id: nia_ids::ModuleId(0),
                    def_id: nia_ids::DefId(0),
                },
                args: Vec::new(),
            }),
            count: 0,
        };

        assert!(is_zero_static_init(&init));
    }
}
