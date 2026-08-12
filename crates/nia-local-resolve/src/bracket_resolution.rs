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
        ty.is_none() || self.expr_is_known_local(expr)
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

    fn expr_is_known_local(&self, expr: &Expr) -> bool {
        let ExprKind::Ident(name) = &expr.kind else {
            return false;
        };
        self.lookup_local(name).is_some() || self.lookup_static(name).is_some()
    }

    fn callee_is_indexable_expr(&self, callee: &Expr) -> bool {
        matches!(
            callee.kind,
            ExprKind::Field { .. } | ExprKind::Index { .. } | ExprKind::BracketSuffix { .. }
        )
    }
}
