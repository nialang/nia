// SPDX-License-Identifier: GPL-3.0-or-later
//! Scalar const identities known before a generic instance exists.
//!
//! These are semantic values, not failed evaluations or interpreter values.
//! Computations preserve typed operands and resolved callees. Concrete
//! execution continues to use the ordinary interpreter after substitution.
//!
//! Classifying an expression has three outcomes ([`SymbolicResult`]). It is
//! symbolic when its value depends on a generic instance, and then it is a
//! term that each instance executes. It is closed when ordinary evaluation can
//! compute it now. Otherwise it depends on an instance but has no term form,
//! and the reason is reported here, so no caller falls back to evaluating it.
use super::*;

/// The classification of a const expression in a generic body: `Ok(Some)`
/// is its term, `Ok(None)` means it is closed, and `Err` means it depends on
/// an instance but has no term form, which is already reported.
pub(crate) type SymbolicResult = Result<Option<ConstGenericArg>, SymbolicReported>;

/// An expression that depends on a generic instance has no term form; the
/// reason is already reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolicReported;

/// Classifies a resolved expression without executing a symbolic parameter or
/// layout query. Body checking uses the same classification as initializer and
/// array-length analysis. The diagnostics explain an expression that depends
/// on an instance but has no term form.
pub fn classify_symbolic_const_expr(
    input: TypedConstQueryInput<'_>,
    expr: &ResolvedConstExpr,
    expected: Option<InternedTyId>,
) -> nia_ice::IceResult<Result<Option<ConstGenericArg>, Vec<Diagnostic>>> {
    let mut analyzer = Analyzer::for_typed_query(input);
    let value = analyzer.symbolic_const_expr_with_expected(expr, expected);
    let diagnostics = std::mem::take(&mut analyzer.diagnostics);
    analyzer.finish(value.map_err(|SymbolicReported| diagnostics))
}

impl Analyzer<'_> {
    pub(super) fn symbolic_const_expr(&mut self, expr: &ResolvedConstExpr) -> SymbolicResult {
        self.symbolic_const_expr_with_expected(expr, None)
    }

    pub(super) fn symbolic_const_expr_with_expected(
        &mut self,
        expr: &ResolvedConstExpr,
        expected: Option<InternedTyId>,
    ) -> SymbolicResult {
        match expr.kind() {
            ResolvedConstExprKind::Name(ConstNameResolution::GenericParam(name)) => {
                if let Some(arg) = self
                    .active_execution_frames()
                    .find_map(|frame| frame.const_substitutions.get(name))
                    .cloned()
                {
                    return Ok(arg
                        .contains_generic(|ty| self.type_contains_generic(ty))
                        .then_some(arg));
                }
                Ok(self
                    .const_parameter_type_at(expr.span(), *name)
                    .map(|ty| ConstGenericArg {
                        ty,
                        value: ConstGenericValue::GenericParam(*name),
                    }))
            }
            ResolvedConstExprKind::Name(ConstNameResolution::Local(local)) => {
                if let Some((_, arg)) = self
                    .term_bindings
                    .iter()
                    .rev()
                    .find(|(binding, _)| binding == local)
                {
                    return Ok(Some(arg.clone()));
                }
                self.symbolic_local_const(*local)
            }
            ResolvedConstExprKind::LayoutBuiltin { builtin, type_arg } => {
                let ty = self.substitute_ty_generics(type_arg.ty());
                let Some(ty) = self.type_for_module_or_none(ty, self.current_execution_module_id())
                else {
                    return Ok(None);
                };
                if !self.type_contains_generic(ty) {
                    return Ok(None);
                }
                Ok(Some(ConstGenericArg {
                    ty: self.current_runtime_primitive_type(PrimitiveTy::Usize),
                    value: ConstGenericValue::Layout {
                        builtin: *builtin,
                        ty,
                    },
                }))
            }
            ResolvedConstExprKind::ConstArgument(arg) => Ok(arg
                .contains_generic(|ty| self.type_contains_generic(ty))
                .then(|| arg.clone())),
            ResolvedConstExprKind::Name(_)
            | ResolvedConstExprKind::Integer(_)
            | ResolvedConstExprKind::Char(_)
            | ResolvedConstExprKind::ByteChar(_)
            | ResolvedConstExprKind::Float(_)
            | ResolvedConstExprKind::Bool(_) => Ok(None),
            _ => {
                // A closed expression is ordinary evaluation's to compute and
                // report; only one that reads an instance's value is a term.
                if !self.reads_instance_value(expr) {
                    return Ok(None);
                }
                let before = self.diagnostics.len();
                self.resolved_expr_facts
                    .push(ResolvedConstExprFacts::default());
                let ty = self.resolved_const_arg_runtime_type(expr, expected);
                let term = match ty {
                    Some(ty) => self.const_term(expr, ty),
                    None => {
                        self.diagnostics.truncate(before);
                        Err(self.report_not_a_term(expr))
                    }
                };
                self.resolved_expr_facts.pop();
                match term {
                    Ok(arg) if arg.contains_generic(|ty| self.type_contains_generic(ty)) => {
                        Ok(Some(arg))
                    }
                    // The instance-dependent reads folded away, as in `N * 0`
                    // is not folded but `size[T]()` of a closed `T` is.
                    Ok(_) => {
                        self.diagnostics.truncate(before);
                        Ok(None)
                    }
                    Err(SymbolicReported) => Err(SymbolicReported),
                }
            }
        }
    }

    /// Whether `expr` reads a value only an instance has, through the forms a
    /// term is built from ([`Self::const_term`]): an unsubstituted const
    /// parameter, a symbolic local constant, a layout query of an open type,
    /// or an open const argument. Any other form is evaluated as a whole,
    /// and fails there if it reads such a value. This never executes the
    /// expression.
    fn reads_instance_value(&mut self, expr: &ResolvedConstExpr) -> bool {
        use nia_const_ir::{ResolvedConstMatchArmBodyKind as Body, ResolvedConstPatternKind as P};
        match expr.kind() {
            ResolvedConstExprKind::Name(ConstNameResolution::GenericParam(_))
            | ResolvedConstExprKind::Name(ConstNameResolution::Local(_))
            | ResolvedConstExprKind::LayoutBuiltin { .. }
            | ResolvedConstExprKind::ConstArgument(_) => {
                !matches!(self.symbolic_const_expr(expr), Ok(None))
            }
            ResolvedConstExprKind::Unary { expr: inner, .. }
            | ResolvedConstExprKind::Cast { expr: inner, .. } => self.reads_instance_value(inner),
            ResolvedConstExprKind::Binary { lhs, rhs, .. }
            | ResolvedConstExprKind::Index { lhs, index: rhs } => {
                self.reads_instance_value(lhs) || self.reads_instance_value(rhs)
            }
            ResolvedConstExprKind::ArrayLiteral { elems } => match elems.kind() {
                ResolvedConstArrayElementsKind::List(elems) => {
                    elems.iter().any(|elem| self.reads_instance_value(elem))
                }
                ResolvedConstArrayElementsKind::Repeat { value, count } => {
                    self.reads_instance_value(value) || self.reads_instance_value(count)
                }
            },
            ResolvedConstExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.reads_instance_value(cond)
                    || then_branch
                        .tail()
                        .is_some_and(|tail| self.reads_instance_value(tail))
                    || else_branch
                        .as_deref()
                        .is_some_and(|branch| self.reads_instance_value(branch))
            }
            ResolvedConstExprKind::Block(block) => block
                .tail()
                .is_some_and(|tail| self.reads_instance_value(tail)),
            ResolvedConstExprKind::Match(matched) => {
                self.reads_instance_value(matched.target())
                    || matched.arms().iter().any(|arm| {
                        arm.patterns().iter().any(|pattern| match pattern.kind() {
                            P::Expr(value) => self.reads_instance_value(value),
                            P::Range { start, end, .. } => {
                                self.reads_instance_value(start) || self.reads_instance_value(end)
                            }
                            _ => false,
                        }) || match arm.body().kind() {
                            Body::Expr(value) => self.reads_instance_value(value),
                            Body::Block(block) => block
                                .tail()
                                .is_some_and(|tail| self.reads_instance_value(tail)),
                            Body::Stmt(_) => false,
                        }
                    })
            }
            ResolvedConstExprKind::Call {
                callee,
                generic_args,
                args,
            } => {
                self.reads_instance_value(callee)
                    || args.iter().any(|arg| self.reads_instance_value(arg))
                    || generic_args.iter().any(|arg| match arg {
                        nia_const_ir::ResolvedConstGenericArg::Const(expr) => {
                            self.reads_instance_value(expr)
                        }
                        nia_const_ir::ResolvedConstGenericArg::Type(arg) => {
                            let ty = self.substitute_ty_generics(arg.ty());
                            self.type_contains_generic(ty)
                        }
                        nia_const_ir::ResolvedConstGenericArg::Infer(_) => false,
                    })
                    // An inferred argument, such as `width(N)` choosing
                    // `width[N]`, is in the call's instantiation.
                    || self.call_instantiations().get(&expr.span()).is_some_and(|inst| {
                        inst.const_substitutions.values().any(|arg| {
                            arg.contains_generic(|ty| self.type_contains_generic(ty))
                        }) || inst
                            .type_substitutions
                            .values()
                            .any(|ty| self.type_contains_generic(*ty))
                    })
            }
            ResolvedConstExprKind::Field { lhs, .. }
            | ResolvedConstExprKind::TupleField { lhs, .. } => self.reads_instance_value(lhs),
            _ => false,
        }
    }

    /// A typed term is constructed without executing its operators. In
    /// particular a closed operand of a lazy operator or an unselected branch
    /// is not evaluated while residualizing it, and one that fails is a
    /// deferred failure ([`nia_ty::ConstComputationKind::Fail`]).
    fn const_term(
        &mut self,
        expr: &ResolvedConstExpr,
        ty: InternedTyId,
    ) -> Result<ConstGenericArg, SymbolicReported> {
        use nia_ty::{ConstComputation, ConstComputationKind as C};
        let kind = match expr.kind() {
            ResolvedConstExprKind::Name(ConstNameResolution::GenericParam(_))
            | ResolvedConstExprKind::Name(ConstNameResolution::Local(_))
            | ResolvedConstExprKind::LayoutBuiltin { .. }
            | ResolvedConstExprKind::ConstArgument(_) => {
                if let Some(arg) = self.symbolic_const_expr(expr)? {
                    return Ok(arg);
                }
                return self.concrete_const_term(expr, ty);
            }
            ResolvedConstExprKind::Unary {
                op: ConstUnaryOp::Neg,
                expr: inner,
            } if matches!(inner.kind(), ResolvedConstExprKind::Integer(_)) => {
                return self.concrete_const_term(expr, ty);
            }
            ResolvedConstExprKind::Unary { op, expr: inner } => C::Unary {
                op: *op,
                operand: self.operand_term(inner)?,
            },
            ResolvedConstExprKind::Binary { op, lhs, rhs } => C::Binary {
                op: *op,
                lhs: self.operand_term(lhs)?,
                rhs: self.operand_term(rhs)?,
            },
            ResolvedConstExprKind::Cast { expr: inner, .. } => C::Cast(self.operand_term(inner)?),
            ResolvedConstExprKind::ArrayLiteral { elems } => match elems.kind() {
                ResolvedConstArrayElementsKind::List(elems) => C::ArrayList(
                    elems
                        .iter()
                        .map(|elem| self.operand_term(elem))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                ResolvedConstArrayElementsKind::Repeat { value, count } => C::ArrayRepeat {
                    value: self.operand_term(value)?,
                    count: self.operand_term(count)?,
                },
            },
            ResolvedConstExprKind::Index { lhs, index } => C::Index {
                array: self.operand_term(lhs)?,
                index: self.operand_term(index)?,
            },
            ResolvedConstExprKind::If {
                cond,
                then_branch,
                else_branch: Some(else_branch),
            } => {
                let Some(then_value) = then_branch
                    .tail()
                    .filter(|_| then_branch.stmts().is_empty())
                else {
                    return self.concrete_const_term(expr, ty);
                };
                C::If {
                    condition: self.operand_term(cond)?,
                    then_value: self.const_term(then_value, ty)?,
                    else_value: self.const_term(else_branch, ty)?,
                }
            }
            ResolvedConstExprKind::Block(block) if block.stmts().is_empty() => {
                let Some(tail) = block.tail() else {
                    return self.concrete_const_term(expr, ty);
                };
                return self.const_term(tail, ty);
            }
            ResolvedConstExprKind::Match(matched) => match self.match_term(matched, ty)? {
                Some(kind) => kind,
                None => return self.concrete_const_term(expr, ty),
            },
            ResolvedConstExprKind::Call { callee, args, .. } => {
                match self.call_term(expr, callee, args)? {
                    Some(kind) => kind,
                    None => return self.concrete_const_term(expr, ty),
                }
            }
            _ => return self.concrete_const_term(expr, ty),
        };
        let arg = ConstGenericArg {
            ty,
            value: ConstGenericValue::Computation(Arc::new(ConstComputation {
                module_id: self.current_execution_module_id(),
                span: expr.span(),
                kind,
            })),
        };
        // A closed subterm is its value, so `K + (1 as i32)` and `K + 1` are
        // one term; a closed subterm that fails stays a deferred failure.
        if arg.contains_generic(|ty| self.type_contains_generic(ty)) {
            Ok(arg)
        } else {
            self.concrete_const_term(expr, ty)
        }
    }

    /// An operand's term, typed by what checking recorded for it, or by its
    /// own inference when it has no recorded type.
    fn operand_term(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Result<ConstGenericArg, SymbolicReported> {
        let ty = self
            .resolved_expr_facts
            .last()
            .and_then(|facts| facts.types.get(&expr.span()))
            .copied()
            .or_else(|| {
                self.resolved_const_arg_runtime_type(expr, None)
                    .or_else(|| {
                        self.resolved_const_expr_with_numeric_defaults(expr)
                            .and_then(|ty| ty.runtime())
                    })
            });
        match ty {
            Some(ty) => self.const_term(expr, ty),
            None => {
                self.require_closed(expr)?;
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    expr.span(),
                    "the type of this const computation operand cannot be inferred",
                ));
                Err(SymbolicReported)
            }
        }
    }

    /// The `match` of a scalar as a term, or `None` when its form is not one
    /// (such as a pattern that destructures), so it is evaluated as a whole.
    fn match_term(
        &mut self,
        matched: &nia_const_ir::ResolvedConstMatch,
        ty: InternedTyId,
    ) -> Result<Option<nia_ty::ConstComputationKind>, SymbolicReported> {
        let scrutinee = self.operand_term(matched.target())?;
        let mut arms = Vec::new();
        for arm in matched.arms() {
            // A binding names the scrutinee only within its own arm.
            let bindings = self.term_bindings.len();
            let arm = self.match_arm_term(arm, &scrutinee, ty);
            self.term_bindings.truncate(bindings);
            match arm? {
                Some(arm) => arms.push(arm),
                None => return Ok(None),
            }
        }
        Ok(Some(nia_ty::ConstComputationKind::Match {
            scrutinee,
            arms,
        }))
    }

    fn match_arm_term(
        &mut self,
        arm: &nia_const_ir::ResolvedConstMatchArm,
        scrutinee: &ConstGenericArg,
        ty: InternedTyId,
    ) -> Result<Option<nia_ty::ConstMatchArm>, SymbolicReported> {
        use nia_const_ir::{ResolvedConstMatchArmBodyKind as Body, ResolvedConstPatternKind as P};
        let mut patterns = Vec::new();
        for pattern in arm.patterns() {
            patterns.push(match pattern.kind() {
                P::Wildcard { .. } => nia_ty::ConstPattern::Wildcard,
                P::Bind { local_id, .. } => {
                    self.term_bindings.push((*local_id, scrutinee.clone()));
                    nia_ty::ConstPattern::Wildcard
                }
                P::Expr(value) => {
                    nia_ty::ConstPattern::Value(self.const_term(value, scrutinee.ty)?)
                }
                P::Range {
                    start,
                    end,
                    inclusive,
                    ..
                } => nia_ty::ConstPattern::Range {
                    start: self.const_term(start, scrutinee.ty)?,
                    end: self.const_term(end, scrutinee.ty)?,
                    inclusive: *inclusive,
                },
                _ => return Ok(None),
            });
        }
        let body = match arm.body().kind() {
            Body::Expr(value) => Some(value),
            Body::Block(block) if block.stmts().is_empty() => block.tail(),
            Body::Block(_) | Body::Stmt(_) => None,
        };
        let Some(body) = body else {
            return Ok(None);
        };
        Ok(Some(nia_ty::ConstMatchArm {
            patterns,
            value: self.const_term(body, ty)?,
        }))
    }

    /// A `const fun` call as a term, or `None` when the callee is not one, so
    /// the call is evaluated as a whole.
    fn call_term(
        &mut self,
        expr: &ResolvedConstExpr,
        callee: &ResolvedConstExpr,
        args: &[ResolvedConstExpr],
    ) -> Result<Option<nia_ty::ConstComputationKind>, SymbolicReported> {
        let ResolvedConstCalleeSelection::Unique(callee) = self.resolved_const_callee(callee)
        else {
            return Ok(None);
        };
        let Some(signature) = self.function_signature(callee.function_id) else {
            return Ok(None);
        };
        if !signature.is_const {
            return Ok(None);
        }
        let Some(instantiation) = self.call_instantiations().get(&expr.span()).cloned() else {
            return Ok(None);
        };
        let mut type_args = instantiation
            .type_substitutions
            .into_iter()
            .collect::<Vec<_>>();
        let mut const_args = instantiation
            .const_substitutions
            .into_iter()
            .collect::<Vec<_>>();
        // SymbolId ordering is stable within a session and independent of
        // HashMap iteration. Names belong to the resolved declaration.
        type_args.sort_by_key(|(name, _)| *name);
        const_args.sort_by_key(|(name, _)| *name);
        let args = callee
            .receiver
            .as_deref()
            .into_iter()
            .chain(args)
            .map(|arg| self.operand_term(arg))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(nia_ty::ConstComputationKind::Call {
            function: callee.function_id,
            type_args,
            const_args,
            args,
        }))
    }

    /// A subexpression without a term form of its own, evaluated now. It is a
    /// value when evaluation succeeds. When evaluation fails, a closed
    /// subexpression becomes a deferred failure, reported only if an instance
    /// executes it; one that needs an instance's value cannot be a term, and
    /// that is reported here.
    fn concrete_const_term(
        &mut self,
        expr: &ResolvedConstExpr,
        ty: InternedTyId,
    ) -> Result<ConstGenericArg, SymbolicReported> {
        let before = self.diagnostics.len();
        let open_reads = self.open_reads;
        let read_failed_const = std::mem::replace(&mut self.read_failed_const, false);
        let value = nia_const_eval::eval_resolved_const_expr(expr, self);
        let failed_read = std::mem::replace(&mut self.read_failed_const, read_failed_const);
        self.read_failed_const |= failed_read;
        match value {
            Ok(value) => self.const_term_from_value(expr.span(), ty, value),
            Err(_) if self.open_reads > open_reads => {
                self.diagnostics.truncate(before);
                Err(self.report_not_a_term(expr))
            }
            // A failed constant was reported where it is declared.
            Err(_) if failed_read => {
                self.diagnostics.truncate(before);
                Err(SymbolicReported)
            }
            Err(error) => {
                self.diagnostics.truncate(before);
                Ok(ConstGenericArg {
                    ty,
                    value: ConstGenericValue::Computation(Arc::new(nia_ty::ConstComputation {
                        module_id: self.current_execution_module_id(),
                        span: error.span,
                        kind: nia_ty::ConstComputationKind::Fail(error.message.into()),
                    })),
                })
            }
        }
    }

    /// Reports an expression that needs an instance's value but has no term
    /// form; one that is closed is left to ordinary evaluation.
    fn require_closed(&mut self, expr: &ResolvedConstExpr) -> Result<(), SymbolicReported> {
        let before = self.diagnostics.len();
        let open_reads = self.open_reads;
        let read_failed_const = self.read_failed_const;
        let value = nia_const_eval::eval_resolved_const_expr(expr, self);
        self.read_failed_const = read_failed_const;
        self.diagnostics.truncate(before);
        if value.is_err() && self.open_reads > open_reads {
            return Err(self.report_not_a_term(expr));
        }
        Ok(())
    }

    fn report_not_a_term(&mut self, expr: &ResolvedConstExpr) -> SymbolicReported {
        let summary = "this expression depends on a generic parameter, but it is not a const \
                       computation each instance can evaluate";
        self.diagnostics.push(
            Diagnostic::user_error(codes::CONST, summary)
                .primary(expr.span(), summary)
                .help(
                    "a const computation consists of operators, casts, `if` and `match` on \
                     scalar values, array literals and indexing, and `const fun` calls; move \
                     other code into a `const fun`",
                )
                .finish(),
        );
        SymbolicReported
    }

    /// Closed array operands remain values in the residual term. In particular
    /// reading ARR must not force execution of an enclosing ARR[index] before
    /// a symbolic branch chooses it. Reuse ArrayList rather than introducing a
    /// second interpreter representation for immutable aggregate operands.
    /// A scalar must be a value of its type ([`ConstGenericValue::from_int`]):
    /// `300` in a `u8` position is an error, not `44`.
    fn const_term_from_value(
        &mut self,
        span: Span,
        ty: InternedTyId,
        value: ConstValue,
    ) -> Result<ConstGenericArg, SymbolicReported> {
        let value = match (self.ty_kind(ty), value) {
            (Some(TyKind::Array { elem, .. }), ConstValue::Array(values)) => {
                let values = values
                    .into_iter()
                    .map(|value| self.const_term_from_value(span, elem, value))
                    .collect::<Result<Vec<_>, _>>()?;
                ConstGenericValue::Computation(Arc::new(nia_ty::ConstComputation {
                    module_id: self.current_execution_module_id(),
                    span,
                    kind: nia_ty::ConstComputationKind::ArrayList(values),
                }))
            }
            (Some(TyKind::Primitive(primitive)), value) => {
                let converted = match value {
                    ConstValue::Int(value) => ConstGenericValue::from_int(
                        primitive,
                        value,
                        self.input.target.pointer_width(),
                    ),
                    ConstValue::Bool(value) => ConstGenericValue::from_bool(primitive, value),
                    _ => Err(nia_ty::ConstScalarError::KindMismatch(primitive)),
                };
                match converted {
                    Ok(value) => value,
                    Err(error) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::CONST,
                            span,
                            error.to_string(),
                        ));
                        return Err(SymbolicReported);
                    }
                }
            }
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::CONST,
                    span,
                    "a const computation operand must be a scalar or an array of scalars",
                ));
                return Err(SymbolicReported);
            }
        };
        Ok(ConstGenericArg { ty, value })
    }

    /// The classification of a local const where it is used. A use never
    /// reports why its initializer is not a term: the declaration does, once
    /// ([`Self::classify_local_initializer`]).
    pub(super) fn symbolic_local_const(&mut self, local: LocalId) -> SymbolicResult {
        let before = self.diagnostics.len();
        let result = self.classify_local_initializer(local);
        self.diagnostics.truncate(before);
        result
    }

    /// Classifies a local const's initializer with its declared type, and
    /// reports an initializer that depends on an instance but has no term form.
    pub(super) fn classify_local_initializer(&mut self, local: LocalId) -> SymbolicResult {
        // An invocation evaluates its own initializer with that invocation's
        // substitutions, never a declaration value cached across instances.
        if self.current_execution_function_id().is_some() {
            return Ok(None);
        }
        let module_id = self.current_execution_module_id();
        let key = (module_id, local);
        if let Some(result) = self.symbolic_local_cache.get(&key) {
            return result.clone();
        }
        let initializer = if module_id == self.input.defs.module_id {
            self.input.module.local_initializers().get(&local).cloned()
        } else {
            self.input
                .program
                .module
                .and_then(|module| module(module_id))
                .and_then(|module| module.local_initializers().get(&local).cloned())
        };
        let Some(initializer) = initializer else {
            return Ok(None);
        };
        if !self.active_symbolic_locals.insert(key) {
            return Ok(None);
        }
        let result = self
            .symbolic_const_expr_with_expected(initializer.value(), initializer.explicit_type());
        self.active_symbolic_locals.remove(&key);
        self.symbolic_local_cache.insert(key, result.clone());
        if let Ok(Some(value)) = &result
            && module_id == self.input.defs.module_id
        {
            self.symbolic_locals.insert(local, value.clone());
        }
        result
    }

    /// Name resolution already identified a const parameter. Read its declared
    /// type from the innermost lexical owner, including extension parameters.
    fn const_parameter_type_at(&self, span: Span, name: SymbolId) -> Option<InternedTyId> {
        let signatures = self.signatures_for_module(self.current_execution_module_id())?;
        let signatures = signatures.as_ref();
        let mut candidates = Vec::new();
        let mut add = |owner: Span, params: &[nia_item_signatures::GenericParamSignature]| {
            if owner.start <= span.start && span.end <= owner.end {
                for param in params {
                    if param.name == name
                        && let GenericParamSignatureKind::Const { ty } = param.kind
                    {
                        candidates.push((owner.end - owner.start, ty));
                    }
                }
            }
        };
        for signature in signatures.functions.values() {
            add(signature.span, &signature.generic_params);
        }
        for extension in &signatures.trait_impls {
            // Extension signatures retain component spans rather than an
            // enclosing span: `span` covers only the target type. Generic
            // parameters remain in scope in the trait reference, predicates,
            // associated types, and methods.
            add(extension.span, &extension.generic_params);
            if let Some(trait_span) = extension.trait_span {
                add(trait_span, &extension.generic_params);
            }
            for predicate in &extension.where_predicates {
                add(predicate.span, &extension.generic_params);
            }
            for associated_type in &extension.associated_types {
                add(associated_type.span, &extension.generic_params);
            }
            for method in &extension.methods {
                add(method.span, &extension.generic_params);
            }
            for value in &extension.associated_values {
                add(value.span, &extension.generic_params);
            }
        }
        for signature in signatures.traits.values() {
            add(signature.span, &signature.generic_params);
            for method in &signature.methods {
                add(method.signature.span, &method.signature.generic_params);
                add(method.signature.span, &signature.generic_params);
            }
        }
        for signature in signatures.structs.values() {
            add(signature.span, &signature.generic_params);
        }
        for signature in signatures.unions.values() {
            add(signature.span, &signature.generic_params);
        }
        for signature in signatures.enums.values() {
            add(signature.span, &signature.generic_params);
        }
        for signature in signatures.type_aliases.values() {
            add(signature.span, &signature.generic_params);
        }
        candidates
            .into_iter()
            .min_by_key(|(width, _)| *width)
            .map(|(_, ty)| ty)
    }
}
