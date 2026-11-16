// SPDX-License-Identifier: GPL-3.0-or-later
//! The shared result type of the branches of `if` and `match` expressions,
//! and which expressions take their type from their context.
use super::*;
use nia_ast::{BinaryOp, UnaryOp};

/// One branch of an `if` or `match` expression whose value is the value of
/// the whole expression.
#[derive(Clone, Copy)]
pub(crate) enum Branch<'a> {
    /// The block of an `if`.
    Block(&'a nia_ast::Block),
    /// The `else` of an `if`: a block or another `if`.
    Expr(&'a Expr),
    /// The body of a `match` arm.
    Arm(&'a nia_ast::MatchArmBody),
}

/// Where a branch's type comes from when the branches have no expected type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BranchTyping {
    /// The branch has a type of its own.
    Own,
    /// From the other branches, else the branch's default, as for an
    /// unsuffixed literal.
    ContextOrDefault,
    /// Only from the other branches, as for `null`.
    ContextOnly,
}

/// Whether `expr` has no type without one from its context: `null`, an
/// omitted member or aggregate, or a block, `if`, or `match` whose every
/// result is such.
fn requires_context_type(expr: &Expr) -> bool {
    let block_requires =
        |block: &nia_ast::Block| block.tail.as_deref().is_some_and(requires_context_type);
    match &expr.kind {
        ExprKind::Null
        | ExprKind::OmittedMember { .. }
        | ExprKind::OmittedAggregateLiteral { .. } => true,
        ExprKind::Block(block) => block_requires(block),
        ExprKind::If {
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => block_requires(then_branch) && requires_context_type(else_branch),
        ExprKind::Match(matched) => {
            !matched.arms.is_empty()
                && matched.arms.iter().all(|arm| match &arm.body {
                    nia_ast::MatchArmBody::Expr(expr) => requires_context_type(expr),
                    nia_ast::MatchArmBody::Block(block) => block_requires(block),
                    nia_ast::MatchArmBody::Stmt(_) => false,
                })
        }
        _ => false,
    }
}

impl<'a> BodyChecker<'a> {
    /// Types branches that share one result type, and returns it. The type is
    /// `expected` when the context gives one; otherwise it is the type of the
    /// first branch that has a type of its own. A branch whose type comes from
    /// its context takes that type wherever the branch is: one that also has
    /// a default, such as an unsuffixed literal, gives its default only when
    /// no branch has a type of its own, and one with no type of its own, such
    /// as `null`, never gives the type. A branch of type `never` gives no
    /// type; `None` when no branch gives one.
    pub(crate) fn check_branches(
        &mut self,
        branches: &[Branch<'_>],
        expected: Option<InternedTyId>,
        context: &str,
    ) -> Option<InternedTyId> {
        let mut result = expected;
        let mut checked = vec![false; branches.len()];
        for tier in [BranchTyping::Own, BranchTyping::ContextOrDefault] {
            if result.is_some() {
                break;
            }
            for (index, branch) in branches.iter().enumerate() {
                if checked[index] || self.branch_typing(*branch) != tier {
                    continue;
                }
                checked[index] = true;
                let ty = self.check_branch(*branch, None);
                if !self.is_never(ty) {
                    result = Some(ty);
                    break;
                }
            }
        }
        for (index, branch) in branches.iter().enumerate() {
            if checked[index] {
                continue;
            }
            let ty = self.check_branch(*branch, result);
            match result {
                Some(expected) => self.expect_branch_type(*branch, expected, ty, context),
                None if !self.is_never(ty) => result = Some(ty),
                None => {}
            }
        }
        result
    }

    fn check_branch(&mut self, branch: Branch<'_>, expected: Option<InternedTyId>) -> InternedTyId {
        match branch {
            Branch::Block(block) => self.check_block_with_expected(block, expected),
            Branch::Expr(expr) => self.check_expr_with_expected(expr, expected),
            Branch::Arm(body) => self.check_match_arm_body(body, expected),
        }
    }

    fn expect_branch_type(
        &mut self,
        branch: Branch<'_>,
        expected: InternedTyId,
        actual: InternedTyId,
        context: &str,
    ) {
        if self.is_never(actual) {
            return;
        }
        match branch {
            Branch::Block(block) => self.expect_block_tail_type(block, expected, actual, context),
            Branch::Expr(expr) => {
                self.expect_expr_or_block_tail_type(expr, expected, actual, context)
            }
            Branch::Arm(body) => self.expect_match_arm_type(body, expected, actual, context),
        }
    }

    fn branch_typing(&self, branch: Branch<'_>) -> BranchTyping {
        let tail = match branch {
            Branch::Block(block) => block.tail.as_deref(),
            Branch::Arm(nia_ast::MatchArmBody::Block(block)) => block.tail.as_deref(),
            Branch::Expr(expr) => Some(expr),
            Branch::Arm(nia_ast::MatchArmBody::Expr(expr)) => Some(&**expr),
            Branch::Arm(nia_ast::MatchArmBody::Stmt(_)) => None,
        };
        match tail {
            Some(expr) if self.is_context_typed(expr) => BranchTyping::ContextOrDefault,
            Some(expr) if requires_context_type(expr) => BranchTyping::ContextOnly,
            _ => BranchTyping::Own,
        }
    }

    /// Whether the type of `expr` can only come from its context: an
    /// unsuffixed numeric literal, arithmetic over such, a present optional
    /// `?value` of such, or a block, `if`, or `match` whose every result is
    /// such. It takes the type its context
    /// gives, as a single unsuffixed literal does: the other operand of an
    /// arithmetic or comparison operator, or the type of the other branches.
    pub(crate) fn is_context_typed(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Integer(_) | ExprKind::Float(_) => self.is_untyped_numeric_literal_expr(expr),
            ExprKind::Unary {
                op: UnaryOp::Neg | UnaryOp::BitNot,
                expr: operand,
            } => self.is_context_typed(operand),
            ExprKind::OptionalSome { expr: value } => self.is_context_typed(value),
            ExprKind::Binary { op, lhs, rhs } => {
                matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Mul
                        | BinaryOp::Div
                        | BinaryOp::Rem
                        | BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                ) && self.is_context_typed(lhs)
                    && self.is_context_typed(rhs)
            }
            ExprKind::Block(block) => self.block_is_context_typed(block),
            ExprKind::If {
                then_branch,
                else_branch: Some(else_branch),
                ..
            } => self.block_is_context_typed(then_branch) && self.is_context_typed(else_branch),
            ExprKind::Match(matched) => {
                !matched.arms.is_empty()
                    && matched.arms.iter().all(|arm| match &arm.body {
                        nia_ast::MatchArmBody::Expr(expr) => self.is_context_typed(expr),
                        nia_ast::MatchArmBody::Block(block) => self.block_is_context_typed(block),
                        nia_ast::MatchArmBody::Stmt(_) => false,
                    })
            }
            _ => false,
        }
    }

    /// Whether a context-typed expression can take `ty`: a numeric type, or
    /// an optional of one for `?value`.
    pub(crate) fn context_typed_expr_can_take(&self, ty: InternedTyId) -> bool {
        match self.interner.get(ty) {
            Some(TyKind::Optional { elem }) => self.context_typed_expr_can_take(*elem),
            _ => self.is_numeric(ty),
        }
    }

    fn block_is_context_typed(&self, block: &nia_ast::Block) -> bool {
        block
            .tail
            .as_deref()
            .is_some_and(|tail| self.is_context_typed(tail))
    }
}
