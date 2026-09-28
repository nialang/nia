// SPDX-License-Identifier: GPL-3.0-or-later
//! Expression lowering.

use nia_ast::{
    ArrayElements, AssignOp, BinaryOp, BracketArg, ClosureCapture, Expr, ExprKind, FieldInit,
    IfPatternChainClause, IfPatternChainExpr, IfPatternExpr, IndexArg, MatchArm, MatchArmBody,
    MatchExpr, PathSegmentKind, SliceRange, StringLiteral, UnaryOp,
};
use nia_lexer::TokenKind;
use nia_node_id::SyntaxKind as NodeSyntaxKind;
use nia_span::Span;
use nia_syntax::{AlternateKind, GreenNode, SyntaxKind};

use super::{Child, Lower, children, has_token, nodes_where, token_kind};

impl Lower<'_> {
    /// Lowers the first direct expression child.
    pub(crate) fn child_expr(&mut self, node: &GreenNode) -> Option<Expr> {
        let expr = node.child_nodes().find(|child| child.kind().is_expr())?;
        Some(self.expr(expr))
    }

    pub(crate) fn make_expr(&mut self, span: Span, kind: ExprKind) -> Expr {
        let node_key = self.node_key(NodeSyntaxKind::Expr, span);
        Expr {
            span,
            node_key,
            kind,
        }
    }

    fn exprs(&mut self, node: &GreenNode) -> Vec<Expr> {
        nodes_where(node, SyntaxKind::is_expr)
            .map(|expr| self.expr(expr))
            .collect()
    }

    fn boxed_expr(&mut self, node: Option<&GreenNode>, fallback: Span) -> Box<Expr> {
        Box::new(match node {
            Some(node) => self.expr(node),
            None => self.make_expr(fallback, ExprKind::Error),
        })
    }

    fn first_expr_child(node: &GreenNode) -> Option<&GreenNode> {
        node.child_nodes().find(|child| child.kind().is_expr())
    }

    pub(crate) fn expr(&mut self, node: &GreenNode) -> Expr {
        let span = node.span();
        let kind = match node.kind() {
            SyntaxKind::ParenExpr => {
                return match Self::first_expr_child(node) {
                    Some(inner) => self.expr(inner),
                    None => self.make_expr(span, ExprKind::Error),
                };
            }
            SyntaxKind::LiteralExpr => self.literal(node),
            SyntaxKind::StringExpr => {
                let parts = node
                    .child_tokens()
                    .map(|token| token.text().to_string())
                    .collect();
                let literal = StringLiteral { parts };
                if node
                    .first_token()
                    .is_some_and(|token| token.is(&TokenKind::ByteString))
                {
                    ExprKind::ByteString(literal)
                } else {
                    ExprKind::String(literal)
                }
            }
            SyntaxKind::NameExpr => match node.first_token() {
                Some(token) => match token_kind(token) {
                    TokenKind::SelfValue => ExprKind::SelfValue,
                    TokenKind::Pkg => ExprKind::PathRoot(PathSegmentKind::Package),
                    TokenKind::Super => ExprKind::PathRoot(PathSegmentKind::Super),
                    TokenKind::Underscore => ExprKind::Underscore,
                    _ => ExprKind::Ident(self.name(token)),
                },
                None => ExprKind::Error,
            },
            SyntaxKind::TupleExpr => ExprKind::Tuple(self.exprs(node)),
            SyntaxKind::Block => ExprKind::Block(self.block(node)),
            SyntaxKind::IfExpr => self.if_expr(node),
            SyntaxKind::MatchExpr => ExprKind::Match(Box::new(self.match_expr(node))),
            SyntaxKind::ClosureExpr => self.closure(node),
            SyntaxKind::ArrayExpr => {
                let elems = self.exprs(node);
                ExprKind::ArrayLiteral {
                    elems: if has_token(node, TokenKind::Semicolon) {
                        let mut elems = elems.into_iter();
                        match (elems.next(), elems.next()) {
                            (Some(value), Some(count)) => ArrayElements::Repeat {
                                value: Box::new(value),
                                count: Box::new(count),
                            },
                            (value, _) => ArrayElements::List(value.into_iter().collect()),
                        }
                    } else {
                        ArrayElements::List(elems)
                    },
                }
            }
            SyntaxKind::TypeTargetExpr => match self.child_type(node) {
                Some(ty) => ExprKind::TypeTarget { ty },
                None => ExprKind::Error,
            },
            SyntaxKind::TraitTargetExpr => {
                let mut types = nodes_where(node, SyntaxKind::is_type);
                match (types.next(), types.next()) {
                    (Some(ty), Some(trait_ref)) => ExprKind::TraitTarget {
                        ty: self.type_ref(ty),
                        trait_ref: self.type_ref(trait_ref),
                    },
                    _ => ExprKind::Error,
                }
            }
            SyntaxKind::TypedStructLiteral => {
                let Some(ty) = self.child_type(node) else {
                    return self.make_expr(span, ExprKind::Error);
                };
                ExprKind::TypedStructLiteral {
                    ty,
                    fields: self.field_inits(node),
                }
            }
            SyntaxKind::QualifiedStructLiteral => ExprKind::QualifiedStructLiteral {
                target: self.boxed_expr(Self::first_expr_child(node), span),
                fields: self.field_inits(node),
            },
            SyntaxKind::OmittedAggregateLiteral => ExprKind::OmittedAggregateLiteral {
                fields: self.field_inits(node),
            },
            SyntaxKind::OmittedMemberExpr => ExprKind::OmittedMember {
                name: self.first_name(node),
            },
            SyntaxKind::PrefixExpr => {
                let expr = self.boxed_expr(Self::first_expr_child(node), span);
                match node.first_token().map(token_kind) {
                    Some(TokenKind::Minus) => ExprKind::Unary {
                        op: UnaryOp::Neg,
                        expr,
                    },
                    Some(TokenKind::Tilde) => ExprKind::Unary {
                        op: UnaryOp::BitNot,
                        expr,
                    },
                    Some(TokenKind::Amp) => ExprKind::Unary {
                        op: if has_token(node, TokenKind::Mut) {
                            UnaryOp::Ref
                        } else {
                            UnaryOp::RefReadOnly
                        },
                        expr,
                    },
                    Some(TokenKind::Question) => ExprKind::OptionalSome { expr },
                    _ => ExprKind::ErrorOk { expr },
                }
            }
            SyntaxKind::NotExpr => ExprKind::Unary {
                op: UnaryOp::Not,
                expr: self.boxed_expr(Self::first_expr_child(node), span),
            },
            SyntaxKind::CastExpr => {
                let expr = self.boxed_expr(Self::first_expr_child(node), span);
                match self.child_type(node) {
                    Some(ty) => ExprKind::Cast { expr, ty },
                    None => return *expr,
                }
            }
            SyntaxKind::BinaryExpr => {
                let (lhs, rhs) = self.operands(node);
                let op = node
                    .child_tokens()
                    .find_map(|token| binary_op(token_kind(token)))
                    .unwrap_or(BinaryOp::Add);
                ExprKind::Binary { lhs, op, rhs }
            }
            SyntaxKind::AssignExpr => {
                let (lhs, rhs) = self.operands(node);
                let op = node
                    .child_tokens()
                    .find_map(|token| assign_op(token_kind(token)))
                    .unwrap_or(AssignOp::Assign);
                ExprKind::Assign { lhs, op, rhs }
            }
            SyntaxKind::RangeExpr => ExprKind::Range(self.range(node)),
            SyntaxKind::CallExpr => ExprKind::Call {
                callee: self.boxed_expr(Self::first_expr_child(node), span),
                args: node
                    .child(&SyntaxKind::ArgList)
                    .map_or_else(Vec::new, |args| self.exprs(args)),
            },
            SyntaxKind::FieldExpr => ExprKind::Field {
                lhs: self.boxed_expr(Self::first_expr_child(node), span),
                name: self.first_name(node),
            },
            SyntaxKind::TupleFieldExpr => ExprKind::TupleField {
                lhs: self.boxed_expr(Self::first_expr_child(node), span),
                index: node
                    .token(&TokenKind::Integer)
                    .and_then(|token| token.text().parse().ok())
                    .unwrap_or(0),
            },
            SyntaxKind::TryExpr => ExprKind::Try {
                expr: self.boxed_expr(Self::first_expr_child(node), span),
            },
            SyntaxKind::DerefExpr => ExprKind::Unary {
                op: UnaryOp::Deref,
                expr: self.boxed_expr(Self::first_expr_child(node), span),
            },
            SyntaxKind::ErrorErrExpr => ExprKind::ErrorErr {
                expr: self.boxed_expr(Self::first_expr_child(node), span),
            },
            SyntaxKind::QualifiedExpr => {
                let lhs = self.boxed_expr(Self::first_expr_child(node), span);
                let name_token = node.token(&TokenKind::Ident);
                ExprKind::Qualified {
                    lhs,
                    name: name_token.map_or(nia_symbol::SymbolId::EMPTY, |token| self.name(token)),
                    name_span: name_token.map_or(span, |token| token.span()),
                }
            }
            SyntaxKind::BracketExpr => {
                let lhs = self.boxed_expr(Self::first_expr_child(node), span);
                if let Some(range) = node.child(&SyntaxKind::SliceRange) {
                    ExprKind::Index {
                        lhs,
                        index: IndexArg::Range(self.range(range)),
                    }
                } else {
                    ExprKind::BracketSuffix {
                        callee: lhs,
                        args: node
                            .child(&SyntaxKind::BracketArgList)
                            .map_or_else(Vec::new, |args| self.bracket_args(args)),
                    }
                }
            }
            SyntaxKind::RawExpr => ExprKind::Raw(self.text(span)),
            _ => ExprKind::Error,
        };
        self.make_expr(span, kind)
    }

    fn literal(&mut self, node: &GreenNode) -> ExprKind {
        let Some(token) = node.first_token() else {
            return ExprKind::Error;
        };
        let text = token.text().to_string();
        match token_kind(token) {
            TokenKind::Integer => ExprKind::Integer(text),
            TokenKind::Float => ExprKind::Float(text),
            TokenKind::Char => ExprKind::Char(text),
            TokenKind::ByteChar => ExprKind::ByteChar(text),
            TokenKind::True => ExprKind::Bool(true),
            TokenKind::False => ExprKind::Bool(false),
            TokenKind::Null => ExprKind::Null,
            _ => ExprKind::Error,
        }
    }

    fn operands(&mut self, node: &GreenNode) -> (Box<Expr>, Box<Expr>) {
        let mut operands = nodes_where(node, SyntaxKind::is_expr);
        let lhs = operands.next();
        let rhs = operands.next();
        (
            self.boxed_expr(lhs, node.span()),
            self.boxed_expr(rhs, node.span()),
        )
    }

    /// Range bounds before and after the `..`/`..=` operator.
    fn range(&mut self, node: &GreenNode) -> SliceRange {
        let mut range = SliceRange {
            start: None,
            end: None,
            inclusive: false,
        };
        let mut after_operator = false;
        for child in children(node) {
            match child {
                Child::Token(token)
                    if matches!(token_kind(token), TokenKind::DotDot | TokenKind::DotDotEq) =>
                {
                    range.inclusive = token.is(&TokenKind::DotDotEq);
                    after_operator = true;
                }
                Child::Node(expr) if expr.kind().is_expr() => {
                    let expr = Box::new(self.expr(expr));
                    if after_operator {
                        range.end = Some(expr);
                    } else {
                        range.start = Some(expr);
                    }
                }
                _ => {}
            }
        }
        range
    }

    fn bracket_args(&mut self, list: &GreenNode) -> Vec<BracketArg> {
        nodes_where(list, |kind| *kind == SyntaxKind::BracketArg)
            .map(|arg| {
                let span = arg.span();
                let expr = self.child_expr(arg);
                let ty = match &expr {
                    Some(_) => self
                        .alternate(AlternateKind::BracketType, span)
                        .cloned()
                        .map(|ty| self.type_ref(&ty)),
                    None => self.child_type(arg),
                };
                BracketArg { span, expr, ty }
            })
            .collect()
    }

    fn field_inits(&mut self, node: &GreenNode) -> Vec<FieldInit> {
        let Some(list) = node.child(&SyntaxKind::FieldInitList) else {
            return Vec::new();
        };
        nodes_where(list, |kind| *kind == SyntaxKind::FieldInit)
            .filter_map(|field| {
                let name_token = field.token(&TokenKind::Ident)?;
                let name = self.name(name_token);
                // A bare field is the canonical same-name initialization
                // shorthand; later phases see one field-initialization model.
                let value = match self.child_expr(field) {
                    Some(value) => value,
                    None => self.make_expr(name_token.span(), ExprKind::Ident(name)),
                };
                Some(FieldInit {
                    span: Span::new(field.span().start, value.span.end),
                    name,
                    value,
                })
            })
            .collect()
    }

    fn if_expr(&mut self, node: &GreenNode) -> ExprKind {
        // Conditions precede the then-block, which is the last node before
        // `else`; a condition may itself be a block expression.
        let mut head = Vec::new();
        let mut else_node = None;
        let mut after_else = false;
        for child in children(node) {
            match child {
                Child::Token(token) if token.is(&TokenKind::Else) => after_else = true,
                Child::Node(child) if after_else => {
                    if matches!(child.kind(), SyntaxKind::Block | SyntaxKind::IfExpr) {
                        else_node = Some(child);
                    }
                }
                Child::Node(child)
                    if child.kind().is_expr() || *child.kind() == SyntaxKind::IsClause =>
                {
                    head.push(child);
                }
                _ => {}
            }
        }
        let Some(then_node) = head.pop().filter(|node| *node.kind() == SyntaxKind::Block) else {
            return ExprKind::Error;
        };
        let mut clauses = Vec::new();
        let mut has_pattern = false;
        for clause in head {
            if *clause.kind() == SyntaxKind::IsClause {
                has_pattern = true;
                let target = self.boxed_expr(Self::first_expr_child(clause), clause.span());
                if let Some(pattern) = clause.child_nodes().find(|node| node.kind().is_pattern()) {
                    let pattern = self.pattern(pattern, true);
                    clauses.push(IfPatternChainClause::Pattern {
                        target: *target,
                        pattern,
                    });
                }
            } else {
                clauses.push(IfPatternChainClause::Condition(self.expr(clause)));
            }
        }
        let then_branch = self.block(then_node);
        let else_branch = else_node.map(|branch| {
            Box::new(if *branch.kind() == SyntaxKind::Block {
                let block = self.block(branch);
                self.make_expr(block.span, ExprKind::Block(block))
            } else {
                self.expr(branch)
            })
        });
        if !has_pattern {
            let cond = match clauses.pop() {
                Some(IfPatternChainClause::Condition(cond)) => cond,
                _ => self.make_expr(node.span(), ExprKind::Error),
            };
            return ExprKind::If {
                cond: Box::new(cond),
                then_branch,
                else_branch,
            };
        }
        if let [IfPatternChainClause::Pattern { .. }] = clauses.as_slice()
            && let Some(IfPatternChainClause::Pattern { target, pattern }) = clauses.pop()
        {
            return ExprKind::IfPattern(Box::new(IfPatternExpr {
                target,
                pattern,
                then_branch,
                else_branch,
            }));
        }
        ExprKind::IfPatternChain(Box::new(IfPatternChainExpr {
            clauses,
            then_branch,
            else_branch,
        }))
    }

    pub(crate) fn match_expr(&mut self, node: &GreenNode) -> MatchExpr {
        let target = Self::first_expr_child(node);
        let target = *self.boxed_expr(target, node.span());
        let arms = nodes_where(node, |kind| *kind == SyntaxKind::MatchArm)
            .filter_map(|arm| self.match_arm(arm))
            .collect();
        MatchExpr { target, arms }
    }

    fn match_arm(&mut self, node: &GreenNode) -> Option<MatchArm> {
        let mut patterns = Vec::new();
        let mut body = None;
        let mut after_arrow = false;
        for child in children(node) {
            match child {
                Child::Token(token) if token.is(&TokenKind::FatArrow) => after_arrow = true,
                Child::Node(pattern) if !after_arrow && pattern.kind().is_pattern() => {
                    patterns.push(self.pattern(pattern, true));
                }
                Child::Node(arm) if after_arrow => {
                    body = match arm.kind() {
                        SyntaxKind::Block => Some(MatchArmBody::Block(Box::new(self.block(arm)))),
                        kind if kind.is_stmt() => self
                            .stmt(arm)
                            .map(|stmt| MatchArmBody::Stmt(Box::new(stmt))),
                        kind if kind.is_expr() => {
                            Some(MatchArmBody::Expr(Box::new(self.expr(arm))))
                        }
                        _ => continue,
                    };
                }
                _ => {}
            }
        }
        Some(MatchArm {
            span: node.span(),
            patterns,
            body: body?,
        })
    }

    fn closure(&mut self, node: &GreenNode) -> ExprKind {
        let captures = node
            .child(&SyntaxKind::CaptureList)
            .map_or_else(Vec::new, |list| self.captures(list));
        let params = node
            .child(&SyntaxKind::ClosureParamList)
            .map_or_else(Vec::new, |list| self.params(list));
        let body = Self::first_expr_child(node);
        ExprKind::Closure {
            captures,
            params,
            body: self.boxed_expr(body, node.span()),
        }
    }

    fn captures(&mut self, list: &GreenNode) -> Vec<ClosureCapture> {
        nodes_where(list, |kind| *kind == SyntaxKind::Capture)
            .filter_map(|capture| {
                let name_token = capture.token(&TokenKind::Ident)?;
                let name = self.name(name_token);
                let span = capture.span();
                let value = self.make_expr(name_token.span(), ExprKind::Ident(name));
                // Capture modes reuse ordinary address expressions so
                // ownership, mutability, and lowering share one semantic path.
                let value = if has_token(capture, TokenKind::Amp) {
                    let op = if has_token(capture, TokenKind::Mut) {
                        UnaryOp::Ref
                    } else {
                        UnaryOp::RefReadOnly
                    };
                    self.make_expr(
                        span,
                        ExprKind::Unary {
                            op,
                            expr: Box::new(value),
                        },
                    )
                } else {
                    value
                };
                let node_key = self.node_key(NodeSyntaxKind::Expr, span);
                Some(ClosureCapture {
                    name,
                    value,
                    span,
                    node_key,
                })
            })
            .collect()
    }
}

fn binary_op(kind: &TokenKind) -> Option<BinaryOp> {
    Some(match kind {
        TokenKind::Or => BinaryOp::Or,
        TokenKind::And => BinaryOp::And,
        TokenKind::Pipe => BinaryOp::BitOr,
        TokenKind::Caret => BinaryOp::BitXor,
        TokenKind::Amp => BinaryOp::BitAnd,
        TokenKind::EqEq => BinaryOp::Eq,
        TokenKind::BangEq => BinaryOp::Ne,
        TokenKind::Lt => BinaryOp::Lt,
        TokenKind::LtEq => BinaryOp::Le,
        TokenKind::Gt => BinaryOp::Gt,
        TokenKind::GtEq => BinaryOp::Ge,
        TokenKind::LtLt => BinaryOp::Shl,
        TokenKind::GtGt => BinaryOp::Shr,
        TokenKind::Plus => BinaryOp::Add,
        TokenKind::Minus => BinaryOp::Sub,
        TokenKind::Star => BinaryOp::Mul,
        TokenKind::Slash => BinaryOp::Div,
        TokenKind::Percent => BinaryOp::Rem,
        _ => return None,
    })
}

fn assign_op(kind: &TokenKind) -> Option<AssignOp> {
    Some(match kind {
        TokenKind::Eq => AssignOp::Assign,
        TokenKind::PlusEq => AssignOp::Add,
        TokenKind::MinusEq => AssignOp::Sub,
        TokenKind::LtLtEq => AssignOp::Shl,
        TokenKind::GtGtEq => AssignOp::Shr,
        TokenKind::StarEq => AssignOp::Mul,
        TokenKind::SlashEq => AssignOp::Div,
        TokenKind::PercentEq => AssignOp::Rem,
        TokenKind::AmpEq => AssignOp::BitAnd,
        TokenKind::CaretEq => AssignOp::BitXor,
        TokenKind::PipeEq => AssignOp::BitOr,
        _ => return None,
    })
}
