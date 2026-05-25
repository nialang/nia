// SPDX-License-Identifier: GPL-3.0-or-later
use super::expr::expr_can_terminate_statement_without_semicolon;
use super::*;

impl<'a> Parser<'a> {
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
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Using(using),
            });
        }
        if self.at(TokenKind::Var) || self.at(TokenKind::Const) {
            let binding = self.parse_binding_stmt()?;
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Binding(binding),
            });
        }
        if self.eat(TokenKind::Return).is_some() {
            let value = if self.at(TokenKind::Semicolon) {
                None
            } else {
                Some(self.parse_expr()?)
            };
            self.expect(TokenKind::Semicolon, "expected `;` after return")?;
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Return(value),
            });
        }
        if self.eat(TokenKind::Break).is_some() {
            self.expect(TokenKind::Semicolon, "expected `;` after break")?;
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Break,
            });
        }
        if self.eat(TokenKind::Continue).is_some() {
            self.expect(TokenKind::Semicolon, "expected `;` after continue")?;
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Continue,
            });
        }
        if self.eat(TokenKind::Defer).is_some() {
            let expr = self.parse_expr_until_tokens(&[TokenKind::Semicolon, TokenKind::RBrace])?;
            self.expect(TokenKind::Semicolon, "expected `;` after defer")?;
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Defer(expr),
            });
        }
        if self.at(TokenKind::For) {
            let for_stmt = self.parse_for_stmt()?;
            return Some(Stmt {
                span: Span::new(start, for_stmt.body.span.end),
                kind: StmtKind::For(Box::new(for_stmt)),
            });
        }
        if self.at(TokenKind::Switch) {
            let switch = self.parse_switch_stmt()?;
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Switch(switch),
            });
        }
        None
    }

    fn parse_binding_stmt(&mut self) -> Option<BindingStmt> {
        let is_const = if self.eat(TokenKind::Const).is_some() {
            true
        } else {
            self.eat(TokenKind::Var);
            false
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
            is_const,
        })
    }

    fn parse_for_stmt(&mut self) -> Option<ForStmt> {
        self.expect(TokenKind::For, "expected `for`")?;
        let header = if self.at(TokenKind::LBrace) {
            ForHeader::Infinite
        } else {
            let header_start = self.peek().span.start;
            if self.has_top_level_semicolon_before_lbrace() {
                let init = if self.at(TokenKind::Semicolon) {
                    None
                } else if self.at(TokenKind::Var) || self.at(TokenKind::Const) {
                    let init_start = self.peek().span.start;
                    let binding = self.parse_binding_stmt()?;
                    Some(ForInit::Binding {
                        span: Span::new(init_start, self.previous_end()),
                        binding,
                    })
                } else {
                    let expr = self.parse_expr_until(&[TokenKind::Semicolon])?;
                    self.expect(TokenKind::Semicolon, "expected `;` in for header")?;
                    Some(ForInit::Expr(expr))
                };
                if init.is_none() {
                    self.expect(TokenKind::Semicolon, "expected `;` in for header")?;
                }
                let cond = if self.at(TokenKind::Semicolon) {
                    None
                } else {
                    Some(self.parse_expr_until(&[TokenKind::Semicolon])?)
                };
                self.expect(TokenKind::Semicolon, "expected second `;` in for header")?;
                let step = if self.at(TokenKind::LBrace) {
                    None
                } else {
                    Some(self.parse_expr_until(&[TokenKind::LBrace])?)
                };
                ForHeader::CStyle {
                    init: init.map(Box::new),
                    cond: cond.map(Box::new),
                    step: step.map(Box::new),
                }
            } else {
                let cond = self.parse_expr().or_else(|| {
                    let span = self.collect_until(&[TokenKind::LBrace])?;
                    Some(Expr {
                        span,
                        kind: ExprKind::Raw(self.source_text(span)),
                    })
                })?;
                if cond.span.start < header_start {
                    self.error_here("invalid for condition");
                }
                ForHeader::Condition(cond)
            }
        };
        let body = self.parse_block()?;
        Some(ForStmt { header, body })
    }

    fn parse_switch_stmt(&mut self) -> Option<SwitchStmt> {
        self.expect(TokenKind::Switch, "expected `switch`")?;
        let target = self.parse_expr()?;
        self.expect(TokenKind::LBrace, "expected `{` after switch target")?;
        let mut arms = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let start = self.peek().span.start;
            let pattern = if self.eat(TokenKind::Underscore).is_some() {
                SwitchPattern::Default
            } else {
                SwitchPattern::Expr(self.parse_expr()?)
            };
            self.expect(TokenKind::FatArrow, "expected `=>` in switch arm")?;
            let body = self.parse_switch_arm_body()?;
            self.eat(TokenKind::Comma);
            let end = body.span().end;
            arms.push(SwitchArm {
                span: Span::new(start, end),
                pattern,
                body,
            });
        }
        self.expect(TokenKind::RBrace, "expected `}` after switch")?;
        Some(SwitchStmt { target, arms })
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
            return Some(Stmt {
                span: Span::new(
                    start,
                    value
                        .as_ref()
                        .map_or(self.previous_end(), |expr| expr.span.end),
                ),
                kind: StmtKind::Return(value),
            });
        }
        if self.eat(TokenKind::Break).is_some() {
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Break,
            });
        }
        if self.eat(TokenKind::Continue).is_some() {
            return Some(Stmt {
                span: Span::new(start, self.previous_end()),
                kind: StmtKind::Continue,
            });
        }
        if self.eat(TokenKind::Defer).is_some() {
            let expr = self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?;
            return Some(Stmt {
                span: Span::new(start, expr.span.end),
                kind: StmtKind::Defer(expr),
            });
        }
        self.parse_stmt()
    }

    pub(super) fn starts_stmt(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Const
                | TokenKind::Var
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
                | TokenKind::For
                | TokenKind::Switch
                | TokenKind::Using
        )
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
                stmts.push(Stmt {
                    span,
                    kind: StmtKind::Expr(expr),
                });
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
