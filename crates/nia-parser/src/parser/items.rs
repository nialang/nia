// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl Parser {
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
            } else if self.at(TokenKind::Union) {
                ItemKind::Union(self.parse_union(true)?)
            } else if self.at(TokenKind::Fn) {
                ItemKind::Function(self.parse_function(true)?)
            } else if self.at(TokenKind::Const) || self.at(TokenKind::Var) {
                ItemKind::Binding(self.parse_binding(true)?)
            } else {
                self.error_here(
                    "expected `struct`, `union`, `fn`, `var`, or `const` after `extern`",
                );
                return None;
            }
        } else if self.at(TokenKind::Struct) {
            ItemKind::Struct(self.parse_struct(false)?)
        } else if self.at(TokenKind::Union) {
            ItemKind::Union(self.parse_union(false)?)
        } else if self.at(TokenKind::Trait) {
            ItemKind::Trait(self.parse_trait()?)
        } else if self.at(TokenKind::Extend) {
            ItemKind::Extend(self.parse_extend()?)
        } else if self.at(TokenKind::Enum) {
            ItemKind::Enum(self.parse_enum()?)
        } else if self.at(TokenKind::Type) {
            ItemKind::TypeAlias(self.parse_type_alias()?)
        } else if self.at(TokenKind::Fn) {
            ItemKind::Function(self.parse_function(false)?)
        } else if self.at(TokenKind::Comptime)
            || self.at(TokenKind::Const)
            || self.at(TokenKind::Var)
        {
            ItemKind::Binding(self.parse_binding(false)?)
        } else {
            self.error_here("expected item");
            return None;
        };

        let end = self.previous_end();
        Some(self.make_item(Span::new(start, end), vis, kind))
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
        if self.eat(TokenKind::LBrace).is_some() {
            let mut items = Vec::new();
            while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                items.push(self.parse_using_group_item()?);
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RBrace, "expected `}` after using group")?;
            return Some(UsingItem {
                host: Vec::new(),
                selector: UsingSelector::Group(items),
            });
        }

        let mut host = Vec::new();
        host.push(self.parse_using_host_segment("expected name after `using`")?);
        if self.eat(TokenKind::ColonColon).is_none() {
            return Some(UsingItem {
                host,
                selector: UsingSelector::SelfName,
            });
        }
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
            let next_kind = self.tokens.nth_kind(1).cloned();
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
        let selector = if let Some(star) = self.eat(TokenKind::Star) {
            UsingSelector::Wildcard { span: star.span }
        } else if self.eat(TokenKind::LBrace).is_some() {
            let mut items = Vec::new();
            while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                items.push(self.parse_using_group_item()?);
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

    fn parse_using_host_segment(&mut self, message: &str) -> Option<UsingHostSegment> {
        let token = self.eat(TokenKind::Ident).or_else(|| {
            self.error_here(message);
            None
        })?;
        Some(UsingHostSegment {
            name: self.token_text(&token).to_string(),
            span: token.span,
        })
    }

    fn parse_using_group_item(&mut self) -> Option<UsingGroupItem> {
        let checkpoint = self.tokens.checkpoint();
        let errors_len = self.errors.len();
        let mut host = Vec::new();
        while self.at(TokenKind::Ident)
            && matches!(self.tokens.nth_kind(1), Some(TokenKind::ColonColon))
        {
            let segment_token = self.bump();
            host.push(UsingHostSegment {
                name: self.token_text(&segment_token).to_string(),
                span: segment_token.span,
            });
            self.expect(TokenKind::ColonColon, "expected `::`")?;
        }
        if !host.is_empty() {
            let selector = if self.at(TokenKind::Comma) || self.at(TokenKind::RBrace) {
                UsingSelector::SelfName
            } else if let Some(star) = self.eat(TokenKind::Star) {
                UsingSelector::Wildcard { span: star.span }
            } else if self.eat(TokenKind::LBrace).is_some() {
                let mut items = Vec::new();
                while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                    items.push(self.parse_using_group_item()?);
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                self.expect(TokenKind::RBrace, "expected `}` after using group")?;
                UsingSelector::Group(items)
            } else {
                UsingSelector::Single(self.parse_using_name()?)
            };
            return Some(UsingGroupItem::Nested {
                host,
                selector: Box::new(selector),
            });
        }
        self.tokens.rewind(checkpoint);
        self.errors.truncate(errors_len);
        self.parse_using_name().map(UsingGroupItem::Name)
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
        let where_clause = self.parse_where_clause();
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
            where_clause,
            fields,
            is_extern,
        })
    }

    fn parse_union(&mut self, is_extern: bool) -> Option<UnionItem> {
        self.expect(TokenKind::Union, "expected `union`")?;
        let name = self.expect_text(TokenKind::Ident, "expected union name")?;
        let generics = self.parse_generic_params();
        let where_clause = self.parse_where_clause();
        self.expect(TokenKind::LBrace, "expected `{` after union name")?;
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
        self.expect(TokenKind::RBrace, "expected `}` after union body")?;
        Some(UnionItem {
            name,
            generics,
            where_clause,
            fields,
            is_extern,
        })
    }

    fn parse_trait(&mut self) -> Option<TraitItem> {
        self.expect(TokenKind::Trait, "expected `trait`")?;
        let name = self.expect_text(TokenKind::Ident, "expected trait name")?;
        let generics = self.parse_generic_params();
        let supertraits = self.parse_supertraits();
        let where_clause = self.parse_where_clause();
        self.expect(TokenKind::LBrace, "expected `{` after trait name")?;
        let mut associated_types = Vec::new();
        let mut methods = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Pub).is_some() {
                self.error_here("trait members cannot be marked `pub`");
            }
            if self.at(TokenKind::Type) {
                if let Some(associated_type) = self.parse_trait_associated_type() {
                    associated_types.push(associated_type);
                }
            } else if self.at(TokenKind::Fn) {
                if let Some(function) = self.parse_function(false) {
                    methods.push(TraitMethod { function });
                }
            } else {
                self.error_here("expected associated type or method in trait body");
                self.recover_to_member_boundary();
            }
        }
        self.expect(TokenKind::RBrace, "expected `}` after trait body")?;
        Some(TraitItem {
            name,
            generics,
            supertraits,
            where_clause,
            associated_types,
            methods,
        })
    }

    fn parse_supertraits(&mut self) -> Vec<TypeRef> {
        let mut supertraits = Vec::new();
        if self.eat(TokenKind::Colon).is_none() {
            return supertraits;
        }
        while let Some(supertrait) =
            self.parse_type_until(&[TokenKind::Plus, TokenKind::Where, TokenKind::LBrace])
        {
            supertraits.push(supertrait);
            if self.eat(TokenKind::Plus).is_none() {
                break;
            }
        }
        supertraits
    }

    fn parse_trait_associated_type(&mut self) -> Option<TraitAssociatedType> {
        let start = self.expect(TokenKind::Type, "expected `type`")?.start;
        let name = self.expect_text(TokenKind::Ident, "expected associated type name")?;
        if self.eat(TokenKind::LBracket).is_some() {
            self.error_here("associated type generics are not supported");
            self.collect_until(&[TokenKind::RBracket])?;
            self.expect(
                TokenKind::RBracket,
                "expected `]` after associated type generics",
            )?;
        }
        self.expect(
            TokenKind::Semicolon,
            "expected `;` after associated type declaration",
        )?;
        Some(TraitAssociatedType {
            name,
            span: Span::new(start, self.previous_end()),
        })
    }

    fn parse_extend(&mut self) -> Option<ExtendItem> {
        self.expect(TokenKind::Extend, "expected `extend`")?;
        let generics = self.parse_generic_params();
        let target =
            self.parse_type_until(&[TokenKind::Colon, TokenKind::Where, TokenKind::LBrace])?;
        let trait_ref = if self.eat(TokenKind::Colon).is_some() {
            Some(self.parse_type_until(&[TokenKind::Where, TokenKind::LBrace])?)
        } else {
            None
        };
        let where_clause = self.parse_where_clause();
        self.expect(TokenKind::LBrace, "expected `{` after extend target")?;
        let mut associated_types = Vec::new();
        let mut methods = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let vis = if self.eat(TokenKind::Pub).is_some() {
                Visibility::Public
            } else {
                Visibility::Private
            };
            if self.at(TokenKind::Type) {
                if vis == Visibility::Public {
                    self.error_here("trait associated type definitions cannot be marked `pub`");
                }
                if let Some(associated_type) = self.parse_extend_associated_type() {
                    associated_types.push(associated_type);
                }
            } else if self.at(TokenKind::Fn) {
                if let Some(function) = self.parse_function(false) {
                    methods.push(ExtendMethod { vis, function });
                }
            } else {
                self.error_here("expected associated type or method in extend block");
                self.recover_to_member_boundary();
            }
        }
        self.expect(TokenKind::RBrace, "expected `}` after extend body")?;
        Some(ExtendItem {
            generics,
            target,
            trait_ref,
            where_clause,
            associated_types,
            methods,
        })
    }

    fn parse_extend_associated_type(&mut self) -> Option<nia_ast::ExtendAssociatedType> {
        let start = self.expect(TokenKind::Type, "expected `type`")?.start;
        let name = self.expect_text(TokenKind::Ident, "expected associated type name")?;
        if self.eat(TokenKind::LBracket).is_some() {
            self.error_here("associated type generics are not supported");
            self.collect_until(&[TokenKind::RBracket])?;
            self.expect(
                TokenKind::RBracket,
                "expected `]` after associated type generics",
            )?;
        }
        self.expect(TokenKind::Eq, "expected `=` in associated type definition")?;
        let ty = self.parse_type_until(&[TokenKind::Semicolon])?;
        self.expect(
            TokenKind::Semicolon,
            "expected `;` after associated type definition",
        )?;
        Some(nia_ast::ExtendAssociatedType {
            name,
            ty,
            span: Span::new(start, self.previous_end()),
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
        let where_clause = self.parse_where_clause();
        self.expect(TokenKind::Eq, "expected `=` in type alias")?;
        let ty = self.parse_type_until(&[TokenKind::Semicolon])?;
        self.expect(TokenKind::Semicolon, "expected `;` after type alias")?;
        Some(TypeAliasItem {
            name,
            generics,
            where_clause,
            ty,
        })
    }

    fn parse_function(&mut self, is_extern: bool) -> Option<FunctionItem> {
        let start = self.peek().span.start;
        self.expect(TokenKind::Fn, "expected `fn`")?;
        let name = self.expect_text(TokenKind::Ident, "expected function name")?;
        let generics = self.parse_generic_params();
        self.expect(TokenKind::LParen, "expected `(` after function name")?;
        let (params, is_variadic) = self.parse_params();
        self.expect(TokenKind::RParen, "expected `)` after parameters")?;
        let return_type = if self.at(TokenKind::Where)
            || self.at(TokenKind::LBrace)
            || self.at(TokenKind::Semicolon)
        {
            None
        } else {
            Some(self.parse_type_until(&[
                TokenKind::Where,
                TokenKind::LBrace,
                TokenKind::Semicolon,
            ])?)
        };
        let where_clause = self.parse_where_clause();
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
            where_clause,
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
        let checkpoint = self.tokens.checkpoint();
        if self.eat(TokenKind::Amp).is_some() {
            let is_const_receiver = self.eat(TokenKind::Const).is_some();
            let receiver = if is_const_receiver {
                ReceiverKind::RefConst
            } else {
                ReceiverKind::Ref
            };
            if self.at(TokenKind::Ident) && self.token_text(self.peek()) == "self" {
                self.bump();
                return Some(self.make_param(
                    Span::new(start, self.previous_end()),
                    Some(receiver),
                    Some("self".to_string()),
                    None,
                ));
            }
            self.tokens.rewind(checkpoint);
            let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?;
            return Some(self.make_param(Span::new(start, ty.span.end), None, None, Some(ty)));
        }
        let name = self.expect_text(TokenKind::Ident, "expected parameter name")?;
        if name == "self" {
            return Some(self.make_param(
                Span::new(start, self.previous_end()),
                Some(ReceiverKind::Value),
                Some(name),
                None,
            ));
        }
        self.expect(TokenKind::Colon, "expected `:` after parameter name")?;
        let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?;
        Some(self.make_param(Span::new(start, ty.span.end), None, Some(name), Some(ty)))
    }

    fn parse_binding(&mut self, is_extern: bool) -> Option<BindingItem> {
        let (is_const, is_comptime) = if self.eat(TokenKind::Comptime).is_some() {
            if is_extern {
                self.error_here("extern binding cannot be `comptime`");
                return None;
            }
            (true, true)
        } else if self.eat(TokenKind::Const).is_some() {
            (true, false)
        } else if self.eat(TokenKind::Var).is_some() {
            (false, false)
        } else {
            self.error_here("expected `var`, `const`, or `comptime`");
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
        if is_comptime && value.is_none() {
            self.error_here("comptime binding requires an initializer");
            return None;
        }
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
            is_comptime,
            is_extern,
        })
    }
}
