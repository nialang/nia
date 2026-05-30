// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl Parser {
    pub(super) fn parse_generic_params(&mut self) -> Vec<String> {
        let mut generics = Vec::new();
        if self.eat(TokenKind::LBracket).is_none() {
            return generics;
        }
        while !self.at(TokenKind::RBracket) && !self.at(TokenKind::Eof) {
            if let Some(name) = self.expect_text(TokenKind::Ident, "expected generic parameter") {
                generics.push(name);
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
            self.error_at(
                span,
                "function pointer types must be written as `&const fn(...)`",
            );
        }
        Some(self.error_type_ref(span))
    }

    pub(super) fn parse_type(&mut self) -> Option<TypeRef> {
        let start = self.peek().span.start;
        let kind = if self.eat(TokenKind::Amp).is_some() {
            self.parse_type_after_amp(start)?
        } else if self.eat(TokenKind::LBracket).is_some() {
            let len = if self.eat(TokenKind::Underscore).is_some() {
                ArrayLen::Infer
            } else {
                ArrayLen::Expr(Box::new(self.parse_expr()?))
            };
            self.expect(TokenKind::RBracket, "expected `]` in array type")?;
            let elem = self.parse_type()?;
            TypeKind::Array {
                len,
                elem: Box::new(elem),
            }
        } else if self.eat(TokenKind::Underscore).is_some() {
            TypeKind::Infer
        } else if self.at(TokenKind::Fn) {
            self.error_here("function pointer types must be written as `&const fn(...)`");
            return None;
        } else if self.eat(TokenKind::Void).is_some() {
            TypeKind::Void
        } else if self.eat(TokenKind::Bang).is_some() {
            TypeKind::Never
        } else if self.at(TokenKind::Ident) || self.at(TokenKind::Bool) {
            TypeKind::Path {
                segments: self.parse_type_path_segments()?,
            }
        } else {
            self.error_here("expected type");
            return None;
        };
        let span = Span::new(start, self.previous_end());
        Some(self.make_type_ref(span, kind))
    }

    pub(super) fn parse_type_after_amp(&mut self, _start: usize) -> Option<TypeKind> {
        let is_const = self.eat(TokenKind::Const).is_some();
        if self.at(TokenKind::Fn) {
            if !is_const {
                self.error_here("function pointer types must be written as `&const fn(...)`");
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
                params.push(self.parse_type()?);
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "expected `)` in function pointer type")?;
            let return_type = if self.type_can_start() {
                Some(Box::new(self.parse_type()?))
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
                let elem = self.parse_type()?;
                self.expect(TokenKind::RBracket, "expected `]` in slice type")?;
                Some(TypeKind::Slice {
                    is_const,
                    elem: Box::new(elem),
                })
            } else {
                let elem = self.parse_type()?;
                Some(TypeKind::Pointer {
                    is_const,
                    elem: Box::new(elem),
                })
            }
        }
    }

    fn parse_type_path_segments(&mut self) -> Option<Vec<TypePathSegment>> {
        let mut segments = Vec::new();
        loop {
            let name = match self.peek().kind {
                TokenKind::Ident | TokenKind::Bool => {
                    let token = self.bump();
                    self.token_text(&token).to_string()
                }
                _ => {
                    self.error_here("expected type path segment");
                    return None;
                }
            };
            let args = self.parse_type_args();
            segments.push(TypePathSegment { name, args });
            if self.eat(TokenKind::ColonColon).is_none() {
                break;
            }
        }
        Some(segments)
    }

    pub(super) fn parse_type_args(&mut self) -> Vec<TypeArg> {
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

    fn type_can_start(&self) -> bool {
        self.token_can_start_type(&self.peek().kind)
    }

    pub(super) fn token_can_start_type(&self, kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::Amp
                | TokenKind::LBracket
                | TokenKind::Ident
                | TokenKind::Bool
                | TokenKind::Void
                | TokenKind::Bang
                | TokenKind::Underscore
        )
    }

    fn error_type_ref(&mut self, span: Span) -> TypeRef {
        self.make_type_ref(span, TypeKind::Error)
    }
}
