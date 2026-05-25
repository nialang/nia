// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_item(&mut self) -> Option<Item> {
        let start = self.peek().span.start;
        let pub_span = self.eat(TokenKind::Pub).map(|token| token.span);
        let vis = if pub_span.is_some() {
            Visibility::Public
        } else {
            Visibility::Private
        };

        let kind = if self.at(TokenKind::Import) {
            if let Some(span) = pub_span {
                self.error_at(span, "`pub` cannot be applied to `import`");
            }
            ItemKind::Import(self.parse_import()?)
        } else if self.at(TokenKind::Using) {
            ItemKind::Using(self.parse_using()?)
        } else if self.at(TokenKind::Extern) {
            self.bump();
            if self.at(TokenKind::Struct) {
                ItemKind::Struct(self.parse_struct(true)?)
            } else if self.at(TokenKind::Fn) {
                ItemKind::Function(self.parse_function(true)?)
            } else if self.at(TokenKind::Const) || self.at(TokenKind::Var) {
                ItemKind::Binding(self.parse_binding(true)?)
            } else {
                self.error_here("expected `struct`, `fn`, `var`, or `const` after `extern`");
                return None;
            }
        } else if self.at(TokenKind::Struct) {
            ItemKind::Struct(self.parse_struct(false)?)
        } else if self.at(TokenKind::Extend) {
            ItemKind::Extend(self.parse_extend()?)
        } else if self.at(TokenKind::Enum) {
            ItemKind::Enum(self.parse_enum()?)
        } else if self.at(TokenKind::Type) {
            ItemKind::TypeAlias(self.parse_type_alias()?)
        } else if self.at(TokenKind::Fn) {
            ItemKind::Function(self.parse_function(false)?)
        } else if self.at(TokenKind::Const) || self.at(TokenKind::Var) {
            ItemKind::Binding(self.parse_binding(false)?)
        } else {
            self.error_here("expected item");
            return None;
        };

        let end = self.previous_end();
        Some(Item {
            span: Span::new(start, end),
            vis,
            kind,
        })
    }

    fn parse_import(&mut self) -> Option<ImportItem> {
        self.expect(TokenKind::Import, "expected `import`")?;
        let path = self.parse_import_path()?;
        let alias = if self.eat(TokenKind::As).is_some() {
            Some(self.expect_text(TokenKind::Ident, "expected import alias")?)
        } else {
            None
        };
        self.expect(TokenKind::Semicolon, "expected `;` after import")?;
        Some(ImportItem { path, alias })
    }

    fn parse_import_path(&mut self) -> Option<ImportPath> {
        let kind = if self.eat(TokenKind::DotDot).is_some() {
            ImportPathKind::Relative { parents: 1 }
        } else if self.eat(TokenKind::Dot).is_some() {
            ImportPathKind::Relative { parents: 0 }
        } else if self.at(TokenKind::Ellipsis) {
            self.error_here("relative import supports only `.` or `..`");
            return None;
        } else if self.at(TokenKind::Ident) {
            ImportPathKind::Root
        } else {
            self.error_here("expected module path after `import`");
            return None;
        };
        let mut segments = Vec::new();
        segments.push(self.expect_text(TokenKind::Ident, "expected module path segment")?);
        while self.eat(TokenKind::Dot).is_some() {
            segments.push(self.expect_text(TokenKind::Ident, "expected module path segment")?);
        }
        Some(ImportPath { kind, segments })
    }

    fn parse_using(&mut self) -> Option<UsingItem> {
        self.expect(TokenKind::Using, "expected `using`")?;
        let item = self.parse_using_after_keyword()?;
        self.expect(TokenKind::Semicolon, "expected `;` after using")?;
        Some(item)
    }

    pub(super) fn parse_using_after_keyword(&mut self) -> Option<UsingItem> {
        let head_token = self.eat(TokenKind::Ident).or_else(|| {
            self.error_here("expected name after `using`");
            None
        })?;
        let mut host = vec![UsingHostSegment {
            name: self.token_text(&head_token).to_string(),
            span: head_token.span,
        }];
        self.expect(TokenKind::ColonColon, "expected `::` after using head")?;
        // Greedily accept additional host segments as long as we see `IDENT '::'`
        // before either `*`, `{`, or a single-name selector.
        loop {
            if self.at(TokenKind::Star) || self.at(TokenKind::LBrace) {
                break;
            }
            if !self.at(TokenKind::Ident) {
                self.error_here("expected name in using selector");
                return None;
            }
            // Two-token lookahead: if `IDENT '::'`, treat IDENT as another host segment.
            // Otherwise it's a single-name selector.
            let next_kind = self.tokens.get(self.pos + 1).map(|t| t.kind.clone());
            if matches!(next_kind, Some(TokenKind::ColonColon)) {
                let segment_token = self.bump();
                host.push(UsingHostSegment {
                    name: self.token_text(&segment_token).to_string(),
                    span: segment_token.span,
                });
                self.expect(TokenKind::ColonColon, "expected `::`")?;
                continue;
            }
            break;
        }
        if host.len() > 2 {
            self.error_at(
                host[2].span,
                "`using` host accepts at most two segments (`alias::Enum`)",
            );
        }
        let selector = if let Some(star) = self.eat(TokenKind::Star) {
            UsingSelector::Wildcard { span: star.span }
        } else if self.eat(TokenKind::LBrace).is_some() {
            let mut items = Vec::new();
            while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                items.push(self.parse_using_name()?);
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RBrace, "expected `}` after using group")?;
            UsingSelector::Group(items)
        } else {
            UsingSelector::Single(self.parse_using_name()?)
        };
        Some(UsingItem { host, selector })
    }

    fn parse_using_name(&mut self) -> Option<UsingName> {
        let name_token = self.eat(TokenKind::Ident).or_else(|| {
            self.error_here("expected name in `using`");
            None
        })?;
        let name = self.token_text(&name_token).to_string();
        let name_span = name_token.span;
        let (alias, alias_span) = if self.eat(TokenKind::As).is_some() {
            let alias_token = self.eat(TokenKind::Ident).or_else(|| {
                self.error_here("expected alias after `as`");
                None
            })?;
            let alias_text = self.token_text(&alias_token).to_string();
            (Some(alias_text), Some(alias_token.span))
        } else {
            (None, None)
        };
        Some(UsingName {
            name,
            name_span,
            alias,
            alias_span,
        })
    }

    fn parse_struct(&mut self, is_extern: bool) -> Option<StructItem> {
        self.expect(TokenKind::Struct, "expected `struct`")?;
        let name = self.expect_text(TokenKind::Ident, "expected struct name")?;
        let generics = self.parse_generic_params();
        self.expect(TokenKind::LBrace, "expected `{` after struct name")?;
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Fn) {
                self.error_here("methods must be declared in an `extend Type { ... }` block");
                self.recover_to_member_boundary();
                continue;
            }
            if let Some(field) = self.parse_field() {
                fields.push(field);
            } else {
                self.recover_to_member_boundary();
            }
        }
        self.expect(TokenKind::RBrace, "expected `}` after struct body")?;
        Some(StructItem {
            name,
            generics,
            fields,
            is_extern,
        })
    }

    fn parse_extend(&mut self) -> Option<ExtendItem> {
        self.expect(TokenKind::Extend, "expected `extend`")?;
        let generics = self.parse_generic_params();
        let target = self.parse_type_until(&[TokenKind::LBrace])?;
        self.expect(TokenKind::LBrace, "expected `{` after extend target")?;
        let mut methods = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let vis = if self.eat(TokenKind::Pub).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            if self.at(TokenKind::Fn) {
                if let Some(function) = self.parse_function(false) {
                    methods.push(ExtendMethod { vis, function });
                }
            } else {
                self.error_here("expected method in extend block");
                self.recover_to_member_boundary();
            }
        }
        self.expect(TokenKind::RBrace, "expected `}` after extend body")?;
        Some(ExtendItem {
            generics,
            target,
            methods,
        })
    }

    fn parse_field(&mut self) -> Option<Field> {
        let start = self.peek().span.start;
        let name = self.expect_text(TokenKind::Ident, "expected field name")?;
        self.expect(TokenKind::Colon, "expected `:` after field name")?;
        let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RBrace])?;
        self.eat(TokenKind::Comma);
        Some(Field {
            name,
            span: Span::new(start, ty.span.end),
            ty,
        })
    }

    fn parse_enum(&mut self) -> Option<EnumItem> {
        self.expect(TokenKind::Enum, "expected `enum`")?;
        let name = self.expect_text(TokenKind::Ident, "expected enum name")?;
        let backing_type = if self.eat(TokenKind::Colon).is_some() {
            Some(self.parse_type_until(&[TokenKind::LBrace])?)
        } else {
            None
        };
        self.expect(TokenKind::LBrace, "expected `{` after enum name")?;
        let mut variants = Vec::new();
        let mut is_open = false;
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Underscore) {
                let marker = self.bump();
                if is_open {
                    self.error_at(marker.span, "duplicate open enum marker");
                }
                is_open = true;
                if self.eat(TokenKind::Eq).is_some() {
                    let _ = self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace]);
                    self.error_at(marker.span, "open enum marker cannot have a value");
                }
                self.eat(TokenKind::Comma);
                if !self.at(TokenKind::RBrace) {
                    self.error_at(marker.span, "open enum marker must be last");
                }
                continue;
            }
            let start = self.peek().span.start;
            let name = self.expect_text(TokenKind::Ident, "expected enum variant")?;
            let value = if self.eat(TokenKind::Eq).is_some() {
                Some(self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?)
            } else {
                None
            };
            let end = value
                .as_ref()
                .map_or_else(|| self.previous_end(), |expr| expr.span.end);
            variants.push(EnumVariant {
                name,
                value,
                span: Span::new(start, end),
            });
            self.eat(TokenKind::Comma);
        }
        self.expect(TokenKind::RBrace, "expected `}` after enum body")?;
        Some(EnumItem {
            name,
            backing_type,
            is_open,
            variants,
        })
    }

    fn parse_type_alias(&mut self) -> Option<TypeAliasItem> {
        self.expect(TokenKind::Type, "expected `type`")?;
        let name = self.expect_text(TokenKind::Ident, "expected type alias name")?;
        let generics = self.parse_generic_params();
        self.expect(TokenKind::Eq, "expected `=` in type alias")?;
        let ty = self.parse_type_until(&[TokenKind::Semicolon])?;
        self.expect(TokenKind::Semicolon, "expected `;` after type alias")?;
        Some(TypeAliasItem { name, generics, ty })
    }

    fn parse_function(&mut self, is_extern: bool) -> Option<FunctionItem> {
        let start = self.peek().span.start;
        self.expect(TokenKind::Fn, "expected `fn`")?;
        let name = self.expect_text(TokenKind::Ident, "expected function name")?;
        let generics = self.parse_generic_params();
        self.expect(TokenKind::LParen, "expected `(` after function name")?;
        let (params, is_variadic) = self.parse_params();
        self.expect(TokenKind::RParen, "expected `)` after parameters")?;
        let return_type = if self.at(TokenKind::LBrace) || self.at(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_type_until(&[TokenKind::LBrace, TokenKind::Semicolon])?)
        };
        let body = if self.at(TokenKind::LBrace) {
            Some(self.parse_block()?)
        } else {
            self.expect(TokenKind::Semicolon, "expected function body or `;`")?;
            None
        };
        let end = body
            .as_ref()
            .map_or_else(|| self.previous_end(), |body| body.span.end);
        Some(FunctionItem {
            name,
            generics,
            params,
            return_type,
            body,
            is_extern,
            is_variadic,
            span: Span::new(start, end),
        })
    }

    fn parse_params(&mut self) -> (Vec<Param>, bool) {
        let mut params = Vec::new();
        let mut is_variadic = false;
        while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Ellipsis).is_some() {
                is_variadic = true;
                break;
            }
            if let Some(param) = self.parse_param() {
                params.push(param);
            }
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        (params, is_variadic)
    }

    fn parse_param(&mut self) -> Option<Param> {
        let start = self.peek().span.start;
        let checkpoint = self.pos;
        if self.eat(TokenKind::Amp).is_some() {
            let is_const_receiver = self.eat(TokenKind::Const).is_some();
            let receiver = if is_const_receiver {
                ReceiverKind::RefConst
            } else {
                ReceiverKind::Ref
            };
            if self.at(TokenKind::Ident) && self.token_text(self.peek()) == "self" {
                self.bump();
                return Some(Param {
                    receiver: Some(receiver),
                    name: Some("self".to_string()),
                    ty: None,
                    span: Span::new(start, self.previous_end()),
                });
            }
            self.pos = checkpoint;
            let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?;
            return Some(Param {
                receiver: None,
                name: None,
                span: Span::new(start, ty.span.end),
                ty: Some(ty),
            });
        }
        let name = self.expect_text(TokenKind::Ident, "expected parameter name")?;
        if name == "self" {
            return Some(Param {
                receiver: Some(ReceiverKind::Value),
                name: Some(name),
                ty: None,
                span: Span::new(start, self.previous_end()),
            });
        }
        self.expect(TokenKind::Colon, "expected `:` after parameter name")?;
        let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?;
        Some(Param {
            receiver: None,
            name: Some(name),
            span: Span::new(start, ty.span.end),
            ty: Some(ty),
        })
    }

    fn parse_binding(&mut self, is_extern: bool) -> Option<BindingItem> {
        let is_const = if self.eat(TokenKind::Const).is_some() {
            true
        } else if self.eat(TokenKind::Var).is_some() {
            false
        } else {
            self.error_here("expected `var` or `const`");
            return None;
        };
        let name = self.expect_text(TokenKind::Ident, "expected binding name")?;
        let ty = if self.eat(TokenKind::Colon).is_some() {
            Some(self.parse_type_until(&[TokenKind::Eq, TokenKind::Semicolon])?)
        } else {
            None
        };
        let value = if self.eat(TokenKind::Eq).is_some() {
            if is_extern {
                self.error_here("extern binding cannot have an initializer");
                return None;
            }
            Some(self.parse_expr_until(&[TokenKind::Semicolon])?)
        } else {
            None
        };
        if !is_extern && value.is_none() && ty.is_none() {
            self.error_here("binding declaration requires an explicit type");
            return None;
        }
        if is_extern && ty.is_none() {
            self.error_here("extern binding requires an explicit type");
            return None;
        }
        let anchor = value
            .as_ref()
            .map(|value| value.span)
            .or_else(|| ty.as_ref().map(|ty| ty.span))
            .unwrap_or_else(|| Span::new(self.previous_end(), self.previous_end()));
        self.expect_semicolon_after(anchor, "expected `;` after binding")?;
        Some(BindingItem {
            name,
            ty,
            value,
            is_const,
            is_extern,
        })
    }
}
