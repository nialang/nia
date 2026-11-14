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

/// Converts an evaluated scalar to the value of a const of type `ty`, which
/// must be a primitive. The value must be representable in that type: this is
/// [`ConstGenericValue::from_int`], never a cast.
pub fn scalar_const_generic_value(
    value: ConstValue,
    ty: InternedTyId,
    store: &nia_ty::TypeStore,
    pointer_width: u32,
) -> Result<ConstGenericValue, String> {
    let Some(TyKind::Primitive(primitive)) = store.get(ty) else {
        return Err("const value does not have a scalar type".into());
    };
    match value {
        ConstValue::Int(value) => ConstGenericValue::from_int(*primitive, value, pointer_width),
        ConstValue::Bool(value) => ConstGenericValue::from_bool(*primitive, value),
        _ => Err(nia_ty::ConstScalarError::KindMismatch(*primitive)),
    }
    .map_err(|error| error.to_string())
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
        diagnostics: analyzer.diagnostics.into_vec(),
        provider_demands: analyzer.provider_demands,
        error_reported: analyzer.read_failed_const,
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
        diagnostics: analyzer.diagnostics.into_vec(),
        provider_demands: analyzer.provider_demands,
        error_reported: analyzer.read_failed_const,
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
                    _ => {
                        self.open_reads += 1;
                        Err(ConstError {
                            span,
                            message: "const computation requires a concrete generic argument"
                                .into(),
                        })
                    }
                }
            }
            ConstGenericValue::Layout { builtin, ty } => {
                let ty = self.substitute_ty_generics(*ty);
                if self.type_contains_generic(ty) {
                    self.open_reads += 1;
                }
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
                self.resolved_expr_facts
                    .push(ResolvedConstExprFacts::default());
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
                            .call_instantiations_mut()
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
                                .call_instantiations()
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
                                self.call_instantiations_mut()
                                    .insert(computation.span, previous);
                            }
                            None => {
                                self.call_instantiations_mut().remove(&computation.span);
                            }
                        }
                        result
                    }
                    // Reached only when this operand is selected.
                    C::Fail(message) => Err(ConstError {
                        span: computation.span,
                        message: message.to_string(),
                    }),
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
                            C::ArrayList(values) => ResolvedConstExprKind::ArrayLiteral {
                                elems: ResolvedConstArrayElements::list(
                                    values.iter().map(|value| *operand(value)).collect(),
                                ),
                            },
                            C::ArrayRepeat { value, count } => {
                                ResolvedConstExprKind::ArrayLiteral {
                                    elems: ResolvedConstArrayElements::repeat(
                                        *operand(value),
                                        *operand(count),
                                    ),
                                }
                            }
                            C::Index { array, index } => ResolvedConstExprKind::Index {
                                lhs: operand(array),
                                index: operand(index),
                            },
                            C::Match { scrutinee, arms } => {
                                let span = computation.span;
                                let arms = arms
                                    .iter()
                                    .map(|arm| {
                                        let patterns = arm
                                            .patterns
                                            .iter()
                                            .map(|pattern| match pattern {
                                                nia_ty::ConstPattern::Wildcard => {
                                                    ResolvedConstPattern::wildcard(span)
                                                }
                                                nia_ty::ConstPattern::Value(value) => {
                                                    ResolvedConstPattern::expr(*operand(value))
                                                }
                                                nia_ty::ConstPattern::Range {
                                                    start,
                                                    end,
                                                    inclusive,
                                                } => ResolvedConstPattern::range(
                                                    *operand(start),
                                                    *operand(end),
                                                    *inclusive,
                                                    span,
                                                ),
                                            })
                                            .collect();
                                        nia_const_ir::ResolvedConstMatchArm::new(
                                            span,
                                            patterns,
                                            nia_const_ir::ResolvedConstMatchArmBody::expr(
                                                *operand(&arm.value),
                                            ),
                                        )
                                    })
                                    .collect();
                                ResolvedConstExprKind::Match(Box::new(
                                    nia_const_ir::ResolvedConstMatch::new(
                                        span,
                                        *operand(scrutinee),
                                        arms,
                                    ),
                                ))
                            }
                            C::Call { .. } | C::Fail(_) => unreachable!("handled above"),
                        };
                        let expr = ResolvedConstExpr::from_parts(computation.span, kind);
                        if let Some(types) = self.resolved_expr_facts.last_mut() {
                            types.types.insert(computation.span, arg.ty);
                        }
                        nia_const_eval::eval_resolved_const_expr(&expr, self)
                    }
                };
                if let Err(error) = &result {
                    nia_const_eval::ConstCommonEnv::observe_const_error(self, error);
                }
                self.resolved_expr_facts.pop();
                self.execution_module_overrides.pop();
                result
            }
        }
    }
}
