// SPDX-License-Identifier: GPL-3.0-or-later
//! Block and statement lowering.

use nia_ast::{
    BindingStmt, Block, Expr, ExprKind, ForInStmt, LocalBindingKind, LoopStmt, Stmt, StmtKind,
    WhileStmt,
};
use nia_lexer::TokenKind;
use nia_node_id::SyntaxKind as NodeSyntaxKind;
use nia_syntax::{GreenNode, SyntaxKind};

use super::{Lower, attributed_span, has_token, nodes_where};

impl Lower<'_> {
    pub(crate) fn block(&mut self, node: &GreenNode) -> Block {
        let mut stmts = Vec::new();
        let mut tail = None;
        for child in node.child_nodes() {
            let kind = child.kind();
            if kind.is_stmt() {
                if let Some(stmt) = self.stmt(child) {
                    stmts.push(stmt);
                }
            } else if kind.is_expr() {
                tail = Some(Box::new(self.expr(child)));
            }
        }
        Block {
            span: node.span(),
            stmts,
            tail,
        }
    }

    pub(crate) fn stmt(&mut self, node: &GreenNode) -> Option<Stmt> {
        let attributes = self.attributes(node);
        let mut span = attributed_span(node);
        let kind = match node.kind() {
            SyntaxKind::LetStmt => StmtKind::Binding(Box::new(self.let_stmt(node)?)),
            SyntaxKind::StaticStmt => StmtKind::Static(Box::new(
                self.binding(node.child(&SyntaxKind::BindingDecl)?, false),
            )),
            SyntaxKind::UsingStmt => {
                StmtKind::Using(self.using_tree(node.child(&SyntaxKind::UsingTree)?))
            }
            SyntaxKind::ReturnStmt => StmtKind::Return(self.child_expr(node).map(Box::new)),
            SyntaxKind::BreakStmt => StmtKind::Break,
            SyntaxKind::ContinueStmt => StmtKind::Continue,
            SyntaxKind::DeferStmt => StmtKind::Defer(Box::new(self.child_expr(node)?)),
            SyntaxKind::ForStmt => StmtKind::ForIn(Box::new(self.for_stmt(node)?)),
            SyntaxKind::WhileStmt => {
                let mut exprs = nodes_where(node, SyntaxKind::is_expr);
                let cond = self.expr(exprs.next()?);
                let body = self.block(exprs.next()?);
                StmtKind::While(Box::new(WhileStmt { cond, body }))
            }
            SyntaxKind::LoopStmt => StmtKind::Loop(Box::new(LoopStmt {
                body: self.block(node.child(&SyntaxKind::Block)?),
            })),
            SyntaxKind::ExprStmt => {
                let expr = self.child_expr(node)?;
                // Block expression statements span their expression; an
                // attributed statement also spans its attributes and `;`.
                if attributes.is_empty() {
                    span = expr.span;
                }
                StmtKind::Expr(Box::new(expr))
            }
            _ => return None,
        };
        let node_key = self.node_key(NodeSyntaxKind::Stmt, span);
        Some(Stmt {
            span,
            node_key,
            attributes,
            kind,
        })
    }

    fn let_stmt(&mut self, node: &GreenNode) -> Option<BindingStmt> {
        let is_const = has_token(node, TokenKind::Const);
        let is_mutable = has_token(node, TokenKind::Mut);
        let pattern = node.child_nodes().find(|child| child.kind().is_pattern())?;
        let mut pattern = self.pattern(pattern, false);
        if is_mutable {
            mark_bindings_mutable(&mut pattern);
        }
        Some(BindingStmt {
            pattern,
            ty: self.child_type(node),
            value: self.child_expr(node),
            kind: if is_const {
                LocalBindingKind::Const
            } else {
                LocalBindingKind::Let { is_mutable }
            },
        })
    }

    fn for_stmt(&mut self, node: &GreenNode) -> Option<ForInStmt> {
        let pattern = node.child_nodes().find(|child| child.kind().is_pattern())?;
        let pattern = self.pattern(pattern, false);
        let exprs = nodes_where(node, SyntaxKind::is_expr).collect::<Vec<_>>();
        let (body, iter) = exprs.split_last()?;
        let iter: Expr = match iter.first() {
            Some(iter) if has_token(node, TokenKind::In) => self.expr(iter),
            _ => self.make_expr(pattern.span, ExprKind::Error),
        };
        Some(ForInStmt {
            pattern,
            iter,
            body: self.block(body),
        })
    }
}

pub(crate) fn mark_bindings_mutable(pattern: &mut nia_ast::Pattern) {
    use nia_ast::{NominalPatternFields, PatternKind};
    match &mut pattern.kind {
        PatternKind::Bind { is_mutable, .. } => *is_mutable = true,
        PatternKind::Pointer(inner)
        | PatternKind::MutPointer(inner)
        | PatternKind::OptionalSome(inner)
        | PatternKind::ErrorOk(inner)
        | PatternKind::ErrorErr(inner) => mark_bindings_mutable(inner),
        PatternKind::Tuple(fields) => fields.iter_mut().for_each(mark_bindings_mutable),
        PatternKind::Nominal { fields, .. } => match fields {
            NominalPatternFields::Tuple(fields) => {
                fields.iter_mut().for_each(mark_bindings_mutable);
            }
            NominalPatternFields::Named { fields, .. } => {
                for field in fields {
                    mark_bindings_mutable(&mut field.pattern);
                }
            }
        },
        PatternKind::Wildcard
        | PatternKind::OptionalNull
        | PatternKind::Expr(_)
        | PatternKind::Range { .. } => {}
    }
}
