// SPDX-License-Identifier: GPL-3.0-or-later
//! Scalar const identities known before a generic instance exists.
//!
//! These are semantic values, not failed evaluations or interpreter values.
//! Computations preserve typed operands and resolved callees. Concrete
//! execution continues to use the ordinary interpreter after substitution.
use super::*;

/// Classifies a resolved expression without executing a symbolic parameter or
/// layout query. Body checking uses the same classification as initializer and
/// array-length analysis.
pub fn classify_symbolic_const_expr(
    input: TypedConstQueryInput<'_>,
    expr: &ResolvedConstExpr,
) -> nia_ice::IceResult<Option<ConstGenericArg>> {
    let mut analyzer = Analyzer::for_typed_query(input);
    let value = analyzer.symbolic_const_expr(expr);
    analyzer.finish(value)
}

impl Analyzer<'_> {
    pub(super) fn symbolic_const_expr(
        &mut self,
        expr: &ResolvedConstExpr,
    ) -> Option<ConstGenericArg> {
        match expr.kind() {
            ResolvedConstExprKind::Name(ConstNameResolution::GenericParam(name)) => {
                if let Some(arg) = self
                    .active_execution_frames()
                    .find_map(|frame| frame.const_substitutions.get(name))
                    .cloned()
                {
                    return arg
                        .contains_generic(|ty| self.type_contains_generic(ty))
                        .then_some(arg);
                }
                let ty = self.const_parameter_type_at(expr.span(), *name)?;
                Some(ConstGenericArg {
                    ty,
                    value: ConstGenericValue::GenericParam(*name),
                })
            }
            ResolvedConstExprKind::Name(ConstNameResolution::Local(local)) => {
                self.symbolic_local_const(*local)
            }
            ResolvedConstExprKind::LayoutBuiltin { builtin, type_arg } => {
                let ty = self.substitute_ty_generics(type_arg.ty());
                let ty = self.type_for_module_or_none(ty, self.current_execution_module_id())?;
                if !self.type_contains_generic(ty) {
                    return None;
                }
                Some(ConstGenericArg {
                    ty: self.current_runtime_primitive_type(PrimitiveTy::Usize),
                    value: ConstGenericValue::Layout {
                        builtin: *builtin,
                        ty,
                    },
                })
            }
            ResolvedConstExprKind::Unary { .. }
            | ResolvedConstExprKind::Binary { .. }
            | ResolvedConstExprKind::Cast { .. }
            | ResolvedConstExprKind::ArrayLiteral { .. }
            | ResolvedConstExprKind::Index { .. }
            | ResolvedConstExprKind::Call { .. }
            | ResolvedConstExprKind::If { .. }
            | ResolvedConstExprKind::Block(_) => {
                let before = self.diagnostics.len();
                self.resolved_expr_types.push(HashMap::new());
                let ty = self.resolved_const_arg_runtime_type(expr, None);
                let value = ty.and_then(|ty| self.const_term(expr, ty));
                self.resolved_expr_types.pop();
                let value =
                    value.filter(|arg| arg.contains_generic(|ty| self.type_contains_generic(ty)));
                if value.is_none() {
                    self.diagnostics.truncate(before);
                }
                value
            }
            ResolvedConstExprKind::ConstArgument(arg) => arg
                .contains_generic(|ty| self.type_contains_generic(ty))
                .then(|| arg.clone()),
            _ => None,
        }
    }

    /// A typed term is constructed without executing its operators. In particular
    /// a closed RHS of a lazy operator is not evaluated while residualizing it.
    fn const_term(
        &mut self,
        expr: &ResolvedConstExpr,
        ty: InternedTyId,
    ) -> Option<ConstGenericArg> {
        use nia_ty::{ConstComputation, ConstComputationKind as C};
        let operand = |this: &mut Self, expr: &ResolvedConstExpr| {
            let ty = this
                .resolved_expr_types
                .last()
                .and_then(|types| types.get(&expr.span()))
                .copied()
                .or_else(|| {
                    this.resolved_const_arg_runtime_type(expr, None)
                        .or_else(|| {
                            this.resolved_const_expr_with_numeric_defaults(expr)
                                .and_then(|ty| ty.runtime())
                        })
                })?;
            this.const_term(expr, ty)
        };
        let kind = match expr.kind() {
            ResolvedConstExprKind::Name(ConstNameResolution::GenericParam(_))
            | ResolvedConstExprKind::Name(ConstNameResolution::Local(_))
            | ResolvedConstExprKind::LayoutBuiltin { .. } => {
                if let Some(arg) = self.symbolic_const_expr(expr) {
                    return Some(arg);
                }
                return self.concrete_const_term(expr, ty);
            }
            ResolvedConstExprKind::ConstArgument(arg) => return Some(arg.clone()),
            ResolvedConstExprKind::Unary {
                op: ConstUnaryOp::Neg,
                expr: inner,
            } if matches!(inner.kind(), ResolvedConstExprKind::Integer(_)) => {
                return self.concrete_const_term(expr, ty);
            }
            ResolvedConstExprKind::Unary { op, expr: inner } => C::Unary {
                op: *op,
                operand: operand(self, inner)?,
            },
            ResolvedConstExprKind::Binary { op, lhs, rhs } => C::Binary {
                op: *op,
                lhs: operand(self, lhs)?,
                rhs: operand(self, rhs)?,
            },
            ResolvedConstExprKind::Cast { expr: inner, .. } => C::Cast(operand(self, inner)?),
            ResolvedConstExprKind::ArrayLiteral { elems } => match elems.kind() {
                ResolvedConstArrayElementsKind::List(elems) => C::ArrayList(
                    elems
                        .iter()
                        .map(|elem| operand(self, elem))
                        .collect::<Option<Vec<_>>>()?,
                ),
                ResolvedConstArrayElementsKind::Repeat { value, count } => C::ArrayRepeat {
                    value: operand(self, value)?,
                    count: operand(self, count)?,
                },
            },
            ResolvedConstExprKind::Index { lhs, index } => C::Index {
                array: operand(self, lhs)?,
                index: operand(self, index)?,
            },
            ResolvedConstExprKind::If {
                cond,
                then_branch,
                else_branch,
            } if then_branch.stmts().is_empty() => C::If {
                condition: operand(self, cond)?,
                then_value: operand(self, then_branch.tail()?)?,
                else_value: operand(self, else_branch.as_deref()?)?,
            },
            ResolvedConstExprKind::Block(block) if block.stmts().is_empty() => {
                return self.const_term(block.tail()?, ty);
            }
            ResolvedConstExprKind::Call { callee, args, .. } => {
                let ResolvedConstCalleeSelection::Unique(callee) =
                    self.resolved_const_callee(callee)
                else {
                    return None;
                };
                let signature = self.function_signature(callee.function_id)?;
                if !signature.is_const {
                    return None;
                }
                let instantiation = self.resolved_call_instantiations.get(&expr.span())?.clone();
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
                    .map(|arg| operand(self, arg))
                    .collect::<Option<Vec<_>>>()?;
                C::Call {
                    function: callee.function_id,
                    type_args,
                    const_args,
                    args,
                }
            }
            _ => return self.concrete_const_term(expr, ty),
        };
        Some(ConstGenericArg {
            ty,
            value: ConstGenericValue::Computation(Arc::new(ConstComputation {
                module_id: self.current_execution_module_id(),
                span: expr.span(),
                kind,
            })),
        })
    }

    fn concrete_const_term(
        &mut self,
        expr: &ResolvedConstExpr,
        ty: InternedTyId,
    ) -> Option<ConstGenericArg> {
        let value = nia_const_eval::eval_resolved_const_expr(expr, self).ok()?;
        let value = match value {
            ConstValue::Int(value)
                if matches!(self.ty_kind(ty), Some(TyKind::Primitive(PrimitiveTy::Char))) =>
            {
                ConstGenericValue::Char(char::from_u32(value.bits().try_into().ok()?)?)
            }
            ConstValue::Int(value) => ConstGenericValue::Int(value),
            ConstValue::Bool(value) => ConstGenericValue::Bool(value),
            _ => return None,
        };
        Some(ConstGenericArg { ty, value })
    }

    pub(super) fn symbolic_local_const(&mut self, local: LocalId) -> Option<ConstGenericArg> {
        // An invocation evaluates its own initializer with that invocation's
        // substitutions, never a declaration value cached across instances.
        if self.current_execution_function_id().is_some() {
            return None;
        }
        let module_id = self.current_execution_module_id();
        let key = (module_id, local);
        if let Some(value) = self.symbolic_local_cache.get(&key) {
            return Some(value.clone());
        }
        let initializer = if module_id == self.input.defs.module_id {
            self.input.module.local_initializers().get(&local)?.clone()
        } else {
            (self.input.program.module?)(module_id)?
                .local_initializers()
                .get(&local)?
                .clone()
        };
        if !self.active_symbolic_locals.insert(key) {
            return None;
        }
        let value = self.symbolic_const_expr(initializer.value());
        self.active_symbolic_locals.remove(&key);
        if let Some(value) = &value {
            self.symbolic_local_cache.insert(key, value.clone());
            if module_id == self.input.defs.module_id {
                self.symbolic_locals.insert(local, value.clone());
            }
        }
        value
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
