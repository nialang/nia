// SPDX-License-Identifier: GPL-3.0-or-later
//! Syntax-tree walk that builds AST values and their origins.
//!
//! Every function takes a completed grammar node. Completed nodes contain
//! only well-formed children plus `Missing` markers and `Error` regions,
//! which lowering skips; the grammar already reported every problem in them.

mod exprs;
mod patterns;
mod stmts;
mod types;

use nia_ast::{
    Attribute, AttributeKind, AttributeMeta, BindingItem, ConditionBinaryOp, ConditionExpr,
    ConditionExprKind, ConditionUnaryOp, EnumItem, EnumVariant, EnumVariantPayload, ExtendItem,
    ExtendMethod, Field, FunctionItem, GenericParam, Item, ItemBindingKind, ItemKind, Module,
    ModuleItem, Param, PathSegmentKind, ProfileKind, ReceiverKind, StructItem, TraitItem,
    TraitMethod, TypeAliasItem, UnionItem, UsingGroupItem, UsingHostSegment, UsingItem, UsingName,
    UsingSelector, Visibility,
};
use nia_lexer::TokenKind;
use nia_node_id::{
    NodeOriginTable, NodeOriginTableBuilder, NodeStore, SyntaxKind as NodeSyntaxKind,
    VersionedNodeKey,
};
use nia_source::{SourceId, SourceRevision, SourceVersion};
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_identity_key};
use nia_symbol_table::{SymbolCollision, SymbolTable};
use nia_syntax::{
    GreenElement, GreenNode, GreenToken, Parse, ParseError, ParseErrorKind, SyntaxKind, SyntaxToken,
};

use crate::LoweredModule;

pub(crate) fn lower_module(
    parse: &Parse,
    node_store: &NodeStore,
    symbols: SymbolTable,
) -> LoweredModule {
    let tokens = parse.tree.tokens();
    let fallback_version = parse.tree.version().unwrap_or(SourceVersion {
        id: SourceId::isolated(),
        revision: SourceRevision::INITIAL,
    });
    let mut lower = Lower {
        parse,
        tokens,
        symbols,
        errors: parse.errors.clone(),
        origins: NodeOriginTable::builder(node_store),
        fallback_version,
    };
    let items = parse
        .tree
        .green_root()
        .child_nodes()
        .filter(|node| *node.kind() == SyntaxKind::Item)
        .filter_map(|node| lower.item(node))
        .collect();
    LoweredModule {
        module: Module { items },
        errors: lower.errors,
        origins: lower.origins.finish(),
    }
}

pub(crate) struct Lower<'a> {
    parse: &'a Parse,
    tokens: Vec<SyntaxToken>,
    symbols: SymbolTable,
    errors: Vec<ParseError>,
    origins: NodeOriginTableBuilder,
    fallback_version: SourceVersion,
}

// ---- tree access ------------------------------------------------------------

/// Direct significant tokens and child nodes in source order.
pub(crate) enum Child<'a> {
    Node(&'a GreenNode),
    Token(&'a GreenToken),
}

pub(crate) fn children(node: &GreenNode) -> impl Iterator<Item = Child<'_>> {
    node.children().iter().filter_map(|child| match child {
        GreenElement::Node(node) => Some(Child::Node(node)),
        GreenElement::Token(token) if !token.kind().is_trivia() => Some(Child::Token(token)),
        GreenElement::Token(_) => None,
    })
}

pub(crate) fn nodes_where(
    node: &GreenNode,
    predicate: impl Fn(&SyntaxKind) -> bool,
) -> impl Iterator<Item = &GreenNode> {
    node.child_nodes()
        .filter(move |child| predicate(child.kind()))
}

pub(crate) fn has_token(node: &GreenNode, kind: TokenKind) -> bool {
    node.token(&kind).is_some()
}

pub(crate) fn token_kind(token: &GreenToken) -> &TokenKind {
    token
        .kind()
        .token_kind()
        .unwrap_or_else(|| unreachable!("child_tokens yields significant tokens"))
}

/// Span of an AST value that starts at its first attribute when present.
///
/// Attribute recovery regions before the first accepted attribute do not
/// move the start; without accepted attributes the value starts at its first
/// non-attribute element.
pub(crate) fn attributed_span(node: &GreenNode) -> Span {
    let start = children(node)
        .find_map(|child| match child {
            Child::Node(node) if *node.kind() == SyntaxKind::Attribute => Some(node.span().start),
            Child::Node(node) if matches!(node.kind(), SyntaxKind::Error | SyntaxKind::Missing) => {
                None
            }
            Child::Node(node) => Some(node.span().start),
            Child::Token(token) => Some(token.span().start),
        })
        .unwrap_or(node.span().start);
    Span::new(start, node.span().end)
}

impl Lower<'_> {
    pub(crate) fn source(&self) -> &str {
        self.parse.tree.source()
    }

    pub(crate) fn text(&self, span: Span) -> String {
        self.source()
            .get(span.start..span.end)
            .unwrap_or("")
            .trim()
            .to_string()
    }

    pub(crate) fn alternate(
        &self,
        kind: nia_syntax::AlternateKind,
        span: Span,
    ) -> Option<&GreenNode> {
        self.parse.alternate(kind, span)
    }

    // ---- identity -----------------------------------------------------------

    pub(crate) fn node_key(&mut self, kind: NodeSyntaxKind, span: Span) -> VersionedNodeKey {
        let start = self
            .tokens
            .partition_point(|token| token.kind != TokenKind::Eof && token.span.end <= span.start);
        let end = self
            .tokens
            .partition_point(|token| token.kind != TokenKind::Eof && token.span.start < span.end);
        let first = self.tokens[start..]
            .iter()
            .find(|token| token.kind != TokenKind::Eof);
        let last = self.tokens[..end]
            .iter()
            .rev()
            .find(|token| token.kind != TokenKind::Eof);
        let key = match (first, last) {
            (Some(first), Some(last)) => VersionedNodeKey::child_path_range(
                first.source_version().unwrap_or(self.fallback_version),
                kind,
                first.child_path().clone(),
                last.child_path().clone(),
            ),
            _ => VersionedNodeKey::span(self.fallback_version, kind, span),
        };
        self.origins.insert(kind, span, key.clone());
        key
    }

    // ---- names --------------------------------------------------------------

    pub(crate) fn intern(&mut self, text: &str, span: Span) -> SymbolId {
        match self.symbols.intern(text) {
            Ok(symbol) => symbol,
            Err(SymbolCollision {
                symbol,
                existing,
                incoming,
            }) => {
                self.error(
                    span,
                    format!(
                        "symbol collision for {}: `{existing}` and `{incoming}`",
                        symbol_identity_key(symbol)
                    ),
                );
                symbol
            }
        }
    }

    pub(crate) fn name(&mut self, token: &GreenToken) -> SymbolId {
        match token_kind(token) {
            TokenKind::Bool => nia_symbol::known::BOOL,
            TokenKind::Char => nia_symbol::known::CHAR,
            TokenKind::Never => nia_symbol::known::NEVER,
            _ => self.intern(token.text(), token.span()),
        }
    }

    /// Interns the first direct identifier child.
    pub(crate) fn first_name(&mut self, node: &GreenNode) -> SymbolId {
        match node.token(&TokenKind::Ident) {
            Some(token) => self.name(token),
            None => SymbolId::EMPTY,
        }
    }

    pub(crate) fn path_segment_kind(&mut self, token: &GreenToken) -> PathSegmentKind {
        match token_kind(token) {
            TokenKind::Pkg => PathSegmentKind::Package,
            TokenKind::Super => PathSegmentKind::Super,
            TokenKind::SelfValue => PathSegmentKind::SelfValue,
            _ => PathSegmentKind::Name(self.name(token)),
        }
    }

    pub(crate) fn error(&mut self, span: Span, message: String) {
        self.errors.push(ParseError {
            span,
            kind: ParseErrorKind::Grammar,
            message,
            node_key: None,
        });
    }

    // ---- items --------------------------------------------------------------

    fn item(&mut self, node: &GreenNode) -> Option<Item> {
        let attributes = self.attributes(node);
        let vis = node
            .child(&SyntaxKind::Visibility)
            .map_or(Visibility::Private, visibility);
        let is_extern = has_token(node, TokenKind::Extern);
        let decl = node
            .child_nodes()
            .find(|child| child.kind().is_declaration())?;
        let kind = match decl.kind() {
            SyntaxKind::ModuleDecl => ItemKind::Module(ModuleItem {
                name: self.first_name(decl),
            }),
            SyntaxKind::UsingDecl => {
                ItemKind::Using(self.using_tree(decl.child(&SyntaxKind::UsingTree)?))
            }
            SyntaxKind::StructDecl => ItemKind::Struct(self.struct_decl(decl, is_extern)),
            SyntaxKind::UnionDecl => ItemKind::Union(self.union_decl(decl, is_extern)),
            SyntaxKind::TraitDecl => ItemKind::Trait(self.trait_decl(decl)),
            SyntaxKind::ExtendDecl => ItemKind::Extend(self.extend_decl(decl)?),
            SyntaxKind::EnumDecl => ItemKind::Enum(self.enum_decl(decl)),
            SyntaxKind::TypeAliasDecl => ItemKind::TypeAlias(self.type_alias_decl(decl)),
            SyntaxKind::FunctionDecl => ItemKind::Function(self.function(decl, is_extern)),
            SyntaxKind::BindingDecl => ItemKind::Binding(self.binding(decl, is_extern)),
            _ => return None,
        };
        let span = attributed_span(node);
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        Some(Item {
            span,
            node_key,
            attributes,
            vis,
            kind,
        })
    }

    pub(crate) fn attributes(&mut self, node: &GreenNode) -> Vec<Attribute> {
        nodes_where(node, |kind| *kind == SyntaxKind::Attribute)
            .filter_map(|attribute| self.attribute(attribute))
            .collect()
    }

    fn attribute(&mut self, node: &GreenNode) -> Option<Attribute> {
        let span = node.span();
        if has_token(node, TokenKind::If) {
            let condition = node
                .child_nodes()
                .find(|child| is_condition(child.kind()))?;
            return Some(Attribute {
                kind: AttributeKind::If(self.condition(condition)),
                span,
            });
        }
        let path = node
            .child_tokens()
            .filter(|token| token.is(&TokenKind::Ident))
            .map(|token| self.name(token))
            .collect::<Vec<_>>();
        let args = match node.child(&SyntaxKind::AttributeArgList) {
            Some(list) => nodes_where(list, SyntaxKind::is_expr)
                .map(|arg| self.expr(arg))
                .collect(),
            None => Vec::new(),
        };
        if args.is_empty() && path.len() == 1 {
            let profile = match path[0] {
                nia_symbol::known::DEBUG => Some(ProfileKind::Debug),
                nia_symbol::known::RELEASE => Some(ProfileKind::Release),
                _ => None,
            };
            if let Some(profile) = profile {
                return Some(Attribute {
                    kind: AttributeKind::Profile(profile),
                    span,
                });
            }
            if path[0] == nia_symbol::known::TEST {
                return Some(Attribute {
                    kind: AttributeKind::Test,
                    span,
                });
            }
        }
        Some(Attribute {
            kind: AttributeKind::Meta(AttributeMeta { path, args }),
            span,
        })
    }

    fn condition(&mut self, node: &GreenNode) -> ConditionExpr {
        let span = node.span();
        let kind = match node.kind() {
            SyntaxKind::ConditionParen => {
                return match node.child_nodes().find(|child| is_condition(child.kind())) {
                    Some(inner) => ConditionExpr {
                        span,
                        kind: self.condition(inner).kind,
                    },
                    None => ConditionExpr {
                        span,
                        kind: ConditionExprKind::Bool(false),
                    },
                };
            }
            SyntaxKind::ConditionNot => ConditionExprKind::Unary {
                op: ConditionUnaryOp::Not,
                expr: Box::new(self.child_condition(node, 0)),
            },
            SyntaxKind::ConditionBinary => {
                let op = match node.child_tokens().map(token_kind).next() {
                    Some(TokenKind::Or) => ConditionBinaryOp::Or,
                    Some(TokenKind::And) => ConditionBinaryOp::And,
                    Some(TokenKind::BangEq) => ConditionBinaryOp::Ne,
                    _ => ConditionBinaryOp::Eq,
                };
                ConditionExprKind::Binary {
                    lhs: Box::new(self.child_condition(node, 0)),
                    op,
                    rhs: Box::new(self.child_condition(node, 1)),
                }
            }
            _ => {
                let token = node.first_token();
                match token.map(token_kind) {
                    Some(TokenKind::True) => ConditionExprKind::Bool(true),
                    Some(TokenKind::False) => ConditionExprKind::Bool(false),
                    Some(TokenKind::Integer) => {
                        ConditionExprKind::Integer(token.map_or("", GreenToken::text).to_string())
                    }
                    Some(TokenKind::String) => {
                        ConditionExprKind::String(token.map_or("", GreenToken::text).to_string())
                    }
                    _ => match token {
                        Some(token) => ConditionExprKind::Ident(self.name(token)),
                        None => ConditionExprKind::Bool(false),
                    },
                }
            }
        };
        ConditionExpr { span, kind }
    }

    fn child_condition(&mut self, node: &GreenNode, index: usize) -> ConditionExpr {
        match node
            .child_nodes()
            .filter(|child| is_condition(child.kind()))
            .nth(index)
        {
            Some(child) => self.condition(child),
            None => ConditionExpr {
                span: node.span(),
                kind: ConditionExprKind::Bool(false),
            },
        }
    }

    // ---- using --------------------------------------------------------------

    pub(crate) fn using_tree(&mut self, node: &GreenNode) -> UsingItem {
        let host = node
            .child_tokens()
            .filter(|token| {
                matches!(
                    token_kind(token),
                    TokenKind::Ident | TokenKind::Pkg | TokenKind::Super | TokenKind::SelfValue
                )
            })
            .map(|token| UsingHostSegment {
                kind: self.path_segment_kind(token),
                span: token.span(),
            })
            .collect::<Vec<_>>();
        let selector = self.using_selector(node, host.is_empty());
        UsingItem { host, selector }
    }

    fn using_selector(&mut self, node: &GreenNode, _hostless: bool) -> UsingSelector {
        if let Some(star) = node.token(&TokenKind::Star) {
            return UsingSelector::Wildcard { span: star.span() };
        }
        if let Some(group) = node.child(&SyntaxKind::UsingGroup) {
            return UsingSelector::Group(self.using_group(group));
        }
        if let Some(name) = node.child(&SyntaxKind::UsingName) {
            return UsingSelector::Single(self.using_name(name));
        }
        UsingSelector::SelfName
    }

    fn using_group(&mut self, node: &GreenNode) -> Vec<UsingGroupItem> {
        node.child_nodes()
            .filter_map(|child| match child.kind() {
                SyntaxKind::UsingName => Some(UsingGroupItem::Name(self.using_name(child))),
                SyntaxKind::UsingTree => {
                    let tree = self.using_tree(child);
                    Some(UsingGroupItem::Nested {
                        host: tree.host,
                        selector: Box::new(tree.selector),
                    })
                }
                _ => None,
            })
            .collect()
    }

    fn using_name(&mut self, node: &GreenNode) -> UsingName {
        let mut idents = node
            .child_tokens()
            .filter(|token| token.is(&TokenKind::Ident));
        let name_token = idents.next();
        let alias_token = idents.next();
        UsingName {
            name: name_token.map_or(SymbolId::EMPTY, |token| self.name(token)),
            name_span: name_token.map_or(node.span(), GreenToken::span),
            alias: alias_token.map(|token| self.name(token)),
            alias_span: alias_token.map(GreenToken::span),
        }
    }

    // ---- aggregates ---------------------------------------------------------

    fn struct_decl(&mut self, node: &GreenNode, is_extern: bool) -> StructItem {
        let name = self.first_name(node);
        let generics = self.generic_params(node);
        let where_clause = self.where_clause(node);
        let (fields, is_tuple) = match node.child(&SyntaxKind::TupleFieldList) {
            Some(list) => (self.tuple_fields(list), true),
            None => (
                node.child(&SyntaxKind::FieldList)
                    .map_or_else(Vec::new, |list| self.fields(list)),
                false,
            ),
        };
        StructItem {
            name,
            generics,
            where_clause,
            fields,
            is_tuple,
            is_extern,
        }
    }

    fn union_decl(&mut self, node: &GreenNode, is_extern: bool) -> UnionItem {
        UnionItem {
            name: self.first_name(node),
            generics: self.generic_params(node),
            where_clause: self.where_clause(node),
            fields: node
                .child(&SyntaxKind::FieldList)
                .map_or_else(Vec::new, |list| self.fields(list)),
            is_extern,
        }
    }

    fn fields(&mut self, list: &GreenNode) -> Vec<Field> {
        nodes_where(list, |kind| *kind == SyntaxKind::Field)
            .filter_map(|field| self.field(field))
            .collect()
    }

    fn field(&mut self, node: &GreenNode) -> Option<Field> {
        let attributes = self.attributes(node);
        let name = self.first_name(node);
        let ty = self.child_type(node)?;
        let span = Span::new(attributed_span(node).start, ty.span.end);
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        Some(Field {
            name,
            ty,
            attributes,
            span,
            node_key,
        })
    }

    fn tuple_fields(&mut self, list: &GreenNode) -> Vec<Field> {
        let mut fields = Vec::new();
        for field in nodes_where(list, |kind| *kind == SyntaxKind::TupleField) {
            let Some(ty) = self.child_type(field) else {
                continue;
            };
            let span = Span::new(field.span().start, ty.span.end);
            let name = match self.symbols.intern(&fields.len().to_string()) {
                Ok(symbol) => symbol,
                Err(collision) => {
                    self.error(
                        span,
                        format!("failed to create positional field name: {collision}"),
                    );
                    SymbolId::EMPTY
                }
            };
            let node_key = self.node_key(NodeSyntaxKind::Item, span);
            fields.push(Field {
                name,
                ty,
                attributes: Vec::new(),
                span,
                node_key,
            });
        }
        fields
    }

    fn trait_decl(&mut self, node: &GreenNode) -> TraitItem {
        let name = self.first_name(node);
        let generics = self.generic_params(node);
        let supertraits = match node.child(&SyntaxKind::SupertraitList) {
            Some(list) => nodes_where(list, SyntaxKind::is_type)
                .map(|ty| self.type_ref(ty))
                .collect(),
            None => Vec::new(),
        };
        let where_clause = self.where_clause(node);
        let mut item = TraitItem {
            name,
            generics,
            supertraits,
            where_clause,
            associated_types: Vec::new(),
            associated_values: Vec::new(),
            methods: Vec::new(),
        };
        let Some(members) = node.child(&SyntaxKind::MemberList) else {
            return item;
        };
        for member in nodes_where(members, |kind| *kind == SyntaxKind::Member) {
            let attributes = self.attributes(member);
            for decl in member.child_nodes() {
                match decl.kind() {
                    SyntaxKind::AssocTypeDecl => {
                        let span = decl.span();
                        let node_key = self.node_key(NodeSyntaxKind::Item, span);
                        item.associated_types.push(nia_ast::TraitAssociatedType {
                            name: self.first_name(decl),
                            span,
                            node_key,
                        });
                    }
                    SyntaxKind::AssocConstDecl => {
                        let Some(ty) = self.child_type(decl) else {
                            continue;
                        };
                        let span = decl.span();
                        let name = self.first_name(decl);
                        let node_key = self.node_key(NodeSyntaxKind::Item, span);
                        item.associated_values.push(nia_ast::TraitAssociatedValue {
                            name,
                            ty,
                            span,
                            node_key,
                        });
                    }
                    SyntaxKind::FunctionDecl => item.methods.push(TraitMethod {
                        attributes: attributes.clone(),
                        function: self.function(decl, false),
                    }),
                    _ => {}
                }
            }
        }
        item
    }

    fn extend_decl(&mut self, node: &GreenNode) -> Option<ExtendItem> {
        let generics = self.generic_params(node);
        let mut types = nodes_where(node, SyntaxKind::is_type);
        let target = types.next()?;
        let trait_ref = types.next();
        let target = self.type_ref(target);
        let trait_ref = trait_ref.map(|ty| self.type_ref(ty));
        let where_clause = self.where_clause(node);
        let mut item = ExtendItem {
            generics,
            target,
            trait_ref,
            where_clause,
            associated_types: Vec::new(),
            associated_values: Vec::new(),
            methods: Vec::new(),
        };
        let Some(members) = node.child(&SyntaxKind::MemberList) else {
            return Some(item);
        };
        for member in nodes_where(members, |kind| *kind == SyntaxKind::Member) {
            let attributes = self.attributes(member);
            let vis = member
                .child(&SyntaxKind::Visibility)
                .map_or(Visibility::Private, visibility);
            for decl in member.child_nodes() {
                match decl.kind() {
                    SyntaxKind::AssocTypeDecl => {
                        let Some(ty) = self.child_type(decl) else {
                            continue;
                        };
                        let span = decl.span();
                        let name = self.first_name(decl);
                        let node_key = self.node_key(NodeSyntaxKind::Item, span);
                        item.associated_types.push(nia_ast::ExtendAssociatedType {
                            name,
                            ty,
                            span,
                            node_key,
                        });
                    }
                    SyntaxKind::BindingDecl => {
                        let binding = self.binding(decl, false);
                        item.associated_values.push(nia_ast::ExtendAssociatedValue {
                            vis,
                            binding,
                            span: decl.span(),
                        });
                    }
                    SyntaxKind::FunctionDecl => item.methods.push(ExtendMethod {
                        attributes: attributes.clone(),
                        vis,
                        function: self.function(decl, false),
                    }),
                    _ => {}
                }
            }
        }
        Some(item)
    }

    fn enum_decl(&mut self, node: &GreenNode) -> EnumItem {
        let name = self.first_name(node);
        let backing_type = self.child_type(node);
        let mut item = EnumItem {
            name,
            backing_type,
            is_open: false,
            variants: Vec::new(),
        };
        let Some(list) = node.child(&SyntaxKind::VariantList) else {
            return item;
        };
        for child in list.child_nodes() {
            match child.kind() {
                SyntaxKind::OpenVariant => item.is_open = true,
                SyntaxKind::Variant => {
                    let variant = self.variant(child);
                    item.variants.push(variant);
                }
                _ => {}
            }
        }
        item
    }

    fn variant(&mut self, node: &GreenNode) -> EnumVariant {
        let name = self.first_name(node);
        let payload = if let Some(list) = node.child(&SyntaxKind::TupleFieldList) {
            EnumVariantPayload::Tuple(
                nodes_where(list, |kind| *kind == SyntaxKind::TupleField)
                    .filter_map(|field| self.child_type(field))
                    .collect(),
            )
        } else if let Some(list) = node.child(&SyntaxKind::FieldList) {
            EnumVariantPayload::Named(self.fields(list))
        } else {
            EnumVariantPayload::Unit
        };
        let value = self.child_expr(node);
        let span = node.span();
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        EnumVariant {
            name,
            payload,
            value,
            span,
            node_key,
        }
    }

    fn type_alias_decl(&mut self, node: &GreenNode) -> TypeAliasItem {
        let name = node
            .child_tokens()
            .nth(1)
            .map_or(SymbolId::EMPTY, |token| self.name(token));
        TypeAliasItem {
            name,
            generics: self.generic_params(node),
            where_clause: self.where_clause(node),
            ty: self.child_type(node),
        }
    }

    // ---- functions and bindings ---------------------------------------------

    pub(crate) fn function(&mut self, node: &GreenNode, is_extern: bool) -> FunctionItem {
        let name = self.first_name(node);
        let generics = self.generic_params(node);
        let (params, is_variadic) = match node.child(&SyntaxKind::ParamList) {
            Some(list) => (self.params(list), has_token(list, TokenKind::Ellipsis)),
            None => (Vec::new(), false),
        };
        let return_type = self.child_type(node);
        let where_clause = self.where_clause(node);
        let body = node
            .child(&SyntaxKind::Block)
            .map(|block| self.block(block));
        let span = node.span();
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        FunctionItem {
            name,
            generics,
            where_clause,
            params,
            return_type,
            body,
            is_extern,
            is_const: has_token(node, TokenKind::Const),
            is_variadic,
            span,
            node_key,
        }
    }

    pub(crate) fn params(&mut self, list: &GreenNode) -> Vec<Param> {
        nodes_where(list, |kind| *kind == SyntaxKind::Param)
            .map(|param| self.param(param))
            .collect()
    }

    pub(crate) fn param(&mut self, node: &GreenNode) -> Param {
        let ty = self.child_type(node);
        let receiver = (ty.is_none() && has_token(node, TokenKind::SelfValue)).then(|| {
            if !has_token(node, TokenKind::Amp) {
                ReceiverKind::Value
            } else if has_token(node, TokenKind::Mut) {
                ReceiverKind::Ref
            } else {
                ReceiverKind::RefReadOnly
            }
        });
        let name = node.token(&TokenKind::Ident).map(|token| self.name(token));
        let span = node.span();
        let node_key = self.node_key(NodeSyntaxKind::Param, span);
        Param {
            receiver,
            name,
            ty,
            span,
            node_key,
        }
    }

    fn generic_params(&mut self, node: &GreenNode) -> Vec<GenericParam> {
        let Some(list) = node.child(&SyntaxKind::GenericParamList) else {
            return Vec::new();
        };
        nodes_where(list, |kind| *kind == SyntaxKind::GenericParam)
            .filter_map(|param| {
                let token = param.token(&TokenKind::Ident)?;
                let name = self.name(token);
                Some(match self.child_type(param) {
                    Some(ty) => GenericParam::const_param(name, token.span(), ty),
                    None => GenericParam::type_param(name, token.span()),
                })
            })
            .collect()
    }

    pub(crate) fn binding(&mut self, node: &GreenNode, is_extern: bool) -> BindingItem {
        let kind = if has_token(node, TokenKind::Const) {
            ItemBindingKind::Const
        } else {
            ItemBindingKind::Static {
                is_mutable: has_token(node, TokenKind::Mut),
                is_extern,
            }
        };
        let name = self.first_name(node);
        let ty = self.child_type(node);
        let value = self.child_expr(node);
        let node_key = self.node_key(NodeSyntaxKind::Item, node.span());
        BindingItem {
            name,
            ty,
            value,
            kind,
            node_key,
        }
    }
}

fn visibility(node: &GreenNode) -> Visibility {
    if has_token(node, TokenKind::Super) {
        Visibility::PublicSuper
    } else if has_token(node, TokenKind::Pkg) {
        Visibility::PublicPkg
    } else {
        Visibility::Public
    }
}

fn is_condition(kind: &SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::ConditionAtom
            | SyntaxKind::ConditionBinary
            | SyntaxKind::ConditionNot
            | SyntaxKind::ConditionParen
    )
}
