// SPDX-License-Identifier: GPL-3.0-or-later
//! The one report of the constructor generic-argument rule.
//!
//! A constructor names the type it builds, so it writes every generic
//! argument: `Pair[i32, bool] { .. }`, `Wrapper[i32](1)`,
//! `Choice[i32]::Some(1)`. Only the omitted forms `.{ .. }` and `.Some(1)`
//! take the instance from the expected type. [`nia_sema::constructor_generic_args`]
//! states the rule; this pass applies it to every constructor written in a
//! module and reports each violation once.
//!
//! The rule depends only on how a constructor is spelled and what its prefix
//! names, not on types, so it runs once name resolution is complete and
//! before const lowering and body checking. Those phases consume the same
//! classification and never report it: const and runtime code see one rule
//! with one diagnostic, in every position — function bodies, constant and
//! static initializers, enum discriminants, and constant expressions in
//! types such as array lengths.
//!
//! A prefix that writes arguments, `E[i32]::B` or `W[i32]::new`, writes all
//! of them wherever it appears, as a type written as a [`nia_ast::TypeRef`]
//! does. This pass reports such a prefix too, so every count of arguments
//! written after a type name in an expression or pattern has this one owner;
//! type lowering owns the same count in type positions, including the target
//! of `W[i32] { .. }`.
//!
//! Patterns are not constructors. `M::Some(v)` and `M::None` in a pattern
//! match against the scrutinee's instance and take nothing from their
//! spelling, and an associated call such as `W::new(1)` infers its target's
//! arguments, which body checking owns.

use std::sync::Arc;

use nia_ast::{Expr, ExprKind, Pattern, PatternKind};
use nia_ast_walk::Visitor;
use nia_defs::{DefCollection, DefKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, ModuleId};
use nia_item_tree::ActiveModuleItemTree;
use nia_sema::{ConstructorGenericArgs, ConstructorSpelling, constructor_generic_args};
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_symbol::{SymbolText, symbol_text_from_optional_resolver};

/// Inputs for checking a module's constructors.
pub struct ConstructorCheckInput<'a> {
    /// Items whose bodies and initializers are checked.
    pub active_item_tree: &'a ActiveModuleItemTree,
    /// Constant expressions written inside types, such as array lengths.
    pub type_const_exprs: &'a [&'a Expr],
    /// Resolved prefix and value identities.
    pub semantic_uses: &'a SemanticUseTable,
    /// Definitions of the checked module.
    pub defs: &'a DefCollection,
    /// Definitions of another module that a prefix names.
    pub defs_for_module: &'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>,
    /// Names for diagnostics.
    pub symbols: Option<&'a dyn SymbolText>,
}

/// Reports every constructor whose prefix does not write the generic
/// arguments its type declares.
pub fn check_module_constructors(input: ConstructorCheckInput<'_>) -> Vec<Diagnostic> {
    let mut checker = ConstructorChecker {
        input: &input,
        diagnostics: Vec::new(),
    };
    for item in input.active_item_tree.items.iter() {
        checker.visit_item(item);
    }
    for expr in input.type_const_exprs {
        checker.visit_expr(expr);
    }
    checker.diagnostics
}

struct ConstructorChecker<'i, 'a> {
    input: &'i ConstructorCheckInput<'a>,
    diagnostics: Vec<Diagnostic>,
}

impl ConstructorChecker<'_, '_> {
    /// The prefix of a constructor, when `expr` is one.
    fn constructor_prefix<'e>(&self, expr: &'e Expr) -> Option<&'e Expr> {
        match &expr.kind {
            // `E::B`, and the variant of `E::A(..)` and `E::C { .. }`.
            ExprKind::Qualified { lhs, .. } if self.names_enum_variant(expr) => Some(lhs),
            // `T2(..)`, `T2[i32](..)`, `Alias(..)`.
            ExprKind::Call { callee, .. } if self.names_constructible_type(callee) => Some(callee),
            // `module::W { .. }`, `Alias { .. }` written as a path.
            ExprKind::QualifiedStructLiteral { target, .. }
                if self.names_constructible_type(target) =>
            {
                Some(target)
            }
            _ => None,
        }
    }

    fn names_enum_variant(&self, expr: &Expr) -> bool {
        matches!(
            self.input.semantic_uses.node_value_use(&expr.node_key),
            Some(SemanticValueUse::Global(id)) if self.def_kind(id) == Some(DefKind::EnumVariant)
        )
    }

    fn names_constructible_type(&self, prefix: &Expr) -> bool {
        self.prefix_type(prefix).is_some_and(|(id, _)| {
            matches!(
                self.def_kind(id),
                Some(DefKind::Struct | DefKind::Union | DefKind::TypeAlias)
            )
        })
    }

    /// The type a constructor prefix names and the arguments written after
    /// it. A prefix spelled as a type (`<T>::A`) is a type position.
    fn prefix_type(&self, prefix: &Expr) -> Option<(GlobalDefId, usize)> {
        let uses = self.input.semantic_uses;
        match &prefix.kind {
            ExprKind::BracketSuffix { callee, args } => uses
                .node_type_prefix(&callee.node_key)
                .map(|id| (id, args.len())),
            _ => uses.node_type_prefix(&prefix.node_key).map(|id| (id, 0)),
        }
    }

    /// Checks a constructor whose prefix writes no arguments. A prefix that
    /// writes them is checked wherever it appears, by [`Self::check_written_prefix`].
    fn check_constructor(&mut self, prefix: &Expr) {
        if matches!(prefix.kind, ExprKind::BracketSuffix { .. }) {
            return;
        }
        let Some((type_id, written)) = self.prefix_type(prefix) else {
            return;
        };
        self.check_prefix(prefix, type_id, ConstructorSpelling::Prefix { written });
    }

    /// Checks a type prefix that writes arguments, as in `E[i32]::B`.
    fn check_written_prefix(&mut self, expr: &Expr) {
        let ExprKind::BracketSuffix { callee, args } = &expr.kind else {
            return;
        };
        let Some(type_id) = self.input.semantic_uses.node_type_prefix(&callee.node_key) else {
            return;
        };
        self.check_prefix(
            expr,
            type_id,
            ConstructorSpelling::Prefix {
                written: args.len(),
            },
        );
    }

    fn check_prefix(&mut self, prefix: &Expr, type_id: GlobalDefId, spelling: ConstructorSpelling) {
        let Some((name, declared)) = self.with_def(type_id, |def| (def.name, def.generics.len()))
        else {
            return;
        };
        if let ConstructorGenericArgs::Mismatch { expected, actual } =
            constructor_generic_args(spelling, declared)
        {
            let name = symbol_text_from_optional_resolver(self.input.symbols, name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_NORMALIZATION,
                prefix.span,
                format!(
                    "generic argument count mismatch for `{name}`: expected {expected}, got {actual}"
                ),
            ));
        }
    }

    fn def_kind(&self, id: GlobalDefId) -> Option<DefKind> {
        self.with_def(id, |def| def.kind)
    }

    fn with_def<R>(&self, id: GlobalDefId, f: impl FnOnce(&nia_defs::Def) -> R) -> Option<R> {
        if id.module_id == self.input.defs.module_id {
            return self.input.defs.defs.get(id.def_id).map(f);
        }
        let defs = (self.input.defs_for_module)(id.module_id)?;
        defs.defs.get(id.def_id).map(f)
    }

    /// Visits what a pattern contains other than its constructor path.
    fn visit_pattern_path(&mut self, path: &Expr) {
        match &path.kind {
            ExprKind::Qualified { lhs, .. } if self.names_enum_variant(path) => {
                self.check_written_prefix(lhs);
            }
            _ => self.visit_expr(path),
        }
    }
}

impl<'ast> Visitor<'ast> for ConstructorChecker<'_, '_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        if let Some(prefix) = self.constructor_prefix(expr) {
            self.check_constructor(prefix);
        }
        self.check_written_prefix(expr);
        nia_ast_walk::walk_expr(self, expr);
    }

    fn visit_pattern(&mut self, pattern: &'ast Pattern) {
        match &pattern.kind {
            PatternKind::Nominal {
                constructor,
                fields,
            } => {
                self.visit_pattern_path(constructor);
                match fields {
                    nia_ast::NominalPatternFields::Tuple(fields) => {
                        for field in fields {
                            self.visit_pattern(field);
                        }
                    }
                    nia_ast::NominalPatternFields::Named { fields, .. } => {
                        for field in fields {
                            self.visit_pattern(&field.pattern);
                        }
                    }
                }
            }
            PatternKind::Expr(expr) => self.visit_pattern_path(expr),
            _ => nia_ast_walk::walk_pattern(self, pattern),
        }
    }
}
