use crate::{
    ConstKey, ConstValueType, TypedConstValue,
    analyzer::{
        Analyzer, ConstCallFrame, ConstFunctionInstantiationInput, ResolvedConstCallee,
        ResolvedConstCalleeSelection,
    },
    support::{
        cast_const_integer, cast_float_to_float, cast_float_to_integer, cast_int_to_float,
        is_float_primitive, primitive_integer_layout, validate_assignment_shape,
    },
};
use nia_const_eval::{
    ConstAbiField, ConstAbiType, ConstAllocationId, ConstAllocationOrigin, ConstCommonEnv,
    ConstEndianness, ConstError, ConstPointerPathElem, ConstPointerValue, ConstScalarType,
    ConstUnionValue, ConstValue, ResolvedConstEnv,
};
use nia_const_ir::{
    ConstNameResolution, ResolvedConstAssignTarget, ResolvedConstAssignTargetKind,
    ResolvedConstBinding, ResolvedConstExpr, ResolvedConstGenericArg, ResolvedConstParam,
    ResolvedConstTypeArg,
};
use nia_defs::DefKind;
use nia_ids::{
    BuiltinConstValue, BuiltinFunction, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId,
    ModuleId, TraitId, ValueBuiltin,
};
use nia_item_signatures::{FunctionAttribute, FunctionSignature};
use nia_local_resolve::LocalKind;
use nia_sema_ir::BuiltinAssociatedValue;
use nia_span::Span;
use nia_symbol::symbol_identity_key;
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{IntConst, PrimitiveTy, TyKind};
use std::collections::BTreeMap;
use std::collections::HashSet;
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

    fn reference_const_value(
        &mut self,
        span: Span,
        value: ConstValue,
        is_readonly: bool,
    ) -> Result<ConstValue, ConstError> {
        if !self
            .call_locals
            .iter()
            .any(|frame| frame.is_execution_frame)
        {
            return Ok(ConstValue::Pointer(ConstPointerValue::Frozen {
                origin: ConstAllocationOrigin::new(Some(self.current_execution_module_id()), span),
                is_readonly,
                pointee: Box::new(value),
            }));
        }
        let allocation = self.next_const_allocation_id(span)?;
        let Some(frame) = self.call_locals.last_mut() else {
            unreachable!("nonempty const frame stack lost its last frame")
        };
        frame.temporary_allocations.insert(allocation, value);
        Ok(ConstValue::Pointer(ConstPointerValue::Place {
            allocation,
            path: Vec::new(),
        }))
    }

    fn dereference_const_pointer(
        &mut self,
        span: Span,
        pointer: &ConstPointerValue,
    ) -> Result<ConstValue, ConstError> {
        match pointer {
            ConstPointerValue::Frozen { pointee, .. } => Ok((**pointee).clone()),
            ConstPointerValue::Place { allocation, path } => {
                let root = self
                    .call_locals
                    .iter()
                    .rev()
                    .find_map(|frame| {
                        let local =
                            frame
                                .allocation_ids
                                .iter()
                                .find_map(|(local_id, candidate)| {
                                    (candidate == allocation)
                                        .then(|| frame.locals.get(local_id).cloned())
                                        .flatten()
                                });
                        local.or_else(|| frame.temporary_allocations.get(allocation).cloned())
                    })
                    .ok_or_else(|| ConstError {
                        span,
                        message: "const pointer refers to storage whose lifetime has ended"
                            .to_string(),
                    })?;
                read_const_pointer_path(span, root, path, self)
            }
        }
    }

    fn validate_const_root_result(
        &mut self,
        span: Span,
        value: &ConstValue,
    ) -> Result<(), ConstError> {
        validate_const_pointer_escape(value, &|_| false)
            .map_err(|message| ConstError { span, message })
    }

    fn validate_const_function_result(
        &mut self,
        span: Span,
        value: &ConstValue,
    ) -> Result<(), ConstError> {
        let Some(function_frame) = self.call_locals.last() else {
            return Err(ConstError {
                span,
                message: "const function frame is missing during pointer escape validation"
                    .to_string(),
            });
        };
        let owned = function_frame
            .allocation_ids
            .values()
            .copied()
            .chain(function_frame.temporary_allocations.keys().copied())
            .collect::<HashSet<_>>();
        let alive = self
            .call_locals
            .iter()
            .flat_map(|frame| {
                frame
                    .allocation_ids
                    .values()
                    .copied()
                    .chain(frame.temporary_allocations.keys().copied())
            })
            .collect::<HashSet<_>>();
        validate_const_pointer_escape(value, &|allocation| {
            alive.contains(&allocation) && !owned.contains(&allocation)
        })
        .map_err(|message| ConstError { span, message })
    }

    fn push_const_scope(&mut self, _span: Span) -> Result<(), ConstError> {
        self.call_locals.push(ConstCallFrame {
            is_execution_frame: true,
            ..ConstCallFrame::default()
        });
        Ok(())
    }

    fn pop_const_scope(&mut self) {
        self.call_locals.pop();
    }

    fn push_function_frame(&mut self, span: Span) -> Result<(), ConstError> {
        self.const_eval_budget.enter_call(span)?;
        self.call_locals.push(ConstCallFrame {
            is_execution_frame: true,
            ..ConstCallFrame::default()
        });
        self.resolved_expr_types
            .push(std::collections::HashMap::new());
        Ok(())
    }

    fn pop_function_frame(&mut self) {
        self.call_locals.pop();
        self.resolved_expr_types.pop();
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
        if let Some(function_id) = function_id {
            let return_type = self
                .function_signatures_for_module(function_id.module_id)
                .and_then(|signatures| {
                    signatures
                        .as_ref()
                        .functions
                        .get(&function_id.def_id)
                        .map(|signature| signature.return_type)
                })
                .map(|return_type| self.substitute_ty_generics(return_type));
            if let Some(return_type) = return_type
                && let Some(frame) = self.call_locals.last_mut()
            {
                frame.return_type = Some(return_type);
            }
        }
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
    fn reference_resolved_place(
        &mut self,
        span: Span,
        place: &nia_const_eval::ResolvedConstPlace,
        _value: ConstValue,
        _is_readonly: bool,
    ) -> Result<ConstValue, ConstError> {
        let allocation = self
            .call_local_allocation(place.local_id)
            .ok_or_else(|| ConstError {
                span,
                message: "const reference target has no live allocation".to_string(),
            })?;
        let path = place
            .path
            .iter()
            .map(|elem| match elem {
                nia_const_eval::ResolvedConstPlaceElem::Field(name) => {
                    ConstPointerPathElem::Field(*name)
                }
                nia_const_eval::ResolvedConstPlaceElem::Index(index) => {
                    ConstPointerPathElem::Index(*index)
                }
            })
            .collect();
        Ok(ConstValue::Pointer(ConstPointerValue::Place {
            allocation,
            path,
        }))
    }

    fn prepare_resolved_binding(
        &mut self,
        binding: &ResolvedConstBinding,
    ) -> Result<(), ConstError> {
        if !matches!(
            binding.value().kind(),
            nia_const_ir::ResolvedConstExprKind::StructLiteral { .. }
        ) {
            return Ok(());
        }
        let expected = binding
            .explicit_type()
            .map(|ty| self.substitute_ty_generics(ty));
        let _ = self.resolved_const_expr_type(binding.value(), expected);
        Ok(())
    }

    fn prepare_resolved_function_result(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Result<(), ConstError> {
        if !matches!(
            expr.kind(),
            nia_const_ir::ResolvedConstExprKind::StructLiteral { .. }
        ) {
            return Ok(());
        }
        let expected = self
            .active_execution_frames()
            .find_map(|frame| frame.return_type);
        let _ = self.resolved_const_expr_type(expr, expected);
        Ok(())
    }

    fn prepare_resolved_call_arguments(
        &mut self,
        span: Span,
        callee: &ResolvedConstExpr,
        generic_args: &[ResolvedConstGenericArg],
        args: &[ResolvedConstExpr],
    ) -> Result<(), ConstError> {
        if !args.iter().any(|arg| {
            matches!(
                arg.kind(),
                nia_const_ir::ResolvedConstExprKind::StructLiteral { .. }
            )
        }) {
            return Ok(());
        }
        let _ = self.resolved_const_call_return_type(span, callee, generic_args, args, None);
        Ok(())
    }

    fn prepare_resolved_assignment(
        &mut self,
        assign: &nia_const_ir::ResolvedConstAssign,
    ) -> Result<(), ConstError> {
        if matches!(
            assign.rhs().kind(),
            nia_const_ir::ResolvedConstExprKind::StructLiteral { .. }
        ) {
            self.check_resolved_const_assignment(assign.rhs().span(), assign);
        }
        Ok(())
    }

    fn build_resolved_aggregate(
        &mut self,
        span: Span,
        ty: Option<InternedTyId>,
        mut fields: BTreeMap<SymbolId, ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let ty = ty.or_else(|| {
            self.resolved_expr_types
                .last()
                .and_then(|types| types.get(&span).copied())
        });
        let Some(ty) = ty else {
            return Ok(ConstValue::Struct(fields));
        };
        let module_id = self.current_execution_module_id();
        self.ensure_type_context(module_id)
            .ok_or_else(|| ConstError {
                span,
                message: "const aggregate execution module type context is unavailable".to_string(),
            })?;
        let ty = self.substitute_ty_generics(ty);
        let Some((def_id, args, const_args)) = self.expected_nominal_parts(ty) else {
            return Ok(ConstValue::Struct(fields));
        };
        if self.def_kind_of(def_id) != Some(DefKind::Union) {
            return Ok(ConstValue::Struct(fields));
        }
        let signature = self.union_signature_for(def_id).ok_or_else(|| ConstError {
            span,
            message: "const union signature is unavailable".to_string(),
        })?;
        let field_tys = self
            .const_union_field_types(&signature, &args, &const_args)
            .ok_or_else(|| ConstError {
                span,
                message: "const union field types are unavailable".to_string(),
            })?;
        if fields.len() != 1 {
            return Err(ConstError {
                span,
                message: format!(
                    "const union literal requires exactly one field, got {}",
                    fields.len()
                ),
            });
        }
        let target =
            nia_layout::TargetDataLayout::from_pointer_width(self.input.target.pointer_width)
                .ok_or_else(|| ConstError {
                    span,
                    message: "const union evaluation requires a supported target pointer width"
                        .to_string(),
                })?;
        let mut abi_fields = BTreeMap::new();
        let mut field_layouts = Vec::with_capacity(field_tys.len());
        for (name, field_ty) in field_tys {
            let (abi, layout) = self
                .const_union_abi_type(span, field_ty, target)
                .ok_or_else(|| ConstError {
                    span,
                    message: format!(
                        "const union field `{}` requires a supported const ABI type",
                        self.symbol_name(name)
                    ),
                })?;
            abi_fields.insert(name, abi);
            field_layouts.push(layout);
        }
        let layout = nia_layout::union_layout_from_fields(field_layouts.iter());
        let endianness =
            ConstEndianness::from_target_name(&self.input.target.endian).ok_or_else(|| {
                ConstError {
                    span,
                    message: "const union evaluation requires `little` or `big` target endianness"
                        .to_string(),
                }
            })?;
        let (initial_field, value) = fields.pop_first().expect("one const union field");
        let storage_size = usize::try_from(layout.size).map_err(|_| ConstError {
            span,
            message: "const union storage size is not representable".to_string(),
        })?;
        ConstUnionValue::new(abi_fields, storage_size, initial_field, value, endianness)
            .map(ConstValue::Union)
            .map_err(|message| ConstError { span, message })
    }

    fn resolved_integer_semantics(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Option<nia_const_eval::ConstIntegerSemantics> {
        let mut cached = self
            .resolved_expr_types
            .last()
            .and_then(|types| types.get(&expr.span()).copied());
        if cached.is_none()
            && let Some(function_id) = self.current_execution_function_id()
            && let Some(tail) = self
                .const_function_body(function_id)
                .and_then(|function| function.body().tail().cloned())
            && tail.span().start <= expr.span().start
            && tail.span().end >= expr.span().end
        {
            let expected = self
                .active_execution_frames()
                .find_map(|frame| frame.return_type);
            let _ = self.resolved_const_expr_type(&tail, expected);
            cached = self
                .resolved_expr_types
                .last()
                .and_then(|types| types.get(&expr.span()).copied());
        }
        let ty = cached.or_else(|| self.resolved_const_expr_type(expr, None)?.runtime())?;
        let TyKind::Primitive(primitive) = self.ty_kind(ty)? else {
            return None;
        };
        let (bits, signed) = primitive_integer_layout(primitive, self.input.target.pointer_width)?;
        Some(nia_const_eval::ConstIntegerSemantics { bits, signed })
    }

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
        generic_args: &[ResolvedConstGenericArg],
        arg_exprs: &[ResolvedConstExpr],
        receiver_place: Option<&nia_const_eval::ResolvedConstPlace>,
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let resolved_callee = match self.resolved_const_callee(callee) {
            ResolvedConstCalleeSelection::Unique(callee) => callee,
            ResolvedConstCalleeSelection::NoMatch => {
                if let Some((want_start, _)) =
                    self.resolved_const_range_method(callee, generic_args, arg_exprs)
                {
                    let [receiver] = args.as_slice() else {
                        return Err(ConstError {
                            span,
                            message: "const range method requires one receiver".to_string(),
                        });
                    };
                    return nia_const_eval::eval_const_range_bound_value(
                        span,
                        receiver.clone(),
                        want_start,
                    );
                }
                return Err(ConstError {
                    span,
                    message: "const expression can only call `const fn`".to_string(),
                });
            }
            ResolvedConstCalleeSelection::Ambiguous => {
                return Err(ConstError {
                    span,
                    message: "ambiguous const method call".to_string(),
                });
            }
        };
        let function_id = resolved_callee.function_id;
        let Some(signatures) = self.function_signatures_for_module(function_id.module_id) else {
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
                        generic_args,
                        arg_exprs: &call_arg_exprs,
                        expected_return: None,
                        initial: resolved_callee.target_instantiation,
                    },
                )?
            };
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
        if let Some(value) = self.try_call_builtin_function(
            span,
            &signature,
            return_ty,
            generic_args,
            &call_arg_exprs,
            &args,
        )? {
            if builtin_function(&signature).is_some_and(|builtin| {
                matches!(
                    builtin,
                    BuiltinFunction::Splat
                        | BuiltinFunction::Extract
                        | BuiltinFunction::Insert
                        | BuiltinFunction::Bitmask
                )
            }) {
                let return_ty = ConstValueType::Runtime(return_ty);
                let value = self.normalize_typed_const_value(value, &return_ty);
                self.validate_typed_value(span, &value, &return_ty);
                return Ok(value);
            }
            return Ok(value);
        }
        let Some(function) = self.const_function_body(function_id) else {
            return Err(ConstError {
                span,
                message: "selected `const fn` body is unavailable during constant evaluation"
                    .to_string(),
            });
        };
        let output = nia_const_eval::eval_resolved_const_function_call(
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
        if let Some(receiver) = output.mutable_receiver {
            let Some(receiver_place) = receiver_place else {
                return Err(ConstError {
                    span,
                    message: "mutable const receiver requires a place".to_string(),
                });
            };
            nia_const_eval::write_resolved_const_place(span, receiver_place, receiver, self)?;
        }
        let return_ty = ConstValueType::Runtime(return_ty);
        let value = self.normalize_typed_const_value(output.value, &return_ty);
        self.validate_typed_value(span, &value, &return_ty);
        Ok(value)
    }

    fn resolved_for_iterator(
        &mut self,
        span: Span,
        iterable: &ResolvedConstExpr,
        value: ConstValue,
    ) -> Result<nia_const_eval::ResolvedConstIterator, ConstError> {
        let iterable_ty = self
            .resolved_const_expr_type(iterable, None)
            .and_then(|ty| ty.runtime())
            .ok_or_else(|| ConstError {
                span,
                message: "cannot resolve const Iterable type".to_string(),
            })?;
        if self.proves_trait_obligation(
            iterable_ty,
            TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            Vec::new(),
        ) {
            return Ok(nia_const_eval::ResolvedConstIterator {
                ty: iterable_ty,
                value,
            });
        }
        let iterator_ty = self
            .intern_current_ty(TyKind::Projection {
                self_ty: iterable_ty,
                trait_id: TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
                trait_args: Vec::new(),
                trait_const_args: Vec::new(),
                name: nia_symbol::known::ITER,
            })
            .map(|ty| self.normalize_projection(ty))
            .ok_or_else(|| ConstError {
                span,
                message: "cannot resolve const Iterable::Iter type".to_string(),
            })?;
        let callee = self
            .resolved_const_builtin_trait_method(
                span,
                iterable_ty,
                nia_ty::BuiltinTrait::Iterable,
                nia_symbol::known::ITER_METHOD,
            )
            .ok_or_else(|| ConstError {
                span,
                message: "const Iterable::iter requires a const trait implementation".to_string(),
            })?;
        let output = self.eval_selected_const_method(span, callee, vec![value])?;
        Ok(nia_const_eval::ResolvedConstIterator {
            ty: iterator_ty,
            value: output.value,
        })
    }

    fn resolved_iterator_next(
        &mut self,
        span: Span,
        iterator: nia_const_eval::ResolvedConstIterator,
    ) -> Result<(nia_const_eval::ResolvedConstIterator, ConstValue), ConstError> {
        let callee = self
            .resolved_const_builtin_trait_method(
                span,
                iterator.ty,
                nia_ty::BuiltinTrait::Iterator,
                nia_symbol::known::NEXT,
            )
            .ok_or_else(|| ConstError {
                span,
                message: "const Iterator::next requires a const trait implementation".to_string(),
            })?;
        let output = self.eval_selected_const_method(span, callee, vec![iterator.value])?;
        let value = output.mutable_receiver.ok_or_else(|| ConstError {
            span,
            message: "const Iterator::next must use a mutable receiver".to_string(),
        })?;
        Ok((
            nia_const_eval::ResolvedConstIterator {
                ty: iterator.ty,
                value,
            },
            output.value,
        ))
    }

    fn bind_resolved_function_param(
        &mut self,
        span: Span,
        param: &ResolvedConstParam,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let declared_ty = param
            .ty()
            .filter(|ty| !matches!(self.ty_kind(*ty), Some(TyKind::Error)))
            .or_else(|| {
                param.receiver()?;
                let function_id = self.current_execution_function_id()?;
                self.extension_method_target_ty(function_id)
            });
        let ty = declared_ty.map(|ty| ConstValueType::Runtime(self.substitute_ty_generics(ty)));
        self.bind_local_value(
            span,
            param.local_id(),
            param.receiver() == Some(nia_ids::ReceiverKind::Ref),
            value,
            ty,
        )
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
                self.assign_local_value(span, *local_id, Some(name), value)
            }
        }
    }

    fn assign_resolved_place_local(
        &mut self,
        span: Span,
        local_id: LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        self.assign_local_value(span, local_id, None, value)
    }
}

fn const_union_scalar_type(primitive: PrimitiveTy, pointer_width: u32) -> Option<ConstScalarType> {
    match primitive {
        PrimitiveTy::F32 => Some(ConstScalarType::Float32),
        PrimitiveTy::F64 => Some(ConstScalarType::Float64),
        PrimitiveTy::Bool => Some(ConstScalarType::Bool),
        PrimitiveTy::Char => Some(ConstScalarType::Char),
        PrimitiveTy::Void | PrimitiveTy::Never => None,
        primitive => primitive_integer_layout(primitive, pointer_width)
            .map(|(bits, signed)| ConstScalarType::Integer { bits, signed }),
    }
}

impl Analyzer<'_> {
    pub(super) fn const_union_abi_type(
        &mut self,
        span: Span,
        ty: InternedTyId,
        target: nia_layout::TargetDataLayout,
    ) -> Option<(ConstAbiType, nia_layout::TypeLayout)> {
        let ty = self.substitute_ty_generics(ty);
        let ty = self.normalized_ty(ty);
        match self.active_ty_kind(ty) {
            TyKind::Primitive(primitive) => {
                let scalar = const_union_scalar_type(primitive, self.input.target.pointer_width)?;
                Some((
                    ConstAbiType::Scalar(scalar),
                    nia_layout::primitive_layout(primitive, target),
                ))
            }
            TyKind::Pointer { elem, .. } => {
                let size = usize::try_from(target.pointer_size).ok()?;
                Some((
                    ConstAbiType::Pointer {
                        size,
                        pointee: elem,
                    },
                    nia_layout::TypeLayout {
                        size: target.pointer_size,
                        align: target.pointer_align,
                    },
                ))
            }
            TyKind::Array { len, elem } => {
                let len = match len {
                    nia_ty::ArrayLenTy::Builtin { builtin, ty } => {
                        let ty = self.substitute_ty_generics(ty);
                        let ConstValue::Int(value) =
                            self.resolve_layout_builtin_for_ty(span, builtin, ty).ok()?
                        else {
                            return None;
                        };
                        u64::try_from(value.bits()).ok()?
                    }
                    len => self.array_len_const_value(len)?,
                };
                let value_len = usize::try_from(len).ok()?;
                let (element, element_layout) = self.const_union_abi_type(span, elem, target)?;
                let layout = nia_layout::array_layout(&element_layout, len)?;
                let _ = element.byte_len()?.checked_mul(value_len)?;
                Some((
                    ConstAbiType::Array {
                        element: Box::new(element),
                        len: value_len,
                    },
                    layout,
                ))
            }
            TyKind::Vector { elem, lanes } => {
                let lane = const_union_scalar_type(elem, self.input.target.pointer_width)?;
                if lane == ConstScalarType::Char {
                    return None;
                }
                let layout = nia_layout::vector_layout(elem, lanes, target)?;
                Some((
                    ConstAbiType::Vector {
                        lane,
                        lanes: usize::try_from(lanes).ok()?,
                        size: usize::try_from(layout.size).ok()?,
                    },
                    layout,
                ))
            }
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } if self.def_kind_of(def_id) == Some(DefKind::Struct) => {
                let signature = self.struct_signature_for(def_id)?;
                let field_tys = self.const_struct_field_types(&signature, &args, &const_args)?;
                let struct_layout =
                    self.const_struct_instance_layout(ty, def_id, &args, &const_args, target)?;
                let size = usize::try_from(struct_layout.layout.size).ok()?;
                let mut fields = Vec::with_capacity(signature.fields.len());
                for field in &signature.fields {
                    let field_ty = field_tys.get(&field.name).copied()?;
                    let (abi, abi_layout) = self.const_union_abi_type(span, field_ty, target)?;
                    let field_layout = struct_layout
                        .fields
                        .iter()
                        .find(|layout| layout.def_id == field.def_id)?;
                    if field_layout.layout != abi_layout {
                        return None;
                    }
                    fields.push(ConstAbiField {
                        name: field.name,
                        offset: usize::try_from(field_layout.offset).ok()?,
                        ty: abi,
                    });
                }
                Some((ConstAbiType::Struct { fields, size }, struct_layout.layout))
            }
            TyKind::Nominal {
                def_id,
                args,
                const_args,
            } if self.def_kind_of(def_id) == Some(DefKind::Union) => {
                let signature = self.union_signature_for(def_id)?;
                let field_tys = self.const_union_field_types(&signature, &args, &const_args)?;
                let mut fields = BTreeMap::new();
                let mut field_layouts = Vec::with_capacity(signature.fields.len());
                for field in &signature.fields {
                    let field_ty = field_tys.get(&field.name).copied()?;
                    let (abi, layout) = self.const_union_abi_type(span, field_ty, target)?;
                    fields.insert(field.name, abi);
                    field_layouts.push(layout);
                }
                let layout = nia_layout::union_layout_from_fields(field_layouts.iter());
                let size = usize::try_from(layout.size).ok()?;
                Some((ConstAbiType::Union { fields, size }, layout))
            }
            _ => None,
        }
    }

    fn const_struct_instance_layout(
        &mut self,
        ty: InternedTyId,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[nia_ty::ConstGenericArg],
        target: nia_layout::TargetDataLayout,
    ) -> Option<nia_layout::StructLayout> {
        let module_id = self.current_execution_module_id();
        let array_lengths = self.program_array_lengths_for_layout(ty);
        let defs = self.global_defs(module_id)?;
        let signatures = self.signatures_for_module(module_id)?;
        let normalization = self.type_normalization_for_module(module_id)?;
        let array_length = |id| array_lengths.get(&id).copied();
        let layout_query = |module_id| self.compute_program_layout(module_id, &array_lengths);
        let program_struct = |def_id| {
            self.struct_signature_for(def_id)
                .map(|signature| nia_item_signatures::ProgramStructSignature { signature })
        };
        nia_layout::compute_struct_instance_layout_with_program_context(
            &nia_layout::LayoutComputationInput {
                type_store: self.input.type_store,
                defs: defs.as_ref(),
                signatures: signatures.as_ref(),
                root_types: &[],
                normalized: &normalization.as_ref().normalized,
                array_lengths: &array_length,
                target,
                program: nia_layout::ProgramLayoutContext {
                    symbols: Some(self.input.symbols),
                    layouts: Some(&layout_query),
                    array_lengths: Some(&array_length),
                    struct_: Some(&program_struct),
                    ..Default::default()
                },
            },
            nia_layout::InstanceLayoutRequest {
                def_id,
                args,
                const_args,
            },
        )
    }

    fn eval_selected_const_method(
        &mut self,
        span: Span,
        callee: ResolvedConstCallee,
        args: Vec<ConstValue>,
    ) -> Result<nia_const_eval::ResolvedConstCallOutput, ConstError> {
        let function_id = callee.function_id;
        let signature = self
            .function_signatures_for_module(function_id.module_id)
            .and_then(|signatures| {
                signatures
                    .as_ref()
                    .functions
                    .get(&function_id.def_id)
                    .cloned()
            })
            .ok_or_else(|| ConstError {
                span,
                message: "selected const trait method signature is unavailable".to_string(),
            })?;
        let function = self
            .const_function_body(function_id)
            .ok_or_else(|| ConstError {
                span,
                message: "selected const trait method body is unavailable".to_string(),
            })?;
        let type_substitutions = callee
            .target_instantiation
            .type_substitutions
            .into_iter()
            .collect::<Vec<_>>();
        let const_substitutions = callee
            .target_instantiation
            .const_substitutions
            .into_iter()
            .collect::<Vec<_>>();
        let mut output = nia_const_eval::eval_resolved_const_function_call(
            nia_const_eval::ResolvedConstCallInput {
                span,
                function_id,
                function_module_id: function_id.module_id,
                function: &function,
                type_substitutions: type_substitutions.clone(),
                const_substitutions,
                args,
            },
            self,
        )?;
        let return_ty = self
            .substitute_ty_into_current_module(
                function_id.module_id,
                signature.return_type,
                &type_substitutions.into_iter().collect(),
            )
            .ok_or_else(|| ConstError {
                span,
                message: "cannot resolve const trait method return type".to_string(),
            })?;
        let return_ty = ConstValueType::Runtime(return_ty);
        output.value = self.normalize_typed_const_value(output.value, &return_ty);
        self.validate_typed_value(span, &output.value, &return_ty);
        Ok(output)
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
            this.resolved_expr_types
                .push(std::collections::HashMap::new());
            let _ = this.resolved_const_expr_type(&expr, expected_ty);
            let evaluated = nia_const_eval::eval_resolved_const_expr(&expr, this);
            this.resolved_expr_types.pop();
            let value = evaluated?;
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
        return_ty: InternedTyId,
        generic_args: &[ResolvedConstGenericArg],
        _arg_exprs: &[ResolvedConstExpr],
        args: &[ConstValue],
    ) -> Result<Option<ConstValue>, ConstError> {
        let Some(builtin) = builtin_function(signature) else {
            return Ok(None);
        };
        let type_args = generic_args
            .iter()
            .map(|arg| match arg {
                ResolvedConstGenericArg::Type(arg) => Ok(arg),
                ResolvedConstGenericArg::Const(expr) => Err(ConstError {
                    span: expr.span(),
                    message: format!(
                        "builtin `{}` expects type generic arguments",
                        builtin.name()
                    ),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        match builtin {
            BuiltinFunction::ConstError => {
                if !type_args.is_empty() || args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `error` expects exactly one message argument".to_string(),
                    });
                }
                let Some(message) = self.resolve_const_string_message(span, &args[0])? else {
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
                let Some(path) = self.resolve_const_string_message(span, &args[0])? else {
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
                self.resolve_resolved_layout_builtin(span, layout_builtin, type_args[0])
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
                self.resolve_resolved_field_offset_builtin(span, type_args[0], &field)
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
            BuiltinFunction::Splat => {
                if args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `splat` expects exactly one lane value".to_string(),
                    });
                }
                let TyKind::Vector { lanes, .. } = self.active_ty_kind(return_ty) else {
                    return Err(ConstError {
                        span,
                        message: "builtin `splat` requires a concrete vector return type"
                            .to_string(),
                    });
                };
                Ok(Some(ConstValue::Vector(vec![
                    args[0].clone();
                    lanes as usize
                ])))
            }
            BuiltinFunction::Extract => {
                if args.len() != 2 {
                    return Err(ConstError {
                        span,
                        message: "builtin `extract` expects a vector and lane index".to_string(),
                    });
                }
                let ConstValue::Vector(values) = &args[0] else {
                    return Err(ConstError {
                        span,
                        message: "builtin `extract` requires a vector value".to_string(),
                    });
                };
                let index = const_vector_index(span, &args[1], "extract")?;
                values
                    .get(index)
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| ConstError {
                        span,
                        message: format!(
                            "builtin `extract` lane index {index} is out of range for {} lanes",
                            values.len()
                        ),
                    })
            }
            BuiltinFunction::Insert => {
                if args.len() != 3 {
                    return Err(ConstError {
                        span,
                        message: "builtin `insert` expects a vector, lane index, and lane value"
                            .to_string(),
                    });
                }
                let ConstValue::Vector(mut values) = args[0].clone() else {
                    return Err(ConstError {
                        span,
                        message: "builtin `insert` requires a vector value".to_string(),
                    });
                };
                let index = const_vector_index(span, &args[1], "insert")?;
                let lane_count = values.len();
                let Some(lane) = values.get_mut(index) else {
                    return Err(ConstError {
                        span,
                        message: format!(
                            "builtin `insert` lane index {index} is out of range for {lane_count} lanes"
                        ),
                    });
                };
                *lane = args[2].clone();
                Ok(Some(ConstValue::Vector(values)))
            }
            BuiltinFunction::Bitmask => {
                if args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `bitmask` expects exactly one mask vector".to_string(),
                    });
                }
                let ConstValue::Vector(values) = &args[0] else {
                    return Err(ConstError {
                        span,
                        message: "builtin `bitmask` requires a mask vector".to_string(),
                    });
                };
                if values.len() > 64 {
                    return Err(ConstError {
                        span,
                        message: "builtin `bitmask` supports at most 64 mask lanes".to_string(),
                    });
                }
                let mut mask = 0u64;
                for (index, value) in values.iter().enumerate() {
                    let ConstValue::Bool(value) = value else {
                        return Err(ConstError {
                            span,
                            message: "builtin `bitmask` requires boolean mask lanes".to_string(),
                        });
                    };
                    if *value {
                        mask |= 1u64 << index;
                    }
                }
                Ok(Some(ConstValue::Int(nia_ty::IntConst::unsigned(
                    u128::from(mask),
                ))))
            }
            BuiltinFunction::SliceLen => {
                if type_args.len() > 1 || args.len() != 1 {
                    return Err(ConstError {
                        span,
                        message: "builtin `sliceLen` expects exactly one value argument"
                            .to_string(),
                    });
                }
                let mut value = args[0].clone();
                loop {
                    value = match value {
                        ConstValue::Pointer(pointer) => {
                            self.dereference_const_pointer(span, &pointer)?
                        }
                        ConstValue::Array(values) => {
                            break Ok(Some(ConstValue::Int(nia_ty::IntConst::unsigned(
                                u128::try_from(values.len()).map_err(|_| ConstError {
                                    span,
                                    message: "const slice length is too large".to_string(),
                                })?,
                            ))));
                        }
                        _ => {
                            break Err(ConstError {
                                span,
                                message: "builtin `sliceLen` requires a slice pointer".to_string(),
                            });
                        }
                    };
                }
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

fn const_vector_index(span: Span, value: &ConstValue, builtin: &str) -> Result<usize, ConstError> {
    let ConstValue::Int(value) = value else {
        return Err(ConstError {
            span,
            message: format!("builtin `{builtin}` requires an integer lane index"),
        });
    };
    if value.is_signed() && value.as_i128().is_some_and(|value| value < 0) {
        return Err(ConstError {
            span,
            message: format!("builtin `{builtin}` lane index cannot be negative"),
        });
    }
    usize::try_from(value.bits()).map_err(|_| ConstError {
        span,
        message: format!("builtin `{builtin}` lane index is not representable"),
    })
}

impl Analyzer<'_> {
    fn const_string_symbol(
        &mut self,
        span: Span,
        value: &ConstValue,
    ) -> Result<Option<SymbolId>, ConstError> {
        let Some(name) = self.resolve_const_string_message(span, value)? else {
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

    fn resolve_const_string_message(
        &mut self,
        span: Span,
        value: &ConstValue,
    ) -> Result<Option<String>, ConstError> {
        if let ConstValue::Pointer(pointer) = value {
            let value = self.dereference_const_pointer(span, pointer)?;
            return self.resolve_const_string_message(span, &value);
        }
        Ok(const_string_message(value))
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
        ConstValue::Pointer(ConstPointerValue::Frozen { pointee, .. }) => {
            const_string_message(pointee)
        }
        ConstValue::Pointer(ConstPointerValue::Place { .. }) => None,
        _ => None,
    }
}

fn read_const_pointer_path(
    span: Span,
    root: ConstValue,
    path: &[ConstPointerPathElem],
    analyzer: &Analyzer<'_>,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(root);
    };
    let value = match (head, root) {
        (ConstPointerPathElem::Field(name), ConstValue::Struct(mut fields)) => {
            fields.remove(name).ok_or_else(|| ConstError {
                span,
                message: format!(
                    "unknown const pointer field `{}`",
                    analyzer.symbol_name(*name)
                ),
            })?
        }
        (ConstPointerPathElem::Field(name), ConstValue::Union(union)) => {
            union.read(*name).map_err(|message| ConstError {
                span,
                message: format!("{message} `{}`", analyzer.symbol_name(*name)),
            })?
        }
        (ConstPointerPathElem::Index(index), ConstValue::Array(values)) => {
            values.get(*index).cloned().ok_or_else(|| ConstError {
                span,
                message: format!("const pointer index {index} is out of bounds"),
            })?
        }
        (ConstPointerPathElem::Field(_), _) => {
            return Err(ConstError {
                span,
                message: "const pointer field projection requires an aggregate allocation"
                    .to_string(),
            });
        }
        (ConstPointerPathElem::Index(_), _) => {
            return Err(ConstError {
                span,
                message: "const pointer index projection requires an array allocation".to_string(),
            });
        }
    };
    read_const_pointer_path(span, value, tail, analyzer)
}

fn validate_const_pointer_escape(
    value: &ConstValue,
    place_may_escape: &impl Fn(ConstAllocationId) -> bool,
) -> Result<(), String> {
    fn validate(
        value: &ConstValue,
        place_may_escape: &impl Fn(ConstAllocationId) -> bool,
        inside_frozen_allocation: bool,
    ) -> Result<(), String> {
        match value {
            ConstValue::Pointer(ConstPointerValue::Frozen {
                is_readonly,
                pointee,
                ..
            }) => {
                if !is_readonly {
                    return Err(
                        "const value cannot retain a writable pointer to promoted temporary storage"
                            .to_string(),
                    );
                }
                validate(pointee, place_may_escape, true)
            }
            ConstValue::Pointer(ConstPointerValue::Place { allocation, .. }) => {
                if inside_frozen_allocation || !place_may_escape(*allocation) {
                    return Err(
                        "const value cannot retain a pointer to storage whose lifetime ends here"
                            .to_string(),
                    );
                }
                Ok(())
            }
            ConstValue::Array(values) | ConstValue::Vector(values) => values
                .iter()
                .try_for_each(|value| validate(value, place_may_escape, inside_frozen_allocation)),
            ConstValue::Struct(fields) => fields
                .values()
                .try_for_each(|value| validate(value, place_may_escape, inside_frozen_allocation)),
            ConstValue::Enum { payload, .. } => match payload {
                nia_const_eval::ConstEnumPayload::Unit => Ok(()),
                nia_const_eval::ConstEnumPayload::Tuple(values) => {
                    values.iter().try_for_each(|value| {
                        validate(value, place_may_escape, inside_frozen_allocation)
                    })
                }
                nia_const_eval::ConstEnumPayload::Named(fields) => {
                    fields.values().try_for_each(|value| {
                        validate(value, place_may_escape, inside_frozen_allocation)
                    })
                }
            },
            ConstValue::Optional(Some(value)) => {
                validate(value, place_may_escape, inside_frozen_allocation)
            }
            ConstValue::ErrorUnion(Ok(value)) | ConstValue::ErrorUnion(Err(value)) => {
                validate(value, place_may_escape, inside_frozen_allocation)
            }
            ConstValue::Union(union) => union.relocations().iter().try_for_each(|relocation| {
                validate(
                    &ConstValue::Pointer(relocation.pointer().clone()),
                    place_may_escape,
                    inside_frozen_allocation,
                )
            }),
            ConstValue::Int(_)
            | ConstValue::Float(_)
            | ConstValue::Bool(_)
            | ConstValue::String(_)
            | ConstValue::Range(_)
            | ConstValue::Optional(None) => Ok(()),
        }
    }

    validate(value, place_may_escape, false)
}

impl Analyzer<'_> {
    fn next_const_allocation_id(&mut self, span: Span) -> Result<ConstAllocationId, ConstError> {
        let allocation = ConstAllocationId::new(
            self.current_execution_module_id(),
            self.next_const_allocation_serial,
        );
        self.next_const_allocation_serial = self
            .next_const_allocation_serial
            .checked_add(1)
            .ok_or_else(|| ConstError {
                span,
                message: "const allocation identity space was exhausted".to_string(),
            })?;
        Ok(allocation)
    }

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
        let allocation = self.next_const_allocation_id(span)?;
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
        frame.allocation_ids.insert(local_id, allocation);
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
        name: Option<&SymbolId>,
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
                let name = name
                    .map(|name| self.symbol_name(*name))
                    .unwrap_or_else(|| "receiver".to_string());
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
        Err(ConstError {
            span,
            message: name.map_or_else(
                || "unknown const receiver writeback target".to_string(),
                |name| {
                    format!(
                        "unknown const assignment target `{}`",
                        self.symbol_name(*name)
                    )
                },
            ),
        })
    }
}
