// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

impl Parser {
    pub(super) fn parse_item(&mut self) -> Option<Item> {
        let attributes = self.parse_attributes()?;
        let start = attributes
            .first()
            .map_or_else(|| self.peek().span.start, |attr| attr.span.start);
        let pub_token = self.eat(TokenKind::Pub);
        let pub_span = pub_token.map(|token| token.span);
        let vis = if pub_span.is_some() {
            self.parse_pub_visibility_suffix()?
        } else {
            Visibility::Private
        };

        let kind = if self.at(TokenKind::Module) {
            ItemKind::Module(self.parse_module_item()?)
        } else if self.at(TokenKind::Using) {
            ItemKind::Using(self.parse_using()?)
        } else if self.at(TokenKind::Extern) {
            self.bump();
            if self.at(TokenKind::Struct) {
                ItemKind::Struct(self.parse_struct(true)?)
            } else if self.at(TokenKind::Union) {
                ItemKind::Union(self.parse_union(true)?)
            } else if self.at(TokenKind::Fn) {
                ItemKind::Function(self.parse_function(true, false)?)
            } else if self.at(TokenKind::Static) {
                ItemKind::Binding(self.parse_binding(true)?)
            } else {
                self.error_here("expected `struct`, `union`, `fn`, or `static` after `extern`");
                return None;
            }
        } else if self.at(TokenKind::Struct) {
            ItemKind::Struct(self.parse_struct(false)?)
        } else if self.at(TokenKind::Union) {
            ItemKind::Union(self.parse_union(false)?)
        } else if self.at(TokenKind::Trait) {
            ItemKind::Trait(self.parse_trait()?)
        } else if self.at(TokenKind::Extend) {
            ItemKind::Extend(self.parse_extend(item_has_builtin_attribute(&attributes))?)
        } else if self.at(TokenKind::Enum) {
            ItemKind::Enum(self.parse_enum()?)
        } else if self.at(TokenKind::Type) {
            ItemKind::TypeAlias(self.parse_type_alias(item_has_builtin_attribute(&attributes))?)
        } else if self.at(TokenKind::Fn) || self.at_const_fn() {
            ItemKind::Function(self.parse_function(false, self.at_const_fn())?)
        } else if self.at(TokenKind::Const) || self.at(TokenKind::Static) {
            ItemKind::Binding(
                self.parse_binding_with_options(false, !item_has_builtin_attribute(&attributes))?,
            )
        } else if self.at(TokenKind::Let) {
            self.error_here("top-level storage declarations use `static`; `let` is local-only");
            return None;
        } else {
            self.error_here("expected item");
            return None;
        };

        let end = self.previous_end();
        Some(self.make_item(Span::new(start, end), attributes, vis, kind))
    }

    fn parse_pub_visibility_suffix(&mut self) -> Option<Visibility> {
        if self.eat(TokenKind::LParen).is_none() {
            return Some(Visibility::Public);
        }
        let vis = if self.at(TokenKind::Super) {
            self.bump();
            Visibility::PublicSuper
        } else if self.at(TokenKind::Pkg) {
            self.bump();
            Visibility::PublicPkg
        } else {
            self.error_here("expected `super` or `pkg` in visibility");
            Visibility::Public
        };
        self.expect(TokenKind::RParen, "expected `)` after visibility")?;
        Some(vis)
    }

    pub(super) fn parse_attributes(&mut self) -> Option<Vec<Attribute>> {
        let mut attributes = Vec::new();
        while self.at(TokenKind::At) && matches!(self.tokens.nth_kind(1), Some(TokenKind::LBracket))
        {
            attributes.push(self.parse_attribute()?);
        }
        Some(attributes)
    }

    fn parse_attribute(&mut self) -> Option<Attribute> {
        let start = self
            .expect(TokenKind::At, "expected `@` before attribute")?
            .start;
        self.expect(TokenKind::LBracket, "expected `[` after `@` in attribute")?;
        if self.at(TokenKind::If) {
            self.bump();
            let condition = self.parse_condition_expr_until(&[TokenKind::RBracket])?;
            let end = self
                .expect(
                    TokenKind::RBracket,
                    "expected `]` after conditional attribute",
                )?
                .end;
            return Some(Attribute {
                kind: AttributeKind::If(condition),
                span: Span::new(start, end),
            });
        }
        let mut path = Vec::new();
        path.push(self.expect_name(TokenKind::Ident, "expected attribute name")?);
        while self.eat(TokenKind::Dot).is_some() {
            path.push(self.expect_name(TokenKind::Ident, "expected attribute path segment")?);
        }
        let args = if self.eat(TokenKind::LParen).is_some() {
            let mut args = Vec::new();
            while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                args.push(self.parse_expr_until_tokens(&[TokenKind::Comma, TokenKind::RParen])?);
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RParen, "expected `)` after attribute arguments")?;
            args
        } else {
            Vec::new()
        };
        let end = self
            .expect(TokenKind::RBracket, "expected `]` after attribute")?
            .end;
        Some(Attribute {
            kind: AttributeKind::Meta(AttributeMeta { path, args }),
            span: Span::new(start, end),
        })
    }

    fn parse_module_item(&mut self) -> Option<ModuleItem> {
        self.expect(TokenKind::Module, "expected `module`")?;
        let name = self.expect_name(TokenKind::Ident, "expected module name")?;
        self.expect(
            TokenKind::Semicolon,
            "expected `;` after module declaration",
        )?;
        Some(ModuleItem { name })
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
            if !self.at_namespace_segment() {
                self.error_here("expected name in using selector");
                return None;
            }
            // Two-token lookahead: if `NAME '::'`, treat NAME as another host segment.
            // Otherwise it's a single-name selector.
            let next_kind = self.tokens.nth_kind(1).cloned();
            if matches!(next_kind, Some(TokenKind::ColonColon)) {
                let segment_token = self.bump();
                let kind = self.path_segment_kind_from_token(&segment_token)?;
                host.push(UsingHostSegment {
                    kind,
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
        let (kind, span) = self.expect_path_segment_kind(message)?;
        Some(UsingHostSegment { kind, span })
    }

    fn parse_using_group_item(&mut self) -> Option<UsingGroupItem> {
        let checkpoint = self.checkpoint();
        let errors_len = self.errors.len();
        let mut host = Vec::new();
        while self.at_namespace_segment()
            && matches!(self.tokens.nth_kind(1), Some(TokenKind::ColonColon))
        {
            let segment_token = self.bump();
            let kind = self.path_segment_kind_from_token(&segment_token)?;
            host.push(UsingHostSegment {
                kind,
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
        self.rewind(checkpoint);
        self.errors.truncate(errors_len);
        self.parse_using_name().map(UsingGroupItem::Name)
    }

    fn parse_using_name(&mut self) -> Option<UsingName> {
        let name_token = self.eat(TokenKind::Ident).or_else(|| {
            self.error_here("expected name in `using`");
            None
        })?;
        let name = self.token_name(&name_token)?;
        let name_span = name_token.span;
        let (alias, alias_span) = if self.eat(TokenKind::As).is_some() {
            let alias_token = self.eat(TokenKind::Ident).or_else(|| {
                self.error_here("expected alias after `as`");
                None
            })?;
            let alias_text = self.token_name(&alias_token)?;
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
        let name = self.expect_name(TokenKind::Ident, "expected struct name")?;
        let generics = self.parse_generic_params();
        if self.eat(TokenKind::LParen).is_some() {
            if is_extern {
                self.error_here("extern tuple structs are not supported");
            }
            let mut fields = Vec::new();
            while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                let start = self.peek().span.start;
                let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?;
                let index = fields.len();
                let field_name = match self.symbols.intern(&index.to_string()) {
                    Ok(symbol) => symbol,
                    Err(collision) => {
                        self.error_at(
                            Span::new(start, ty.span.end),
                            format!("failed to create positional field name: {collision}"),
                        );
                        nia_symbol::SymbolId::EMPTY
                    }
                };
                fields.push(self.make_field(
                    field_name,
                    ty.clone(),
                    None,
                    Vec::new(),
                    Span::new(start, ty.span.end),
                ));
                if self.eat(TokenKind::Comma).is_none() {
                    break;
                }
            }
            if fields.is_empty() {
                self.error_here(
                    "tuple struct requires at least one field; use `{}` for an empty struct",
                );
            }
            self.expect(TokenKind::RParen, "expected `)` after tuple struct fields")?;
            let where_clause = self.parse_where_clause();
            return Some(StructItem {
                name,
                generics,
                where_clause,
                fields,
                is_tuple: true,
                is_extern,
            });
        }
        let where_clause = self.parse_where_clause();
        self.expect(TokenKind::LBrace, "expected `{` after struct name")?;
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Fn) {
                self.error_here("methods must be declared in an `extend Type { ... }` block");
                let checkpoint = self.checkpoint();
                self.recover_to_member_boundary_with_progress(checkpoint);
                continue;
            }
            let checkpoint = self.checkpoint();
            if let Some(field) = self.parse_field() {
                fields.push(field);
            } else {
                self.origins.rollback(checkpoint.origin);
                self.recover_to_member_boundary_with_progress(checkpoint);
            }
        }
        self.expect(TokenKind::RBrace, "expected `}` after struct body")?;
        Some(StructItem {
            name,
            generics,
            where_clause,
            fields,
            is_tuple: false,
            is_extern,
        })
    }

    fn parse_union(&mut self, is_extern: bool) -> Option<UnionItem> {
        self.expect(TokenKind::Union, "expected `union`")?;
        let name = self.expect_name(TokenKind::Ident, "expected union name")?;
        let generics = self.parse_generic_params();
        let where_clause = self.parse_where_clause();
        self.expect(TokenKind::LBrace, "expected `{` after union name")?;
        let mut fields = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Fn) {
                self.error_here("methods must be declared in an `extend Type { ... }` block");
                let checkpoint = self.checkpoint();
                self.recover_to_member_boundary_with_progress(checkpoint);
                continue;
            }
            let checkpoint = self.checkpoint();
            if let Some(field) = self.parse_field() {
                fields.push(field);
            } else {
                self.origins.rollback(checkpoint.origin);
                self.recover_to_member_boundary_with_progress(checkpoint);
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
        let name = self.expect_name(TokenKind::Ident, "expected trait name")?;
        let generics = self.parse_generic_params();
        let supertraits = self.parse_supertraits();
        let where_clause = self.parse_where_clause();
        self.expect(TokenKind::LBrace, "expected `{` after trait name")?;
        let mut associated_types = Vec::new();
        let mut associated_values = Vec::new();
        let mut methods = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Pub).is_some() {
                self.error_here("trait members cannot be marked `pub`");
            }
            if self.at(TokenKind::Type) {
                let checkpoint = self.checkpoint();
                if let Some(associated_type) = self.parse_trait_associated_type() {
                    associated_types.push(associated_type);
                } else {
                    self.origins.rollback(checkpoint.origin);
                }
            } else if self.at(TokenKind::Fn) || self.at_const_fn() {
                let checkpoint = self.checkpoint();
                if let Some(function) = self.parse_function(false, self.at_const_fn()) {
                    methods.push(TraitMethod { function });
                } else {
                    self.origins.rollback(checkpoint.origin);
                }
            } else if self.at(TokenKind::Const) {
                let checkpoint = self.checkpoint();
                if let Some(associated_value) = self.parse_trait_associated_value() {
                    associated_values.push(associated_value);
                } else {
                    self.origins.rollback(checkpoint.origin);
                }
            } else {
                self.error_here(
                    "expected associated type, associated const value, or method in trait body",
                );
                let checkpoint = self.checkpoint();
                self.recover_to_member_boundary_with_progress(checkpoint);
            }
        }
        self.expect(TokenKind::RBrace, "expected `}` after trait body")?;
        Some(TraitItem {
            name,
            generics,
            supertraits,
            where_clause,
            associated_types,
            associated_values,
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
        let name = self.expect_name(TokenKind::Ident, "expected associated type name")?;
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
        Some(self.make_trait_associated_type(name, Span::new(start, self.previous_end())))
    }

    fn parse_trait_associated_value(&mut self) -> Option<nia_ast::TraitAssociatedValue> {
        let start = self.expect(TokenKind::Const, "expected `const`")?.start;
        if self.eat(TokenKind::Mut).is_some() {
            self.error_here("trait associated const declarations cannot be mutable");
        }
        let name = self.expect_name(TokenKind::Ident, "expected associated const name")?;
        self.expect(TokenKind::Colon, "expected `:` after associated const name")?;
        let ty = self.parse_type_until(&[TokenKind::Eq, TokenKind::Semicolon])?;
        if self.eat(TokenKind::Eq).is_some() {
            self.error_here("trait associated const declarations cannot have initializers");
            self.collect_until(&[TokenKind::Semicolon])?;
        }
        self.expect(
            TokenKind::Semicolon,
            "expected `;` after associated const declaration",
        )?;
        Some(self.make_trait_associated_value(name, ty, Span::new(start, self.previous_end())))
    }

    fn parse_extend(&mut self, is_builtin_extend: bool) -> Option<ExtendItem> {
        self.expect(TokenKind::Extend, "expected `extend`")?;
        let generics = self.parse_extend_generic_params();
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
        let mut associated_values = Vec::new();
        let mut methods = Vec::new();
        while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
            let vis = if self.eat(TokenKind::Pub).is_some() {
                self.parse_pub_visibility_suffix()?
            } else {
                Visibility::Private
            };
            if self.at(TokenKind::Type) {
                if vis == Visibility::Public {
                    self.error_here("trait associated type definitions cannot be marked `pub`");
                }
                let checkpoint = self.checkpoint();
                if let Some(associated_type) = self.parse_extend_associated_type() {
                    associated_types.push(associated_type);
                } else {
                    self.origins.rollback(checkpoint.origin);
                }
            } else if self.at(TokenKind::Fn) || self.at_const_fn() {
                let checkpoint = self.checkpoint();
                if let Some(function) = self.parse_function(false, self.at_const_fn()) {
                    methods.push(ExtendMethod { vis, function });
                } else {
                    self.origins.rollback(checkpoint.origin);
                }
            } else if self.at(TokenKind::Const) {
                let start = self.peek().span.start;
                let checkpoint = self.checkpoint();
                if let Some(binding) = self.parse_extend_associated_value_binding(is_builtin_extend)
                {
                    let span = Span::new(start, self.previous_end());
                    associated_values.push(nia_ast::ExtendAssociatedValue { vis, binding, span });
                } else {
                    self.origins.rollback(checkpoint.origin);
                }
            } else if self.at(TokenKind::Let) {
                self.error_here("extend value members must be declared as `const` values");
                let checkpoint = self.checkpoint();
                self.recover_to_member_boundary_with_progress(checkpoint);
            } else {
                self.error_here(
                    "expected associated type, associated const value, or method in extend block",
                );
                let checkpoint = self.checkpoint();
                self.recover_to_member_boundary_with_progress(checkpoint);
            }
        }
        self.expect(TokenKind::RBrace, "expected `}` after extend body")?;
        Some(ExtendItem {
            generics,
            target,
            trait_ref,
            where_clause,
            associated_types,
            associated_values,
            methods,
        })
    }

    fn parse_extend_associated_value_binding(
        &mut self,
        allow_bodyless: bool,
    ) -> Option<BindingItem> {
        let start = self.peek().span.start;
        let binding = self.parse_binding_with_options(false, false)?;
        if binding.is_const() && binding.value.is_none() && !allow_bodyless {
            self.error_at(
                Span::new(start, self.previous_end()),
                "const binding requires an initializer",
            );
            return None;
        }
        if binding.is_const() && binding.value.is_none() && binding.ty.is_none() {
            self.error_at(
                Span::new(start, self.previous_end()),
                "bodyless associated const declaration requires an explicit type",
            );
            return None;
        }
        Some(binding)
    }

    fn parse_extend_generic_params(&mut self) -> Vec<nia_ast::GenericParam> {
        let checkpoint = self.checkpoint();
        let errors_len = self.errors.len();
        let generics = self.parse_generic_params();
        if generics.is_empty() {
            self.rewind(checkpoint);
            self.errors.truncate(errors_len);
            return Vec::new();
        }
        if self.type_can_start() {
            return generics;
        }
        self.rewind(checkpoint);
        self.errors.truncate(errors_len);
        Vec::new()
    }

    fn parse_extend_associated_type(&mut self) -> Option<nia_ast::ExtendAssociatedType> {
        let start = self.expect(TokenKind::Type, "expected `type`")?.start;
        let name = self.expect_name(TokenKind::Ident, "expected associated type name")?;
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
        Some(self.make_extend_associated_type(name, ty, Span::new(start, self.previous_end())))
    }

    fn parse_field(&mut self) -> Option<Field> {
        let attributes = self.parse_attributes()?;
        let start = attributes
            .first()
            .map_or_else(|| self.peek().span.start, |attr| attr.span.start);
        let name = self.expect_name(TokenKind::Ident, "expected field name")?;
        self.expect(TokenKind::Colon, "expected `:` after field name")?;
        let ty = self.parse_type_until(&[TokenKind::Eq, TokenKind::Comma, TokenKind::RBrace])?;
        let default = if self.eat(TokenKind::Eq).is_some() {
            Some(self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?)
        } else {
            None
        };
        self.eat(TokenKind::Comma);
        let end = default.as_ref().map_or(ty.span.end, |expr| expr.span.end);
        Some(self.make_field(name, ty, default, attributes, Span::new(start, end)))
    }

    fn parse_enum(&mut self) -> Option<EnumItem> {
        self.expect(TokenKind::Enum, "expected `enum`")?;
        let name = self.expect_name(TokenKind::Ident, "expected enum name")?;
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
            let name = self.expect_name(TokenKind::Ident, "expected enum variant")?;
            let payload = if self.eat(TokenKind::LParen).is_some() {
                let mut fields = Vec::new();
                while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                    fields.push(self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?);
                    if self.eat(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                if fields.is_empty() {
                    self.error_here("tuple enum variant requires at least one payload type");
                }
                self.expect(TokenKind::RParen, "expected `)` after enum variant payload")?;
                EnumVariantPayload::Tuple(fields)
            } else if self.eat(TokenKind::LBrace).is_some() {
                let mut fields = Vec::new();
                while !self.at(TokenKind::RBrace) && !self.at(TokenKind::Eof) {
                    let checkpoint = self.checkpoint();
                    if let Some(field) = self.parse_field() {
                        fields.push(field);
                    } else {
                        self.origins.rollback(checkpoint.origin);
                        self.recover_to_member_boundary_with_progress(checkpoint);
                    }
                }
                if fields.is_empty() {
                    self.error_here("named enum variant requires at least one payload field");
                }
                self.expect(TokenKind::RBrace, "expected `}` after enum variant payload")?;
                EnumVariantPayload::Named(fields)
            } else {
                EnumVariantPayload::Unit
            };
            let value = if self.eat(TokenKind::Eq).is_some() {
                Some(self.parse_expr_until(&[TokenKind::Comma, TokenKind::RBrace])?)
            } else {
                None
            };
            let end = value
                .as_ref()
                .map_or_else(|| self.previous_end(), |expr| expr.span.end);
            variants.push(self.make_enum_variant(name, payload, value, Span::new(start, end)));
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

    fn parse_type_alias(&mut self, is_builtin_type: bool) -> Option<TypeAliasItem> {
        let start = self.peek().span.start;
        self.expect(TokenKind::Type, "expected `type`")?;
        let name = if is_builtin_type
            && matches!(
                self.peek().kind,
                TokenKind::Bool | TokenKind::Char | TokenKind::Never
            ) {
            let token = self.bump();

            self.token_name(&token)?
        } else {
            self.expect_name(TokenKind::Ident, "expected type alias name")?
        };
        let generics = self.parse_generic_params();
        let where_clause = self.parse_where_clause();
        let ty = if self.eat(TokenKind::Eq).is_some() {
            Some(self.parse_type_until(&[TokenKind::Semicolon])?)
        } else if is_builtin_type {
            None
        } else {
            self.error_at(
                Span::new(start, self.previous_end()),
                "expected `=` in type alias",
            );
            return None;
        };
        self.expect(TokenKind::Semicolon, "expected `;` after type alias")?;
        Some(TypeAliasItem {
            name,
            generics,
            where_clause,
            ty,
        })
    }

    fn parse_function(&mut self, is_extern: bool, is_const: bool) -> Option<FunctionItem> {
        let start = self.peek().span.start;
        if is_const {
            self.expect(TokenKind::Const, "expected `const`")?;
            if is_extern {
                self.error_here("extern function cannot be `const`");
            }
        }
        self.expect(TokenKind::Fn, "expected `fn`")?;
        let name = self.expect_name(TokenKind::Ident, "expected function name")?;
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
        Some(self.make_function(FunctionParts {
            name,
            generics,
            where_clause,
            params,
            return_type,
            body,
            is_extern,
            is_const,
            is_variadic,
            span: Span::new(start, end),
        }))
    }

    pub(super) fn parse_params(&mut self) -> (Vec<Param>, bool) {
        let mut params = Vec::new();
        let mut is_variadic = false;
        while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Ellipsis).is_some() {
                is_variadic = true;
                break;
            }
            let checkpoint = self.checkpoint();
            if let Some(param) = self.parse_param() {
                params.push(param);
            } else {
                self.origins.rollback(checkpoint.origin);
            }
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        (params, is_variadic)
    }

    pub(super) fn parse_param(&mut self) -> Option<Param> {
        let start = self.peek().span.start;
        let checkpoint = self.checkpoint();
        if self.eat(TokenKind::Amp).is_some() {
            let receiver = if self.eat(TokenKind::Mut).is_some() {
                ReceiverKind::Ref
            } else {
                ReceiverKind::RefReadOnly
            };
            if self.at(TokenKind::SelfValue) {
                self.bump();
                return Some(self.make_param(
                    Span::new(start, self.previous_end()),
                    Some(receiver),
                    None,
                    None,
                ));
            }
            self.rewind(checkpoint);
            let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?;
            return Some(self.make_param(Span::new(start, ty.span.end), None, None, Some(ty)));
        }
        if self.at(TokenKind::SelfValue) {
            self.bump();
            return Some(self.make_param(
                Span::new(start, self.previous_end()),
                Some(ReceiverKind::Value),
                None,
                None,
            ));
        }
        let name = self.expect_name(TokenKind::Ident, "expected parameter name")?;
        self.expect(TokenKind::Colon, "expected `:` after parameter name")?;
        let ty = self.parse_type_until(&[TokenKind::Comma, TokenKind::RParen])?;
        Some(self.make_param(Span::new(start, ty.span.end), None, Some(name), Some(ty)))
    }

    pub(super) fn parse_binding(&mut self, is_extern: bool) -> Option<BindingItem> {
        self.parse_binding_with_options(is_extern, true)
    }

    fn parse_binding_with_options(
        &mut self,
        is_extern: bool,
        require_const_initializer: bool,
    ) -> Option<BindingItem> {
        let start = self.peek().span.start;
        let is_const = if self.eat(TokenKind::Const).is_some() {
            if is_extern {
                self.error_here("extern binding cannot be `const`");
                return None;
            }
            true
        } else {
            false
        };
        let kind = if is_const {
            if self.at(TokenKind::Mut) {
                self.error_here("const bindings cannot be mutable");
                return None;
            }
            ItemBindingKind::Const
        } else if self.eat(TokenKind::Static).is_some() {
            ItemBindingKind::Static {
                is_mutable: self.eat(TokenKind::Mut).is_some(),
                is_extern,
            }
        } else if self.at(TokenKind::Let) {
            self.error_here("top-level storage declarations use `static`; `let` is local-only");
            return None;
        } else {
            self.error_here("expected `static` binding");
            return None;
        };
        let name = self.expect_name(TokenKind::Ident, "expected binding name")?;
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
        if is_const && value.is_none() && require_const_initializer {
            self.error_here("const binding requires an initializer");
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
        Some(self.make_binding(BindingParts {
            name,
            ty,
            value,
            kind,
            span: Span::new(start, self.previous_end()),
        }))
    }
}

fn item_has_builtin_attribute(attributes: &[Attribute]) -> bool {
    let builtin = nia_symbol::known::builtin();
    attributes.iter().any(|attribute| {
        let AttributeKind::Meta(meta) = &attribute.kind else {
            return false;
        };
        meta.path.len() == 1 && meta.path[0] == builtin
    })
}
