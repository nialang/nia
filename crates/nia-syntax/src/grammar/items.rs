// SPDX-License-Identifier: GPL-3.0-or-later
//! Source file, item, attribute, and declaration grammar.

use nia_lexer::TokenKind;
use nia_span::Span;

use super::parser::{Done, Iteration, Parsed, Parser};
use super::{
    collect_until, exprs, list_separator, recover_to_attribute_boundary, recover_to_item_boundary,
    recover_to_list_boundary, recover_to_member_boundary, recover_to_using_boundary, skip_until,
    stmts, types,
};
use crate::{ParseErrorKind, SyntaxKind};

/// Parses top-level iterations until EOF or until `resync` accepts the
/// start byte of the next iteration; returns whether it resynchronized.
///
/// Each iteration parses one item and recovers to the next item boundary.
/// Its output depends only on the tokens it reads, which `iterations`
/// records for incremental reuse.
pub(crate) fn source_file(
    p: &mut Parser<'_>,
    iterations: &mut Vec<Iteration>,
    mut resync: impl FnMut(usize) -> bool,
) -> bool {
    while !p.at(TokenKind::Eof) {
        if resync(p.current_start()) {
            return true;
        }
        let start = p.pos();
        p.begin_iteration();
        let checkpoint = p.checkpoint();
        if item(p).is_none() {
            recover_to_item_boundary(p, checkpoint);
        }
        iterations.push(p.iteration(start));
    }
    false
}

/// Summary of an attribute run that later item decisions depend on.
#[derive(Default)]
pub(crate) struct Attributes {
    /// Start of the first successfully parsed attribute.
    pub(crate) start: Option<usize>,
    /// Number of successfully parsed attributes.
    pub(crate) count: usize,
    /// Whether one attribute is the bare `@[builtin(...)]` marker.
    pub(crate) builtin: bool,
}

fn item(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let attributes = attributes(p);
        let start = attributes.start.unwrap_or(p.span().start);
        if p.at(TokenKind::Pub) {
            visibility(p, &mut false)?;
        }
        if p.at(TokenKind::Module) {
            module_decl(p)?;
        } else if p.at(TokenKind::Using) {
            using_decl(p)?;
        } else if p.at(TokenKind::Extern) {
            p.bump();
            if p.at(TokenKind::Struct) {
                struct_decl(p, true)?;
            } else if p.at(TokenKind::Union) {
                union_decl(p, true)?;
            } else if p.at(TokenKind::Fn) {
                function_decl(p, true, false)?;
            } else if p.at(TokenKind::Static) {
                binding_decl(p, true, true)?;
            } else {
                p.error_here("expected `struct`, `union`, `fn`, or `static` after `extern`");
                return None;
            }
        } else if p.at(TokenKind::Struct) {
            struct_decl(p, false)?;
        } else if p.at(TokenKind::Union) {
            union_decl(p, false)?;
        } else if p.at(TokenKind::Trait) {
            trait_decl(p)?;
        } else if p.at(TokenKind::Extend) {
            extend_decl(p, attributes.builtin)?;
        } else if p.at(TokenKind::Enum) {
            enum_decl(p)?;
        } else if p.at(TokenKind::Type) {
            type_alias_decl(p, attributes.builtin)?;
        } else if p.at(TokenKind::Fn) || p.at_const_fn() {
            let is_const = p.at_const_fn();
            function_decl(p, false, is_const)?;
        } else if p.at(TokenKind::Const) || p.at(TokenKind::Static) {
            binding_decl(p, false, !attributes.builtin)?;
        } else if p.at(TokenKind::Let) {
            p.error_here("top-level storage declarations use `static`; `let` is local-only");
            return None;
        } else {
            p.error_here("expected item");
            return None;
        }
        Done::new(SyntaxKind::Item, Span::new(start, p.previous_end()))
    })
}

/// Parses `pub`, `pub(super)`, or `pub(pkg)`; `public` reports plain `pub`.
fn visibility(p: &mut Parser<'_>, public: &mut bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        *public = true;
        if p.eat(TokenKind::LParen).is_some() {
            if p.at(TokenKind::Super) || p.at(TokenKind::Pkg) {
                p.bump();
                *public = false;
            } else {
                p.error_here("expected `super` or `pkg` in visibility");
            }
            p.expect(TokenKind::RParen, "expected `)` after visibility")?;
        }
        Done::new(SyntaxKind::Visibility, Span::new(start, p.previous_end()))
    })
}

pub(crate) fn attributes(p: &mut Parser<'_>) -> Attributes {
    let mut summary = Attributes::default();
    while p.at_attribute_start() {
        let checkpoint = p.checkpoint();
        let mut builtin = false;
        if let Some(parsed) = attribute(p, &mut builtin) {
            summary.start.get_or_insert(parsed.span.start);
            summary.count += 1;
            summary.builtin |= builtin;
        } else {
            // A malformed attribute is local to its closing bracket. Keep the
            // following item parseable instead of letting item recovery treat
            // the bracket as a second top-level error.
            recover_to_attribute_boundary(p, checkpoint);
        }
    }
    summary
}

fn attribute(p: &mut Parser<'_>, builtin: &mut bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p
            .expect(TokenKind::At, "expected `@` before attribute")?
            .start;
        p.expect(TokenKind::LBracket, "expected `[` after `@` in attribute")?;
        if p.at(TokenKind::If) {
            p.bump();
            exprs::condition_until(p, &[TokenKind::RBracket])?;
            let end = p
                .expect(
                    TokenKind::RBracket,
                    "expected `]` after conditional attribute",
                )?
                .end;
            return Done::new(SyntaxKind::Attribute, Span::new(start, end));
        }
        let first = p.text();
        p.expect_name("expected attribute name")?;
        let mut segments = 1usize;
        while p.eat(TokenKind::Dot).is_some() {
            p.expect_name("expected attribute path segment")?;
            segments += 1;
        }
        *builtin = segments == 1 && first == "builtin";
        if p.at(TokenKind::LParen) {
            attribute_args(p)?;
        }
        let end = p
            .expect(TokenKind::RBracket, "expected `]` after attribute")?
            .end;
        Done::new(SyntaxKind::Attribute, Span::new(start, end))
    })
}

fn attribute_args(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            if exprs::expr_until_tokens(p, &[TokenKind::Comma, TokenKind::RParen]).is_none() {
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
                continue;
            }
            let next = exprs::expr_can_start(p.kind());
            if !list_separator(p, next, "expected `,` or `)` after attribute argument") {
                break;
            }
        }
        p.expect(TokenKind::RParen, "expected `)` after attribute arguments")?;
        Done::new(
            SyntaxKind::AttributeArgList,
            Span::new(start, p.previous_end()),
        )
    })
}

fn module_decl(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        p.expect_name("expected module name")?;
        p.expect(
            TokenKind::Semicolon,
            "expected `;` after module declaration",
        )?;
        Done::new(SyntaxKind::ModuleDecl, Span::new(start, p.previous_end()))
    })
}

fn using_decl(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        using_tree(p)?;
        p.expect(TokenKind::Semicolon, "expected `;` after using")?;
        Done::new(SyntaxKind::UsingDecl, Span::new(start, p.previous_end()))
    })
}

/// Host path and selector after `using`.
pub(crate) fn using_tree(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        if p.at(TokenKind::LBrace) {
            using_group(p)?;
            return Done::new(SyntaxKind::UsingTree, Span::new(start, p.previous_end()));
        }
        if !p.at_namespace_segment() {
            p.expected_here(ParseErrorKind::ExpectedName, "expected name after `using`");
            return None;
        }
        p.bump();
        if p.eat(TokenKind::ColonColon).is_none() {
            return Done::new(SyntaxKind::UsingTree, Span::new(start, p.previous_end()));
        }
        // Greedily accept `NAME ::` host segments before `*`, `{`, or a
        // single-name selector.
        loop {
            if p.at(TokenKind::Star) || p.at(TokenKind::LBrace) {
                break;
            }
            if !p.at_namespace_segment() {
                p.error_here_as(
                    ParseErrorKind::ExpectedName,
                    "expected name in using selector",
                );
                return None;
            }
            if matches!(p.nth_kind(1), Some(TokenKind::ColonColon)) {
                p.bump();
                p.bump();
                continue;
            }
            break;
        }
        if p.eat(TokenKind::Star).is_none() {
            if p.at(TokenKind::LBrace) {
                using_group(p)?;
            } else {
                using_name(p)?;
            }
        }
        Done::new(SyntaxKind::UsingTree, Span::new(start, p.previous_end()))
    })
}

fn using_group(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) && !p.at_top_level_item_start() {
            let checkpoint = p.checkpoint();
            if using_group_item(p).is_none() {
                recover_to_using_boundary(p, checkpoint);
                continue;
            }
            let next = p.at_namespace_segment();
            if !list_separator(p, next, "expected `,` or `}` after using selector") {
                break;
            }
        }
        p.expect(TokenKind::RBrace, "expected `}` after using group")?;
        Done::new(SyntaxKind::UsingGroup, Span::new(start, p.previous_end()))
    })
}

fn using_group_item(p: &mut Parser<'_>) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let mut has_host = false;
    let nested = p.node(|p| {
        let start = p.span().start;
        while p.at_namespace_segment() && matches!(p.nth_kind(1), Some(TokenKind::ColonColon)) {
            p.bump();
            p.bump();
            has_host = true;
        }
        if !has_host {
            return None;
        }
        if !p.at(TokenKind::Comma) && !p.at(TokenKind::RBrace) && p.eat(TokenKind::Star).is_none() {
            if p.at(TokenKind::LBrace) {
                using_group(p)?;
            } else {
                using_name(p)?;
            }
        }
        Done::new(SyntaxKind::UsingTree, Span::new(start, p.previous_end()))
    });
    if has_host {
        return nested;
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
    using_name(p)
}

fn using_name(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        if p.eat(TokenKind::Ident).is_none() {
            p.error_here_as(ParseErrorKind::ExpectedName, "expected name in `using`");
            return None;
        }
        if p.eat(TokenKind::As).is_some() && p.eat(TokenKind::Ident).is_none() {
            p.error_here_as(ParseErrorKind::ExpectedName, "expected alias after `as`");
            return None;
        }
        Done::new(SyntaxKind::UsingName, Span::new(start, p.previous_end()))
    })
}

fn struct_decl(p: &mut Parser<'_>, is_extern: bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        p.expect_name("expected struct name")?;
        types::generic_params(p);
        if p.at(TokenKind::LParen) {
            tuple_field_list(
                p,
                is_extern.then_some("extern tuple structs are not supported"),
                "tuple struct requires at least one field; use `{}` for an empty struct",
                "expected `)` after tuple struct fields",
            )?;
            types::where_clause(p);
            return Done::new(SyntaxKind::StructDecl, Span::new(start, p.previous_end()));
        }
        types::where_clause(p);
        field_list(
            p,
            "expected `{` after struct name",
            "expected `}` after struct body",
        )?;
        Done::new(SyntaxKind::StructDecl, Span::new(start, p.previous_end()))
    })
}

fn union_decl(p: &mut Parser<'_>, _is_extern: bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        p.expect_name("expected union name")?;
        types::generic_params(p);
        types::where_clause(p);
        field_list(
            p,
            "expected `{` after union name",
            "expected `}` after union body",
        )?;
        Done::new(SyntaxKind::UnionDecl, Span::new(start, p.previous_end()))
    })
}

/// `( T, U )` positional fields of a tuple struct or tuple enum variant.
fn tuple_field_list(
    p: &mut Parser<'_>,
    open_error: Option<&str>,
    empty_message: &str,
    close_message: &str,
) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        if let Some(message) = open_error {
            p.error_here(message);
        }
        let mut fields = 0usize;
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            let field = p.node(|p| {
                let ty = types::type_list_element(p, TokenKind::RParen)?;
                Done::new(SyntaxKind::TupleField, ty.span)
            });
            if field.is_none() {
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                break;
            }
            fields += 1;
            let next = types::type_can_start(p.kind());
            if !list_separator(p, next, "expected `,` or `)` after tuple field") {
                break;
            }
        }
        if fields == 0 {
            p.error_here(empty_message);
        }
        p.expect(TokenKind::RParen, close_message)?;
        Done::new(
            SyntaxKind::TupleFieldList,
            Span::new(start, p.previous_end()),
        )
    })
}

/// `{ name: T, ... }` named fields of a struct or union.
fn field_list(p: &mut Parser<'_>, open_message: &str, close_message: &str) -> Option<Parsed> {
    p.node(|p| {
        let start = p.expect(TokenKind::LBrace, open_message)?.start;
        while !p.at(TokenKind::RBrace)
            && !p.at(TokenKind::Eof)
            && !p.at_top_level_item_boundary_except_fn()
        {
            if p.at(TokenKind::Fn) {
                let checkpoint = p.checkpoint();
                let errors = p.errors_len();
                p.error_here("methods must be declared in an `extend Type { ... }` block");
                recover_to_member_boundary(p, Some(checkpoint));
                if p.at_top_level_item_start() || p.at(TokenKind::Eof) {
                    p.rewind(checkpoint);
                    p.truncate_errors(errors);
                    break;
                }
                continue;
            }
            let checkpoint = p.checkpoint();
            if field(p).is_none() {
                recover_to_member_boundary(p, Some(checkpoint));
            }
        }
        p.expect(TokenKind::RBrace, close_message)?;
        Done::new(SyntaxKind::FieldList, Span::new(start, p.previous_end()))
    })
}

fn field(p: &mut Parser<'_>) -> Option<Parsed> {
    let parsed = p.node(|p| {
        let attributes = attributes(p);
        let start = attributes.start.unwrap_or(p.span().start);
        p.expect_name("expected field name")?;
        p.expect(TokenKind::Colon, "expected `:` after field name")?;
        let ty = types::field_type(p)?;
        Done::new(SyntaxKind::Field, Span::new(start, ty.span.end))
    })?;
    if p.eat(TokenKind::Comma).is_none()
        && p.at(TokenKind::Ident)
        && matches!(p.nth_kind(1), Some(TokenKind::Colon))
    {
        p.expected_here(ParseErrorKind::Grammar, "expected `,` or `}` after field");
    }
    Some(parsed)
}

fn trait_decl(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        p.expect_name("expected trait name")?;
        types::generic_params(p);
        supertraits(p);
        types::where_clause(p);
        member_list(p, MemberOwner::Trait, false)?;
        Done::new(SyntaxKind::TraitDecl, Span::new(start, p.previous_end()))
    })
}

fn supertraits(p: &mut Parser<'_>) {
    if !p.at(TokenKind::Colon) {
        return;
    }
    p.node(|p| {
        let start = p.bump().start;
        while !p.at(TokenKind::Where) && !p.at(TokenKind::LBrace) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            let errors = p.errors_len();
            if types::type_(p).is_some()
                && (p.at(TokenKind::Plus)
                    || p.at(TokenKind::Comma)
                    || p.at(TokenKind::Where)
                    || p.at(TokenKind::LBrace)
                    || types::type_can_start(p.kind()))
            {
                if p.eat(TokenKind::Plus).is_some() {
                    continue;
                }
                if p.eat(TokenKind::Comma).is_some() {
                    p.error_here_as(ParseErrorKind::Grammar, "expected `+` after supertrait");
                    continue;
                }
                if types::type_can_start(p.kind()) {
                    p.expected_here(ParseErrorKind::Grammar, "expected `+` after supertrait");
                    continue;
                }
                break;
            }
            p.rewind(checkpoint);
            p.truncate_errors(errors);
            if types::type_until(
                p,
                &[
                    TokenKind::Plus,
                    TokenKind::Comma,
                    TokenKind::Where,
                    TokenKind::LBrace,
                ],
            )
            .is_none()
            {
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                break;
            }
            if p.eat(TokenKind::Plus).is_some() {
                continue;
            }
            if p.eat(TokenKind::Comma).is_some() {
                p.error_here_as(ParseErrorKind::Grammar, "expected `+` after supertrait");
                continue;
            }
            break;
        }
        Done::new(
            SyntaxKind::SupertraitList,
            Span::new(start, p.previous_end()),
        )
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberOwner {
    Trait,
    Extend,
}

fn member_list(p: &mut Parser<'_>, owner: MemberOwner, builtin: bool) -> Option<Parsed> {
    p.node(|p| {
        let open = match owner {
            MemberOwner::Trait => "expected `{` after trait name",
            MemberOwner::Extend => "expected `{` after extend target",
        };
        let start = p.expect(TokenKind::LBrace, open)?.start;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) {
            match owner {
                MemberOwner::Trait => trait_member(p),
                // A malformed member visibility aborts the whole extension.
                MemberOwner::Extend => {
                    extend_member(p, builtin)?;
                }
            }
        }
        let close = match owner {
            MemberOwner::Trait => "expected `}` after trait body",
            MemberOwner::Extend => "expected `}` after extend body",
        };
        p.expect(TokenKind::RBrace, close)?;
        Done::new(SyntaxKind::MemberList, Span::new(start, p.previous_end()))
    })
}

fn trait_member(p: &mut Parser<'_>) {
    p.node(|p| {
        let start = p.span().start;
        let attributes = attributes(p);
        if p.at(TokenKind::Pub) {
            p.recover_in_error(|p| {
                p.bump();
            });
            p.error_here("trait members cannot be marked `pub`");
        }
        if p.at(TokenKind::Type) {
            if attributes.count > 0 {
                p.error_here("attributes on trait associated types are not supported");
            }
            assoc_type_decl(p, MemberOwner::Trait);
        } else if p.at(TokenKind::Fn) || p.at_const_fn() {
            let is_const = p.at_const_fn();
            function_decl(p, false, is_const);
        } else if p.at(TokenKind::Const) {
            if attributes.count > 0 {
                p.error_here("attributes on trait associated values are not supported");
            }
            assoc_const_decl(p);
        } else {
            p.error_here(
                "expected associated type, associated const value, or method in trait body",
            );
            let checkpoint = p.checkpoint();
            recover_to_member_boundary(p, Some(checkpoint));
        }
        Done::new(SyntaxKind::Member, Span::new(start, p.previous_end()))
    });
}

fn extend_member(p: &mut Parser<'_>, builtin: bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        let attributes = attributes(p);
        let mut public = false;
        if p.at(TokenKind::Pub) {
            visibility(p, &mut public)?;
        }
        if p.at(TokenKind::Type) {
            if attributes.count > 0 {
                p.error_here("attributes on extend associated types are not supported");
            }
            if public {
                p.error_here("trait associated type definitions cannot be marked `pub`");
            }
            assoc_type_decl(p, MemberOwner::Extend);
        } else if p.at(TokenKind::Fn) || p.at_const_fn() {
            let is_const = p.at_const_fn();
            function_decl(p, false, is_const);
        } else if p.at(TokenKind::Const) {
            if attributes.count > 0 {
                p.error_here("attributes on extend associated values are not supported");
            }
            binding_decl_with(p, false, false, Some(builtin));
        } else if p.at(TokenKind::Let) {
            p.error_here("extend value members must be declared as `const` values");
            let checkpoint = p.checkpoint();
            recover_to_member_boundary(p, Some(checkpoint));
        } else {
            p.error_here(
                "expected associated type, associated const value, or method in extend block",
            );
            let checkpoint = p.checkpoint();
            recover_to_member_boundary(p, Some(checkpoint));
        }
        Done::new(SyntaxKind::Member, Span::new(start, p.previous_end()))
    })
}

fn assoc_type_decl(p: &mut Parser<'_>, owner: MemberOwner) -> Option<Parsed> {
    p.node(|p| {
        let start = p.expect(TokenKind::Type, "expected `type`")?.start;
        p.expect_name("expected associated type name")?;
        if p.eat(TokenKind::LBracket).is_some() {
            p.error_here("associated type generics are not supported");
            let mut collected = false;
            p.recover_in_error(|p| {
                collected = collect_until(p, &[TokenKind::RBracket]).is_some();
            });
            if !collected {
                return None;
            }
            p.expect(
                TokenKind::RBracket,
                "expected `]` after associated type generics",
            )?;
        }
        match owner {
            MemberOwner::Trait => {
                p.expect(
                    TokenKind::Semicolon,
                    "expected `;` after associated type declaration",
                )?;
            }
            MemberOwner::Extend => {
                p.expect(TokenKind::Eq, "expected `=` in associated type definition")?;
                types::type_until(p, &[TokenKind::Semicolon])?;
                p.expect(
                    TokenKind::Semicolon,
                    "expected `;` after associated type definition",
                )?;
            }
        }
        Done::new(
            SyntaxKind::AssocTypeDecl,
            Span::new(start, p.previous_end()),
        )
    })
}

fn assoc_const_decl(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.expect(TokenKind::Const, "expected `const`")?.start;
        if p.eat(TokenKind::Mut).is_some() {
            p.error_here("trait associated const declarations cannot be mutable");
        }
        p.expect_name("expected associated const name")?;
        p.expect(TokenKind::Colon, "expected `:` after associated const name")?;
        types::type_until(p, &[TokenKind::Eq, TokenKind::Semicolon])?;
        if p.at(TokenKind::Eq) {
            p.recover_in_error(|p| {
                p.bump();
            });
            p.error_here("trait associated const declarations cannot have initializers");
            let mut collected = false;
            p.recover_in_error(|p| {
                collected = collect_until(p, &[TokenKind::Semicolon]).is_some();
            });
            if !collected {
                return None;
            }
        }
        p.expect(
            TokenKind::Semicolon,
            "expected `;` after associated const declaration",
        )?;
        Done::new(
            SyntaxKind::AssocConstDecl,
            Span::new(start, p.previous_end()),
        )
    })
}

fn extend_decl(p: &mut Parser<'_>, builtin: bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        extend_generic_params(p);
        types::type_until(p, &[TokenKind::Colon, TokenKind::Where, TokenKind::LBrace])?;
        if p.eat(TokenKind::Colon).is_some() {
            types::type_until(p, &[TokenKind::Where, TokenKind::LBrace])?;
        }
        types::where_clause(p);
        member_list(p, MemberOwner::Extend, builtin)?;
        Done::new(SyntaxKind::ExtendDecl, Span::new(start, p.previous_end()))
    })
}

fn extend_generic_params(p: &mut Parser<'_>) {
    let checkpoint = p.checkpoint();
    let errors = p.errors_len();
    let count = types::generic_params(p);
    if count > 0 && types::type_can_start(p.kind()) {
        return;
    }
    p.rewind(checkpoint);
    p.truncate_errors(errors);
}

fn enum_decl(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        p.expect_name("expected enum name")?;
        if p.eat(TokenKind::Colon).is_some() {
            types::type_until(p, &[TokenKind::LBrace])?;
        }
        variant_list(p)?;
        Done::new(SyntaxKind::EnumDecl, Span::new(start, p.previous_end()))
    })
}

fn variant_list(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p
            .expect(TokenKind::LBrace, "expected `{` after enum name")?
            .start;
        let mut is_open = false;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) && !p.at_top_level_item_keyword() {
            if p.at(TokenKind::Underscore) {
                open_variant(p, &mut is_open);
                continue;
            }
            let checkpoint = p.checkpoint();
            if !p.at(TokenKind::Ident) {
                p.expected_here(ParseErrorKind::ExpectedName, "expected enum variant");
                // A missing variant name is local to this comma-delimited
                // entry. Preserve later variants and the enclosing module.
                if !p.at(TokenKind::RBrace) {
                    recover_to_list_boundary(p, checkpoint, TokenKind::RBrace);
                }
                continue;
            }
            variant(p)?;
            if p.eat(TokenKind::Comma).is_none() && p.at(TokenKind::Ident) {
                p.expected_here(
                    ParseErrorKind::Grammar,
                    "expected `,` or `}` after enum variant",
                );
            }
        }
        p.expect(TokenKind::RBrace, "expected `}` after enum body")?;
        Done::new(SyntaxKind::VariantList, Span::new(start, p.previous_end()))
    })
}

fn open_variant(p: &mut Parser<'_>, is_open: &mut bool) {
    p.node(|p| {
        let marker = p.bump();
        if *is_open {
            p.error_at(marker, "duplicate open enum marker");
        }
        *is_open = true;
        if p.eat(TokenKind::Eq).is_some() {
            let _ = exprs::expr_until(p, &[TokenKind::Comma, TokenKind::RBrace]);
            p.error_at(marker, "open enum marker cannot have a value");
        }
        p.eat(TokenKind::Comma);
        if !p.at(TokenKind::RBrace) {
            p.error_at(marker, "open enum marker must be last");
        }
        Done::new(SyntaxKind::OpenVariant, marker)
    });
}

fn variant(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        if p.at(TokenKind::LParen) {
            tuple_field_list(
                p,
                None,
                "tuple enum variant requires at least one payload type",
                "expected `)` after enum variant payload",
            )?;
        } else if p.at(TokenKind::LBrace) {
            variant_named_fields(p)?;
        }
        let mut end = p.previous_end();
        if p.eat(TokenKind::Eq).is_some() {
            end = exprs::expr_until(p, &[TokenKind::Comma, TokenKind::RBrace])?
                .span
                .end;
        }
        Done::new(SyntaxKind::Variant, Span::new(start, end))
    })
}

fn variant_named_fields(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        let mut fields = 0usize;
        while !p.at(TokenKind::RBrace) && !p.at(TokenKind::Eof) {
            let checkpoint = p.checkpoint();
            if field(p).is_some() {
                fields += 1;
            } else {
                recover_to_member_boundary(p, Some(checkpoint));
            }
        }
        if fields == 0 {
            p.error_here("named enum variant requires at least one payload field");
        }
        p.expect(TokenKind::RBrace, "expected `}` after enum variant payload")?;
        Done::new(SyntaxKind::FieldList, Span::new(start, p.previous_end()))
    })
}

fn type_alias_decl(p: &mut Parser<'_>, builtin: bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.bump().start;
        if builtin
            && matches!(
                p.kind(),
                TokenKind::Bool | TokenKind::Char | TokenKind::Never
            )
        {
            p.bump();
        } else {
            p.expect_name("expected type alias name")?;
        }
        types::generic_params(p);
        types::where_clause(p);
        if p.eat(TokenKind::Eq).is_some() {
            types::type_until(p, &[TokenKind::Semicolon])?;
        } else if !builtin {
            p.error_at(
                Span::new(start, p.previous_end()),
                "expected `=` in type alias",
            );
            return None;
        }
        p.expect(TokenKind::Semicolon, "expected `;` after type alias")?;
        Done::new(
            SyntaxKind::TypeAliasDecl,
            Span::new(start, p.previous_end()),
        )
    })
}

pub(crate) fn function_decl(p: &mut Parser<'_>, is_extern: bool, is_const: bool) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        if is_const {
            p.expect(TokenKind::Const, "expected `const`")?;
            if is_extern {
                p.error_here("extern function cannot be `const`");
            }
        }
        p.expect(TokenKind::Fn, "expected `fn`")?;
        p.expect_name("expected function name")?;
        types::generic_params(p);
        param_list(p)?;
        if !p.at(TokenKind::Where) && !p.at(TokenKind::LBrace) && !p.at(TokenKind::Semicolon) {
            types::type_until(
                p,
                &[TokenKind::Where, TokenKind::LBrace, TokenKind::Semicolon],
            )?;
        }
        types::where_clause(p);
        let end = if p.at(TokenKind::LBrace) {
            stmts::block(p)?.span.end
        } else {
            p.expect(TokenKind::Semicolon, "expected function body or `;`")?;
            p.previous_end()
        };
        Done::new(SyntaxKind::FunctionDecl, Span::new(start, end))
    })
}

fn param_list(p: &mut Parser<'_>) -> Option<Parsed> {
    p.node(|p| {
        let start = p
            .expect(TokenKind::LParen, "expected `(` after function name")?
            .start;
        while !p.at(TokenKind::RParen) && !p.at(TokenKind::Eof) {
            if p.eat(TokenKind::Ellipsis).is_some() {
                break;
            }
            let checkpoint = p.checkpoint();
            if param(p).is_none() {
                if p.eat(TokenKind::Comma).is_some() {
                    continue;
                }
                if p.at(TokenKind::RParen) || p.at(TokenKind::LBrace) || p.at(TokenKind::Eof) {
                    break;
                }
                recover_to_list_boundary(p, checkpoint, TokenKind::RParen);
                continue;
            }
            if p.eat(TokenKind::Comma).is_none() {
                if p.at(TokenKind::RParen) || p.at(TokenKind::LBrace) || p.at(TokenKind::Eof) {
                    break;
                }
                let next = parameter_can_start(p);
                p.expected_here(
                    ParseErrorKind::Grammar,
                    "expected `,` or `)` after parameter",
                );
                if next {
                    continue;
                }
                skip_until(p, &[TokenKind::RParen, TokenKind::LBrace]);
                break;
            }
        }
        p.expect(TokenKind::RParen, "expected `)` after parameters")?;
        Done::new(SyntaxKind::ParamList, Span::new(start, p.previous_end()))
    })
}

fn parameter_can_start(p: &Parser<'_>) -> bool {
    p.at(TokenKind::Amp)
        || p.at(TokenKind::SelfValue)
        || (p.at(TokenKind::Ident) && matches!(p.nth_kind(1), Some(TokenKind::Colon)))
}

fn param(p: &mut Parser<'_>) -> Option<Parsed> {
    let checkpoint = p.checkpoint();
    let receiver = p.node(|p| {
        let start = p.span().start;
        if p.eat(TokenKind::Amp).is_some() {
            p.eat(TokenKind::Mut);
            p.eat(TokenKind::SelfValue)?;
        } else {
            p.eat(TokenKind::SelfValue)?;
        }
        Done::new(SyntaxKind::Param, Span::new(start, p.previous_end()))
    });
    if receiver.is_some() {
        return receiver;
    }
    p.rewind(checkpoint);
    p.node(|p| {
        let start = p.span().start;
        if !p.at(TokenKind::Amp) {
            p.expect_name("expected parameter name")?;
            p.expect(TokenKind::Colon, "expected `:` after parameter name")?;
        }
        let ty = types::param_type(p)?;
        Done::new(SyntaxKind::Param, Span::new(start, ty.span.end))
    })
}

pub(crate) fn binding_decl(
    p: &mut Parser<'_>,
    is_extern: bool,
    require_const_initializer: bool,
) -> Option<Parsed> {
    binding_decl_with(p, is_extern, require_const_initializer, None)
}

/// Parses a binding; `associated` validates an extension associated value
/// and carries whether a bodyless declaration is allowed.
fn binding_decl_with(
    p: &mut Parser<'_>,
    is_extern: bool,
    require_const_initializer: bool,
    associated: Option<bool>,
) -> Option<Parsed> {
    p.node(|p| {
        let start = p.span().start;
        let mut is_const = false;
        let mut has_type = false;
        let mut has_value = false;
        if p.eat(TokenKind::Const).is_some() {
            if is_extern {
                p.error_here("extern binding cannot be `const`");
                return None;
            }
            is_const = true;
            if p.at(TokenKind::Mut) {
                p.error_here("const bindings cannot be mutable");
                return None;
            }
        } else if p.eat(TokenKind::Static).is_some() {
            p.eat(TokenKind::Mut);
        } else if p.at(TokenKind::Let) {
            p.error_here("top-level storage declarations use `static`; `let` is local-only");
            return None;
        } else {
            p.error_here("expected `static` binding");
            return None;
        }
        p.expect_name("expected binding name")?;
        let mut anchor = None;
        if p.eat(TokenKind::Colon).is_some() {
            anchor = Some(types::type_until(p, &[TokenKind::Eq, TokenKind::Semicolon])?.span);
            has_type = true;
        }
        if p.eat(TokenKind::Eq).is_some() {
            if is_extern {
                p.error_here("extern binding cannot have an initializer");
                return None;
            }
            anchor = Some(exprs::expr_until(p, &[TokenKind::Semicolon])?.span);
            has_value = true;
        }
        if is_const && !has_value && require_const_initializer {
            p.error_here("const binding requires an initializer");
            return None;
        }
        if !is_extern && !has_value && !has_type {
            p.error_here("binding declaration requires an explicit type");
            return None;
        }
        if is_extern && !has_type {
            p.error_here("extern binding requires an explicit type");
            return None;
        }
        let anchor = anchor.unwrap_or_else(|| Span::new(p.previous_end(), p.previous_end()));
        p.expect_semicolon_after(anchor, "expected `;` after binding")?;
        let span = Span::new(start, p.previous_end());
        if let Some(allow_bodyless) = associated
            && is_const
            && !has_value
        {
            if !allow_bodyless {
                p.error_at(span, "const binding requires an initializer");
                return None;
            }
            if !has_type {
                p.error_at(
                    span,
                    "bodyless associated const declaration requires an explicit type",
                );
                return None;
            }
        }
        Done::new(SyntaxKind::BindingDecl, span)
    })
}
