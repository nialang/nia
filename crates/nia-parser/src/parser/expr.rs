// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

enum BracketSuffix {
    Args(Vec<BracketArg>),
    Range(nia_ast::SliceRange),
}

impl<'a> Parser<'a> {
    pub(super) fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Option<Expr> {
        let lhs = self.parse_binary_until(0, &[])?;
        let Some(op) = self.assignment_op() else {
            return Some(lhs);
        };
        self.bump();
        let rhs = self.parse_assignment()?;
        let span = Span::new(lhs.span.start, rhs.span.end);
        Some(Expr {
            span,
            kind: ExprKind::Assign {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
            },
        })
    }

    fn parse_binary_until(&mut self, min_prec: u8, stops: &[TokenKind]) -> Option<Expr> {
        let mut lhs = self.parse_cast()?;
        while let Some((op, prec)) = self.binary_op() {
            if stops.iter().any(|kind| self.at(kind.clone())) {
                break;
            }
            if prec < min_prec {
                break;
            }
            self.bump();
            let rhs = self.parse_binary_until(prec + 1, stops)?;
            let span = Span::new(lhs.span.start, rhs.span.end);
            lhs = Expr {
                span,
                kind: ExprKind::Binary {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            };
        }
        Some(lhs)
    }

    fn parse_cast(&mut self) -> Option<Expr> {
        let mut expr = self.parse_unary()?;
        while self.eat(TokenKind::As).is_some() {
            let ty = self.parse_type()?;
            expr = Expr {
                span: Span::new(expr.span.start, ty.span.end),
                kind: ExprKind::Cast {
                    expr: Box::new(expr),
                    ty,
                },
            };
        }
        Some(expr)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        let start = self.peek().span.start;
        let op = if self.eat(TokenKind::Minus).is_some() {
            Some(UnaryOp::Neg)
        } else if self.eat(TokenKind::Bang).is_some() {
            Some(UnaryOp::Not)
        } else if self.eat(TokenKind::Amp).is_some() {
            if self.eat(TokenKind::Const).is_some() {
                Some(UnaryOp::RefConst)
            } else {
                Some(UnaryOp::Ref)
            }
        } else if self.eat(TokenKind::Star).is_some() {
            Some(UnaryOp::Deref)
        } else {
            None
        };
        if let Some(op) = op {
            let expr = self.parse_unary()?;
            return Some(Expr {
                span: Span::new(start, expr.span.end),
                kind: ExprKind::Unary {
                    op,
                    expr: Box::new(expr),
                },
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
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
                expr = Expr {
                    span: Span::new(expr.span.start, end),
                    kind: ExprKind::Call {
                        callee: Box::new(expr),
                        args,
                    },
                };
                continue;
            }
            if self.eat(TokenKind::Dot).is_some() {
                if self.eat(TokenKind::Star).is_some() {
                    let end = self.previous_end();
                    expr = Expr {
                        span: Span::new(expr.span.start, end),
                        kind: ExprKind::Unary {
                            op: UnaryOp::Deref,
                            expr: Box::new(expr),
                        },
                    };
                    continue;
                }
                let name = self.expect_text(TokenKind::Ident, "expected field name")?;
                let end = self.previous_end();
                expr = Expr {
                    span: Span::new(expr.span.start, end),
                    kind: ExprKind::Field {
                        lhs: Box::new(expr),
                        name,
                    },
                };
                continue;
            }
            if self.eat(TokenKind::ColonColon).is_some() {
                let name = self.expect_text(TokenKind::Ident, "expected name after `::`")?;
                let end = self.previous_end();
                expr = Expr {
                    span: Span::new(expr.span.start, end),
                    kind: ExprKind::Qualified {
                        lhs: Box::new(expr),
                        name,
                    },
                };
                continue;
            }
            if self.eat(TokenKind::LBracket).is_some() {
                let suffix = self.parse_bracket_suffix_after_open()?;
                let end = self
                    .expect(TokenKind::RBracket, "expected `]` after bracket suffix")?
                    .end;
                expr = Expr {
                    span: Span::new(expr.span.start, end),
                    kind: match suffix {
                        BracketSuffix::Args(args) => ExprKind::BracketSuffix {
                            callee: Box::new(expr),
                            args,
                        },
                        BracketSuffix::Range(range) => ExprKind::Index {
                            lhs: Box::new(expr),
                            index: nia_ast::IndexArg::Range(range),
                        },
                    },
                };
                continue;
            }
            break;
        }
        Some(expr)
    }

    pub(super) fn parse_expr_until_tokens(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let checkpoint = self.pos;
        let errors_len = self.errors.len();
        let expr = self.parse_expr()?;
        if stops.iter().any(|kind| self.at(kind.clone())) {
            return Some(expr);
        }
        self.pos = checkpoint;
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

        let checkpoint = self.pos;
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
            self.pos = checkpoint;
            self.errors.truncate(expr_errors_len);
            Some(BracketSuffix::Args(self.parse_bracket_args_after_open()?))
        }
    }

    fn parse_bracket_args_after_open(&mut self) -> Option<Vec<BracketArg>> {
        let mut args = Vec::new();
        while !self.at(TokenKind::RBracket) && !self.at(TokenKind::Eof) {
            let start = self.peek().span.start;
            let type_checkpoint = self.pos;
            let type_errors_len = self.errors.len();
            let ty = if self.at(TokenKind::Bool) {
                let token = self.bump();
                Some(TypeRef {
                    span: token.span,
                    text: self.token_text(&token).to_string(),
                    kind: TypeKind::Path {
                        segments: vec![TypePathSegment {
                            name: self.token_text(&token).to_string(),
                            args: Vec::new(),
                        }],
                    },
                })
            } else {
                self.parse_type()
            };
            let type_end = ty.as_ref().map(|ty| ty.span.end);
            self.pos = type_checkpoint;
            self.errors.truncate(type_errors_len);

            let expr_checkpoint = self.pos;
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
                        | TokenKind::Ident
                        | TokenKind::Underscore
                        | TokenKind::At
                        | TokenKind::LBracket
                        | TokenKind::LParen
                        | TokenKind::LBrace
                        | TokenKind::If
                        | TokenKind::Minus
                        | TokenKind::Bang
                        | TokenKind::Amp
                        | TokenKind::Star
                ) {
                None
            } else {
                self.parse_expr_until_tokens(&[TokenKind::Comma, TokenKind::RBracket])
            };
            let expr_end = expr.as_ref().map(|expr| expr.span.end);
            if expr.is_none() || expr_end.is_some_and(|expr_end| Some(expr_end) < type_end) {
                self.pos = expr_checkpoint;
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
                self.pos = type_checkpoint;
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

    fn parse_primary(&mut self) -> Option<Expr> {
        let token = self.peek().clone();
        match token.kind {
            TokenKind::Integer => Some(self.literal_expr(token, ExprKind::Integer)),
            TokenKind::Float => Some(self.literal_expr(token, ExprKind::Float)),
            TokenKind::String => Some(self.literal_expr(token, ExprKind::String)),
            TokenKind::ByteString => Some(self.literal_expr(token, ExprKind::ByteString)),
            TokenKind::CString => Some(self.literal_expr(token, ExprKind::CString)),
            TokenKind::Char => Some(self.literal_expr(token, ExprKind::Char)),
            TokenKind::ByteChar => Some(self.literal_expr(token, ExprKind::ByteChar)),
            TokenKind::True => {
                self.bump();
                Some(Expr {
                    span: token.span,
                    kind: ExprKind::Bool(true),
                })
            }
            TokenKind::False => {
                self.bump();
                Some(Expr {
                    span: token.span,
                    kind: ExprKind::Bool(false),
                })
            }
            TokenKind::Ident => {
                self.bump();
                Some(Expr {
                    span: token.span,
                    kind: ExprKind::Ident(self.token_text(&token).to_string()),
                })
            }
            TokenKind::Underscore => {
                self.bump();
                Some(Expr {
                    span: token.span,
                    kind: ExprKind::Underscore,
                })
            }
            TokenKind::At => self.parse_builtin_expr(),
            TokenKind::LBracket => self.parse_bracket_array_literal(),
            TokenKind::LParen => {
                self.bump();
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen, "expected `)`")?;
                Some(expr)
            }
            TokenKind::LBrace if self.looks_like_inferred_struct_literal() => {
                self.parse_inferred_struct_literal()
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                Some(Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                })
            }
            TokenKind::If => self.parse_if_expr(),
            _ => {
                self.error_here("expected expression");
                None
            }
        }
    }

    fn parse_if_expr(&mut self) -> Option<Expr> {
        let start = self.expect(TokenKind::If, "expected `if`")?.start;
        let cond = self.parse_expr()?;
        let then_branch = self.parse_block()?;
        let else_branch = if self.eat(TokenKind::Else).is_some() {
            if self.at(TokenKind::If) {
                Some(Box::new(self.parse_if_expr()?))
            } else {
                let block = self.parse_block()?;
                Some(Box::new(Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                }))
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map_or(then_branch.span.end, |expr| expr.span.end);
        Some(Expr {
            span: Span::new(start, end),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch,
                else_branch,
            },
        })
    }

    fn parse_builtin_expr(&mut self) -> Option<Expr> {
        let start = self.expect(TokenKind::At, "expected `@`")?.start;
        let name = self.expect_text(TokenKind::Ident, "expected builtin name")?;
        let type_arg = if self.eat(TokenKind::LBracket).is_some() {
            let ty = self.parse_type()?;
            self.expect(
                TokenKind::RBracket,
                "expected `]` after builtin type argument",
            )?;
            Some(ty)
        } else {
            None
        };
        if self.eat(TokenKind::LBracket).is_some() {
            self.error_here("builtin calls accept at most one type argument");
            self.collect_until(&[TokenKind::RBracket])?;
            self.expect(
                TokenKind::RBracket,
                "expected `]` after builtin type argument",
            )?;
        }
        Some(Expr {
            span: Span::new(start, self.previous_end()),
            kind: ExprKind::Builtin { name, type_arg },
        })
    }

    fn parse_bracket_array_literal(&mut self) -> Option<Expr> {
        let start = self.peek().span.start;
        self.expect(TokenKind::LBracket, "expected `[` before array literal")?;
        let elems = self.parse_array_elements_until_rbracket()?;
        let end = self
            .expect(TokenKind::RBracket, "expected `]` after array literal")?
            .end;
        Some(Expr {
            span: Span::new(start, end),
            kind: ExprKind::ArrayLiteral { elems },
        })
    }

    fn parse_array_elements_until_rbracket(&mut self) -> Option<ArrayElements> {
        let mut elems = Vec::new();
        let elements = if self.at(TokenKind::RBracket) {
            ArrayElements::List(elems)
        } else {
            let first = self.parse_expr()?;
            if self.eat(TokenKind::Semicolon).is_some() {
                let span = self.collect_until(&[TokenKind::RBracket])?;
                ArrayElements::Repeat {
                    value: Box::new(first),
                    count: ExprStub {
                        span,
                        text: self.source_text(span),
                    },
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
        Some(Expr {
            span: Span::new(start, end),
            kind: ExprKind::StructLiteral { fields },
        })
    }

    fn parse_struct_literal_fields(&mut self) -> Option<Vec<FieldInit>> {
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let field_start = self.peek().span.start;
            let name = self.expect_text(TokenKind::Ident, "expected field name")?;
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
            .get(self.pos + 1)
            .is_some_and(|token| token.kind == TokenKind::Ident)
            && self
                .tokens
                .get(self.pos + 2)
                .is_some_and(|token| token.kind == TokenKind::Colon)
    }

    fn literal_expr(&mut self, token: Token, make: impl FnOnce(String) -> ExprKind) -> Expr {
        self.bump();
        Expr {
            span: token.span,
            kind: make(self.token_text(&token).to_string()),
        }
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
    matches!(expr.kind, ExprKind::Block(_) | ExprKind::If { .. })
}
