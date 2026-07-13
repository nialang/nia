use crate::{
    ComptimeKey, ComptimeValueType, TypedComptimeValue,
    analyzer::{Analyzer, ComptimeCallFrame, ComptimeGenericInstantiation},
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
use nia_ids::{
    BuiltinComptime, BuiltinFunction, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId, ModuleId,
    ValueBuiltin,
};
use nia_item_signatures::{FunctionAttribute, FunctionSignature};
use nia_local_resolve::LocalKind;
use nia_sema_ir::BuiltinAssociatedValue;
use nia_span::Span;
use nia_symbol::symbol_identity_key;
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{IntConst, TyKind};
use std::path::{Path, PathBuf};

impl ComptimeCommonEnv for Analyzer<'_> {
    fn symbol_name(&self, symbol: SymbolId) -> String {
        Analyzer::symbol_name(self, symbol)
    }

    fn resolve_builtin_comptime(
        &mut self,
        span: Span,
        builtin: BuiltinComptime,
    ) -> Result<ComptimeValue, ComptimeError> {
        let value = match builtin {
            BuiltinComptime::TargetArch => {
                ComptimeValue::String(self.input.target.arch.as_str().to_string())
            }
            BuiltinComptime::TargetVendor => {
                ComptimeValue::String(self.input.target.vendor.as_str().to_string())
            }
            BuiltinComptime::TargetOs => {
                ComptimeValue::String(self.input.target.os.as_str().to_string())
            }
            BuiltinComptime::TargetEnv => {
                ComptimeValue::String(self.input.target.env.as_str().to_string())
            }
            BuiltinComptime::TargetAbi => {
                ComptimeValue::String(self.input.target.abi.as_str().to_string())
            }
            BuiltinComptime::TargetEndian => {
                ComptimeValue::String(self.input.target.endian.as_str().to_string())
            }
            BuiltinComptime::TargetPointerWidth => ComptimeValue::Int(IntConst::unsigned(
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
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = span;
        match builtin {
            ValueBuiltin::Error => Err(ComptimeError {
                span,
                message: "builtin `error` must be called with a message".to_string(),
            }),
        }
    }

    fn resolve_embed(&mut self, span: Span, path: &str) -> Result<ComptimeValue, ComptimeError> {
        if path.is_empty() {
            return Err(ComptimeError {
                span,
                message: "builtin `embed` path cannot be empty".to_string(),
            });
        }
        let Some(source_path) = self.current_execution_source_path() else {
            return Err(ComptimeError {
                span,
                message: "builtin `embed` cannot resolve the current source path".to_string(),
            });
        };
        let resolved = resolve_embed_path(source_path.as_str(), path);
        let bytes = std::fs::read(&resolved).map_err(|error| ComptimeError {
            span,
            message: format!("failed to embed `{}`: {error}", resolved.display()),
        })?;
        Ok(ComptimeValue::Array(
            bytes
                .into_iter()
                .map(|byte| ComptimeValue::Int(IntConst::unsigned(byte as u128)))
                .collect(),
        ))
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
        substitutions: Vec<(SymbolId, InternedTyId)>,
        const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    ) -> Result<(), ComptimeError> {
        let resolved_const_substitutions = const_substitutions
            .into_iter()
            .map(|(name, arg)| (name, self.resolve_comptime_const_generic_arg(arg)))
            .collect::<Vec<_>>();
        let Some(frame) = self.call_locals.last_mut() else {
            return Err(ComptimeError {
                span,
                message: "failed to bind comptime function type substitutions".to_string(),
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
            ComptimeNameResolution::GenericParam(name) => self
                .call_locals
                .iter()
                .rev()
                .find_map(|frame| frame.const_substitutions.get(&name))
                .and_then(comptime_value_from_const_generic_arg)
                .ok_or_else(|| ComptimeError {
                    span,
                    message: format!(
                        "failed to evaluate comptime generic parameter `{}`",
                        self.symbol_name(name)
                    ),
                }),
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
            ComptimeNameResolution::AssociatedComptimeProjection(projection) => {
                match self.resolve_associated_comptime_projection(&projection) {
                    Some(nia_trait_solve::AssociatedComptimeResolution::Const(arg)) => {
                        comptime_value_from_const_generic_arg(&arg).ok_or_else(|| ComptimeError {
                            span,
                            message: format!(
                                "failed to evaluate associated comptime value `{}`",
                                self.symbol_name(projection.name)
                            ),
                        })
                    }
                    Some(nia_trait_solve::AssociatedComptimeResolution::User(user)) => {
                        self.eval_user_associated_comptime(span, user)
                    }
                    None => Err(ComptimeError {
                        span,
                        message: format!(
                            "failed to resolve associated comptime value `{}`",
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
        field: &SymbolId,
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
                message: "cannot resolve type argument for comptime builtin `offset`".to_string(),
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
        let Some(signatures) = self.signatures_for_module(function_id.module_id) else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let Some(signature) = signatures
            .as_ref()
            .functions
            .get(&function_id.def_id)
            .cloned()
        else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let instantiation = if let Some(substitutions) =
            self.resolved_call_type_substitutions.get(&span).cloned()
        {
            ComptimeGenericInstantiation {
                type_substitutions: substitutions,
                const_substitutions: SymbolMap::default(),
            }
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
        if let Some(value) =
            self.try_call_builtin_function(span, &signature, type_args, arg_exprs, &args)?
        {
            return Ok(value);
        }
        let return_ty = self
            .substitute_ty_into_current_module(
                function_id.module_id,
                signature.return_type,
                &instantiation.type_substitutions,
            )
            .ok_or_else(|| ComptimeError {
                span,
                message: "cannot resolve comptime function return type".to_string(),
            })?;
        let Some(function) = self.comptime_function_body(function_id) else {
            return Err(ComptimeError {
                span,
                message: "comptime expression can only call `comptime fn`".to_string(),
            });
        };
        let value = nia_comptime_engine::eval_resolved_comptime_function_call(
            nia_comptime_engine::ResolvedComptimeCallInput {
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
        _name: &SymbolId,
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

fn comptime_value_from_const_generic_arg(arg: &nia_ty::ConstGenericArg) -> Option<ComptimeValue> {
    match arg.value {
        nia_ty::ConstGenericValue::Int(value) => Some(ComptimeValue::Int(value)),
        nia_ty::ConstGenericValue::Bool(value) => Some(ComptimeValue::Bool(value)),
        nia_ty::ConstGenericValue::Char(value) => Some(ComptimeValue::Int(
            nia_ty::IntConst::unsigned(value as u32 as u128),
        )),
        nia_ty::ConstGenericValue::GenericParam(_) | nia_ty::ConstGenericValue::ConstExpr(_) => {
            None
        }
    }
}

impl Analyzer<'_> {
    fn eval_user_associated_comptime(
        &mut self,
        span: Span,
        user: nia_trait_solve::UserAssociatedComptime,
    ) -> Result<ComptimeValue, ComptimeError> {
        let key = ComptimeKey::Global(user.def_id);
        if !self.active.insert(key) {
            return Err(ComptimeError {
                span,
                message: "cyclic comptime dependency".to_string(),
            });
        }
        let Some(expr) = self.initializer_for_key(key) else {
            self.active.remove(&key);
            return Err(ComptimeError {
                span,
                message: "associated comptime value has no initializer".to_string(),
            });
        };
        self.ensure_working_interner(user.impl_module_id);
        let Some(interner) = self.working_interners.get_mut(&user.impl_module_id) else {
            self.active.remove(&key);
            return Err(ComptimeError {
                span,
                message: "failed to prepare associated comptime evaluation".to_string(),
            });
        };
        let type_substitutions = user
            .substitutions
            .into_iter()
            .map(|(name, ty)| {
                (
                    name,
                    nia_ty::import_type_into(interner, &user.resolution_interner, ty),
                )
            })
            .collect::<SymbolMap<_>>();
        let const_substitutions = user
            .const_substitutions
            .into_iter()
            .map(|(name, mut arg)| {
                arg.ty = nia_ty::import_type_into(interner, &user.resolution_interner, arg.ty);
                (name, arg)
            })
            .collect::<SymbolMap<_>>();
        let frame = ComptimeCallFrame {
            module_id: Some(user.impl_module_id),
            function_id: None,
            type_substitutions,
            const_substitutions,
            ..ComptimeCallFrame::default()
        };
        self.call_locals.push(frame);
        let result = self.with_execution_module(user.impl_module_id, |this| {
            let expected_ty = this
                .explicit_type_for_key(key)
                .map(|ty| this.substitute_ty_generics(ty));
            let expected = expected_ty.map(ComptimeValueType::Runtime);
            let _ = this.resolved_comptime_expr_type(&expr, expected_ty);
            let value = nia_comptime_engine::eval_resolved_comptime_expr(&expr, this)?;
            let value = if let Some(expected) = expected {
                let value = this.normalize_typed_comptime_value(value, &expected);
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

    fn resolve_comptime_const_generic_arg(
        &self,
        mut arg: nia_ty::ConstGenericArg,
    ) -> nia_ty::ConstGenericArg {
        if let nia_ty::ConstGenericValue::GenericParam(name) = &arg.value
            && let Some(resolved) = self
                .call_locals
                .iter()
                .rev()
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
        type_args: &[ResolvedComptimeTypeArg],
        _arg_exprs: &[ResolvedComptimeExpr],
        args: &[ComptimeValue],
    ) -> Result<Option<ComptimeValue>, ComptimeError> {
        let Some(builtin) = builtin_function(signature) else {
            return Ok(None);
        };
        match builtin {
            BuiltinFunction::ComptimeError => {
                if !type_args.is_empty() || args.len() != 1 {
                    return Err(ComptimeError {
                        span,
                        message: "builtin `error` expects exactly one message argument".to_string(),
                    });
                }
                let Some(message) = comptime_string_message(&args[0]) else {
                    return Err(ComptimeError {
                        span,
                        message: "builtin `error` requires a comptime string message".to_string(),
                    });
                };
                Err(ComptimeError { span, message })
            }
            BuiltinFunction::Embed => {
                if !type_args.is_empty() || args.len() != 1 {
                    return Err(ComptimeError {
                        span,
                        message: "builtin `embed` expects exactly one path argument".to_string(),
                    });
                }
                let Some(path) = comptime_string_message(&args[0]) else {
                    return Err(ComptimeError {
                        span,
                        message: "builtin `embed` requires a comptime string path".to_string(),
                    });
                };
                self.resolve_embed(span, &path).map(Some)
            }
            BuiltinFunction::SizeOf | BuiltinFunction::AlignOf => {
                if !args.is_empty() || type_args.len() != 1 {
                    return Err(ComptimeError {
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
                    return Err(ComptimeError {
                        span,
                        message: "builtin `offset` expects one type argument and one field name"
                            .to_string(),
                    });
                }
                let Some(field) = self.comptime_string_symbol(span, &args[0])? else {
                    return Err(ComptimeError {
                        span,
                        message: "builtin `offset` requires a comptime string field name"
                            .to_string(),
                    });
                };
                self.resolve_resolved_field_offset_builtin(span, &type_args[0], &field)
                    .map(Some)
            }
            _ => Err(ComptimeError {
                span,
                message: format!(
                    "builtin `{}` is not supported in comptime function-call form",
                    builtin.name()
                ),
            }),
        }
    }
}

impl Analyzer<'_> {
    fn comptime_string_symbol(
        &self,
        span: Span,
        value: &ComptimeValue,
    ) -> Result<Option<SymbolId>, ComptimeError> {
        let Some(name) = comptime_string_message(value) else {
            return Ok(None);
        };
        self.input
            .symbols
            .intern(&name)
            .map(Some)
            .map_err(|collision| ComptimeError {
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

fn comptime_string_message(value: &ComptimeValue) -> Option<String> {
    match value {
        ComptimeValue::String(value) => Some(value.clone()),
        ComptimeValue::Array(values) => values
            .iter()
            .map(|value| match value {
                ComptimeValue::Int(value) => {
                    let scalar = u32::try_from(value.bits()).ok()?;
                    char::from_u32(scalar)
                }
                _ => None,
            })
            .collect(),
        ComptimeValue::Pointer(value) => comptime_string_message(value),
        _ => None,
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
        name: &SymbolId,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        for index in (0..self.call_locals.len()).rev() {
            if !self.call_locals[index].locals.contains_key(&local_id) {
                continue;
            }
            if !self.call_locals[index].mutable_locals.contains(&local_id) {
                let name = self.symbol_name(*name);
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
        Err(ComptimeError {
            span,
            message: format!("unknown comptime assignment target `{name}`"),
        })
    }
}
