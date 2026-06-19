use crate::{
    ComptimeKey, ComptimeValueType, TypedComptimeValue,
    analyzer::{Analyzer, ComptimeCallFrame},
    support::{
        cast_comptime_integer, cast_float_to_float, cast_float_to_integer, cast_int_to_float,
        is_float_primitive, primitive_integer_layout, validate_assignment_shape,
    },
};
use nia_comptime_engine::{ComptimeCommonEnv, ComptimeError, ComptimeValue, ResolvedComptimeEnv};
use nia_comptime_ir::{
    ComptimeNameResolution, ResolvedComptimeAssignTarget, ResolvedComptimeAssignTargetKind,
    ResolvedComptimeBinding, ResolvedComptimeExpr, ResolvedComptimeParam, ResolvedComptimeTypeArg,
};
use nia_defs::DefKind;
use nia_ids::{GlobalDefId, InternedTyId, LayoutBuiltin, LocalId, ModuleId, ValueBuiltin};
use nia_local_resolve::LocalKind;
use nia_sema_ir::BuiltinAssociatedValue;
use nia_span::Span;
use nia_ty::{IntConst, TyKind};

impl ComptimeCommonEnv for Analyzer<'_> {
    fn resolve_builtin_value(
        &mut self,
        span: Span,
        builtin: ValueBuiltin,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = span;
        match builtin {
            ValueBuiltin::Error => Err(ComptimeError {
                span,
                message: "builtin `@error` must be called with a message".to_string(),
            }),
        }
    }

    fn cast_value(
        &mut self,
        span: Span,
        value: ComptimeValue,
        ty: InternedTyId,
    ) -> Result<ComptimeValue, ComptimeError> {
        let ty = self.substitute_ty_generics(ty);
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return Ok(value);
        };
        let value = match value {
            ComptimeValue::Int(value) => {
                if is_float_primitive(primitive) {
                    let Some(value) = cast_int_to_float(value, primitive) else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ComptimeValue::Float(value)
                } else {
                    let Some(value) =
                        cast_comptime_integer(value, primitive, self.input.target.pointer_width)
                    else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ComptimeValue::Int(value)
                }
            }
            ComptimeValue::Float(value) => {
                if is_float_primitive(primitive) {
                    let Some(value) = cast_float_to_float(value, primitive) else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ComptimeValue::Float(value)
                } else if primitive_integer_layout(primitive, self.input.target.pointer_width)
                    .is_some()
                {
                    let Some(value) =
                        cast_float_to_integer(value, primitive, self.input.target.pointer_width)
                    else {
                        return Err(ComptimeError {
                            span,
                            message: format!(
                                "comptime cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ComptimeValue::Int(
                        cast_comptime_integer(
                            IntConst::from_i128(value),
                            primitive,
                            self.input.target.pointer_width,
                        )
                        .unwrap_or_else(|| IntConst::from_i128(value)),
                    )
                } else {
                    ComptimeValue::Float(value)
                }
            }
            value => value,
        };
        Ok(value)
    }

    fn push_comptime_scope(&mut self, _span: Span) -> Result<(), ComptimeError> {
        self.call_locals.push(ComptimeCallFrame::default());
        Ok(())
    }

    fn pop_comptime_scope(&mut self) {
        self.call_locals.pop();
    }

    fn bind_function_context(
        &mut self,
        span: Span,
        module_id: ModuleId,
        function_id: Option<GlobalDefId>,
        substitutions: Vec<(String, InternedTyId)>,
    ) -> Result<(), ComptimeError> {
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ComptimeError {
                span,
                message: "failed to bind comptime function type substitutions".to_string(),
            });
        };
        frame.module_id = Some(module_id);
        frame.function_id = function_id;
        frame.type_substitutions.extend(substitutions);
        Ok(())
    }
}

impl ResolvedComptimeEnv for Analyzer<'_> {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: ComptimeNameResolution,
    ) -> Result<ComptimeValue, ComptimeError> {
        match resolution {
            ComptimeNameResolution::Local(local_id) => {
                if let Some(value) = self.call_local_value(local_id) {
                    return Ok(value);
                }
                if !self
                    .input
                    .locals
                    .locals
                    .get(local_id)
                    .is_some_and(|local| local.kind == LocalKind::ComptimeBinding)
                {
                    return Err(ComptimeError {
                        span,
                        message: "resolved comptime expression can only use comptime bindings"
                            .to_string(),
                    });
                }
                self.eval_key(ComptimeKey::Local(local_id), span)
                    .ok_or_else(|| ComptimeError {
                        span,
                        message: "failed to evaluate resolved comptime local".to_string(),
                    })
            }
            ComptimeNameResolution::Global(global_id) => {
                if self.def_kind_of(global_id) == Some(DefKind::Comptime) {
                    return self
                        .eval_key(ComptimeKey::Global(global_id), span)
                        .ok_or_else(|| ComptimeError {
                            span,
                            message: "failed to evaluate resolved comptime global".to_string(),
                        });
                }
                Err(ComptimeError {
                    span,
                    message: "resolved comptime expression can only use comptime bindings"
                        .to_string(),
                })
            }
            ComptimeNameResolution::BuiltinAssociatedValue(value) => {
                let BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind } = value;
                let Some(value) = kind.value(primitive, self.input.target.pointer_width) else {
                    return Err(ComptimeError {
                        span,
                        message: "builtin associated value is not representable at comptime"
                            .to_string(),
                    });
                };
                Ok(ComptimeValue::Int(value))
            }
        }
    }

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &ResolvedComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError> {
        let module_id = self.current_execution_module_id();
        let ty_id = (|| {
            self.ensure_working_interner(module_id)?;
            self.import_ty_into_module_or_none(type_arg.ty(), module_id)
        })()
        .map(|ty| self.substitute_ty_generics(ty));
        let Some(ty_id) = ty_id else {
            return Err(ComptimeError {
                span,
                message: format!(
                    "cannot resolve type argument for comptime builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        self.resolve_layout_builtin_for_ty(span, builtin, ty_id)
    }

    fn resolve_resolved_field_offset_builtin(
        &mut self,
        span: Span,
        type_arg: &ResolvedComptimeTypeArg,
        field: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        let module_id = self.current_execution_module_id();
        let ty_id = (|| {
            self.ensure_working_interner(module_id)?;
            self.import_ty_into_module_or_none(type_arg.ty(), module_id)
        })()
        .map(|ty| self.substitute_ty_generics(ty));
        let Some(ty_id) = ty_id else {
            return Err(ComptimeError {
                span,
                message: "cannot resolve type argument for comptime builtin `@offset`".to_string(),
            });
        };
        self.resolve_field_offset_builtin_for_ty(span, ty_id, field)
    }

    fn call_resolved_function(
        &mut self,
        span: Span,
        callee: &ResolvedComptimeExpr,
        type_args: &[ResolvedComptimeTypeArg],
        arg_exprs: &[ResolvedComptimeExpr],
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let Some(function_id) = self.resolved_comptime_function(callee) else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let Some(signature) = self
            .signatures_for_module(function_id.module_id)
            .and_then(|signatures| signatures.functions.get(&function_id.def_id))
            .cloned()
        else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let type_substitutions = if let Some(substitutions) =
            self.resolved_call_type_substitutions.get(&span).cloned()
        {
            substitutions
        } else {
            self.instantiate_resolved_function_generics(
                span,
                function_id.module_id,
                &signature,
                type_args,
                arg_exprs,
                None,
            )?
        };
        let return_ty = self
            .substitute_ty_into_current_module(
                function_id.module_id,
                signature.return_type,
                &type_substitutions,
            )
            .ok_or_else(|| ComptimeError {
                span,
                message: "cannot resolve comptime function return type".to_string(),
            })?;
        let Some(function) = self.comptime_function_body(function_id).cloned() else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let value = nia_comptime_engine::eval_resolved_comptime_function_call(
            span,
            function_id,
            function_id.module_id,
            &function,
            type_substitutions.into_iter().collect(),
            args,
            self,
        )?;
        let return_ty = ComptimeValueType::Runtime(return_ty);
        let value = self.normalize_typed_comptime_value(value, &return_ty);
        self.validate_typed_value(span, &value, &return_ty);
        Ok(value)
    }

    fn bind_resolved_function_param(
        &mut self,
        span: Span,
        param: &ResolvedComptimeParam,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let ty = param
            .ty()
            .map(|ty| ComptimeValueType::Runtime(self.substitute_ty_generics(ty)));
        self.bind_local_value(span, param.local_id(), false, value, ty)
    }

    fn bind_resolved_function_local(
        &mut self,
        span: Span,
        binding: &ResolvedComptimeBinding,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let ty = binding
            .explicit_type()
            .map(|ty| ComptimeValueType::Runtime(self.substitute_ty_generics(ty)))
            .or_else(|| self.resolved_comptime_expr_type(binding.value(), None));
        self.bind_local_value(span, binding.local_id(), binding.is_mutable(), value, ty)
    }

    fn bind_resolved_pattern_local(
        &mut self,
        span: Span,
        _name: &str,
        local_id: LocalId,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let ty = self
            .find_local_binding_type(local_id)
            .map(|ty| ComptimeValueType::Runtime(self.substitute_ty_generics(ty)));
        self.bind_local_value(span, local_id, false, value, ty)
    }

    fn assign_resolved_local(
        &mut self,
        span: Span,
        target: &ResolvedComptimeAssignTarget,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        match target.kind() {
            ResolvedComptimeAssignTargetKind::Local { name, local_id, .. } => {
                self.assign_local_value(span, *local_id, name, value)
            }
        }
    }
}

impl Analyzer<'_> {
    fn bind_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        is_mutable: bool,
        value: ComptimeValue,
        ty: Option<ComptimeValueType>,
    ) -> Result<(), ComptimeError> {
        let (value, ty) = if let Some(ty) = ty {
            let value = self.normalize_typed_comptime_value(value, &ty);
            self.validate_typed_value(span, &value, &ty);
            (value, Some(ty))
        } else {
            (value, None)
        };
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ComptimeError {
                span,
                message: "internal comptime function frame is missing".to_string(),
            });
        };
        if is_mutable {
            frame.mutable_locals.insert(local_id);
        }
        frame.locals.insert(local_id, value.clone());
        if let Some(ty) = ty {
            let typed = TypedComptimeValue { value, ty };
            frame.local_types.insert(local_id, typed.ty.clone());
        }
        Ok(())
    }

    fn assign_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        name: &str,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        for index in (0..self.call_locals.len()).rev() {
            if !self.call_locals[index].locals.contains_key(&local_id) {
                continue;
            }
            if !self.call_locals[index].mutable_locals.contains(&local_id) {
                return Err(ComptimeError {
                    span,
                    message: format!("cannot assign to immutable comptime local `{name}`"),
                });
            }
            let previous_ty = self.call_locals[index].local_types.get(&local_id).cloned();
            let previous_value = self.call_locals[index].locals.get(&local_id).cloned();
            let value = if let Some(previous_ty) = previous_ty.as_ref() {
                let value = self.normalize_typed_comptime_value(value, previous_ty);
                self.validate_typed_value(span, &value, previous_ty);
                value
            } else {
                value
            };
            if let Some(previous_value) = previous_value.as_ref() {
                validate_assignment_shape(&mut self.diagnostics, span, &value, previous_value);
            }
            let frame = &mut self.call_locals[index];
            frame.locals.insert(local_id, value.clone());
            return Ok(());
        }
        Err(ComptimeError {
            span,
            message: format!("unknown comptime assignment target `{name}`"),
        })
    }
}
