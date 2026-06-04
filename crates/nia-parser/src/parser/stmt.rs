// SPDX-License-Identifier: GPL-3.0-or-later
use super::expr::expr_can_terminate_statement_without_semicolon;
use super::*;

impl Parser {
    pub(super) fn parse_stmt(&mut self) -> Option<Stmt> {
        let start = self.peek().span.start;
        if self.at(TokenKind::Pub) {
            let pub_span = self.peek().span;
            self.error_at(
                pub_span,
                "`pub` is not allowed on statements; only top-level `pub using` is permitted",
            );
            self.bump();
        }
        if self.at(TokenKind::Using) {
            self.bump();
            let using = self.parse_using_after_keyword()?;
            self.expect(TokenKind::Semicolon, "expected `;` after using")?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                StmtKind::Using(using),
            ));
        }
        if (self.at(TokenKind::Comptime) && !self.at_comptime_if())
            || self.at(TokenKind::Var)
            || self.at(TokenKind::Let)
        {
            let binding = self.parse_binding_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                StmtKind::Binding(binding),
            ));
        }
        if self.eat(TokenKind::Return).is_some() {
            let value = if self.at(TokenKind::Semicolon) {
                None
            } else {
                Some(self.parse_expr()?)
            };
            self.expect(TokenKind::Semicolon, "expected `;` after return")?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                StmtKind::Return(value),
            ));
        }
        if self.eat(TokenKind::Break).is_some() {
            self.expect(TokenKind::Semicolon, "expected `;` after break")?;
            return Some(self.make_stmt(Span::new(start, self.previous_end()), StmtKind::Break));
        }
        if self.eat(TokenKind::Continue).is_some() {
            self.expect(TokenKind::Semicolon, "expected `;` after continue")?;
            return Some(self.make_stmt(Span::new(start, self.previous_end()), StmtKind::Continue));
        }
        if self.eat(TokenKind::Defer).is_some() {
            let expr = self.parse_expr_until_tokens(&[TokenKind::Semicolon, TokenKind::RBrace])?;
            self.expect(TokenKind::Semicolon, "expected `;` after defer")?;
            return Some(
                self.make_stmt(Span::new(start, self.previous_end()), StmtKind::Defer(expr)),
            );
        }
        if self.at(TokenKind::For) {
            let for_stmt = self.parse_for_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, for_stmt.body.span.end),
                StmtKind::ForIn(Box::new(for_stmt)),
            ));
        }
        if self.at(TokenKind::While) {
            let while_stmt = self.parse_while_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, while_stmt.body.span.end),
                StmtKind::While(Box::new(while_stmt)),
            ));
        }
        if self.at(TokenKind::Loop) {
            let loop_stmt = self.parse_loop_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, loop_stmt.body.span.end),
                StmtKind::Loop(Box::new(loop_stmt)),
            ));
        }
        None
    }

    fn parse_binding_stmt(&mut self) -> Option<BindingStmt> {
        let is_comptime = self.eat(TokenKind::Comptime).is_some();
        let is_let = if self.eat(TokenKind::Let).is_some() {
            true
        } else if self.eat(TokenKind::Var).is_some() {
            false
        } else {
            self.error_here("expected `let` or `var` binding");
            return None;
        };
        let name = self.expect_text(TokenKind::Ident, "expected binding name")?;
        let ty = if self.eat(TokenKind::Colon).is_some() {
            Some(self.parse_type_until(&[TokenKind::Eq, TokenKind::Semicolon])?)
        } else {
            None
        };
        let value = if self.eat(TokenKind::Eq).is_some() {
            Some(self.parse_expr_until_tokens(&[TokenKind::Semicolon])?)
        } else {
            None
        };
        if is_comptime && value.is_none() {
            self.error_here("comptime binding requires an initializer");
            return None;
        }
        let anchor = value
            .as_ref()
            .map(|value| value.span)
            .or_else(|| ty.as_ref().map(|ty| ty.span))
            .unwrap_or_else(|| Span::new(self.previous_end(), self.previous_end()));
        self.expect_semicolon_after(anchor, "expected `;` after binding")?;
        Some(BindingStmt {
            name,
            ty,
            value,
            is_let,
            is_comptime,
        })
    }

    fn parse_for_stmt(&mut self) -> Option<ForInStmt> {
        self.expect(TokenKind::For, "expected `for`")?;
        let binding_start = self.peek().span.start;
        let is_let = self.eat(TokenKind::Let).is_some();
        if self.eat(TokenKind::Var).is_some() {
            self.error_here("`var` is not allowed in for-in bindings; write `for name in iter`");
        }
        let name = self.expect_text(TokenKind::Ident, "expected for binding name")?;
        let ty = if self.eat(TokenKind::Colon).is_some() {
            Some(self.parse_type_until(&[TokenKind::In])?)
        } else {
            None
        };
        let binding_end = ty
            .as_ref()
            .map(|ty| ty.span.end)
            .unwrap_or_else(|| self.previous_end());
        self.expect(TokenKind::In, "expected `in` after for binding")?;
        let iter = self.parse_expr_until(&[TokenKind::LBrace])?;
        let body = self.parse_block()?;
        Some(ForInStmt {
            binding: ForBinding {
                span: Span::new(binding_start, binding_end),
                name,
                ty,
                is_let,
            },
            iter,
            body,
        })
    }

    fn parse_while_stmt(&mut self) -> Option<WhileStmt> {
        self.expect(TokenKind::While, "expected `while`")?;
        let cond = self.parse_expr_until(&[TokenKind::LBrace])?;
        let body = self.parse_block()?;
        Some(WhileStmt { cond, body })
    }

    fn parse_loop_stmt(&mut self) -> Option<LoopStmt> {
        self.expect(TokenKind::Loop, "expected `loop`")?;
        let body = self.parse_block()?;
        Some(LoopStmt { body })
    }

    pub(super) fn parse_switch_expr(&mut self) -> Option<Expr> {
        let start = self.peek().span.start;
        let switch = self.parse_switch()?;
        let end = self.previous_end();
        Some(self.make_expr(Span::new(start, end), ExprKind::Switch(Box::new(switch))))
    }

    fn parse_switch(&mut self) -> Option<SwitchStmt> {
        self.expect(TokenKind::Switch, "expected `switch`")?;
        let target = self.parse_expr_until_tokens(&[TokenKind::LBrace])?;
        self.expect(TokenKind::LBrace, "expected `{` after switch target")?;
        let mut arms = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let start = self.peek().span.start;
            let patterns = self.parse_switch_arm_patterns()?;
            self.expect(TokenKind::FatArrow, "expected `=>` in switch arm")?;
            let body = self.parse_switch_arm_body()?;
            self.eat(TokenKind::Comma);
            let end = body.span().end;
            arms.push(SwitchArm {
                span: Span::new(start, end),
                patterns,
                body,
            });
        }
        self.expect(TokenKind::RBrace, "expected `}` after switch")?;
        Some(SwitchStmt { target, arms })
    }

    fn parse_switch_arm_patterns(&mut self) -> Option<Vec<SwitchPattern>> {
        let mut patterns = Vec::new();
        loop {
            patterns.push(self.parse_switch_arm_pattern()?);
            if self.at(TokenKind::FatArrow) {
                break;
            }
            self.expect(
                TokenKind::Comma,
                "expected `,` or `=>` after switch pattern",
            )?;
            if self.at(TokenKind::FatArrow) {
                self.error_here("trailing comma is not allowed in switch pattern list");
                break;
            }
        }
        Some(patterns)
    }

    fn parse_switch_arm_pattern(&mut self) -> Option<SwitchPattern> {
        if self.eat(TokenKind::Underscore).is_some() {
            return Some(SwitchPattern::Default);
        }
        if self.at(TokenKind::Question) && matches!(self.tokens.nth_kind(1), Some(TokenKind::Ident))
        {
            let start = self
                .expect(
                    TokenKind::Question,
                    "expected `?` in optional switch pattern",
                )?
                .start;
            let name = self.expect_text(TokenKind::Ident, "expected optional payload name")?;
            let span = Span::new(start, self.previous_end());
            return Some(SwitchPattern::OptionalSome { name, span });
        }
        if self.at(TokenKind::Null) {
            let span = self.expect(
                TokenKind::Null,
                "expected `null` in optional switch pattern",
            )?;
            return Some(SwitchPattern::OptionalNull { span });
        }
        if self.at(TokenKind::Bang) && matches!(self.tokens.nth_kind(1), Some(TokenKind::Ident)) {
            let start = self
                .expect(
                    TokenKind::Bang,
                    "expected `!` in error success switch pattern",
                )?
                .start;
            let name = self.expect_text(TokenKind::Ident, "expected error-union success name")?;
            let span = Span::new(start, self.previous_end());
            return Some(SwitchPattern::ErrorOk { name, span });
        }
        if self.at(TokenKind::Ident) && matches!(self.tokens.nth_kind(1), Some(TokenKind::Bang)) {
            let start = self.peek().span.start;
            let name = self.expect_text(TokenKind::Ident, "expected error name")?;
            self.expect(TokenKind::Bang, "expected `!` after error name")?;
            let span = Span::new(start, self.previous_end());
            return Some(SwitchPattern::ErrorErr { name, span });
        }
        let expr = self.parse_expr_until_tokens(&[TokenKind::Comma, TokenKind::FatArrow])?;
        let ExprKind::Range(range) = expr.kind else {
            return Some(SwitchPattern::Expr(expr));
        };
        match (&range.start, &range.end) {
            (Some(start), Some(end)) => Some(SwitchPattern::Range {
                start: (**start).clone(),
                end: (**end).clone(),
                inclusive: range.inclusive,
                span: expr.span,
            }),
            _ => {
                self.error_at(
                    expr.span,
                    "open-ended switch range patterns are not supported; use `_` for the default arm",
                );
                Some(SwitchPattern::Expr(
                    self.make_expr(expr.span, ExprKind::Range(range)),
                ))
            }
        }
    }

    fn parse_switch_arm_body(&mut self) -> Option<SwitchArmBody> {
        if self.at(TokenKind::LBrace) {
            return self
                .parse_block()
                .map(|block| SwitchArmBody::Block(Box::new(block)));
        }
        if self.starts_stmt() {
            return self
                .parse_switch_arm_stmt()
                .map(|stmt| SwitchArmBody::Stmt(Box::new(stmt)));
        }
        self.parse_expr().map(SwitchArmBody::Expr)
    }

    fn parse_switch_arm_stmt(&mut self) -> Option<Stmt> {
        let start = self.peek().span.start;
        if self.eat(TokenKind::Return).is_some() {
            let value = if self.at(TokenKind::Comma) || self.at(TokenKind::RBrace) {
                None
            } else {
                Some(self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?)
            };
            return Some(
                self.make_stmt(
                    Span::new(
                        start,
                        value
                            .as_ref()
                            .map_or(self.previous_end(), |expr| expr.span.end),
                    ),
                    StmtKind::Return(value),
                ),
            );
        }
        if self.eat(TokenKind::Break).is_some() {
            return Some(self.make_stmt(Span::new(start, self.previous_end()), StmtKind::Break));
        }
        if self.eat(TokenKind::Continue).is_some() {
            return Some(self.make_stmt(Span::new(start, self.previous_end()), StmtKind::Continue));
        }
        if self.eat(TokenKind::Defer).is_some() {
            let expr = self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?;
            return Some(self.make_stmt(Span::new(start, expr.span.end), StmtKind::Defer(expr)));
        }
        self.parse_stmt()
    }

    pub(super) fn starts_stmt(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Let
                | TokenKind::Var
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
                | TokenKind::For
                | TokenKind::While
                | TokenKind::Loop
                | TokenKind::Using
        ) || (self.at(TokenKind::Comptime) && !self.at_comptime_if())
    }

    pub(super) fn parse_block(&mut self) -> Option<Block> {
        let start = self.expect(TokenKind::LBrace, "expected `{`")?.start;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.starts_stmt() {
                if let Some(stmt) = self.parse_stmt() {
                    stmts.push(stmt);
                } else {
                    self.recover_to_stmt_boundary();
                }
                continue;
            }

            let expr = self.parse_expr()?;
            let has_semicolon = self.eat(TokenKind::Semicolon).is_some();
            if has_semicolon || !self.at(TokenKind::RBrace) {
                if !has_semicolon && !expr_can_terminate_statement_without_semicolon(&expr) {
                    self.error_at_end(expr.span, "expected `;` after expression");
                }
                let span = expr.span;
                stmts.push(self.make_stmt(span, StmtKind::Expr(expr)));
            } else {
                tail = Some(Box::new(expr));
                break;
            }
        }
        let end = self
            .expect(TokenKind::RBrace, "expected `}` after block")?
            .end;
        Some(Block {
            span: Span::new(start, end),
            stmts,
            tail,
        })
    }
}
