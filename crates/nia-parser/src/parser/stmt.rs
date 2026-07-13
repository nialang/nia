// SPDX-License-Identifier: GPL-3.0-or-later
use super::expr::expr_can_terminate_statement_without_semicolon;
use super::*;

impl Parser {
    pub(super) fn parse_stmt(&mut self) -> Option<Stmt> {
        let attributes = self.parse_attributes()?;
        let start = attributes
            .first()
            .map_or_else(|| self.peek().span.start, |attr| attr.span.start);
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
                attributes,
                StmtKind::Using(using),
            ));
        }
        if self.at(TokenKind::Static) {
            let binding = self.parse_binding(false)?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Static(Box::new(binding)),
            ));
        }
        if self.at(TokenKind::Comptime) || self.at(TokenKind::Let) {
            let binding = self.parse_binding_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Binding(Box::new(binding)),
            ));
        }
        if self.eat(TokenKind::Return).is_some() {
            let value = if self.at(TokenKind::Semicolon) {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            };
            self.expect(TokenKind::Semicolon, "expected `;` after return")?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Return(value),
            ));
        }
        if self.eat(TokenKind::Break).is_some() {
            self.expect(TokenKind::Semicolon, "expected `;` after break")?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Break,
            ));
        }
        if self.eat(TokenKind::Continue).is_some() {
            self.expect(TokenKind::Semicolon, "expected `;` after continue")?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Continue,
            ));
        }
        if self.eat(TokenKind::Defer).is_some() {
            let expr = self.parse_expr_until_tokens(&[TokenKind::Semicolon, TokenKind::RBrace])?;
            self.expect(TokenKind::Semicolon, "expected `;` after defer")?;
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Defer(Box::new(expr)),
            ));
        }
        if self.at(TokenKind::For) {
            let for_stmt = self.parse_for_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, for_stmt.body.span.end),
                attributes,
                StmtKind::ForIn(Box::new(for_stmt)),
            ));
        }
        if self.at(TokenKind::While) {
            let while_stmt = self.parse_while_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, while_stmt.body.span.end),
                attributes,
                StmtKind::While(Box::new(while_stmt)),
            ));
        }
        if self.at(TokenKind::Loop) {
            let loop_stmt = self.parse_loop_stmt()?;
            return Some(self.make_stmt(
                Span::new(start, loop_stmt.body.span.end),
                attributes,
                StmtKind::Loop(Box::new(loop_stmt)),
            ));
        }
        if attributes.is_empty() {
            return None;
        }
        let expr = self.parse_expr()?;
        let has_semicolon = self.eat(TokenKind::Semicolon).is_some();
        if !has_semicolon && !expr_can_terminate_statement_without_semicolon(&expr) {
            self.error_at_end(expr.span, "expected `;` after expression");
        }
        Some(self.make_stmt(
            Span::new(start, self.previous_end()),
            attributes,
            StmtKind::Expr(Box::new(expr)),
        ))
    }

    fn parse_binding_stmt(&mut self) -> Option<BindingStmt> {
        let is_comptime = self.eat(TokenKind::Comptime).is_some();
        let mut is_mutable = if is_comptime {
            if self.eat(TokenKind::Let).is_none()
                && !self.at(TokenKind::Mut)
                && !self.starts_binding_pattern()
            {
                self.error_here("expected `let` binding");
                return None;
            }
            false
        } else if self.eat(TokenKind::Let).is_some() {
            false
        } else {
            self.error_here("expected `let` binding");
            return None;
        };
        if self.eat(TokenKind::Mut).is_some() {
            is_mutable = true;
        }
        let pattern = self.parse_irrefutable_pattern_until(&[
            TokenKind::Colon,
            TokenKind::Eq,
            TokenKind::Semicolon,
        ])?;
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
            pattern: self.apply_pattern_binding_mutability(pattern, is_mutable),
            ty,
            value,
            is_mutable,
            is_comptime,
        })
    }

    fn parse_for_stmt(&mut self) -> Option<ForInStmt> {
        self.expect(TokenKind::For, "expected `for`")?;
        let pattern = self.parse_irrefutable_pattern_until(&[TokenKind::In, TokenKind::Colon])?;
        if self.at(TokenKind::Colon) {
            self.error_here("for patterns do not support type annotations");
            self.collect_until(&[TokenKind::LBrace])?;
            let body = self.parse_block()?;
            let pattern_span = pattern.span;
            return Some(ForInStmt {
                pattern,
                iter: self.make_expr(pattern_span, ExprKind::Error),
                body,
            });
        }
        self.expect(TokenKind::In, "expected `in` after for pattern")?;
        let iter = self.parse_expr_until(&[TokenKind::LBrace])?;
        let body = self.parse_block()?;
        Some(ForInStmt {
            pattern,
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
            patterns.push(
                self.parse_switch_pattern_until_tokens(&[TokenKind::Comma, TokenKind::FatArrow])?,
            );
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

    fn parse_switch_pattern_until_tokens(&mut self, stops: &[TokenKind]) -> Option<SwitchPattern> {
        if self.at(TokenKind::Underscore) {
            let span = self.expect(TokenKind::Underscore, "expected `_` in switch pattern")?;
            return Some(SwitchPattern {
                span,
                kind: SwitchPatternKind::Wildcard,
            });
        }

        let expr = self.parse_expr_until_tokens(stops)?;
        let ExprKind::Range(range) = expr.kind else {
            return Some(SwitchPattern {
                span: expr.span,
                kind: SwitchPatternKind::Expr(Box::new(expr)),
            });
        };
        match (&range.start, &range.end) {
            (Some(start), Some(end)) => Some(SwitchPattern {
                span: expr.span,
                kind: SwitchPatternKind::Range {
                    start: Box::new((**start).clone()),
                    end: Box::new((**end).clone()),
                    inclusive: range.inclusive,
                },
            }),
            _ => {
                self.error_at(
                    expr.span,
                    "open-ended switch range patterns are not supported; use `_` for the default arm",
                );
                Some(SwitchPattern {
                    span: expr.span,
                    kind: SwitchPatternKind::Expr(Box::new(
                        self.make_expr(expr.span, ExprKind::Range(range)),
                    )),
                })
            }
        }
    }

    pub(super) fn parse_binding_pattern_until_tokens(
        &mut self,
        stops: &[TokenKind],
    ) -> Option<Pattern> {
        self.parse_pattern_until(stops)
    }

    fn starts_binding_pattern(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Ident | TokenKind::Underscore | TokenKind::Amp | TokenKind::Mut
        )
    }

    fn parse_irrefutable_pattern_until(&mut self, stops: &[TokenKind]) -> Option<Pattern> {
        let pattern = self.parse_irrefutable_pattern_atom_until(stops)?;
        if self.eat(TokenKind::Bang).is_some() {
            self.error_here("binding patterns do not support error payload suffix `!`");
        }
        Some(pattern)
    }

    fn parse_irrefutable_pattern_atom_until(&mut self, stops: &[TokenKind]) -> Option<Pattern> {
        let start = self.peek().span.start;
        if self.eat(TokenKind::Amp).is_some() {
            if self.eat(TokenKind::Mut).is_some() {
                let inner = self.parse_irrefutable_pattern_atom_until(stops)?;
                return Some(Pattern {
                    span: Span::new(start, inner.span.end),
                    kind: PatternKind::MutPointer(Box::new(inner)),
                });
            }
            let inner = self.parse_irrefutable_pattern_atom_until(stops)?;
            return Some(Pattern {
                span: Span::new(start, inner.span.end),
                kind: PatternKind::Pointer(Box::new(inner)),
            });
        }
        if self.eat(TokenKind::Mut).is_some() {
            let mut inner = self.parse_irrefutable_pattern_atom_until(stops)?;
            Self::mark_pattern_bindings_mutable(&mut inner);
            return Some(Pattern {
                span: Span::new(start, inner.span.end),
                kind: inner.kind,
            });
        }
        if self.at(TokenKind::Underscore) {
            let span = self.expect(TokenKind::Underscore, "expected `_` in pattern")?;
            return Some(Pattern {
                span,
                kind: PatternKind::Wildcard,
            });
        }
        if self.at(TokenKind::Ident)
            && self
                .tokens
                .nth_kind(1)
                .is_some_and(|kind| stops.contains(kind))
        {
            let span = self.peek().span;
            let name = self.expect_name(TokenKind::Ident, "expected pattern binding")?;
            return Some(Pattern {
                span,
                kind: PatternKind::Bind {
                    name,
                    node_key: self.node_key(NodeSyntaxKind::Pattern, span),
                    is_mutable: false,
                },
            });
        }
        self.error_here("expected binding pattern");
        None
    }

    fn apply_pattern_binding_mutability(
        &mut self,
        mut pattern: Pattern,
        is_mutable: bool,
    ) -> Pattern {
        if is_mutable {
            Self::mark_pattern_bindings_mutable(&mut pattern);
        }
        pattern
    }

    fn mark_pattern_bindings_mutable(pattern: &mut Pattern) {
        match &mut pattern.kind {
            PatternKind::Bind { is_mutable, .. } => *is_mutable = true,
            PatternKind::Pointer(inner)
            | PatternKind::MutPointer(inner)
            | PatternKind::OptionalSome(inner)
            | PatternKind::ErrorOk(inner)
            | PatternKind::ErrorErr(inner) => Self::mark_pattern_bindings_mutable(inner),
            PatternKind::Wildcard
            | PatternKind::OptionalNull
            | PatternKind::Expr(_)
            | PatternKind::Range { .. } => {}
        }
    }

    fn parse_payload_pattern_until(&mut self, stops: &[TokenKind]) -> Option<Pattern> {
        self.parse_pattern_until(stops)
    }

    fn parse_pattern_until(&mut self, stops: &[TokenKind]) -> Option<Pattern> {
        let mut pattern = self.parse_pattern_atom_until(stops)?;
        while self.eat(TokenKind::Bang).is_some() {
            let span = Span::new(pattern.span.start, self.previous_end());
            pattern = Pattern {
                span,
                kind: PatternKind::ErrorErr(Box::new(pattern)),
            };
        }
        Some(pattern)
    }

    fn parse_pattern_atom_until(&mut self, stops: &[TokenKind]) -> Option<Pattern> {
        let start = self.peek().span.start;
        if self.eat(TokenKind::Amp).is_some() {
            if self.eat(TokenKind::Mut).is_some() {
                let inner = self.parse_pattern_atom_until(stops)?;
                return Some(Pattern {
                    span: Span::new(start, inner.span.end),
                    kind: PatternKind::MutPointer(Box::new(inner)),
                });
            }
            let inner = self.parse_pattern_atom_until(stops)?;
            return Some(Pattern {
                span: Span::new(start, inner.span.end),
                kind: PatternKind::Pointer(Box::new(inner)),
            });
        }
        if self.eat(TokenKind::Mut).is_some() {
            let mut inner = self.parse_pattern_atom_until(stops)?;
            Self::mark_pattern_bindings_mutable(&mut inner);
            return Some(Pattern {
                span: Span::new(start, inner.span.end),
                kind: inner.kind,
            });
        }
        if self.at(TokenKind::Underscore) {
            let span = self.expect(TokenKind::Underscore, "expected `_` in pattern")?;
            return Some(Pattern {
                span,
                kind: PatternKind::Wildcard,
            });
        }
        if self.at(TokenKind::Question) {
            let start = self
                .expect(TokenKind::Question, "expected `?` in optional pattern")?
                .start;
            let pattern = self.parse_payload_pattern_until(stops)?;
            return Some(Pattern {
                span: Span::new(start, pattern.span.end),
                kind: PatternKind::OptionalSome(Box::new(pattern)),
            });
        }
        if self.at(TokenKind::Null) {
            let span = self.expect(TokenKind::Null, "expected `null` in optional pattern")?;
            return Some(Pattern {
                span,
                kind: PatternKind::OptionalNull,
            });
        }
        if self.at(TokenKind::Bang) {
            let start = self
                .expect(
                    TokenKind::Bang,
                    "expected `!` in error-union success pattern",
                )?
                .start;
            let pattern = self.parse_payload_pattern_until(stops)?;
            return Some(Pattern {
                span: Span::new(start, pattern.span.end),
                kind: PatternKind::ErrorOk(Box::new(pattern)),
            });
        }
        if self.at_bare_pattern_binding(stops) {
            let span = self.peek().span;
            let name = self.expect_name(TokenKind::Ident, "expected pattern binding")?;
            return Some(Pattern {
                span,
                kind: PatternKind::Bind {
                    name,
                    node_key: self.node_key(NodeSyntaxKind::Pattern, span),
                    is_mutable: false,
                },
            });
        }
        let mut expr_stops = stops.to_vec();
        if !expr_stops.contains(&TokenKind::Bang) {
            expr_stops.push(TokenKind::Bang);
        }
        let expr = self.parse_expr_until_tokens(&expr_stops)?;
        let ExprKind::Range(range) = expr.kind else {
            return Some(Pattern {
                span: expr.span,
                kind: PatternKind::Expr(Box::new(expr)),
            });
        };
        match (&range.start, &range.end) {
            (Some(start), Some(end)) => Some(Pattern {
                span: expr.span,
                kind: PatternKind::Range {
                    start: Box::new((**start).clone()),
                    end: Box::new((**end).clone()),
                    inclusive: range.inclusive,
                },
            }),
            _ => {
                self.error_at(
                    expr.span,
                    "open-ended switch range patterns are not supported; use `_` for the default arm",
                );
                Some(Pattern {
                    span: expr.span,
                    kind: PatternKind::Expr(Box::new(
                        self.make_expr(expr.span, ExprKind::Range(range)),
                    )),
                })
            }
        }
    }

    fn at_bare_pattern_binding(&self, stops: &[TokenKind]) -> bool {
        self.at(TokenKind::Ident)
            && self
                .tokens
                .nth_kind(1)
                .is_some_and(|kind| *kind == TokenKind::Bang || stops.contains(kind))
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
        self.parse_expr()
            .map(|expr| SwitchArmBody::Expr(Box::new(expr)))
    }

    fn parse_switch_arm_stmt(&mut self) -> Option<Stmt> {
        let attributes = self.parse_attributes()?;
        let start = attributes
            .first()
            .map_or_else(|| self.peek().span.start, |attr| attr.span.start);
        if self.eat(TokenKind::Return).is_some() {
            let value = if self.at(TokenKind::Comma) || self.at(TokenKind::RBrace) {
                None
            } else {
                Some(Box::new(
                    self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?,
                ))
            };
            return Some(
                self.make_stmt(
                    Span::new(
                        start,
                        value
                            .as_ref()
                            .map_or(self.previous_end(), |expr| expr.span.end),
                    ),
                    attributes,
                    StmtKind::Return(value),
                ),
            );
        }
        if self.eat(TokenKind::Break).is_some() {
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Break,
            ));
        }
        if self.eat(TokenKind::Continue).is_some() {
            return Some(self.make_stmt(
                Span::new(start, self.previous_end()),
                attributes,
                StmtKind::Continue,
            ));
        }
        if self.eat(TokenKind::Defer).is_some() {
            let expr = self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?;
            return Some(self.make_stmt(
                Span::new(start, expr.span.end),
                attributes,
                StmtKind::Defer(Box::new(expr)),
            ));
        }
        if !attributes.is_empty() {
            self.error_here("attributes must apply to a statement");
        }
        self.parse_stmt()
    }

    pub(super) fn starts_stmt(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Let
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
                | TokenKind::For
                | TokenKind::While
                | TokenKind::Loop
                | TokenKind::Using
                | TokenKind::Static
        ) || self.at(TokenKind::Comptime)
    }

    pub(super) fn parse_block(&mut self) -> Option<Block> {
        let start = self.expect(TokenKind::LBrace, "expected `{`")?.start;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.starts_stmt()
                || (self.at(TokenKind::At)
                    && matches!(self.tokens.nth_kind(1), Some(TokenKind::LBracket)))
            {
                if let Some(stmt) = self.parse_stmt() {
                    stmts.push(stmt);
                } else {
                    let checkpoint = self.checkpoint();
                    self.recover_to_stmt_boundary_with_progress(checkpoint);
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
                stmts.push(self.make_stmt(span, Vec::new(), StmtKind::Expr(Box::new(expr))));
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
