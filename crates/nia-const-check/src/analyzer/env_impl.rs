use crate::{
    ConstKey, ConstValueType, TypedConstValue,
    analyzer::{Analyzer, ConstCallFrame, ConstFunctionInstantiationInput},
    support::{
        cast_const_integer, cast_float_to_float, cast_float_to_integer, cast_int_to_float,
        is_float_primitive, primitive_integer_layout, validate_assignment_shape,
    },
};
use nia_const_eval::{ConstCommonEnv, ConstError, ConstValue, ResolvedConstEnv};
use nia_const_ir::{
    ConstNameResolution, ResolvedConstAssignTarget, ResolvedConstAssignTargetKind,
    ResolvedConstBinding, ResolvedConstExpr, ResolvedConstParam, ResolvedConstTypeArg,
};
use nia_defs::DefKind;
use nia_ids::{
    BuiltinConstValue, BuiltinFunction, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId,
    ModuleId, ValueBuiltin,
};
use nia_item_signatures::{FunctionAttribute, FunctionSignature};
use nia_local_resolve::LocalKind;
use nia_sema_ir::BuiltinAssociatedValue;
use nia_span::Span;
use nia_symbol::symbol_identity_key;
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{IntConst, TyKind};
use std::path::{Path, PathBuf};

impl ConstCommonEnv for Analyzer<'_> {
    fn begin_const_eval(&mut self) {
        self.const_eval_budget.begin_session();
    }

    fn end_const_eval(&mut self) {
        self.const_eval_budget.end_session();
    }

    fn consume_const_eval_step(&mut self, span: Span) -> Result<(), ConstError> {
        self.const_eval_budget.consume_step(span)
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        Analyzer::symbol_name(self, symbol)
    }

    fn is_enum_variant(&self, def_id: GlobalDefId) -> bool {
        self.def_kind_of(def_id) == Some(DefKind::EnumVariant)
    }

    fn resolve_builtin_const(
        &mut self,
        span: Span,
        builtin: BuiltinConstValue,
    ) -> Result<ConstValue, ConstError> {
        let value = match builtin {
            BuiltinConstValue::TargetArch => {
                ConstValue::String(self.input.target.arch.as_str().to_string())
            }
            BuiltinConstValue::TargetVendor => {
                ConstValue::String(self.input.target.vendor.as_str().to_string())
            }
            BuiltinConstValue::TargetOs => {
                ConstValue::String(self.input.target.os.as_str().to_string())
            }
            BuiltinConstValue::TargetEnv => {
                ConstValue::String(self.input.target.env.as_str().to_string())
            }
            BuiltinConstValue::TargetAbi => {
                ConstValue::String(self.input.target.abi.as_str().to_string())
            }
            BuiltinConstValue::TargetEndian => {
                ConstValue::String(self.input.target.endian.as_str().to_string())
            }
            BuiltinConstValue::TargetPointerWidth => ConstValue::Int(IntConst::unsigned(
                u128::from(self.input.target.pointer_width),
            )),
        };
        let _ = span;
        Ok(value)
    }

    fn resolve_builtin_value(
        &mut self,
        span: Span,
        builtin: ValueBuiltin,
    ) -> Result<ConstValue, ConstError> {
        let _ = span;
        match builtin {
            ValueBuiltin::Error => Err(ConstError {
                span,
                message: "builtin `error` must be called with a message".to_string(),
            }),
        }
    }

    fn resolve_embed(&mut self, span: Span, path: &str) -> Result<ConstValue, ConstError> {
        if path.is_empty() {
            return Err(ConstError {
                span,
                message: "builtin `embed` path cannot be empty".to_string(),
            });
        }
        let Some(source_path) = self.current_execution_source_path() else {
            return Err(ConstError {
                span,
                message: "builtin `embed` cannot resolve the current source path".to_string(),
            });
        };
        let resolved = resolve_embed_path(source_path.as_str(), path);
        let bytes = std::fs::read(&resolved).map_err(|error| ConstError {
            span,
            message: format!("failed to embed `{}`: {error}", resolved.display()),
        })?;
        Ok(ConstValue::Array(
            bytes
                .into_iter()
                .map(|byte| ConstValue::Int(IntConst::unsigned(byte as u128)))
                .collect(),
        ))
    }

    fn cast_value(
        &mut self,
        span: Span,
        value: ConstValue,
        ty: InternedTyId,
    ) -> Result<ConstValue, ConstError> {
        let ty = self.substitute_ty_generics(ty);
        let Some(TyKind::Primitive(primitive)) = self.ty_kind(ty) else {
            return Ok(value);
        };
        let value = match value {
            ConstValue::Int(value) => {
                if is_float_primitive(primitive) {
                    let Some(value) = cast_int_to_float(value, primitive) else {
                        return Err(ConstError {
                            span,
                            message: format!(
                                "const cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ConstValue::Float(value)
                } else {
                    let Some(value) =
                        cast_const_integer(value, primitive, self.input.target.pointer_width)
                    else {
                        return Err(ConstError {
                            span,
                            message: format!(
                                "const cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ConstValue::Int(value)
                }
            }
            ConstValue::Float(value) => {
                if is_float_primitive(primitive) {
                    let Some(value) = cast_float_to_float(value, primitive) else {
                        return Err(ConstError {
                            span,
                            message: format!(
                                "const cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ConstValue::Float(value)
                } else if primitive_integer_layout(primitive, self.input.target.pointer_width)
                    .is_some()
                {
                    let Some(value) =
                        cast_float_to_integer(value, primitive, self.input.target.pointer_width)
                    else {
                        return Err(ConstError {
                            span,
                            message: format!(
                                "const cast result cannot be represented as `{}`",
                                primitive.name()
                            ),
                        });
                    };
                    ConstValue::Int(
                        cast_const_integer(
                            IntConst::from_i128(value),
                            primitive,
                            self.input.target.pointer_width,
                        )
                        .unwrap_or_else(|| IntConst::from_i128(value)),
                    )
                } else {
                    ConstValue::Float(value)
                }
            }
            value => value,
        };
        Ok(value)
    }

    fn push_const_scope(&mut self, _span: Span) -> Result<(), ConstError> {
        self.call_locals.push(ConstCallFrame::default());
        Ok(())
    }

    fn pop_const_scope(&mut self) {
        self.call_locals.pop();
    }

    fn push_function_frame(&mut self, span: Span) -> Result<(), ConstError> {
        self.const_eval_budget.enter_call(span)?;
        self.call_locals.push(ConstCallFrame::default());
        Ok(())
    }

    fn pop_function_frame(&mut self) {
        self.call_locals.pop();
        self.const_eval_budget.leave_call();
    }

    fn bind_function_context(
        &mut self,
        span: Span,
        module_id: ModuleId,
        function_id: Option<GlobalDefId>,
        substitutions: Vec<(SymbolId, InternedTyId)>,
        const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    ) -> Result<(), ConstError> {
        let resolved_const_substitutions = const_substitutions
            .into_iter()
            .map(|(name, arg)| (name, self.resolve_const_const_generic_arg(arg)))
            .collect::<Vec<_>>();
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ConstError {
                span,
                message: "failed to bind const function type substitutions".to_string(),
            });
        };
        frame.module_id = Some(module_id);
        frame.function_id = function_id;
        frame.type_substitutions.extend(substitutions);
        frame
            .const_substitutions
            .extend(resolved_const_substitutions);
        Ok(())
    }
}

pub(super) fn resolve_embed_path(source_path: &str, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    Path::new(source_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(path)
}

impl ResolvedConstEnv for Analyzer<'_> {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: ConstNameResolution,
    ) -> Result<ConstValue, ConstError> {
        match resolution {
            ConstNameResolution::Local(local_id) => {
                if let Some(value) = self.call_local_value(local_id) {
                    return Ok(value);
                }
                if !self
                    .input
                    .locals
                    .locals
                    .get(local_id)
                    .is_some_and(|local| local.kind == LocalKind::ConstBinding)
                {
                    return Err(ConstError {
                        span,
                        message: "resolved const expression can only use const bindings"
                            .to_string(),
                    });
                }
                self.eval_key(ConstKey::Local(local_id), span)
                    .ok_or_else(|| ConstError {
                        span,
                        message: "failed to evaluate resolved const local".to_string(),
                    })
            }
            ConstNameResolution::Global(global_id) => {
                if self.def_kind_of(global_id) == Some(DefKind::Const) {
                    return self
                        .eval_key(ConstKey::Global(global_id), span)
                        .ok_or_else(|| ConstError {
                            span,
                            message: "failed to evaluate resolved const global".to_string(),
                        });
                }
                Err(ConstError {
                    span,
                    message: "resolved const expression can only use const bindings".to_string(),
                })
            }
            ConstNameResolution::GenericParam(name) => self
                .active_execution_frames()
                .find_map(|frame| frame.const_substitutions.get(&name))
                .and_then(const_value_from_const_generic_arg)
                .ok_or_else(|| ConstError {
                    span,
                    message: format!(
                        "failed to evaluate const generic parameter `{}`",
                        self.symbol_name(name)
                    ),
                }),
            ConstNameResolution::BuiltinAssociatedValue(value) => {
                let BuiltinAssociatedValue::PrimitiveIntLimit { primitive, kind } = value;
                let Some(value) = kind.value(primitive, self.input.target.pointer_width) else {
                    return Err(ConstError {
                        span,
                        message: "builtin associated value is not representable at const"
                            .to_string(),
                    });
                };
                Ok(ConstValue::Int(value))
            }
            ConstNameResolution::AssociatedConstProjection(projection) => {
                match self.resolve_associated_const_projection(&projection) {
                    Some(nia_trait_solve::AssociatedConstResolution::Const(arg)) => {
                        const_value_from_const_generic_arg(&arg).ok_or_else(|| ConstError {
                            span,
                            message: format!(
                                "failed to evaluate associated const value `{}`",
                                self.symbol_name(projection.name)
                            ),
                        })
                    }
                    Some(nia_trait_solve::AssociatedConstResolution::User(user)) => {
                        self.eval_user_associated_const(span, *user)
                    }
                    None => Err(ConstError {
                        span,
                        message: format!(
                            "failed to resolve associated const value `{}`",
                            self.symbol_name(projection.name)
                        ),
                    }),
                }
            }
        }
    }

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &ResolvedConstTypeArg,
    ) -> Result<ConstValue, ConstError> {
        let module_id = self.current_execution_module_id();
        let ty_id = (|| {
            self.ensure_type_context(module_id)?;
            self.type_for_module_or_none(type_arg.ty(), module_id)
        })()
        .map(|ty| self.substitute_ty_generics(ty));
        let Some(ty_id) = ty_id else {
            return Err(ConstError {
                span,
                message: format!(
                    "cannot resolve type argument for const builtin `@{}`",
                    builtin.name()
                ),
            });
        };
        self.resolve_layout_builtin_for_ty(span, builtin, ty_id)
    }

    fn resolve_resolved_field_offset_builtin(
        &mut self,
        span: Span,
        type_arg: &ResolvedConstTypeArg,
        field: &SymbolId,
    ) -> Result<ConstValue, ConstError> {
        let module_id = self.current_execution_module_id();
        let ty_id = (|| {
            self.ensure_type_context(module_id)?;
            self.type_for_module_or_none(type_arg.ty(), module_id)
        })()
        .map(|ty| self.substitute_ty_generics(ty));
        let Some(ty_id) = ty_id else {
            return Err(ConstError {
                span,
                message: "cannot resolve type argument for const builtin `offset`".to_string(),
            });
        };
        self.resolve_field_offset_builtin_for_ty(span, ty_id, field)
    }

    fn call_resolved_function(
        &mut self,
        span: Span,
        callee: &ResolvedConstExpr,
        type_args: &[ResolvedConstTypeArg],
        arg_exprs: &[ResolvedConstExpr],
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let Some(resolved_callee) = self.resolved_const_callee(callee) else {
            return Err(ConstError {
                span,
                message: "const expression can only call `const fn`".to_string(),
            });
        };
        let function_id = resolved_callee.function_id;
        let Some(signatures) = self.signatures_for_module(function_id.module_id) else {
            return Err(ConstError {
                span,
                message: "const expression can only call `const fn`".to_string(),
            });
        };
        let Some(signature) = signatures
            .as_ref()
            .functions
            .get(&function_id.def_id)
            .cloned()
        else {
            return Err(ConstError {
                span,
                message: "const expression can only call `const fn`".to_string(),
            });
        };
        if !signature.is_const {
            return Err(ConstError {
                span,
                message: "const expression can only call `const fn`".to_string(),
            });
        }
        let call_arg_exprs = resolved_callee
            .receiver
            .into_iter()
            .chain(arg_exprs.iter().cloned())
            .collect::<Vec<_>>();
        let instantiation =
            if let Some(instantiation) = self.resolved_call_instantiations.get(&span).cloned() {
                instantiation
            } else {
                self.instantiate_resolved_function_generics(
                    span,
                    ConstFunctionInstantiationInput {
                        signature_module_id: function_id.module_id,
                        signature: &signature,
                        type_args,
                        arg_exprs: &call_arg_exprs,
                        expected_return: None,
                        initial: resolved_callee.target_instantiation,
                    },
                )?
            };
        if let Some(value) =
            self.try_call_builtin_function(span, &signature, type_args, &call_arg_exprs, &args)?
        {
            return Ok(value);
        }
        let return_ty = self
            .substitute_ty_into_current_module(
                function_id.module_id,
                signature.return_type,
                &instantiation.type_substitutions,
            )
            .ok_or_else(|| ConstError {
                span,
                message: "cannot resolve const function return type".to_string(),
            })?;
        let Some(function) = self.const_function_body(function_id) else {
            return Err(ConstError {
                span,
                message: "selected `const fn` body is unavailable during constant evaluation"
                    .to_string(),
            });
        };
        let value = nia_const_eval::eval_resolved_const_function_call(
            nia_const_eval::ResolvedConstCallInput {
                span,
                function_id,
                function_module_id: function_id.module_id,
                function: &function,
                type_substitutions: instantiation.type_substitutions.into_iter().collect(),
                const_substitutions: instantiation.const_substitutions.into_iter().collect(),
                args,
            },
            self,
        )?;
        let return_ty = ConstValueType::Runtime(return_ty);
        let value = self.normalize_typed_const_value(value, &return_ty);
        self.validate_typed_value(span, &value, &return_ty);
        Ok(value)
    }

    fn bind_resolved_function_param(
        &mut self,
        span: Span,
        param: &ResolvedConstParam,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let ty = param
            .ty()
            .map(|ty| ConstValueType::Runtime(self.substitute_ty_generics(ty)));
        self.bind_local_value(span, param.local_id(), false, value, ty)
    }

    fn bind_resolved_function_local(
        &mut self,
        span: Span,
        binding: &ResolvedConstBinding,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let ty = binding
            .explicit_type()
            .map(|ty| ConstValueType::Runtime(self.substitute_ty_generics(ty)))
            .or_else(|| self.resolved_const_expr_type(binding.value(), None));
        self.bind_local_value(span, binding.local_id(), binding.is_mutable(), value, ty)
    }

    fn bind_resolved_pattern_local(
        &mut self,
        span: Span,
        _name: &SymbolId,
        local_id: LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let ty = self
            .find_local_binding_type(local_id)
            .map(|ty| ConstValueType::Runtime(self.substitute_ty_generics(ty)));
        self.bind_local_value(span, local_id, false, value, ty)
    }

    fn assign_resolved_local(
        &mut self,
        span: Span,
        target: &ResolvedConstAssignTarget,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        match target.kind() {
            ResolvedConstAssignTargetKind::Local { name, local_id, .. } => {
                self.assign_local_value(span, *local_id, name, value)
            }
        }
    }
}

fn const_value_from_const_generic_arg(arg: &nia_ty::ConstGenericArg) -> Option<ConstValue> {
    match arg.value {
        nia_ty::ConstGenericValue::Int(value) => Some(ConstValue::Int(value)),
        nia_ty::ConstGenericValue::Bool(value) => Some(ConstValue::Bool(value)),
        nia_ty::ConstGenericValue::Char(value) => Some(ConstValue::Int(
            nia_ty::IntConst::unsigned(value as u32 as u128),
        )),
        nia_ty::ConstGenericValue::GenericParam(_) | nia_ty::ConstGenericValue::ConstExpr(_) => {
            None
        }
    }
}

impl Analyzer<'_> {
    fn eval_user_associated_const(
        &mut self,
        span: Span,
        user: nia_trait_solve::UserAssociatedConst,
    ) -> Result<ConstValue, ConstError> {
        let key = ConstKey::Global(user.def_id);
        if !self.active.insert(key) {
            return Err(ConstError {
                span,
                message: "cyclic const dependency".to_string(),
            });
        }
        let Some(expr) = self.initializer_for_key(key) else {
            self.active.remove(&key);
            return Err(ConstError {
                span,
                message: "associated const value has no initializer".to_string(),
            });
        };
        self.ensure_type_context(user.impl_module_id);
        if !self.type_contexts.contains_key(&user.impl_module_id) {
            self.active.remove(&key);
            return Err(ConstError {
                span,
                message: "failed to prepare associated const evaluation".to_string(),
            });
        }
        let type_substitutions = user.substitutions.into_iter().collect::<SymbolMap<_>>();
        let const_substitutions = user
            .const_substitutions
            .into_iter()
            .collect::<SymbolMap<_>>();
        let frame = ConstCallFrame {
            module_id: Some(user.impl_module_id),
            function_id: None,
            type_substitutions,
            const_substitutions,
            ..ConstCallFrame::default()
        };
        self.call_locals.push(frame);
        let result = self.with_execution_module(user.impl_module_id, |this| {
            let expected_ty = this
                .explicit_type_for_key(key)
                .map(|ty| this.substitute_ty_generics(ty));
            let expected = expected_ty.map(ConstValueType::Runtime);
            let _ = this.resolved_const_expr_type(&expr, expected_ty);
            let value = nia_const_eval::eval_resolved_const_expr(&expr, this)?;
            let value = if let Some(expected) = expected {
                let value = this.normalize_typed_const_value(value, &expected);
                this.validate_typed_value(span, &value, &expected);
                value
            } else {
                value
            };
            Ok(value)
        });
        self.call_locals.pop();
        self.active.remove(&key);
        result
    }

    fn resolve_const_const_generic_arg(
        &self,
        mut arg: nia_ty::ConstGenericArg,
    ) -> nia_ty::ConstGenericArg {
        if let nia_ty::ConstGenericValue::GenericParam(name) = &arg.value
            && let Some(resolved) = self
                .active_execution_frames()
                .find_map(|frame| frame.const_substitutions.get(name))
        {
            arg = resolved.clone();
        }
        arg
    }
}

impl Analyzer<'_> {
    fn try_call_builtin_function(
        &mut self,
        span: Span,
        signature: &FunctionSignature,
        type_args: &[ResolvedConstTypeArg],
        _arg_exprs: &[ResolvedConstExpr],
        args: &[ConstValue],
    ) -> Result<Option<ConstValue>, ConstError> {
        let Some(builtin) = builtin_function(signature) else {
            return Ok(None);
        };
        match builtin {
            BuiltinFunction::ConstError => {
                if !type_args.is_empty() || args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `error` expects exactly one message argument".to_string(),
                    });
                }
                let Some(message) = const_string_message(&args[0]) else {
                    return Err(ConstError {
                        span,
                        message: "builtin `error` requires a const string message".to_string(),
                    });
                };
                Err(ConstError { span, message })
            }
            BuiltinFunction::Embed => {
                if !type_args.is_empty() || args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `embed` expects exactly one path argument".to_string(),
                    });
                }
                let Some(path) = const_string_message(&args[0]) else {
                    return Err(ConstError {
                        span,
                        message: "builtin `embed` requires a const string path".to_string(),
                    });
                };
                self.resolve_embed(span, &path).map(Some)
            }
            BuiltinFunction::SizeOf | BuiltinFunction::AlignOf => {
                if !args.is_empty() || type_args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: format!(
                            "builtin `{}` expects exactly one type argument and no value arguments",
                            builtin.name()
                        ),
                    });
                }
                let layout_builtin = match builtin {
                    BuiltinFunction::SizeOf => LayoutBuiltin::Size,
                    BuiltinFunction::AlignOf => LayoutBuiltin::Align,
                    _ => unreachable!(),
                };
                self.resolve_resolved_layout_builtin(span, layout_builtin, &type_args[0])
                    .map(Some)
            }
            BuiltinFunction::Offset => {
                if type_args.len() != 1 || args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `offset` expects one type argument and one field name"
                            .to_string(),
                    });
                }
                let Some(field) = self.const_string_symbol(span, &args[0])? else {
                    return Err(ConstError {
                        span,
                        message: "builtin `offset` requires a const string field name".to_string(),
                    });
                };
                self.resolve_resolved_field_offset_builtin(span, &type_args[0], &field)
                    .map(Some)
            }
            BuiltinFunction::CharFromU32 => {
                if !type_args.is_empty() || args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `charFromU32` expects exactly one value argument"
                            .to_string(),
                    });
                }
                let ConstValue::Int(value) = args[0] else {
                    return Err(ConstError {
                        span,
                        message: "builtin `charFromU32` requires a u32 value".to_string(),
                    });
                };
                let scalar = u32::try_from(value.bits()).ok().and_then(char::from_u32);
                Ok(Some(ConstValue::Optional(scalar.map(|value| {
                    Box::new(ConstValue::Int(nia_ty::IntConst::unsigned(
                        value as u32 as u128,
                    )))
                }))))
            }
            _ => Err(ConstError {
                span,
                message: format!(
                    "builtin `{}` is not supported in const function-call form",
                    builtin.name()
                ),
            }),
        }
    }
}

impl Analyzer<'_> {
    fn const_string_symbol(
        &self,
        span: Span,
        value: &ConstValue,
    ) -> Result<Option<SymbolId>, ConstError> {
        let Some(name) = const_string_message(value) else {
            return Ok(None);
        };
        self.input
            .symbols
            .intern(&name)
            .map(Some)
            .map_err(|collision| ConstError {
                span,
                message: format!(
                    "symbol collision for {}: `{}` and `{}`",
                    symbol_identity_key(collision.symbol),
                    collision.existing,
                    collision.incoming
                ),
            })
    }
}

fn builtin_function(signature: &FunctionSignature) -> Option<BuiltinFunction> {
    signature
        .attributes
        .iter()
        .find_map(|attribute| match attribute {
            FunctionAttribute::Builtin(builtin) => Some(*builtin),
            FunctionAttribute::Naked => None,
        })
}

fn const_string_message(value: &ConstValue) -> Option<String> {
    match value {
        ConstValue::String(value) => Some(value.clone()),
        ConstValue::Array(values) => values
            .iter()
            .map(|value| match value {
                ConstValue::Int(value) => {
                    let scalar = u32::try_from(value.bits()).ok()?;
                    char::from_u32(scalar)
                }
                _ => None,
            })
            .collect(),
        ConstValue::Pointer(value) => const_string_message(value),
        _ => None,
    }
}

impl Analyzer<'_> {
    fn bind_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        is_mutable: bool,
        value: ConstValue,
        ty: Option<ConstValueType>,
    ) -> Result<(), ConstError> {
        let (value, ty) = if let Some(ty) = ty {
            let value = self.normalize_typed_const_value(value, &ty);
            self.validate_typed_value(span, &value, &ty);
            (value, Some(ty))
        } else {
            (value, None)
        };
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ConstError {
                span,
                message: "internal const function frame is missing".to_string(),
            });
        };
        if is_mutable {
            frame.mutable_locals.insert(local_id);
        }
        frame.locals.insert(local_id, value.clone());
        if let Some(ty) = ty {
            let typed = TypedConstValue { value, ty };
            frame.local_types.insert(local_id, typed.ty.clone());
        }
        Ok(())
    }

    fn assign_local_value(
        &mut self,
        span: Span,
        local_id: LocalId,
        name: &SymbolId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        for index in (0..self.call_locals.len()).rev() {
            let execution_boundary = self.call_locals[index].module_id.is_some();
            if !self.call_locals[index].locals.contains_key(&local_id) {
                if execution_boundary {
                    break;
                }
                continue;
            }
            if !self.call_locals[index].mutable_locals.contains(&local_id) {
                let name = self.symbol_name(*name);
                return Err(ConstError {
                    span,
                    message: format!("cannot assign to immutable const local `{name}`"),
                });
            }
            let previous_ty = self.call_locals[index].local_types.get(&local_id).cloned();
            let previous_value = self.call_locals[index].locals.get(&local_id).cloned();
            let value = if let Some(previous_ty) = previous_ty.as_ref() {
                let value = self.normalize_typed_const_value(value, previous_ty);
                self.validate_typed_value(span, &value, previous_ty);
                value
            } else {
                value
            };
            if let Some(previous_value) = previous_value.as_ref() {
                let resolver = self.input.symbols.resolver();
                let symbol_name = |symbol| resolver.display(symbol).to_string();
                validate_assignment_shape(
                    &mut self.diagnostics,
                    span,
                    &value,
                    previous_value,
                    &symbol_name,
                );
            }
            let frame = &mut self.call_locals[index];
            frame.locals.insert(local_id, value.clone());
            return Ok(());
        }
        let name = self.symbol_name(*name);
        Err(ConstError {
            span,
            message: format!("unknown const assignment target `{name}`"),
        })
    }
}
