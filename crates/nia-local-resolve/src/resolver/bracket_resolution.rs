//! Disambiguation helpers for expression bracket suffixes.

use super::*;

impl LocalResolver<'_> {
    /// Brackets can be generic arguments or an index. Resolve their expression
    /// payload only when the callee/context cannot make them a type argument;
    /// this keeps value names inside `value[index]` visible without treating
    /// generic type syntax as an executable expression.
    pub(super) fn should_resolve_expr_bracket_args(
        &self,
        callee: &Expr,
        args: &[nia_ast::BracketArg],
    ) -> bool {
        self.bracket_suffix_can_be_index(args) || !self.bracket_suffix_can_be_generic(callee)
    }

    pub(super) fn should_resolve_callee_bracket_args(
        &self,
        callee: &Expr,
        args: &[nia_ast::BracketArg],
    ) -> bool {
        self.bracket_suffix_is_unambiguous_index(args)
            || (self.bracket_suffix_can_be_index(args) && self.callee_is_indexable_expr(callee))
            || !self.bracket_suffix_can_be_generic(callee)
    }

    fn bracket_suffix_can_be_generic(&self, callee: &Expr) -> bool {
        match &callee.kind {
            ExprKind::Ident(name) => {
                matches!(
                    self.values.node_names.get(&callee.node_key),
                    Some(ValueNameResolution::Def(_))
                ) || (self.lookup_any(name).is_none()
                    && (self.defs.module_scope.types.get(name).is_some()
                        || self
                            .values
                            .node_qualified_type_prefixes
                            .contains_key(&callee.node_key)))
            }
            ExprKind::Qualified { .. } => {
                self.values
                    .node_qualified_values
                    .contains_key(&callee.node_key)
                    || self
                        .values
                        .node_qualified_type_prefixes
                        .contains_key(&callee.node_key)
            }
            ExprKind::TypeTarget { .. } | ExprKind::TraitTarget { .. } => true,
            ExprKind::Field { .. } => true,
            ExprKind::BracketSuffix { callee, .. } => self.bracket_suffix_can_be_generic(callee),
            _ => false,
        }
    }

    fn bracket_suffix_can_be_index(&self, args: &[nia_ast::BracketArg]) -> bool {
        let [
            nia_ast::BracketArg {
                expr: Some(expr),
                ty,
                ..
            },
        ] = args
        else {
            return false;
        };
        ty.is_none() || self.expr_names_value(expr)
    }

    fn bracket_suffix_is_unambiguous_index(&self, args: &[nia_ast::BracketArg]) -> bool {
        matches!(
            args,
            [nia_ast::BracketArg {
                expr: Some(_),
                ty: None,
                ..
            }]
        )
    }

    /// Reports whether an argument that also parses as a type names only a
    /// value: a lexical local or static, or a module value that value
    /// resolution found and no type shares. Such an argument can only be an
    /// index, so its value use must be recorded.
    fn expr_names_value(&self, expr: &Expr) -> bool {
        let is_type_candidate = self
            .values
            .node_qualified_type_prefixes
            .contains_key(&expr.node_key)
            || self
                .values
                .node_imported_type_prefixes
                .contains_key(&expr.node_key);
        match &expr.kind {
            ExprKind::Ident(name) => {
                self.lookup_local(name).is_some()
                    || self.lookup_static(name).is_some()
                    || (matches!(
                        self.values.node_names.get(&expr.node_key),
                        Some(ValueNameResolution::Def(_) | ValueNameResolution::External(_))
                    ) && !is_type_candidate
                        && self.defs.module_scope.types.get(name).is_none())
            }
            ExprKind::Qualified { .. } => {
                self.values
                    .node_qualified_values
                    .contains_key(&expr.node_key)
                    && !is_type_candidate
            }
            _ => false,
        }
    }

    fn callee_is_indexable_expr(&self, callee: &Expr) -> bool {
        matches!(
            callee.kind,
            ExprKind::Field { .. } | ExprKind::Index { .. } | ExprKind::BracketSuffix { .. }
        )
    }
}
