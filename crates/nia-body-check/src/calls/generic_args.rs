// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use crate::aggregates::ConstEvalError;
use crate::literals::{has_numeric_literal_suffix, integer_literal_value};
use nia_ast::{BracketArg, ExprKind, PathSegmentKind, TypeKind, TypeRef};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{ConstExprId, GlobalDefId, InternedTyId, ModuleId};
use nia_item_signatures::{FunctionSignature, GenericParamSignature, GenericParamSignatureKind};
use nia_sema_ir::GenericInstantiation;
use nia_span::Span;
use nia_symbol::{SymbolId, SymbolMap};
use nia_ty::{ConstGenericArg, ConstGenericValue, PrimitiveTy, TyKind};

/// Type and const parameter bindings for one generic signature.
///
/// Type and const parameters share one binding set so a probe, an explicit
/// argument list, or an inference pass commits or discards both together.
#[derive(Debug, Clone, Default)]
pub(crate) struct GenericSubstitutions {
    pub(crate) types: SymbolMap<InternedTyId>,
    pub(crate) consts: SymbolMap<ConstGenericArg>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LoweredGenericArgs {
    pub(crate) type_args: Vec<InternedTyId>,
    pub(crate) const_args: Vec<ConstGenericArg>,
    pub(crate) substitutions: GenericSubstitutions,
}

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_recovery_bracket_args(&mut self, args: &[BracketArg]) {
        // An unresolved callable cannot select a type or value interpretation.
        for arg in args {
            if let Some(ty) = &arg.ty {
                let ty = self.ty_for_type(ty);
                if !self.is_error_recovery_ty(ty) {
                    continue;
                }
            }
            if let Some(value) = &arg.expr {
                self.check_expr(value);
            }
        }
    }

    /// Publishes the common explanation for an incomplete generic call.
    ///
    /// The summary remains stable for existing consumers; the note and help
    /// carry the actionable context that used to be lost in the one-line
    /// diagnostic. Keeping this constructor shared prevents function and
    /// method calls from drifting into different advice.
    pub(crate) fn report_uninferred_generic(&mut self, span: Span, name: SymbolId, is_const: bool) {
        let name = self.symbol_name(name);
        let summary = if is_const {
            format!("cannot infer const generic parameter `{name}`")
        } else {
            format!("cannot infer generic parameter `{name}`")
        };
        let (note, help) = if is_const {
            (
                format!(
                    "`{name}` is not determined by the call arguments, expected result, or other generic constraints"
                ),
                format!(
                    "provide an explicit const argument for `{name}` (for example `function[4](...)`), or pass an argument whose value determines it"
                ),
            )
        } else {
            (
                format!(
                    "`{name}` is not determined by the call arguments, expected result, or other generic constraints"
                ),
                format!(
                    "provide an explicit type argument for `{name}` (for example `function[Type](...)`), or add a type annotation that determines it"
                ),
            )
        };
        self.diagnostics.push(
            Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(span, summary)
                .note(note)
                .help(help)
                .finish(),
        );
    }

    pub(crate) fn report_conflicting_generic_type(
        &mut self,
        span: Span,
        name: SymbolId,
        existing: InternedTyId,
        actual: InternedTyId,
    ) {
        let name = self.symbol_name(name);
        let existing_name = self.ty_name(existing);
        let actual_name = self.ty_name(actual);
        let summary = format!(
            "conflicting inferred type for generic parameter `{name}`: expected {existing_name}, got {actual_name}"
        );
        self.diagnostics.push(
            Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(span, summary)
                .note(format!(
                    "the same generic parameter was already inferred as `{existing_name}`, but this argument requires `{actual_name}`"
                ))
                .help(format!(
                    "make the arguments agree on one type, or provide an explicit type argument for `{name}`"
                ))
                .finish(),
        );
    }

    pub(crate) fn report_conflicting_const_generic(
        &mut self,
        span: Span,
        name: SymbolId,
        existing: &ConstGenericArg,
        actual: &ConstGenericArg,
    ) {
        let name = self.symbol_name(name);
        let existing_value = self.const_generic_value_name(&existing.value);
        let actual_value = self.const_generic_value_name(&actual.value);
        let summary = format!("conflicting inferred value for const generic parameter `{name}`");
        self.diagnostics.push(
            Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(span, summary)
                .note(format!(
                    "`{name}` was inferred as `{existing_value}` earlier, but this argument requires `{actual_value}`"
                ))
                .help(format!(
                    "make the const arguments agree, or provide an explicit const argument for `{name}`"
                ))
                .finish(),
        );
    }

    pub(crate) fn const_generic_value_name(&self, value: &ConstGenericValue) -> String {
        match value {
            ConstGenericValue::Computation(_) => format!("`{}`", self.const_term_text(value, None)),
            ConstGenericValue::ConstExpr(id) => self
                .const_expr_source(id.module_id, &id.const_expr_id)
                .map_or_else(
                    || "an unevaluated constant expression".to_string(),
                    |source| format!("`{source}`"),
                ),
            ConstGenericValue::Char(value) => format!("`{value}`"),
            _ => self.const_term_text(value, None),
        }
    }

    /// A typed const argument written as Nia source, as in a type name.
    pub(crate) fn const_arg_name(&self, arg: &ConstGenericArg) -> String {
        match &arg.value {
            ConstGenericValue::Char(_) => self.const_generic_value_name(&arg.value),
            value => self.const_term_text(value, Some(arg.ty)),
        }
    }

    /// The source spelling of a const expression written in a type.
    pub(crate) fn const_expr_site_text(&self, id: &nia_ids::GlobalConstExprId) -> Option<String> {
        self.const_expr_source(id.module_id, &id.const_expr_id)
    }

    /// A const value written as Nia source, from its structure; `ty` is the
    /// type of the term holding it, which a cast renders as its target. A
    /// computation is rendered from its operands, never from the source it
    /// was written as: after substitution `Q + 1` is `3 + 1`, while its span
    /// still holds the declaration's `Q + 1`.
    fn const_term_text(&self, value: &ConstGenericValue, ty: Option<InternedTyId>) -> String {
        use nia_ty::ConstComputationKind as C;
        let term = match value {
            ConstGenericValue::Computation(term) => term,
            ConstGenericValue::GenericParam(name) => return self.symbol_name(*name),
            ConstGenericValue::Int(value) => {
                return value
                    .as_i128()
                    .map_or_else(|| value.bits().to_string(), |value| value.to_string());
            }
            ConstGenericValue::Bool(value) => return value.to_string(),
            ConstGenericValue::Char(value) => return format!("{value:?}"),
            ConstGenericValue::Layout { builtin, ty } => {
                return format!("@{}[{}]()", builtin.name(), self.ty_name(*ty));
            }
            ConstGenericValue::ConstExpr(id) => {
                return self
                    .const_expr_source(id.module_id, &id.const_expr_id)
                    .unwrap_or_else(|| "_".to_string());
            }
        };
        let text = |arg: &ConstGenericArg| self.const_term_text(&arg.value, Some(arg.ty));
        // An operand that is itself an operation is parenthesized, so the
        // rendering keeps the term's structure whatever the precedence.
        let operand = |arg: &ConstGenericArg| match &arg.value {
            ConstGenericValue::Computation(inner)
                if !matches!(
                    inner.kind,
                    C::Call { .. } | C::Index { .. } | C::ArrayList(_) | C::ArrayRepeat { .. }
                ) =>
            {
                format!("({})", text(arg))
            }
            _ => text(arg),
        };
        let list = |args: &mut dyn Iterator<Item = &ConstGenericArg>| {
            args.map(&text).collect::<Vec<_>>().join(", ")
        };
        match &term.kind {
            C::Unary { op, operand: arg } => match op {
                nia_ty::ConstUnaryOp::Neg => format!("-{}", operand(arg)),
                nia_ty::ConstUnaryOp::Not => format!("not {}", operand(arg)),
                nia_ty::ConstUnaryOp::BitNot => format!("~{}", operand(arg)),
                nia_ty::ConstUnaryOp::RefReadOnly => format!("&{}", operand(arg)),
                nia_ty::ConstUnaryOp::Ref => format!("&mut {}", operand(arg)),
                nia_ty::ConstUnaryOp::Deref => format!("{}.*", operand(arg)),
            },
            C::Binary { op, lhs, rhs } => {
                format!("{} {} {}", operand(lhs), binary_op_text(*op), operand(rhs))
            }
            C::Cast(arg) => format!(
                "{} as {}",
                operand(arg),
                ty.map_or_else(|| "_".to_string(), |ty| self.ty_name(ty))
            ),
            C::If {
                condition,
                then_value,
                else_value,
            } => format!(
                "if {} {{ {} }} else {{ {} }}",
                text(condition),
                text(then_value),
                text(else_value)
            ),
            C::ArrayList(values) => format!("[{}]", list(&mut values.iter())),
            C::ArrayRepeat { value, count } => format!("[{}; {}]", text(value), text(count)),
            C::Index { array, index } => format!("{}[{}]", operand(array), text(index)),
            C::Call {
                function,
                const_args,
                args,
                ..
            } => {
                let name = self
                    .defs_for_module(function.module_id)
                    .and_then(|defs| {
                        defs.as_ref()
                            .defs
                            .get(function.def_id)
                            .map(|def| self.symbol_name(def.name))
                    })
                    .unwrap_or_else(|| "_".to_string());
                let generic = if const_args.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", list(&mut const_args.iter().map(|(_, arg)| arg)))
                };
                format!("{name}{generic}({})", list(&mut args.iter()))
            }
            C::Match { scrutinee, arms } => {
                let arms = arms
                    .iter()
                    .map(|arm| {
                        let patterns = arm
                            .patterns
                            .iter()
                            .map(|pattern| match pattern {
                                nia_ty::ConstPattern::Wildcard => "_".to_string(),
                                nia_ty::ConstPattern::Value(value) => text(value),
                                nia_ty::ConstPattern::Range {
                                    start,
                                    end,
                                    inclusive,
                                } => format!(
                                    "{}{}{}",
                                    text(start),
                                    if *inclusive { "..=" } else { ".." },
                                    text(end)
                                ),
                            })
                            .collect::<Vec<_>>()
                            .join(" | ");
                        format!("{patterns} => {}", text(&arm.value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("match {} {{ {arms} }}", text(scrutinee))
            }
            C::Fail(_) => self
                .source_expression(term.module_id, term.span)
                .unwrap_or_else(|| "_".to_string()),
        }
    }

    fn const_expr_source(&self, module_id: ModuleId, id: &ConstExprId) -> Option<String> {
        if module_id == self.defs.module_id
            && let Some(expr) = self
                .type_lowering
                .const_exprs
                .get(&nia_ids::GlobalConstExprId {
                    module_id,
                    const_expr_id: id.clone(),
                })
        {
            return self.source_expression(module_id, expr.span);
        }
        let ConstExprId::Syntax(node) = id else {
            return None;
        };
        let nia_node_id::NodePosition::Span(span) = node.position() else {
            return None;
        };
        self.source_expression(module_id, *span)
    }

    fn source_expression(&self, module_id: ModuleId, span: Span) -> Option<String> {
        let source = if module_id == self.defs.module_id {
            self.source_text.get(span.start..span.end)?
        } else {
            let source = (self.program.module_source_text?)(module_id)?;
            return source
                .get(span.start..span.end)
                .map(str::trim)
                .filter(|source| !source.is_empty())
                .map(str::to_owned);
        };
        let source = source.trim();
        (!source.is_empty()).then(|| source.to_owned())
    }

    pub(super) fn lower_bracket_type_args(
        &mut self,
        type_args: &[BracketArg],
    ) -> Vec<InternedTyId> {
        let mut lowered = Vec::new();
        for arg in type_args {
            if let Some(ty) = &arg.ty {
                if matches!(ty.kind, TypeKind::Infer) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        arg.span,
                        "`_` inference is not available for this generic argument list",
                    ));
                    lowered.push(self.error());
                    continue;
                }
                lowered.push(self.lower_required_bracket_type(ty, arg));
            } else {
                if let Some(expr) = &arg.expr {
                    let expr_ty = self.check_expr(expr);
                    if !self.is_error_recovery_ty(expr_ty) {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            arg.span,
                            "generic argument resolved as a value; expected a type",
                        ));
                    }
                } else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        arg.span,
                        "generic arguments must be types",
                    ));
                }
            }
        }
        lowered
    }

    fn lower_required_bracket_type(
        &mut self,
        ty: &nia_ast::TypeRef,
        arg: &BracketArg,
    ) -> InternedTyId {
        let lowered = self.ty_for_type(ty);
        self.report_nested_required_type_candidates(ty);
        if !self.is_error_recovery_ty(lowered)
            || !self
                .type_resolution
                .unresolved_type_candidates
                .contains(ty.node_key.site())
        {
            return lowered;
        }
        let TypeKind::Path { segments } = &ty.kind else {
            return lowered;
        };
        let Some(PathSegmentKind::Name(name)) = segments.last().map(|segment| &segment.kind) else {
            return lowered;
        };
        let name = self.symbol_name(*name);
        let value_exists = arg.expr.as_ref().is_some_and(|expr| match &expr.kind {
            ExprKind::Ident(_) => self.values.node_names.get(&expr.node_key).is_some_and(
                |resolution| match resolution {
                    nia_value_resolve::ValueNameResolution::Def(_)
                    | nia_value_resolve::ValueNameResolution::External(_)
                    | nia_value_resolve::ValueNameResolution::Module => true,
                    nia_value_resolve::ValueNameResolution::LocalDeferred => self
                        .locals
                        .node_uses
                        .get(&expr.node_key)
                        .is_some_and(|local_use| {
                            !matches!(local_use, nia_local_resolve::LocalUse::Unresolved)
                        }),
                    nia_value_resolve::ValueNameResolution::Error => false,
                },
            ),
            ExprKind::Qualified { .. } => self
                .values
                .node_qualified_values
                .get(&expr.node_key)
                .is_some(),
            _ => false,
        });
        let summary = if value_exists {
            "generic argument resolved as a value; expected a type".to_string()
        } else {
            format!("unknown type `{name}`")
        };
        let code = if value_exists {
            codes::TYPE_CHECK
        } else {
            codes::NAME_RESOLUTION
        };
        self.diagnostics.push(
            Diagnostic::user_error(code, summary.clone())
                .primary(arg.span, summary)
                .help(if value_exists {
                    "use a type name in this generic argument"
                } else {
                    "check the type name, module path, and whether the type is public"
                })
                .finish(),
        );
        lowered
    }

    /// Type-candidate resolution records the unresolved leaf, not a recovered
    /// pointer/tuple/nominal wrapper around that leaf. Walk those wrappers so
    /// a required type argument still reports the source-owned unknown type.
    fn report_nested_required_type_candidates(&mut self, ty: &TypeRef) {
        struct RequiredTypes<'a, 'ast> {
            lowering: &'a nia_type_lower::TypeLowering,
            types: Vec<&'ast TypeRef>,
        }
        impl<'ast> nia_ast_walk::Visitor<'ast> for RequiredTypes<'_, 'ast> {
            fn visit_type(&mut self, ty: &'ast TypeRef) {
                // A const generic's unused type interpretation has no lowered
                // type site. Its expression owns any diagnostic instead.
                if self.lowering.ty_for_key(&ty.node_key).is_some() {
                    self.types.push(ty);
                    nia_ast_walk::walk_type(self, ty);
                }
            }

            fn visit_expr(&mut self, _: &'ast nia_ast::Expr) {}
        }
        let mut required = RequiredTypes {
            lowering: self.type_lowering,
            types: Vec::new(),
        };
        nia_ast_walk::walk_type(&mut required, ty);
        for nested in required.types {
            if !self
                .type_resolution
                .unresolved_type_candidates
                .contains(nested.node_key.site())
            {
                continue;
            }
            let TypeKind::Path { segments } = &nested.kind else {
                continue;
            };
            let Some(PathSegmentKind::Name(name)) = segments.last().map(|segment| &segment.kind)
            else {
                continue;
            };
            let summary = format!("unknown type `{}`", self.symbol_name(*name));
            self.diagnostics.push(
                Diagnostic::user_error(codes::NAME_RESOLUTION, summary.clone())
                    .primary(nested.span, summary)
                    .help("check the type name, module path, and whether the type is public")
                    .finish(),
            );
        }
    }

    pub(super) fn lower_bracket_args_for_generic_params(
        &mut self,
        span: Span,
        params: &[GenericParamSignature],
        args: &[BracketArg],
    ) -> Option<LoweredGenericArgs> {
        self.lower_bracket_args_for_generic_params_impl(span, params, args, false)
    }

    pub(super) fn lower_call_bracket_args_for_generic_params(
        &mut self,
        span: Span,
        params: &[GenericParamSignature],
        args: &[BracketArg],
    ) -> Option<LoweredGenericArgs> {
        if args.len() != params.len() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "generic argument count mismatch: expected {}, got {}; use `_` for arguments that should be inferred",
                    params.len(),
                    args.len()
                ),
            ));
            return None;
        }
        self.lower_bracket_args_for_generic_params_impl(span, params, args, true)
    }

    fn lower_bracket_args_for_generic_params_impl(
        &mut self,
        span: Span,
        params: &[GenericParamSignature],
        args: &[BracketArg],
        allow_infer: bool,
    ) -> Option<LoweredGenericArgs> {
        if args.len() > params.len() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "generic argument count mismatch: expected {}, got {}",
                    params.len(),
                    args.len()
                ),
            ));
            return None;
        }
        let mut lowered = LoweredGenericArgs::default();
        for (param, arg) in params.iter().zip(args) {
            if matches!(arg.ty.as_ref().map(|ty| &ty.kind), Some(TypeKind::Infer)) {
                if allow_infer {
                    continue;
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    arg.span,
                    "`_` inference is not available for this generic argument list",
                ));
                return None;
            }
            match &param.kind {
                GenericParamSignatureKind::Type => {
                    if let Some(ty) = &arg.ty {
                        // A substitution is a semantic type, so a length it
                        // writes, such as `[u8; N + 1]`, is its term; the
                        // spelling is kept for diagnostics elsewhere.
                        let ty = self.lower_required_bracket_type(ty, arg);
                        let ty = self.normalization.normalize(ty);
                        lowered.type_args.push(ty);
                        lowered.substitutions.types.insert(param.name, ty);
                    } else {
                        if let Some(expr) = &arg.expr {
                            let expr_ty = self.check_expr(expr);
                            if self.is_error_recovery_ty(expr_ty) {
                                // The expression already owns the source error;
                                // do not replace it with a generic category error.
                                return None;
                            }
                        }
                        let name = self.symbol_name(param.name);
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            arg.span,
                            format!("generic argument `{name}` must be a type"),
                        ));
                        return None;
                    }
                }
                GenericParamSignatureKind::Const { ty } => {
                    let diagnostics_before = self.diagnostics.len();
                    let Some(value) = self.const_generic_value_from_bracket_arg(arg, *ty) else {
                        if self.diagnostics.len() == diagnostics_before {
                            let name = self.symbol_name(param.name);
                            self.diagnostics.push(Diagnostic::user_error_at(
                                codes::TYPE_CHECK,
                                arg.span,
                                format!("generic argument `{name}` must be a const value"),
                            ));
                        }
                        return None;
                    };
                    let arg = ConstGenericArg { ty: *ty, value };
                    lowered.const_args.push(arg.clone());
                    lowered.substitutions.consts.insert(param.name, arg);
                }
            }
        }
        Some(lowered)
    }

    fn const_generic_value_from_bracket_arg(
        &mut self,
        arg: &BracketArg,
        expected_ty: InternedTyId,
    ) -> Option<ConstGenericValue> {
        if let Some(expr) = &arg.expr {
            let value = self.eval_const_generic_expr(expr, expected_ty)?;
            return self.const_generic_value_for_type(value, expected_ty);
        }
        None
    }

    pub(crate) fn eval_const_generic_expr(
        &mut self,
        expr: &nia_ast::Expr,
        expected_ty: InternedTyId,
    ) -> Option<ConstGenericValue> {
        self.with_const_context(|this| {
            // Check the expression on its own before applying the const
            // parameter's representation.  A const expression may produce an
            // integer in a wider type and then be range-checked/cast below;
            // feeding the parameter type as contextual typing here would
            // reject that expression before evaluation (for example a
            // `usize` const function used for a `u8` parameter).  It would
            // also turn a non-const value such as `true` into a generic type
            // mismatch instead of the normal "must be a const value"
            // diagnostic.
            // An unsuffixed literal has no source integer width yet. It takes
            // the parameter's width, exactly as at any other typed boundary;
            // defaulting it to i32 would reject valid u64/u128/isize values.
            // Computed expressions and suffixed literals keep their own type.
            let literal_expected = integer_literal_value(expr).is_some()
                && !has_numeric_literal_suffix(expr)
                && matches!(this.interner.get(this.normalization.normalize(expected_ty)),
                    Some(TyKind::Primitive(primitive)) if primitive.is_integer());
            let mut actual = if literal_expected {
                this.check_expr_with_expected(expr, Some(expected_ty))
            } else {
                this.check_expr(expr)
            };
            // A named constant can produce the recovery type without a
            // runtime projection. Retry with the parameter context only in
            // that case; ordinary expressions keep their own type so their
            // evaluated value can be range-checked even when its source type
            // is wider than the parameter type.
            if this.is_error_recovery_ty(actual) {
                let projected = this.check_expr_with_expected(expr, Some(expected_ty));
                if !this.is_error_recovery_ty(projected) {
                    actual = projected;
                }
            }
            if this.is_error_recovery_ty(actual) {
                return None;
            }
            match this.symbolic_const_expr(expr, Some(expected_ty)) {
                Ok(Some(arg)) => {
                    if !this.types_match(expected_ty, arg.ty) {
                        this.expect_expr_type(expr, expected_ty, arg.ty, "const generic argument");
                        return None;
                    }
                    return Some(arg.value);
                }
                Ok(None) => {}
                Err(diagnostics) => {
                    this.diagnostics.extend(diagnostics);
                    return None;
                }
            }
            let const_expr = match this.lower_const_expr(expr) {
                Ok(expr) => expr,
                Err(err) => {
                    this.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        err.span,
                        err.message,
                    ));
                    return None;
                }
            };
            let value = match this.evaluate_const_expr(&const_expr) {
                Ok(value) => value,
                Err(ConstEvalError::Reported) => return None,
                Err(ConstEvalError::Error(err)) => {
                    this.diagnostics.push(Diagnostic::user_error_at(
                        codes::CONST,
                        err.span,
                        err.message,
                    ));
                    return None;
                }
            };
            this.const_generic_value_from_const_value(expr.span, value, expected_ty)
        })
    }

    fn const_generic_value_from_const_value(
        &mut self,
        span: Span,
        value: nia_const_check::ConstValue,
        expected_ty: InternedTyId,
    ) -> Option<ConstGenericValue> {
        let primitive = self.const_generic_primitive(expected_ty)?;
        let converted = match value {
            nia_const_check::ConstValue::Int(value) => {
                ConstGenericValue::from_int(primitive, value, self.target.pointer_width())
            }
            nia_const_check::ConstValue::Bool(value) => {
                ConstGenericValue::from_bool(primitive, value)
            }
            _ => return None,
        };
        match converted {
            Ok(value) => Some(value),
            Err(nia_ty::ConstScalarError::KindMismatch(_)) => None,
            Err(error) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    error.to_string(),
                ));
                None
            }
        }
    }

    /// The canonical value of a const argument for a parameter of type
    /// `expected_ty`, or `None` when the value is of another kind. An integer
    /// takes the parameter type's canonical form ([`ConstGenericValue::from_int`]);
    /// a symbolic value keeps its form.
    fn const_generic_value_for_type(
        &mut self,
        value: ConstGenericValue,
        expected_ty: InternedTyId,
    ) -> Option<ConstGenericValue> {
        let primitive = self.const_generic_primitive(expected_ty)?;
        match value {
            ConstGenericValue::GenericParam(_)
            | ConstGenericValue::ConstExpr(_)
            | ConstGenericValue::Computation(_) => Some(value),
            // A size or alignment is a `usize`.
            ConstGenericValue::Layout { .. } => (primitive == PrimitiveTy::Usize).then_some(value),
            ConstGenericValue::Int(_) => value
                .canonical_for(primitive, self.target.pointer_width())
                .ok(),
            ConstGenericValue::Bool(_) => (primitive == PrimitiveTy::Bool).then_some(value),
            ConstGenericValue::Char(_) => (primitive == PrimitiveTy::Char).then_some(value),
        }
    }

    /// The primitive type of a const parameter, through aliases.
    fn const_generic_primitive(&mut self, ty: InternedTyId) -> Option<PrimitiveTy> {
        let ty = self.normalization.normalize(ty);
        match self.interner.get(ty) {
            Some(TyKind::Primitive(primitive)) => Some(*primitive),
            _ => None,
        }
    }

    pub(crate) fn complete_instance_args_for_generics(
        &mut self,
        span: Span,
        generics: &[SymbolId],
        substitutions: &SymbolMap<InternedTyId>,
    ) -> Option<Vec<InternedTyId>> {
        let mut complete = true;
        for generic in generics {
            if !substitutions.contains_key(generic) {
                complete = false;
                self.report_uninferred_generic(span, *generic, false);
            }
        }
        if !complete {
            return None;
        }
        Some(
            generics
                .iter()
                .filter_map(|generic| substitutions.get(generic).copied())
                .collect::<Vec<_>>(),
        )
    }

    pub(crate) fn complete_const_instance_args_for_generic_params(
        &mut self,
        span: Span,
        generic_params: &[GenericParamSignature],
        substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<Vec<ConstGenericArg>> {
        let mut complete = true;
        let mut args = Vec::new();
        for generic in generic_params {
            if !matches!(generic.kind, GenericParamSignatureKind::Const { .. }) {
                continue;
            }
            if let Some(arg) = substitutions.get(&generic.name) {
                args.push(arg.clone());
            } else {
                complete = false;
                self.report_uninferred_generic(span, generic.name, true);
            }
        }
        complete.then_some(args)
    }

    pub(crate) fn complete_instance_args_and_const_args_for_def(
        &mut self,
        span: Span,
        def_id: GlobalDefId,
        substitutions: &SymbolMap<InternedTyId>,
        const_substitutions: &SymbolMap<ConstGenericArg>,
    ) -> Option<(Vec<InternedTyId>, Vec<ConstGenericArg>)> {
        let mut complete = true;
        let mut args = Vec::new();
        let mut const_args = Vec::new();
        for generic in self.effective_generics_for_def(def_id) {
            if let Some(arg) = substitutions.get(&generic).copied() {
                args.push(arg);
            } else if let Some(arg) = const_substitutions.get(&generic).cloned() {
                const_args.push(arg);
            } else {
                complete = false;
                self.report_uninferred_generic(span, generic, false);
            }
        }
        complete.then_some((args, const_args))
    }

    pub(crate) fn record_generic_instantiation_with_self_arg(
        &mut self,
        def_id: GlobalDefId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        span: Span,
    ) {
        self.record_generic_instantiation_with_self_and_const_args(
            def_id,
            self_arg,
            args,
            &[],
            span,
        );
    }

    pub(crate) fn record_generic_instantiation_with_const_args(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        span: Span,
    ) {
        self.record_generic_instantiation_with_self_and_const_args(
            def_id, None, args, const_args, span,
        );
    }

    /// Records an instance the checked code executes. A call made while
    /// evaluating a constant runs at compile time, so like its resolved call
    /// it is no instance of the program.
    pub(crate) fn record_generic_instantiation_with_self_and_const_args(
        &mut self,
        def_id: GlobalDefId,
        self_arg: Option<InternedTyId>,
        args: &[InternedTyId],
        const_args: &[ConstGenericArg],
        span: Span,
    ) {
        if self.in_const_context() {
            return;
        }
        let instantiation = GenericInstantiation {
            def_id,
            self_arg,
            args: args.to_vec(),
            const_args: const_args.to_vec(),
            generics: self.effective_generics_for_def(def_id),
            span,
            source_def_id: self.current_def_id,
        };
        self.generic_instantiations.push(instantiation);
    }

    pub(crate) fn effective_generics_for_def(&mut self, def_id: GlobalDefId) -> Vec<SymbolId> {
        if let Some(generics) = self.trait_method_effective_generics(def_id) {
            return generics;
        }
        let mut generics = self.extension_method_effective_generics(def_id);
        if def_id.module_id == self.defs.module_id
            && generics.is_empty()
            && let Some(def) = self.defs.defs.get(def_id.def_id)
        {
            generics = def
                .parent
                .and_then(|parent| self.defs.defs.get(parent))
                .map(|parent| parent.generics.clone())
                .unwrap_or_default();
            generics.extend(def.generics.clone());
        }
        if generics.is_empty()
            && def_id.module_id != self.defs.module_id
            && let Some(signature) = self.program_signature_scope.function(def_id)
        {
            // External ordinary functions do not participate in the local
            // extension index, but their generic parameters still define the
            // function-pointer instantiation contract.
            generics = signature
                .signature
                .generic_params
                .iter()
                .map(|param| param.name)
                .collect();
        }
        generics
    }

    pub(crate) fn effective_const_generics_for_def(
        &mut self,
        def_id: GlobalDefId,
        signature: &FunctionSignature,
    ) -> Vec<SymbolId> {
        if let Some(lookup) = self
            .extension_method_lookup_for_id(def_id)
            .cloned()
            .or_else(|| self.ensure_extension_method_lookup_for_id(def_id).cloned())
        {
            return lookup.effective_const_generics;
        }

        let mut const_generics = signature
            .generic_params
            .iter()
            .filter_map(|generic| {
                matches!(generic.kind, GenericParamSignatureKind::Const { .. })
                    .then_some(generic.name)
            })
            .collect::<Vec<_>>();
        if let Some((trait_id, trait_signature)) =
            self.program_signature_scope.trait_owning_method(def_id)
            && let Some(method) = trait_signature.signature.methods.iter().find(|method| {
                GlobalDefId {
                    module_id: trait_id.module_id,
                    def_id: method.def_id,
                } == def_id
            })
        {
            const_generics.extend(
                trait_signature
                    .signature
                    .generic_params
                    .iter()
                    .chain(method.signature.generic_params.iter())
                    .filter_map(|generic| {
                        matches!(generic.kind, GenericParamSignatureKind::Const { .. })
                            .then_some(generic.name)
                    }),
            );
            return const_generics;
        }
        if let Some(def) = self.defs.defs.get(def_id.def_id)
            && let Some(parent) = def.parent.and_then(|parent| self.defs.defs.get(parent))
        {
            const_generics.splice(
                0..0,
                parent.generic_params.iter().filter_map(|generic| {
                    matches!(generic.kind, nia_ast::GenericParamKind::Const { .. })
                        .then_some(generic.name)
                }),
            );
        }
        const_generics
    }

    fn trait_method_effective_generics(&self, def_id: GlobalDefId) -> Option<Vec<SymbolId>> {
        if def_id.module_id == self.defs.module_id
            && let Some(def) = self.defs.defs.get(def_id.def_id)
            && def.kind == nia_defs::DefKind::TraitMethod
        {
            let mut generics = def
                .parent
                .and_then(|parent| self.defs.defs.get(parent))
                .map(|parent| parent.generics.clone())
                .unwrap_or_default();
            generics.extend(def.generics.clone());
            return Some(generics);
        }
        self.program_signature_scope
            .trait_owning_method(def_id)
            .and_then(|(trait_id, signature)| {
                signature.signature.methods.iter().find_map(|method| {
                    (GlobalDefId {
                        module_id: trait_id.module_id,
                        def_id: method.def_id,
                    } == def_id)
                        .then(|| {
                            let mut generics = signature.signature.generics.clone();
                            generics.extend(method.signature.generics.iter().cloned());
                            generics
                        })
                })
            })
    }

    fn extension_method_effective_generics(&mut self, def_id: GlobalDefId) -> Vec<SymbolId> {
        self.with_visible_extensions(|extensions| {
            extensions
                .targets()
                .iter()
                .flat_map(|target| target.methods.iter())
                .find(|method| method.def_id == def_id)
                .map(|method| method.effective_generics.clone())
        })
        .or_else(|| {
            self.program_extension_methods
                .method_by_id(def_id)
                .map(|method| method.effective_generics.clone())
        })
        .or_else(|| {
            self.ensure_extension_method_lookup_for_id(def_id)
                .map(|method| method.effective_generics.clone())
        })
        .unwrap_or_default()
    }
}

/// An operator as written in Nia source.
fn binary_op_text(op: nia_ty::ConstBinaryOp) -> &'static str {
    use nia_ty::ConstBinaryOp as Op;
    match op {
        Op::Mul => "*",
        Op::Div => "/",
        Op::Rem => "%",
        Op::Add => "+",
        Op::Sub => "-",
        Op::Shl => "<<",
        Op::Shr => ">>",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
        Op::Eq => "==",
        Op::Ne => "!=",
        Op::BitAnd => "&",
        Op::BitXor => "^",
        Op::BitOr => "|",
        Op::And => "and",
        Op::Or => "or",
    }
}
