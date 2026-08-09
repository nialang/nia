// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

enum BracketSuffix {
    Args(Vec<BracketArg>),
    Range(nia_ast::SliceRange),
}

impl Parser {
    fn bool_expr(&mut self, token: nia_syntax::SyntaxToken, value: bool) -> Expr {
        self.bump();
        self.make_expr(token.span, ExprKind::Bool(value))
    }

    pub(super) fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_assignment_until(&[])
    }

    fn parse_assignment_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let lhs = self.parse_range_until(stops)?;
        let Some(op) = self.assignment_op() else {
            return Some(lhs);
        };
        self.bump();
        let rhs = self.parse_assignment_until(stops)?;
        let span = Span::new(lhs.span.start, rhs.span.end);
        Some(self.make_expr(
            span,
            ExprKind::Assign {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
            },
        ))
    }

    pub(super) fn parse_condition_expr_until(
        &mut self,
        stops: &[TokenKind],
    ) -> Option<ConditionExpr> {
        self.parse_condition_or_until(stops)
    }

    fn parse_condition_or_until(&mut self, stops: &[TokenKind]) -> Option<ConditionExpr> {
        let mut lhs = self.parse_condition_and_until(stops)?;
        while !stops.iter().any(|kind| self.at(kind.clone())) && self.eat(TokenKind::Or).is_some() {
            let rhs = self.parse_condition_and_until(stops)?;
            let span = Span::new(lhs.span.start, rhs.span.end);
            lhs = ConditionExpr {
                span,
                kind: ConditionExprKind::Binary {
                    lhs: Box::new(lhs),
                    op: ConditionBinaryOp::Or,
                    rhs: Box::new(rhs),
                },
            };
        }
        Some(lhs)
    }

    fn parse_condition_and_until(&mut self, stops: &[TokenKind]) -> Option<ConditionExpr> {
        let mut lhs = self.parse_condition_equality_until(stops)?;
        while !stops.iter().any(|kind| self.at(kind.clone())) && self.eat(TokenKind::And).is_some()
        {
            let rhs = self.parse_condition_equality_until(stops)?;
            let span = Span::new(lhs.span.start, rhs.span.end);
            lhs = ConditionExpr {
                span,
                kind: ConditionExprKind::Binary {
                    lhs: Box::new(lhs),
                    op: ConditionBinaryOp::And,
                    rhs: Box::new(rhs),
                },
            };
        }
        Some(lhs)
    }

    fn parse_condition_equality_until(&mut self, stops: &[TokenKind]) -> Option<ConditionExpr> {
        let mut lhs = self.parse_condition_unary_until(stops)?;
        while !stops.iter().any(|kind| self.at(kind.clone())) {
            let op = if self.eat(TokenKind::EqEq).is_some() {
                ConditionBinaryOp::Eq
            } else if self.eat(TokenKind::BangEq).is_some() {
                ConditionBinaryOp::Ne
            } else {
                break;
            };
            let rhs = self.parse_condition_unary_until(stops)?;
            let span = Span::new(lhs.span.start, rhs.span.end);
            lhs = ConditionExpr {
                span,
                kind: ConditionExprKind::Binary {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            };
        }
        Some(lhs)
    }

    fn parse_condition_unary_until(&mut self, stops: &[TokenKind]) -> Option<ConditionExpr> {
        if self.at(TokenKind::Not) {
            let start = self.bump().span.start;
            let expr = self.parse_condition_unary_until(stops)?;
            return Some(ConditionExpr {
                span: Span::new(start, expr.span.end),
                kind: ConditionExprKind::Unary {
                    op: ConditionUnaryOp::Not,
                    expr: Box::new(expr),
                },
            });
        }
        self.parse_condition_primary_until(stops)
    }

    fn parse_condition_primary_until(&mut self, stops: &[TokenKind]) -> Option<ConditionExpr> {
        if stops.iter().any(|kind| self.at(kind.clone())) {
            self.error_here("expected condition expression");
            return None;
        }
        let token = self.peek().clone();
        match token.kind {
            TokenKind::True => {
                self.bump();
                Some(ConditionExpr {
                    span: token.span,
                    kind: ConditionExprKind::Bool(true),
                })
            }
            TokenKind::False => {
                self.bump();
                Some(ConditionExpr {
                    span: token.span,
                    kind: ConditionExprKind::Bool(false),
                })
            }
            TokenKind::Integer => {
                self.bump();
                Some(ConditionExpr {
                    span: token.span,
                    kind: ConditionExprKind::Integer(self.token_text(&token).to_string()),
                })
            }
            TokenKind::String => {
                self.bump();
                Some(ConditionExpr {
                    span: token.span,
                    kind: ConditionExprKind::String(self.token_text(&token).to_string()),
                })
            }
            TokenKind::Ident => {
                self.bump();
                Some(ConditionExpr {
                    span: token.span,
                    kind: ConditionExprKind::Ident(self.token_name(&token)?),
                })
            }
            TokenKind::LParen => {
                let start = self.bump().span.start;
                let expr = self.parse_condition_expr_until(&[TokenKind::RParen])?;
                let end = self
                    .expect(TokenKind::RParen, "expected `)` after condition expression")?
                    .end;
                Some(ConditionExpr {
                    span: Span::new(start, end),
                    kind: expr.kind,
                })
            }
            _ => {
                self.error_here("expected condition expression");
                None
            }
        }
    }

    fn parse_range_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        if self.at(TokenKind::DotDot) || self.at(TokenKind::DotDotEq) {
            let start = self.peek().span.start;
            let inclusive = self.eat(TokenKind::DotDotEq).is_some();
            if !inclusive {
                self.expect(TokenKind::DotDot, "expected `..` in range expression")?;
            }
            let end = if stops.iter().any(|kind| self.at(kind.clone()))
                || self.at(TokenKind::Comma)
                || self.at(TokenKind::RParen)
                || self.at(TokenKind::RBracket)
                || self.at(TokenKind::RBrace)
                || self.at(TokenKind::Semicolon)
                || self.at(TokenKind::Eof)
            {
                None
            } else {
                Some(Box::new(self.parse_binary_until(0, stops)?))
            };
            let span = Span::new(
                start,
                end.as_ref().map_or(self.previous_end(), |end| end.span.end),
            );
            return Some(self.make_expr(
                span,
                ExprKind::Range(nia_ast::SliceRange {
                    start: None,
                    end,
                    inclusive,
                }),
            ));
        }

        let range_stops = self.range_start_stops(stops);
        let start_expr = self.parse_binary_until(0, &range_stops)?;
        if stops.iter().any(|kind| self.at(kind.clone())) {
            return Some(start_expr);
        }
        if self.eat(TokenKind::DotDot).is_some() {
            let end = if stops.iter().any(|kind| self.at(kind.clone()))
                || self.at(TokenKind::Comma)
                || self.at(TokenKind::RParen)
                || self.at(TokenKind::RBracket)
                || self.at(TokenKind::RBrace)
                || self.at(TokenKind::Semicolon)
                || self.at(TokenKind::Eof)
            {
                None
            } else {
                Some(Box::new(self.parse_binary_until(0, stops)?))
            };
            let span = Span::new(
                start_expr.span.start,
                end.as_ref().map_or(self.previous_end(), |end| end.span.end),
            );
            return Some(self.make_expr(
                span,
                ExprKind::Range(nia_ast::SliceRange {
                    start: Some(Box::new(start_expr)),
                    end,
                    inclusive: false,
                }),
            ));
        }
        if self.eat(TokenKind::DotDotEq).is_some() {
            let end = Box::new(self.parse_binary_until(0, stops)?);
            let span = Span::new(start_expr.span.start, end.span.end);
            return Some(self.make_expr(
                span,
                ExprKind::Range(nia_ast::SliceRange {
                    start: Some(Box::new(start_expr)),
                    end: Some(end),
                    inclusive: true,
                }),
            ));
        }
        Some(start_expr)
    }

    fn range_start_stops(&self, stops: &[TokenKind]) -> Vec<TokenKind> {
        let mut range_stops = stops.to_vec();
        for stop in [
            TokenKind::DotDot,
            TokenKind::DotDotEq,
            TokenKind::Comma,
            TokenKind::RParen,
            TokenKind::RBracket,
            TokenKind::RBrace,
            TokenKind::Semicolon,
        ] {
            if !range_stops.contains(&stop) {
                range_stops.push(stop);
            }
        }
        range_stops
    }

    fn parse_binary_until(&mut self, min_prec: u8, stops: &[TokenKind]) -> Option<Expr> {
        let mut lhs = self.parse_not_until(stops)?;
        while let Some((op, prec)) = self.binary_op() {
            if stops.iter().any(|kind| self.at(kind.clone())) {
                break;
            }
            if expr_can_terminate_statement_without_semicolon(&lhs)
                && self.has_line_break_between(lhs.span.end, self.peek().span.start)
            {
                break;
            }
            if prec < min_prec {
                break;
            }
            self.bump();
            let rhs = self.parse_binary_until(prec + 1, stops)?;
            let span = Span::new(lhs.span.start, rhs.span.end);
            lhs = self.make_expr(
                span,
                ExprKind::Binary {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            );
        }
        Some(lhs)
    }

    fn parse_not_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        if !self.at(TokenKind::Not) {
            return self.parse_cast_until(stops);
        }
        let start = self
            .expect(TokenKind::Not, "expected `not` for logical negation")?
            .start;
        let expr = self.parse_binary_until(6, stops)?;
        Some(self.make_expr(
            Span::new(start, expr.span.end),
            ExprKind::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            },
        ))
    }

    fn parse_cast_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let mut expr = self.parse_unary_until(stops)?;
        while self.eat(TokenKind::As).is_some() {
            let ty = self.parse_type()?;
            expr = self.make_expr(
                Span::new(expr.span.start, ty.span.end),
                ExprKind::Cast {
                    expr: Box::new(expr),
                    ty,
                },
            );
        }
        Some(expr)
    }

    fn parse_unary_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let start = self.peek().span.start;
        let op = if self.eat(TokenKind::Minus).is_some() {
            Some(UnaryOp::Neg)
        } else if self.eat(TokenKind::Tilde).is_some() {
            Some(UnaryOp::BitNot)
        } else if self.eat(TokenKind::Amp).is_some() {
            if self.eat(TokenKind::Mut).is_some() {
                Some(UnaryOp::Ref)
            } else {
                Some(UnaryOp::RefReadOnly)
            }
        } else {
            None
        };
        if let Some(op) = op {
            let expr = self.parse_unary_until(stops)?;
            return Some(self.make_expr(
                Span::new(start, expr.span.end),
                ExprKind::Unary {
                    op,
                    expr: Box::new(expr),
                },
            ));
        }
        if self.at(TokenKind::Question) {
            let start = self
                .expect(TokenKind::Question, "expected `?` before optional value")?
                .start;
            let expr = self.parse_unary_until(stops)?;
            return Some(self.make_expr(
                Span::new(start, expr.span.end),
                ExprKind::OptionalSome {
                    expr: Box::new(expr),
                },
            ));
        }
        if self.at(TokenKind::Bang) {
            let start = self
                .expect(
                    TokenKind::Bang,
                    "expected `!` before error-union success value",
                )?
                .start;
            let expr = self.parse_unary_until(stops)?;
            return Some(self.make_expr(
                Span::new(start, expr.span.end),
                ExprKind::ErrorOk {
                    expr: Box::new(expr),
                },
            ));
        }
        self.parse_postfix_until(stops)
    }

    fn parse_postfix_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let mut expr = self.parse_primary_until(stops)?;
        loop {
            if stops.iter().any(|kind| self.at(kind.clone())) {
                break;
            }
            if expr_can_terminate_statement_without_semicolon(&expr)
                && self.has_line_break_between(expr.span.end, self.peek().span.start)
            {
                break;
            }
            if self.eat(TokenKind::LParen).is_some() {
                let mut args = Vec::new();
                while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                    args.push(
                        self.parse_expr_until_tokens(&[TokenKind::Comma, TokenKind::RParen])?,
                    );
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                let end = self
                    .expect(TokenKind::RParen, "expected `)` after call")?
                    .end;
                expr = self.make_expr(
                    Span::new(expr.span.start, end),
                    ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                );
                continue;
            }
            if self.eat(TokenKind::Dot).is_some() {
                if self.eat(TokenKind::Question).is_some() {
                    let end = self.previous_end();
                    expr = self.make_expr(
                        Span::new(expr.span.start, end),
                        ExprKind::Try {
                            expr: Box::new(expr),
                        },
                    );
                    continue;
                }
                if self.eat(TokenKind::Star).is_some() {
                    let end = self.previous_end();
                    expr = self.make_expr(
                        Span::new(expr.span.start, end),
                        ExprKind::Unary {
                            op: UnaryOp::Deref,
                            expr: Box::new(expr),
                        },
                    );
                    continue;
                }
                if self.at(TokenKind::Integer) {
                    let token = self.bump();
                    let text = self.token_text(&token);
                    if !text.chars().all(|ch| ch.is_ascii_digit()) {
                        self.error_at(token.span, "tuple field must be a decimal integer");
                        return None;
                    }
                    if text.len() > 1 && text.starts_with('0') {
                        self.error_at(token.span, "tuple field must not contain leading zeroes");
                        return None;
                    }
                    let Some(index) = text.parse::<usize>().ok() else {
                        self.error_at(token.span, "tuple field index is too large");
                        return None;
                    };
                    let end = token.span.end;
                    expr = self.make_expr(
                        Span::new(expr.span.start, end),
                        ExprKind::TupleField {
                            lhs: Box::new(expr),
                            index,
                        },
                    );
                    continue;
                }
                let name = self.expect_name(TokenKind::Ident, "expected field name")?;
                let end = self.previous_end();
                expr = self.make_expr(
                    Span::new(expr.span.start, end),
                    ExprKind::Field {
                        lhs: Box::new(expr),
                        name,
                    },
                );
                continue;
            }
            if self.eat(TokenKind::Bang).is_some() {
                let end = self.previous_end();
                expr = self.make_expr(
                    Span::new(expr.span.start, end),
                    ExprKind::ErrorErr {
                        expr: Box::new(expr),
                    },
                );
                continue;
            }
            if self.eat(TokenKind::ColonColon).is_some() {
                let name = self.expect_name(TokenKind::Ident, "expected name after `::`")?;
                let end = self.previous_end();
                expr = self.make_expr(
                    Span::new(expr.span.start, end),
                    ExprKind::Qualified {
                        lhs: Box::new(expr),
                        name,
                    },
                );
                continue;
            }
            if self.eat(TokenKind::LBracket).is_some() {
                let suffix = self.parse_bracket_suffix_after_open()?;
                let end = self
                    .expect(TokenKind::RBracket, "expected `]` after bracket suffix")?
                    .end;
                expr = self.make_expr(
                    Span::new(expr.span.start, end),
                    match suffix {
                        BracketSuffix::Args(args) => ExprKind::BracketSuffix {
                            callee: Box::new(expr),
                            args,
                        },
                        BracketSuffix::Range(range) => ExprKind::Index {
                            lhs: Box::new(expr),
                            index: nia_ast::IndexArg::Range(range),
                        },
                    },
                );
                continue;
            }
            break;
        }
        Some(expr)
    }

    pub(super) fn parse_expr_until_tokens(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        let expr = self.parse_assignment_until(stops)?;
        if stops.iter().any(|kind| self.at(kind.clone())) {
            return Some(expr);
        }
        self.tokens.rewind(checkpoint);
        self.errors.truncate(errors_len);
        self.parse_binary_until(0, stops)
    }

    fn parse_bracket_suffix_after_open(&mut self) -> Option<BracketSuffix> {
        if self.eat(TokenKind::DotDot).is_some() {
            let end = if self.at(TokenKind::RBracket) {
                None
            } else {
                Some(Box::new(
                    self.parse_binary_until(0, &[TokenKind::RBracket])?,
                ))
            };
            return Some(BracketSuffix::Range(nia_ast::SliceRange {
                start: None,
                end,
                inclusive: false,
            }));
        }
        if self.eat(TokenKind::DotDotEq).is_some() {
            let end = Some(Box::new(
                self.parse_binary_until(0, &[TokenKind::RBracket])?,
            ));
            return Some(BracketSuffix::Range(nia_ast::SliceRange {
                start: None,
                end,
                inclusive: true,
            }));
        }

        let checkpoint = self.tokens.checkpoint();
        let expr_errors_len = self.errors.len();
        let first = self.parse_binary_until(
            0,
            &[
                TokenKind::Comma,
                TokenKind::RBracket,
                TokenKind::DotDot,
                TokenKind::DotDotEq,
            ],
        );
        if self.eat(TokenKind::DotDot).is_some() {
            let first = first?;
            let end = if self.at(TokenKind::RBracket) {
                None
            } else {
                Some(Box::new(
                    self.parse_binary_until(0, &[TokenKind::RBracket])?,
                ))
            };
            Some(BracketSuffix::Range(nia_ast::SliceRange {
                start: Some(Box::new(first)),
                end,
                inclusive: false,
            }))
        } else if self.eat(TokenKind::DotDotEq).is_some() {
            let first = first?;
            let end = Some(Box::new(
                self.parse_binary_until(0, &[TokenKind::RBracket])?,
            ));
            Some(BracketSuffix::Range(nia_ast::SliceRange {
                start: Some(Box::new(first)),
                end,
                inclusive: true,
            }))
        } else {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(expr_errors_len);
            Some(BracketSuffix::Args(self.parse_bracket_args_after_open()?))
        }
    }

    fn parse_bracket_args_after_open(&mut self) -> Option<Vec<BracketArg>> {
        let mut args = Vec::new();
        while !self.at(TokenKind::RBracket) && !self.at(TokenKind::Eof) {
            let start = self.peek().span.start;
            let type_checkpoint = self.tokens.checkpoint();
            let type_errors_len = self.errors.len();
            let ty = self.parse_type();
            let type_end = ty.as_ref().map(|ty| ty.span.end);
            self.tokens.rewind(type_checkpoint);
            self.errors.truncate(type_errors_len);

            let expr_checkpoint = self.tokens.checkpoint();
            let expr_errors_len = self.errors.len();
            let mut expr = if type_end.is_some()
                && !matches!(
                    self.peek().kind,
                    TokenKind::Integer
                        | TokenKind::Float
                        | TokenKind::String
                        | TokenKind::Char
                        | TokenKind::ByteChar
                        | TokenKind::True
                        | TokenKind::False
                        | TokenKind::Null
                        | TokenKind::Ident
                        | TokenKind::Underscore
                        | TokenKind::At
                        | TokenKind::LBracket
                        | TokenKind::LParen
                        | TokenKind::LBrace
                        | TokenKind::If
                        | TokenKind::Not
                        | TokenKind::Minus
                        | TokenKind::Bang
                        | TokenKind::Question
                        | TokenKind::Amp
                        | TokenKind::Star
                ) {
                None
            } else {
                self.parse_expr_until_tokens(&[TokenKind::Comma, TokenKind::RBracket])
            };
            let expr_end = expr.as_ref().map(|expr| expr.span.end);
            if expr.is_none() || expr_end.is_some_and(|expr_end| Some(expr_end) < type_end) {
                self.tokens.rewind(expr_checkpoint);
                self.errors.truncate(expr_errors_len);
                expr = None;
            }

            let end = match (type_end, expr_end) {
                (Some(type_end), Some(expr_end)) => type_end.max(expr_end),
                (Some(type_end), None) => type_end,
                (None, Some(expr_end)) => expr_end,
                (None, None) => {
                    self.error_here("expected bracket argument");
                    return None;
                }
            };
            if expr_end.is_none() || expr_end.is_some_and(|expr_end| expr_end < end) {
                self.tokens.rewind(type_checkpoint);
                while !self.at(TokenKind::Comma)
                    && !self.at(TokenKind::RBracket)
                    && !self.at(TokenKind::Eof)
                    && self.peek().span.start < end
                {
                    self.bump();
                }
            }
            args.push(BracketArg {
                span: Span::new(start, end),
                expr,
                ty: ty.filter(|ty| ty.span.end == end),
            });
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        Some(args)
    }

    fn parse_primary_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        if !stops.contains(&TokenKind::LBrace)
            && let Some(expr) = self.parse_qualified_struct_literal()
        {
            return Some(expr);
        }
        if let Some(expr) = self.parse_typed_aggregate_literal(stops) {
            return Some(expr);
        }
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Integer => Some(self.literal_expr(token, ExprKind::Integer)),
            TokenKind::Float => Some(self.literal_expr(token, ExprKind::Float)),
            TokenKind::String => self.parse_string_literal_run(TokenKind::String, ExprKind::String),
            TokenKind::ByteString => {
                self.parse_string_literal_run(TokenKind::ByteString, ExprKind::ByteString)
            }
            TokenKind::Char => Some(self.literal_expr(token, ExprKind::Char)),
            TokenKind::ByteChar => Some(self.literal_expr(token, ExprKind::ByteChar)),
            TokenKind::True => Some(self.bool_expr(token, true)),
            TokenKind::False => Some(self.bool_expr(token, false)),
            TokenKind::Null => {
                self.bump();
                Some(self.make_expr(token.span, ExprKind::Null))
            }
            TokenKind::Ident => {
                self.bump();
                let name = self.token_name(&token)?;
                Some(self.make_expr(token.span, ExprKind::Ident(name)))
            }
            TokenKind::SelfValue => {
                self.bump();
                Some(self.make_expr(token.span, ExprKind::SelfValue))
            }
            TokenKind::Pkg => {
                self.bump();
                Some(self.make_expr(token.span, ExprKind::PathRoot(PathSegmentKind::Package)))
            }
            TokenKind::Super => {
                self.bump();
                Some(self.make_expr(token.span, ExprKind::PathRoot(PathSegmentKind::Super)))
            }
            TokenKind::Underscore => {
                self.bump();
                Some(self.make_expr(token.span, ExprKind::Underscore))
            }
            TokenKind::LBracket => self.parse_bracket_primary(),
            TokenKind::LParen => {
                self.bump();
                if let Some(end) = self.eat(TokenKind::RParen) {
                    return Some(self.make_expr(
                        Span::new(token.span.start, end.span.end),
                        ExprKind::Tuple(Vec::new()),
                    ));
                }
                let first = self.parse_expr_until_tokens(&[TokenKind::Comma, TokenKind::RParen])?;
                if self.eat(TokenKind::Comma).is_none() {
                    self.expect(TokenKind::RParen, "expected `)`")?;
                    return Some(first);
                }
                let mut elems = vec![first];
                while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                    elems.push(
                        self.parse_expr_until_tokens(&[TokenKind::Comma, TokenKind::RParen])?,
                    );
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                let end = self
                    .expect(TokenKind::RParen, "expected `)` after tuple")?
                    .end;
                Some(self.make_expr(Span::new(token.span.start, end), ExprKind::Tuple(elems)))
            }
            TokenKind::LBrace if self.looks_like_inferred_struct_literal() => {
                self.parse_inferred_struct_literal()
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Some(self.make_expr(block.span, ExprKind::Block(block)))
            }
            TokenKind::If => self.parse_if_expr(),
            TokenKind::Switch => self.parse_switch_expr(),
            _ => {
                self.error_here("expected expression");
                None
            }
        }
    }

    fn parse_qualified_struct_literal(&mut self) -> Option<Expr> {
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        let Some(target) = self.parse_qualified_value_path() else {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        };
        if self.eat(TokenKind::LBrace).is_none() {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        }
        let fields = self.parse_struct_literal_fields()?;
        let end = self
            .expect(TokenKind::RBrace, "expected `}` after qualified literal")?
            .end;
        Some(self.make_expr(
            Span::new(target.span.start, end),
            ExprKind::QualifiedStructLiteral {
                target: Box::new(target),
                fields,
            },
        ))
    }

    pub(super) fn parse_qualified_value_path(&mut self) -> Option<Expr> {
        let token = self.peek().clone();
        let kind = match token.kind {
            TokenKind::Ident => ExprKind::Ident(self.token_name(&token)?),
            TokenKind::Pkg => ExprKind::PathRoot(PathSegmentKind::Package),
            TokenKind::Super => ExprKind::PathRoot(PathSegmentKind::Super),
            _ => return None,
        };
        self.bump();
        let mut expr = self.make_expr(token.span, kind);
        let mut qualified = false;
        while self.eat(TokenKind::ColonColon).is_some() {
            let name = self.expect_name(TokenKind::Ident, "expected name after `::`")?;
            expr = self.make_expr(
                Span::new(expr.span.start, self.previous_end()),
                ExprKind::Qualified {
                    lhs: Box::new(expr),
                    name,
                },
            );
            qualified = true;
        }
        qualified.then_some(expr)
    }

    fn parse_typed_aggregate_literal(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        if !self.type_can_start() {
            return None;
        }
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        let start = self.peek().span.start;
        let Some(ty) = self.parse_type_before_aggregate_literal() else {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        };
        if self.at(TokenKind::LBrace) && !stops.iter().any(|kind| self.at(kind.clone())) {
            self.expect(TokenKind::LBrace, "expected `{` before struct literal")?;
            let fields = self.parse_struct_literal_fields()?;
            let end = self
                .expect(TokenKind::RBrace, "expected `}` after struct literal")?
                .end;
            return Some(self.make_expr(
                Span::new(start, end),
                ExprKind::TypedStructLiteral { ty, fields },
            ));
        }
        if matches!(ty.kind, TypeKind::Array { .. })
            && self.at(TokenKind::LBracket)
            && !stops.iter().any(|kind| self.at(kind.clone()))
        {
            self.expect(TokenKind::LBracket, "expected `[` before array literal")?;
            let elems = self.parse_array_elements_until_rbracket()?;
            let end = self
                .expect(TokenKind::RBracket, "expected `]` after array literal")?
                .end;
            return Some(self.make_expr(
                Span::new(start, end),
                ExprKind::TypedArrayLiteral { ty, elems },
            ));
        }
        self.tokens.rewind(checkpoint);
        self.errors.truncate(errors_len);
        None
    }

    fn parse_if_expr(&mut self) -> Option<Expr> {
        let start = self.expect(TokenKind::If, "expected `if`")?.start;
        let target = self.parse_expr_until_tokens(&[TokenKind::Is, TokenKind::LBrace])?;
        if self.eat(TokenKind::Is).is_some() {
            return self.parse_if_pattern_expr(start, target);
        }
        let cond = target;
        let then_branch = self.parse_block()?;
        let else_branch = if self.eat(TokenKind::Else).is_some() {
            if self.at(TokenKind::If) {
                Some(Box::new(self.parse_if_expr()?))
            } else {
                let block = self.parse_block()?;
                Some(Box::new(self.make_expr(block.span, ExprKind::Block(block))))
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map_or(then_branch.span.end, |expr| expr.span.end);
        Some(self.make_expr(
            Span::new(start, end),
            ExprKind::If {
                cond: Box::new(cond),
                then_branch,
                else_branch,
            },
        ))
    }

    fn parse_if_pattern_expr(&mut self, start: usize, target: Expr) -> Option<Expr> {
        let pattern = self.parse_binding_pattern_until_tokens(&[TokenKind::LBrace])?;
        let then_branch = self.parse_block()?;
        let else_branch = if self.eat(TokenKind::Else).is_some() {
            if self.at(TokenKind::If) {
                Some(Box::new(self.parse_if_expr()?))
            } else {
                let block = self.parse_block()?;
                Some(Box::new(self.make_expr(block.span, ExprKind::Block(block))))
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map_or(then_branch.span.end, |expr| expr.span.end);
        Some(self.make_expr(
            Span::new(start, end),
            ExprKind::IfPattern(Box::new(IfPatternExpr {
                target,
                pattern,
                then_branch,
                else_branch,
            })),
        ))
    }

    fn parse_bracket_primary(&mut self) -> Option<Expr> {
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        let start = self.peek().span.start;
        self.expect(TokenKind::LBracket, "expected `[` before type target")?;
        if let Some((ty, trait_ref)) = self.parse_trait_target_after_open()
            && self.at(TokenKind::ColonColon)
        {
            let end = self.previous_end();
            return Some(self.make_expr(
                Span::new(start, end),
                ExprKind::TraitTarget { ty, trait_ref },
            ));
        }
        self.tokens.rewind(checkpoint);
        self.errors.truncate(errors_len);

        self.expect(TokenKind::LBracket, "expected `[` before type target")?;
        if let Some(ty) = self.parse_type_target_type_after_open()
            && self.at(TokenKind::ColonColon)
        {
            let end = self.previous_end();
            return Some(self.make_expr(Span::new(start, end), ExprKind::TypeTarget { ty }));
        }
        self.tokens.rewind(checkpoint);
        self.errors.truncate(errors_len);
        self.parse_bracket_array_literal()
    }

    fn parse_trait_target_after_open(&mut self) -> Option<(TypeRef, TypeRef)> {
        if !self.token_can_start_type(&self.peek().kind) {
            return None;
        }
        let ty = self.parse_type()?;
        self.expect(TokenKind::As, "expected `as` in trait target")?;
        let trait_ref = self.parse_type()?;
        self.expect(TokenKind::RBracket, "expected `]` after trait target")?;
        Some((ty, trait_ref))
    }

    fn parse_type_target_type_after_open(&mut self) -> Option<TypeRef> {
        if !self.token_can_start_type(&self.peek().kind) {
            let ty_start = self.peek().span.start;
            self.error_at(
                Span::new(ty_start, self.peek().span.end),
                "expected type target",
            );
            return None;
        }
        let ty = self.parse_type()?;
        self.expect(TokenKind::RBracket, "expected `]` after type target")?;
        Some(ty)
    }

    fn parse_bracket_array_literal(&mut self) -> Option<Expr> {
        let start = self.peek().span.start;
        self.expect(TokenKind::LBracket, "expected `[` before array literal")?;
        let elems = self.parse_array_elements_until_rbracket()?;
        let end = self
            .expect(TokenKind::RBracket, "expected `]` after array literal")?
            .end;
        Some(self.make_expr(Span::new(start, end), ExprKind::ArrayLiteral { elems }))
    }

    fn parse_array_elements_until_rbracket(&mut self) -> Option<ArrayElements> {
        let mut elems = Vec::new();
        let elements = if self.at(TokenKind::RBracket) {
            ArrayElements::List(elems)
        } else {
            let first = self.parse_expr()?;
            if self.eat(TokenKind::Semicolon).is_some() {
                let count = self.parse_expr()?;
                ArrayElements::Repeat {
                    value: Box::new(first),
                    count: Box::new(count),
                }
            } else {
                elems.push(first);
                while self.eat(TokenKind::Comma).is_some() && !self.at(TokenKind::RBracket) {
                    elems.push(self.parse_expr()?);
                }
                ArrayElements::List(elems)
            }
        };
        Some(elements)
    }

    fn parse_inferred_struct_literal(&mut self) -> Option<Expr> {
        let start = self.peek().span.start;
        self.expect(TokenKind::LBrace, "expected `{` before struct literal")?;
        let fields = self.parse_struct_literal_fields()?;
        let end = self
            .expect(TokenKind::RBrace, "expected `}` after struct literal")?
            .end;
        Some(self.make_expr(Span::new(start, end), ExprKind::StructLiteral { fields }))
    }

    fn parse_struct_literal_fields(&mut self) -> Option<Vec<FieldInit>> {
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let field_start = self.peek().span.start;
            let name = self.expect_name(TokenKind::Ident, "expected field name")?;
            self.expect(TokenKind::Colon, "expected `:` after field name")?;
            let value = self.parse_expr()?;
            fields.push(FieldInit {
                span: Span::new(field_start, value.span.end),
                name,
                value,
            });
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        Some(fields)
    }

    fn looks_like_inferred_struct_literal(&self) -> bool {
        self.tokens
            .nth(1)
            .is_some_and(|token| token.kind == TokenKind::Ident)
            && self
                .tokens
                .nth(2)
                .is_some_and(|token| token.kind == TokenKind::Colon)
    }

    fn literal_expr(&mut self, token: SyntaxToken, make: impl FnOnce(String) -> ExprKind) -> Expr {
        self.bump();
        self.make_expr(token.span, make(self.token_text(&token).to_string()))
    }

    fn parse_string_literal_run(
        &mut self,
        kind: TokenKind,
        make: impl FnOnce(StringLiteral) -> ExprKind,
    ) -> Option<Expr> {
        let first = self.peek().clone();
        let start = first.span.start;
        let mut parts = Vec::new();
        let quoted_run = self.token_is_quoted_string_literal(&first);
        let end = if quoted_run {
            let mut end = start;
            while self.at(kind.clone()) && self.token_is_quoted_string_literal(self.peek()) {
                let token = self.bump();
                end = token.span.end;
                parts.push(self.token_text(&token).to_string());
            }
            end
        } else {
            let token = self.bump();
            parts.push(self.token_text(&token).to_string());
            token.span.end
        };
        if quoted_run && self.peek_is_quoted_string_literal() {
            self.error_here("adjacent string literals must use the same literal prefix");
        }
        (!parts.is_empty())
            .then(|| self.make_expr(Span::new(start, end), make(StringLiteral { parts })))
    }

    fn peek_is_quoted_string_literal(&self) -> bool {
        matches!(self.peek().kind, TokenKind::String | TokenKind::ByteString)
            && self.token_is_quoted_string_literal(self.peek())
    }

    fn token_is_quoted_string_literal(&self, token: &SyntaxToken) -> bool {
        let text = self.token_text(token);
        !text.strip_prefix('b').unwrap_or(text).starts_with("\\\\")
    }

    fn has_line_break_between(&self, start: usize, end: usize) -> bool {
        self.source.get(start..end).is_some_and(|text| {
            text.as_bytes()
                .iter()
                .any(|byte| *byte == b'\n' || *byte == b'\r')
        })
    }

    fn assignment_op(&self) -> Option<AssignOp> {
        Some(match self.peek().kind {
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

    fn binary_op(&self) -> Option<(BinaryOp, u8)> {
        Some(match self.peek().kind {
            TokenKind::Or => (BinaryOp::Or, 1),
            TokenKind::And => (BinaryOp::And, 2),
            TokenKind::Pipe => (BinaryOp::BitOr, 3),
            TokenKind::Caret => (BinaryOp::BitXor, 4),
            TokenKind::Amp => (BinaryOp::BitAnd, 5),
            TokenKind::EqEq => (BinaryOp::Eq, 6),
            TokenKind::BangEq => (BinaryOp::Ne, 6),
            TokenKind::Lt => (BinaryOp::Lt, 7),
            TokenKind::LtEq => (BinaryOp::Le, 7),
            TokenKind::Gt => (BinaryOp::Gt, 7),
            TokenKind::GtEq => (BinaryOp::Ge, 7),
            TokenKind::LtLt => (BinaryOp::Shl, 8),
            TokenKind::GtGt => (BinaryOp::Shr, 8),
            TokenKind::Plus => (BinaryOp::Add, 9),
            TokenKind::Minus => (BinaryOp::Sub, 9),
            TokenKind::Star => (BinaryOp::Mul, 10),
            TokenKind::Slash => (BinaryOp::Div, 10),
            TokenKind::Percent => (BinaryOp::Rem, 10),
            _ => return None,
        })
    }
}

pub(super) fn expr_can_terminate_statement_without_semicolon(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Block(_) | ExprKind::If { .. } | ExprKind::IfPattern(_) | ExprKind::Switch(_)
    )
}
