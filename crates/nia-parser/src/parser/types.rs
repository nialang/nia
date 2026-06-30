// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_ast::AssocBindingKey;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypeParseMode {
    Normal,
    BeforeAggregateLiteral,
}

impl Parser {
    pub(super) fn parse_generic_params(&mut self) -> Vec<nia_ast::GenericParam> {
        let mut generics = Vec::new();
        if self.eat(TokenKind::LBracket).is_none() {
            return generics;
        }
        while !self.at(TokenKind::RBracket) && !self.at(TokenKind::Eof) {
            if let Some(token) = self.eat(TokenKind::Ident) {
                let name = self.token_text(&token).to_string();
                if self.eat(TokenKind::Colon).is_some() {
                    if let Some(ty) = self.parse_type() {
                        generics.push(nia_ast::GenericParam::comptime_param(name, token.span, ty));
                    }
                } else {
                    generics.push(nia_ast::GenericParam::type_param(name, token.span));
                }
            } else {
                self.error_here("expected generic parameter");
                break;
            }
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        self.expect(TokenKind::RBracket, "expected `]` after generic parameters");
        generics
    }

    pub(super) fn parse_type_until(&mut self, stops: &[TokenKind]) -> Option<TypeRef> {
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        if let Some(ty) = self.parse_type()
            && stops.iter().any(|kind| self.at(kind.clone()))
        {
            return Some(ty);
        }
        let bare_fn_type = self.at(TokenKind::Fn);
        self.tokens.rewind(checkpoint);
        self.errors.truncate(errors_len);
        let span = self.collect_until(stops)?;
        if bare_fn_type {
            self.error_at(span, "function pointer types must be written as `&fn(...)`");
        }
        Some(self.error_type_ref(span))
    }

    pub(super) fn parse_type(&mut self) -> Option<TypeRef> {
        self.parse_type_with_mode(TypeParseMode::Normal)
    }

    pub(super) fn parse_type_before_aggregate_literal(&mut self) -> Option<TypeRef> {
        self.parse_type_with_mode(TypeParseMode::BeforeAggregateLiteral)
    }

    fn parse_type_with_mode(&mut self, mode: TypeParseMode) -> Option<TypeRef> {
        self.parse_error_union_type_with_mode(mode)
    }

    fn parse_error_union_type_with_mode(&mut self, mode: TypeParseMode) -> Option<TypeRef> {
        let error = self.parse_optional_type_with_mode(mode)?;
        if self.eat(TokenKind::Bang).is_some() {
            let value = self.parse_type_with_mode(mode)?;
            let span = Span::new(error.span.start, value.span.end);
            return Some(self.make_type_ref(
                span,
                TypeKind::ErrorUnion {
                    error: Box::new(error),
                    value: Box::new(value),
                },
            ));
        }
        Some(error)
    }

    fn parse_optional_type_with_mode(&mut self, mode: TypeParseMode) -> Option<TypeRef> {
        let start = self.peek().span.start;
        if self.eat(TokenKind::Question).is_some() {
            let elem = self.parse_optional_type_with_mode(mode)?;
            let span = Span::new(start, elem.span.end);
            return Some(self.make_type_ref(
                span,
                TypeKind::Optional {
                    elem: Box::new(elem),
                },
            ));
        }
        self.parse_range_type_with_mode(mode)
    }

    fn parse_range_type_with_mode(&mut self, mode: TypeParseMode) -> Option<TypeRef> {
        let start = self.peek().span.start;
        if self.eat(TokenKind::DotDot).is_some() {
            let end = if self.type_can_start() {
                Some(Box::new(self.parse_type_with_mode(mode)?))
            } else {
                None
            };
            let span = Span::new(
                start,
                end.as_ref().map_or(self.previous_end(), |end| end.span.end),
            );
            return Some(self.make_type_ref(
                span,
                TypeKind::Range {
                    start: None,
                    end,
                    inclusive: false,
                },
            ));
        }
        if self.eat(TokenKind::DotDotEq).is_some() {
            let end = self.parse_type_with_mode(mode)?;
            let span = Span::new(start, end.span.end);
            return Some(self.make_type_ref(
                span,
                TypeKind::Range {
                    start: None,
                    end: Some(Box::new(end)),
                    inclusive: true,
                },
            ));
        }

        let kind = if self.eat(TokenKind::Amp).is_some() {
            self.parse_type_after_amp_with_mode(start, mode)?
        } else if self.eat(TokenKind::Caret).is_some() {
            self.parse_volatile_pointer_type_after_caret_with_mode(start, mode)?
        } else if self.eat(TokenKind::LBracket).is_some() {
            if let Some(kind) = self.parse_projection_type_after_open() {
                kind
            } else if let Some(kind) = self.try_parse_slice_pointee_after_open(mode) {
                kind
            } else {
                let len = if self.eat(TokenKind::Underscore).is_some() {
                    self.expect(TokenKind::RBracket, "expected `]` in array type")?;
                    ArrayLen::Infer
                } else {
                    let len = ArrayLen::Expr(Box::new(self.parse_expr()?));
                    self.expect(TokenKind::RBracket, "expected `]` in array type")?;
                    len
                };
                let elem = self.parse_type_with_mode(mode)?;
                TypeKind::Array {
                    len,
                    elem: Box::new(elem),
                }
            }
        } else if self.eat(TokenKind::LParen).is_some() {
            let ty = self.parse_type_with_mode(mode)?;
            self.expect(TokenKind::RParen, "expected `)` after parenthesized type")?;
            return Some(self.make_type_ref(Span::new(start, self.previous_end()), ty.kind));
        } else if self.eat(TokenKind::Underscore).is_some() {
            TypeKind::Infer
        } else if self.at(TokenKind::Fn) {
            self.error_here("function pointer types must be written as `&fn(...)`");
            return None;
        } else if self.eat(TokenKind::SelfType).is_some() {
            TypeKind::SelfType
        } else if self.eat(TokenKind::Void).is_some() {
            TypeKind::Void
        } else if self.eat(TokenKind::Never).is_some() {
            TypeKind::Never
        } else if self.at(TokenKind::Ident) || self.at(TokenKind::Bool) {
            TypeKind::Path {
                segments: self.parse_type_path_segments_with_mode(mode)?,
            }
        } else {
            self.error_here("expected type");
            return None;
        };
        let start_bound_end = self.previous_end();
        let start_bound = self.make_type_ref(Span::new(start, start_bound_end), kind);
        if self.eat(TokenKind::DotDot).is_some() {
            let end = if self.type_can_start() {
                Some(Box::new(self.parse_type_with_mode(mode)?))
            } else {
                None
            };
            let span = Span::new(
                start,
                end.as_ref().map_or(self.previous_end(), |end| end.span.end),
            );
            return Some(self.make_type_ref(
                span,
                TypeKind::Range {
                    start: Some(Box::new(start_bound)),
                    end,
                    inclusive: false,
                },
            ));
        }
        if self.eat(TokenKind::DotDotEq).is_some() {
            let end = self.parse_type_with_mode(mode)?;
            let span = Span::new(start, end.span.end);
            return Some(self.make_type_ref(
                span,
                TypeKind::Range {
                    start: Some(Box::new(start_bound)),
                    end: Some(Box::new(end)),
                    inclusive: true,
                },
            ));
        }
        Some(start_bound)
    }

    fn parse_projection_type_after_open(&mut self) -> Option<TypeKind> {
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        let Some(ty) = self.parse_type() else {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        };
        if self.eat(TokenKind::As).is_none() {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        }
        let trait_ref = match self.parse_type() {
            Some(trait_ref) => trait_ref,
            None => {
                self.tokens.rewind(checkpoint);
                self.errors.truncate(errors_len);
                return None;
            }
        };
        if self
            .expect(TokenKind::RBracket, "expected `]` after projection trait")
            .is_none()
            || self
                .expect(TokenKind::ColonColon, "expected `::` after projection type")
                .is_none()
        {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        }
        let Some(name) = self.expect_text(TokenKind::Ident, "expected associated type name") else {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        };
        Some(TypeKind::Projection {
            ty: Box::new(ty),
            trait_ref: Box::new(trait_ref),
            name,
        })
    }

    fn try_parse_slice_pointee_after_open(&mut self, mode: TypeParseMode) -> Option<TypeKind> {
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        if self.at(TokenKind::RBracket) || self.at(TokenKind::Underscore) {
            return None;
        }
        let Some(elem) = self.parse_type_with_mode(mode) else {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        };
        if self.eat(TokenKind::RBracket).is_none() || self.type_can_start() {
            self.tokens.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return None;
        }
        Some(TypeKind::SlicePointee {
            elem: Box::new(elem),
        })
    }

    fn parse_type_after_amp_with_mode(
        &mut self,
        _start: usize,
        mode: TypeParseMode,
    ) -> Option<TypeKind> {
        let is_readonly = self.eat(TokenKind::Mut).is_none();
        if self.at(TokenKind::Fn) {
            if !is_readonly {
                self.error_here("function pointer types must be written as `&fn(...)`");
            }
            self.bump();
            self.expect(TokenKind::LParen, "expected `(` in function pointer type")?;
            let mut params = Vec::new();
            let mut is_variadic = false;
            while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                if self.eat(TokenKind::Ellipsis).is_some() {
                    is_variadic = true;
                    break;
                }
                params.push(self.parse_type_with_mode(mode)?);
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "expected `)` in function pointer type")?;
            let return_type = if self.type_can_start() {
                Some(Box::new(self.parse_type_with_mode(mode)?))
            } else {
                None
            };
            Some(TypeKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            })
        } else {
            if self.eat(TokenKind::LBracket).is_some() {
                let elem = self.parse_type_with_mode(mode)?;
                self.expect(TokenKind::RBracket, "expected `]` in slice type")?;
                Some(TypeKind::Slice {
                    is_readonly,
                    elem: Box::new(elem),
                })
            } else {
                let elem = self.parse_type_with_mode(mode)?;
                Some(TypeKind::Pointer {
                    is_readonly,
                    elem: Box::new(elem),
                })
            }
        }
    }

    fn parse_volatile_pointer_type_after_caret_with_mode(
        &mut self,
        _start: usize,
        mode: TypeParseMode,
    ) -> Option<TypeKind> {
        let is_readonly = self.eat(TokenKind::Mut).is_none();
        let elem = self.parse_type_with_mode(mode)?;
        Some(TypeKind::VolatilePointer {
            is_readonly,
            elem: Box::new(elem),
        })
    }

    fn parse_type_path_segments_with_mode(
        &mut self,
        mode: TypeParseMode,
    ) -> Option<Vec<TypePathSegment>> {
        let mut segments = Vec::new();
        loop {
            let name = match self.peek().kind {
                TokenKind::Ident | TokenKind::Bool | TokenKind::Pkg => {
                    let token = self.bump();
                    self.token_text(&token).to_string()
                }
                _ => {
                    self.error_here("expected type path segment");
                    return None;
                }
            };
            let args_checkpoint = self.tokens.checkpoint();
            let args_errors_len = self.errors.len();
            let args = self.parse_type_args();
            if mode == TypeParseMode::BeforeAggregateLiteral
                && !args.is_empty()
                && !self.at(TokenKind::LBrace)
                && !self.at(TokenKind::LBracket)
            {
                self.tokens.rewind(args_checkpoint);
                self.errors.truncate(args_errors_len);
                segments.push(TypePathSegment {
                    name,
                    args: Vec::new(),
                });
            } else {
                segments.push(TypePathSegment { name, args });
            }
            if self.eat(TokenKind::ColonColon).is_none() {
                break;
            }
        }
        Some(segments)
    }

    pub(super) fn parse_type_args(&mut self) -> Vec<TypeArg> {
        if self.peek().span.start != self.previous_end() {
            return Vec::new();
        }
        if self.eat(TokenKind::LBracket).is_none() {
            return Vec::new();
        }
        self.parse_type_args_after_open()
    }

    pub(super) fn parse_type_args_after_open(&mut self) -> Vec<TypeArg> {
        let mut args = Vec::new();
        while !self.at(TokenKind::RBracket) && !self.at(TokenKind::Eof) {
            let checkpoint = self.tokens.checkpoint();
            let errors_len = self.errors.len();
            if self.at(TokenKind::DotDot) || self.at(TokenKind::DotDotEq) {
                if let Some(ty) = self.parse_type() {
                    args.push(TypeArg::Type(ty));
                } else {
                    self.tokens.rewind(checkpoint);
                    self.errors.truncate(errors_len);
                    if let Some(span) = self.collect_until(&[TokenKind::Comma, TokenKind::RBracket])
                    {
                        args.push(TypeArg::Const(ExprStub {
                            span,
                            text: self.source_text(span),
                        }));
                    }
                }
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
                continue;
            }
            if self.type_can_start() {
                let checkpoint_before_key = self.tokens.checkpoint();
                let errors_before_key = self.errors.len();
                if let Some(key_ty) = self.parse_type()
                    && self.eat(TokenKind::Eq).is_some()
                {
                    let key_start = key_ty.span.start;
                    let Some(ty) = self.parse_type() else {
                        self.error_here("expected associated type binding value");
                        break;
                    };
                    let key = match &key_ty.kind {
                        TypeKind::Path { segments } if segments.len() == 1 => {
                            AssocBindingKey::Name(segments[0].name.clone())
                        }
                        TypeKind::Projection { .. } => AssocBindingKey::Projection(key_ty),
                        _ => {
                            self.error_at(
                                key_ty.span,
                                "associated type binding key must be a name or projection",
                            );
                            AssocBindingKey::Projection(key_ty)
                        }
                    };
                    args.push(TypeArg::AssocBinding {
                        key,
                        span: Span::new(key_start, ty.span.end),
                        ty,
                    });
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                    continue;
                }
                self.tokens.rewind(checkpoint_before_key);
                self.errors.truncate(errors_before_key);
            }
            if let Some(ty) = self.parse_type() {
                args.push(TypeArg::Type(ty));
            } else {
                self.tokens.rewind(checkpoint);
                self.errors.truncate(errors_len);
                if let Some(span) = self.collect_until(&[TokenKind::Comma, TokenKind::RBracket]) {
                    args.push(TypeArg::Const(ExprStub {
                        span,
                        text: self.source_text(span),
                    }));
                }
            }
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        self.expect(TokenKind::RBracket, "expected `]` after type arguments");
        args
    }

    pub(super) fn type_can_start(&self) -> bool {
        self.token_can_start_type(&self.peek().kind)
    }

    pub(super) fn token_can_start_type(&self, kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Amp
                | TokenKind::LBracket
                | TokenKind::Ident
                | TokenKind::Pkg
                | TokenKind::DotDot
                | TokenKind::DotDotEq
                | TokenKind::LParen
                | TokenKind::Bool
                | TokenKind::SelfType
                | TokenKind::Void
                | TokenKind::Never
                | TokenKind::Question
                | TokenKind::Caret
                | TokenKind::Underscore
        )
    }

    fn error_type_ref(&mut self, span: Span) -> TypeRef {
        self.make_type_ref(span, TypeKind::Error)
    }

    pub(super) fn parse_where_clause(&mut self) -> WhereClause {
        let mut predicates = Vec::new();
        if self.eat(TokenKind::Where).is_none() {
            return WhereClause::default();
        }
        while !self.at(TokenKind::LBrace)
            && !self.at(TokenKind::Semicolon)
            && !self.at(TokenKind::Eof)
        {
            let start = self.peek().span.start;
            let Some(ty) = self.parse_type_until(&[TokenKind::Colon]) else {
                break;
            };
            if self
                .expect(TokenKind::Colon, "expected `:` in where predicate")
                .is_none()
            {
                break;
            }
            let mut bounds = Vec::new();
            while let Some(bound) = self.parse_type_until(&[
                TokenKind::Comma,
                TokenKind::Plus,
                TokenKind::LBrace,
                TokenKind::Semicolon,
            ]) {
                let end = bound.span.end;
                bounds.push(bound);
                if self.eat(TokenKind::Plus).is_some() {
                    continue;
                }
                predicates.push(WherePredicate {
                    ty,
                    bounds,
                    span: Span::new(start, end),
                });
                break;
            }
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        WhereClause { predicates }
    }
}
