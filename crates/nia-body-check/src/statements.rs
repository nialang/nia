// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> BodyChecker<'a> {
    pub(super) fn check_block(&mut self, block: &Block) -> InternedTyId {
        self.check_block_with_expected(block, None)
    }

    pub(super) fn check_block_with_expected(
        &mut self,
        block: &Block,
        expected_tail: Option<InternedTyId>,
    ) -> InternedTyId {
        if block.stmts.is_empty()
            && block.tail.is_none()
            && let Some(expected) = expected_tail
            && let Some(TyKind::Nominal { def_id, args, .. }) = self.interner.get(expected)
        {
            let def_id = *def_id;
            let args = args.clone();
            if self.is_union_def(def_id) {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    block.span,
                    "union literal requires exactly one field, got 0",
                ));
                return expected;
            }
            if self.is_empty_struct_type(def_id, &args) {
                return expected;
            }
        }
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        if let Some(tail) = &block.tail {
            self.check_expr_with_expected(tail, expected_tail)
        } else if self.block_ends_with_never_stmt(block) {
            self.never()
        } else {
            self.void()
        }
    }

    pub(super) fn block_ends_with_never_stmt(&mut self, block: &Block) -> bool {
        let Some(stmt) = block.stmts.last() else {
            return false;
        };
        match &stmt.kind {
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue => true,
            StmtKind::Expr(expr) => self.expr_ty(expr).is_some_and(|ty| self.is_never(ty)),
            StmtKind::Binding(_)
            | StmtKind::Static(_)
            | StmtKind::Using(_)
            | StmtKind::Defer(_)
            | StmtKind::ForIn(_)
            | StmtKind::While(_)
            | StmtKind::Loop(_) => false,
        }
    }

    pub(super) fn is_empty_struct_type(
        &mut self,
        def_id: GlobalDefId,
        args: &[InternedTyId],
    ) -> bool {
        let Some(resolved) = self.resolved_struct_signature(def_id) else {
            return false;
        };
        resolved.signature.generics.len() == args.len() && resolved.signature.fields.is_empty()
    }

    pub(super) fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(binding) => {
                self.check_local_binding(stmt, binding);
            }
            StmtKind::Static(binding) => {
                self.check_global_binding_inner(stmt.span, binding, false);
            }
            StmtKind::Using(_) => {
                // Block-scope `using` is a no-op for body type-checking.
            }
            StmtKind::Expr(expr) => {
                let expr_ty = self.check_expr(expr);
                if !self.is_void(expr_ty) && !self.is_never(expr_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        "non-unit expression result is discarded; assign it to `_` explicitly",
                    ));
                }
            }
            StmtKind::Defer(expr) => {
                let expr_ty = self.check_expr(expr);
                if !self.is_void(expr_ty) && !self.is_never(expr_ty) {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        expr.span,
                        "`defer` expression must have type `()`",
                    ));
                }
            }
            StmtKind::Return(value) => {
                let value_ty = match value {
                    Some(value) => self.check_expr_with_expected(value, Some(self.current_return)),
                    None => self.void(),
                };
                if let Some(value) = value {
                    self.expect_expr_type(value, self.current_return, value_ty, "return");
                    self.record_expr_node_type(value, self.current_return);
                } else {
                    self.expect_type(stmt.span, self.current_return, value_ty, "return");
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::ForIn(for_stmt) => {
                let iterable_ty = self.check_expr(&for_stmt.iter);
                let (item_ty, _iterator_ty) = self.for_iterable_parts(&for_stmt.iter, iterable_ty);
                self.check_irrefutable_pattern(&for_stmt.pattern, item_ty, "for pattern");
                self.check_block(&for_stmt.body);
            }
            StmtKind::While(while_stmt) => {
                let cond_ty = self.check_expr(&while_stmt.cond);
                self.expect_type(
                    while_stmt.cond.span,
                    self.bool(),
                    cond_ty,
                    "while condition",
                );
                self.check_block(&while_stmt.body);
            }
            StmtKind::Loop(loop_stmt) => {
                self.check_block(&loop_stmt.body);
            }
        }
    }

    pub(super) fn for_iterable_parts(
        &mut self,
        iter: &Expr,
        iterable_ty: InternedTyId,
    ) -> (InternedTyId, InternedTyId) {
        if !self.current_context_proves_trait_obligation(
            iterable_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            Vec::new(),
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                iter.span,
                format!(
                    "for-in expects an Iterable, found `{}`",
                    self.ty_name(iterable_ty)
                ),
            ));
            return (self.error(), self.error());
        }
        let item_ty = self.iterable_item_projection(iterable_ty);
        let iterator_ty = self.iterable_iter_projection(iterable_ty);
        self.record_trait_method_ref(SemanticTraitMethodRef {
            module_id: self.timing_module_id,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            method_name: known::ITER_METHOD,
            self_ty: iterable_ty,
            trait_args: Vec::new(),
        });
        self.record_trait_method_ref(SemanticTraitMethodRef {
            module_id: self.timing_module_id,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            method_name: known::NEXT,
            self_ty: iterator_ty,
            trait_args: Vec::new(),
        });
        self.check_for_iterator(iter.span, iterator_ty, item_ty);
        if self.body_filter.checks_const_declarations() {
            self.check_const_for_trait_witness(
                iter.span,
                iterable_ty,
                BuiltinTraitMethod::IterableIter,
            );
            self.check_const_for_trait_witness(
                iter.span,
                iterator_ty,
                BuiltinTraitMethod::IteratorNext,
            );
        }
        (item_ty, iterator_ty)
    }

    fn check_const_for_trait_witness(
        &mut self,
        span: Span,
        self_ty: InternedTyId,
        method: BuiltinTraitMethod,
    ) {
        if self.builtin_trait_witness_is_const_capable(self_ty, method) {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::CONST,
            span,
            format!(
                "`{}::{}` trait witness used by const for-in must be declared `const fn`",
                method.trait_id().name(),
                method.name()
            ),
        ));
    }

    pub(super) fn check_for_iterator(
        &mut self,
        span: Span,
        iterator_ty: InternedTyId,
        iterable_item_ty: InternedTyId,
    ) {
        if !self.current_context_proves_trait_obligation(
            iterator_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            Vec::new(),
        ) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!(
                    "for-in Iterable iterator must implement Iterator, found `{}`",
                    self.ty_name(iterator_ty)
                ),
            ));
            return;
        }
        let iterator_item_ty = self.iterator_item_projection(iterator_ty);
        self.expect_type(
            span,
            iterable_item_ty,
            iterator_item_ty,
            "for iterable item",
        );
    }

    pub(super) fn lower_for_iterable_parts(
        &mut self,
        iterable_ty: InternedTyId,
    ) -> (InternedTyId, InternedTyId) {
        if !self.current_context_proves_trait_obligation(
            iterable_ty,
            nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            Vec::new(),
        ) {
            return (self.error(), self.error());
        }
        (
            self.iterable_item_projection(iterable_ty),
            self.iterable_iter_projection(iterable_ty),
        )
    }

    pub(super) fn iterable_item_projection(&mut self, iterable_ty: InternedTyId) -> InternedTyId {
        let item = self.interner.intern(TyKind::Projection {
            self_ty: iterable_ty,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: known::ITEM,
        });
        self.normalize_projection(item)
    }

    pub(super) fn iterable_iter_projection(&mut self, iterable_ty: InternedTyId) -> InternedTyId {
        let iter = self.interner.intern(TyKind::Projection {
            self_ty: iterable_ty,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterable),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: known::ITER,
        });
        self.normalize_projection(iter)
    }

    pub(super) fn iterator_item_projection(&mut self, iter_ty: InternedTyId) -> InternedTyId {
        let item = self.interner.intern(TyKind::Projection {
            self_ty: iter_ty,
            trait_id: nia_ty::TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
            trait_args: Vec::new(),
            trait_const_args: Vec::new(),
            name: known::ITEM,
        });
        self.normalize_projection(item)
    }

    pub(super) fn check_irrefutable_pattern(
        &mut self,
        pattern: &nia_ast::Pattern,
        value_ty: InternedTyId,
        context: &str,
    ) -> InternedTyId {
        match &pattern.kind {
            nia_ast::PatternKind::Wildcard => value_ty,
            nia_ast::PatternKind::Bind { node_key, .. } => {
                if let Some(local_id) = self.local_def(node_key) {
                    self.record_local_type(local_id, value_ty);
                }
                value_ty
            }
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let expected_readonly = matches!(pattern.kind, nia_ast::PatternKind::Pointer(_));
                let elem_ty = match self
                    .interner
                    .get(self.normalization.normalize(value_ty))
                    .cloned()
                {
                    Some(TyKind::Pointer { is_readonly, elem })
                        if is_readonly == expected_readonly =>
                    {
                        elem
                    }
                    Some(TyKind::Pointer { .. }) => {
                        let expected = if expected_readonly {
                            "`&x`"
                        } else {
                            "`&mut x`"
                        };
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} {expected} does not match value type"),
                        ));
                        self.error()
                    }
                    _ => {
                        let expected = if expected_readonly {
                            "read-only pointer"
                        } else {
                            "mutable pointer"
                        };
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} requires value to be a {expected}"),
                        ));
                        self.error()
                    }
                };
                self.check_irrefutable_pattern(inner, elem_ty, context)
            }
            nia_ast::PatternKind::Tuple(patterns) => {
                let elem_types = match self
                    .interner
                    .get(self.normalization.normalize(value_ty))
                    .cloned()
                {
                    Some(TyKind::Tuple(elems)) if elems.len() == patterns.len() => elems,
                    Some(TyKind::Tuple(elems)) => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!(
                                "{context} tuple arity mismatch: expected {}, found {}",
                                elems.len(),
                                patterns.len()
                            ),
                        ));
                        vec![self.error(); patterns.len()]
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::user_error_at(
                            codes::TYPE_CHECK,
                            pattern.span,
                            format!("{context} requires a tuple value"),
                        ));
                        vec![self.error(); patterns.len()]
                    }
                };
                for (pattern, elem_ty) in patterns.iter().zip(elem_types) {
                    self.check_irrefutable_pattern(pattern, elem_ty, context);
                }
                value_ty
            }
            nia_ast::PatternKind::OptionalSome(_)
            | nia_ast::PatternKind::OptionalNull
            | nia_ast::PatternKind::ErrorOk(_)
            | nia_ast::PatternKind::ErrorErr(_)
            | nia_ast::PatternKind::EnumVariant { .. }
            | nia_ast::PatternKind::Expr(_)
            | nia_ast::PatternKind::Range { .. } => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    pattern.span,
                    format!("{context} must be irrefutable"),
                ));
                self.error()
            }
        }
    }

    pub(super) fn pattern_input_ty(
        &mut self,
        pattern: &nia_ast::Pattern,
        binding_ty: InternedTyId,
    ) -> InternedTyId {
        match &pattern.kind {
            nia_ast::PatternKind::Pointer(inner) => {
                let elem = self.pattern_input_ty(inner, binding_ty);
                self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem,
                })
            }
            nia_ast::PatternKind::MutPointer(inner) => {
                let elem = self.pattern_input_ty(inner, binding_ty);
                self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem,
                })
            }
            _ => binding_ty,
        }
    }

    pub(super) fn materialize_explicit_pattern_ty(
        &mut self,
        pattern: &nia_ast::Pattern,
        explicit_binding: InternedTyId,
        value_ty: InternedTyId,
    ) -> InternedTyId {
        match &pattern.kind {
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                let value_elem = match self.interner.get(self.normalization.normalize(value_ty)) {
                    Some(TyKind::Pointer { elem, .. }) => Some(*elem),
                    _ => None,
                };
                value_elem
                    .map(|elem| self.materialize_explicit_pattern_ty(inner, explicit_binding, elem))
                    .unwrap_or(explicit_binding)
            }
            _ => self
                .materialize_inferred_array_type(explicit_binding, value_ty)
                .unwrap_or(explicit_binding),
        }
    }

    pub(super) fn single_pattern_binding_key<'b>(
        &self,
        pattern: &'b nia_ast::Pattern,
    ) -> Option<&'b VersionedNodeKey> {
        match &pattern.kind {
            nia_ast::PatternKind::Bind { node_key, .. } => Some(node_key),
            nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
                self.single_pattern_binding_key(inner)
            }
            _ => None,
        }
    }

    pub(super) fn check_local_binding(&mut self, stmt: &Stmt, binding: &BindingStmt) {
        let span = stmt.span;
        if binding.is_const() && binding.value.is_none() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "const binding requires an initializer",
            ));
        }
        let binding_key = self.single_pattern_binding_key(&binding.pattern);
        if !matches!(binding.pattern.kind, nia_ast::PatternKind::Bind { .. })
            && binding.value.is_none()
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                binding.pattern.span,
                "binding pattern requires an initializer",
            ));
            if let Some(binding_key) = binding_key {
                self.record_error_local_binding(binding_key);
            }
            return;
        }
        let binding_ty = match (&binding.ty, &binding.value) {
            (Some(ty), Some(value)) => {
                let explicit_binding = self.ty_for_type(ty);
                let explicit_input = self.pattern_input_ty(&binding.pattern, explicit_binding);
                let value_ty = if binding.is_const() {
                    self.with_const_context(|this| {
                        this.check_expr_with_expected(value, Some(explicit_input))
                    })
                } else {
                    self.check_expr_with_expected(value, Some(explicit_input))
                };
                if binding.is_const() && self.is_const_only_ty(value_ty) {
                    // The initializer is validated by nia-const-check and has no runtime value.
                } else if self.is_const_only_ty(value_ty) {
                    self.reject_runtime_const_only_value(value.span, "binding initializer");
                    if let Some(binding_key) = binding_key {
                        self.record_error_local_binding(binding_key);
                    }
                    return;
                } else {
                    self.expect_expr_type(value, explicit_input, value_ty, "binding initializer");
                }
                self.materialize_explicit_pattern_ty(&binding.pattern, explicit_binding, value_ty)
            }
            (Some(ty), None) => {
                let explicit = self.ty_for_type(ty);
                if matches!(binding.pattern.kind, nia_ast::PatternKind::Bind { .. }) {
                    explicit
                } else {
                    self.error()
                }
            }
            (None, Some(value)) => {
                let value_ty = if matches!(value.kind, ExprKind::ArrayLiteral { .. }) {
                    if binding.is_const() {
                        self.with_const_context(|this| this.infer_array_literal_expr(value))
                    } else {
                        self.infer_array_literal_expr(value)
                    }
                } else {
                    if binding.is_const() {
                        self.with_const_context(|this| this.check_expr(value))
                    } else {
                        self.check_expr(value)
                    }
                };
                if !binding.is_const() && self.is_const_only_ty(value_ty) {
                    self.reject_runtime_const_only_value(value.span, "binding initializer");
                    self.error()
                } else {
                    self.check_irrefutable_pattern(&binding.pattern, value_ty, "binding pattern")
                }
            }
            (None, None) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    "binding declaration requires an explicit type",
                ));
                self.error()
            }
        };
        if let Some(binding_key) = binding_key
            && let Some(local_id) = self.local_def(binding_key)
        {
            self.record_local_type(local_id, binding_ty);
        }
    }

    pub(super) fn reject_runtime_const_only_value(&mut self, span: Span, context: &str) {
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!("{context} cannot use const-only value"),
        ));
    }

    pub(super) fn record_error_local_binding(&mut self, key: &VersionedNodeKey) {
        if let Some(local_id) = self.local_def(key) {
            self.record_local_type(local_id, self.error());
        }
    }
}
