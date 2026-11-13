// SPDX-License-Identifier: GPL-3.0-or-later
//! Reification into the existing interpreter, shared by semantic instantiation
//! and machine-IR lowering. There is no arithmetic implementation in this layer.
use super::*;
use nia_const_eval::ResolvedConstEnv;
use nia_ty::ConstComputationKind as C;

impl ConstExprEvaluation {
    /// Publishes a scalar only after both execution and typed validation succeed.
    pub fn into_const_generic_arg(
        mut self,
        arg: &ConstGenericArg,
        store: &nia_ty::TypeStore,
        pointer_width: u32,
    ) -> Result<ConstGenericArg, Vec<Diagnostic>> {
        let span = match &arg.value {
            ConstGenericValue::Computation(expr) => expr.span,
            _ => Span::default(),
        };
        let value = match self.value {
            Ok(value) => scalar_const_generic_value(value, arg.ty, store, pointer_width)
                .map_err(|message| ConstError { span, message }),
            Err(error) => Err(error),
        };
        match value {
            Ok(value)
                if !self
                    .diagnostics
                    .iter()
                    .any(|d| d.severity() == nia_diagnostic::Severity::Error) =>
            {
                Ok(ConstGenericArg { ty: arg.ty, value })
            }
            Ok(_) => Err(self.diagnostics),
            Err(error) => {
                let mut diagnostic =
                    Diagnostic::user_error_at(codes::CONST, error.span, error.message);
                diagnostic.source_path = self.error_source.map(|path| path.as_str().to_owned());
                self.diagnostics.push(diagnostic);
                Err(self.diagnostics)
            }
        }
    }
}

/// Converts an evaluated scalar using its checked runtime type and target.
pub fn scalar_const_generic_value(
    value: ConstValue,
    ty: InternedTyId,
    store: &nia_ty::TypeStore,
    pointer_width: u32,
) -> Result<ConstGenericValue, String> {
    match (store.get(ty), value) {
        (Some(TyKind::Primitive(PrimitiveTy::Bool)), ConstValue::Bool(value)) => {
            Ok(ConstGenericValue::Bool(value))
        }
        (Some(TyKind::Primitive(PrimitiveTy::Char)), ConstValue::Int(value)) => value
            .bits()
            .try_into()
            .ok()
            .and_then(char::from_u32)
            .map(ConstGenericValue::Char)
            .ok_or_else(|| "const character is not a Unicode scalar".into()),
        (Some(TyKind::Primitive(primitive)), ConstValue::Int(value))
            if primitive.is_integer() && value.fits_primitive_int(*primitive, pointer_width) =>
        {
            Ok(ConstGenericValue::Int(
                value
                    .cast_to_primitive_int(*primitive, pointer_width)
                    .expect("validated primitive integer"),
            ))
        }
        _ => Err("const computation result does not match its declared scalar type".into()),
    }
}

/// Evaluates a checked, substituted const term with ordinary const semantics.
pub fn evaluate_const_generic_arg(
    input: TypedConstQueryInput<'_>,
    arg: &ConstGenericArg,
) -> nia_ice::IceResult<ConstExprEvaluation> {
    let mut analyzer = Analyzer::for_typed_query(input);
    let span = match &arg.value {
        ConstGenericValue::Computation(expr) => expr.span,
        _ => Span::default(),
    };
    let expr =
        ResolvedConstExpr::from_parts(span, ResolvedConstExprKind::ConstArgument(arg.clone()));
    let value = nia_const_eval::eval_resolved_const_expr(&expr, &mut analyzer);
    analyzer.finish(())?;
    Ok(ConstExprEvaluation {
        value,
        error_source: analyzer.const_error_origin.map(|(_, path)| path),
        diagnostics: analyzer.diagnostics,
        provider_demands: analyzer.provider_demands,
    })
}

/// Executes a closed computation in a module snapshot, without checking its body again.
pub fn evaluate_const_computation(
    input: ConstInput<'_>,
    arg: &ConstGenericArg,
) -> nia_ice::IceResult<ConstExprEvaluation> {
    let mut analyzer = Analyzer::new(input);
    let span = match &arg.value {
        ConstGenericValue::Computation(expr) => expr.span,
        _ => Span::default(),
    };
    let expr =
        ResolvedConstExpr::from_parts(span, ResolvedConstExprKind::ConstArgument(arg.clone()));
    let value = nia_const_eval::eval_resolved_const_expr(&expr, &mut analyzer);
    analyzer.finish(())?;
    Ok(ConstExprEvaluation {
        value,
        error_source: analyzer.const_error_origin.map(|(_, path)| path),
        diagnostics: analyzer.diagnostics,
        provider_demands: analyzer.provider_demands,
    })
}

impl Analyzer<'_> {
    pub(super) fn eval_const_generic_arg(
        &mut self,
        span: Span,
        arg: &ConstGenericArg,
    ) -> Result<ConstValue, ConstError> {
        match &arg.value {
            ConstGenericValue::Int(value) => Ok(ConstValue::Int(*value)),
            ConstGenericValue::Bool(value) => Ok(ConstValue::Bool(*value)),
            ConstGenericValue::Char(value) => {
                Ok(ConstValue::Int(IntConst::unsigned(*value as u128)))
            }
            ConstGenericValue::GenericParam(name) => {
                let resolved = self
                    .active_execution_frames()
                    .find_map(|frame| frame.const_substitutions.get(name))
                    .cloned();
                match resolved {
                    Some(value) if value != *arg => self.eval_const_generic_arg(span, &value),
                    _ => Err(ConstError {
                        span,
                        message: "const computation requires a concrete generic argument".into(),
                    }),
                }
            }
            ConstGenericValue::Layout { builtin, ty } => {
                let ty = self.substitute_ty_generics(*ty);
                self.resolve_layout_builtin_for_ty(span, *builtin, ty)
            }
            ConstGenericValue::ConstExpr(id) => self
                .array_len_const_value(ArrayLenTy::ConstExpr(id.clone()))
                .map(|value| ConstValue::Int(IntConst::unsigned(value.into())))
                .ok_or_else(|| ConstError {
                    span,
                    message: "const expression has no evaluated value".into(),
                }),
            ConstGenericValue::Computation(computation) => {
                self.execution_module_overrides
                    .push((self.call_locals.len(), computation.module_id));
                let operand = |arg: &ConstGenericArg| {
                    Box::new(ResolvedConstExpr::from_parts(
                        computation.span,
                        ResolvedConstExprKind::ConstArgument(arg.clone()),
                    ))
                };
                self.resolved_expr_types.push(HashMap::new());
                // Each computation gets a separate typing/cache frame. Reified
                // operands expose their own type directly, so equal source spans
                // cannot overwrite a comparison's operand width with bool.
                let result = match &computation.kind {
                    C::Call {
                        function,
                        type_args,
                        const_args,
                        args,
                    } => {
                        let instantiation = ConstGenericInstantiation {
                            type_substitutions: type_args
                                .iter()
                                .map(|(name, ty)| (*name, self.substitute_ty_generics(*ty)))
                                .collect(),
                            const_substitutions: const_args.iter().cloned().collect(),
                        };
                        let previous = self
                            .resolved_call_instantiations
                            .insert(computation.span, instantiation);
                        let callee = ResolvedConstExpr::name(
                            computation.span,
                            ConstNameResolution::Global(*function),
                        );
                        let arg_exprs = args.iter().map(|arg| *operand(arg)).collect::<Vec<_>>();
                        let result = (|| {
                            let signature =
                                self.function_signature(*function)
                                    .ok_or_else(|| ConstError {
                                        span: computation.span,
                                        message:
                                            "const computation callee signature is unavailable"
                                                .into(),
                                    })?;
                            let instantiation = self
                                .resolved_call_instantiations
                                .get(&computation.span)
                                .expect("installed instantiation");
                            let generic_args = signature
                                .generic_params
                                .iter()
                                .map(|param| match param.kind {
                                    GenericParamSignatureKind::Type => instantiation
                                        .type_substitutions
                                        .get(&param.name)
                                        .map(|ty| {
                                            nia_const_ir::ResolvedConstGenericArg::Type(
                                                nia_const_ir::ResolvedConstTypeArg::new(
                                                    computation.span,
                                                    computation.span,
                                                    *ty,
                                                ),
                                            )
                                        }),
                                    GenericParamSignatureKind::Const { .. } => instantiation
                                        .const_substitutions
                                        .get(&param.name)
                                        .map(|arg| {
                                            nia_const_ir::ResolvedConstGenericArg::Const(*operand(
                                                arg,
                                            ))
                                        }),
                                })
                                .collect::<Option<Vec<_>>>()
                                .ok_or_else(|| ConstError {
                                    span: computation.span,
                                    message:
                                        "const computation callee substitutions are incomplete"
                                            .into(),
                                })?;
                            let values = arg_exprs
                                .iter()
                                .map(|expr| nia_const_eval::eval_resolved_const_expr(expr, self))
                                .collect::<Result<Vec<_>, _>>()?;
                            self.call_resolved_function(
                                computation.span,
                                &callee,
                                &generic_args,
                                &arg_exprs,
                                None,
                                values,
                            )
                        })();
                        match previous {
                            Some(previous) => {
                                self.resolved_call_instantiations
                                    .insert(computation.span, previous);
                            }
                            None => {
                                self.resolved_call_instantiations.remove(&computation.span);
                            }
                        }
                        result
                    }
                    kind => {
                        let kind = match kind {
                            C::Unary { op, operand: arg } => ResolvedConstExprKind::Unary {
                                op: *op,
                                expr: operand(arg),
                            },
                            C::Binary { op, lhs, rhs } => ResolvedConstExprKind::Binary {
                                op: *op,
                                lhs: operand(lhs),
                                rhs: operand(rhs),
                            },
                            C::Cast(inner) => ResolvedConstExprKind::Cast {
                                expr: operand(inner),
                                ty: arg.ty,
                            },
                            C::If {
                                condition,
                                then_value,
                                else_value,
                            } => ResolvedConstExprKind::If {
                                cond: operand(condition),
                                then_branch: ResolvedConstBlock::new(
                                    computation.span,
                                    vec![],
                                    Some(operand(then_value)),
                                ),
                                else_branch: Some(operand(else_value)),
                            },
                            C::Call { .. } => unreachable!(),
                        };
                        let expr = ResolvedConstExpr::from_parts(computation.span, kind);
                        if let Some(types) = self.resolved_expr_types.last_mut() {
                            types.insert(computation.span, arg.ty);
                        }
                        nia_const_eval::eval_resolved_const_expr(&expr, self)
                    }
                };
                if let Err(error) = &result {
                    nia_const_eval::ConstCommonEnv::observe_const_error(self, error);
                }
                self.resolved_expr_types.pop();
                self.execution_module_overrides.pop();
                result
            }
        }
    }
}
