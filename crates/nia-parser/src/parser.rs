// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    ArrayElements, ArrayLen, AssignOp, Attribute, BinaryOp, BindingItem, BindingStmt, Block,
    BracketArg, ComptimeIfExpr, ComptimeIfItem, ComptimeIfItemElse, EnumItem, EnumVariant, Expr,
    ExprKind, ExprStub, ExtendItem, ExtendMethod, Field, FieldInit, ForBinding, ForInStmt,
    FunctionItem, ImportItem, ImportPath, ImportPathKind, Item, ItemKind, LoopStmt, Module, Param,
    ReceiverKind, Stmt, StmtKind, StringLiteral, StructItem, SwitchArm, SwitchArmBody,
    SwitchPattern, SwitchStmt, TraitAssociatedType, TraitItem, TraitMethod, TypeAliasItem, TypeArg,
    TypeKind, TypePathSegment, TypeRef, UnaryOp, UnionItem, UsingGroupItem, UsingHostSegment,
    UsingItem, UsingName, UsingSelector, Visibility, WhereClause, WherePredicate, WhileStmt,
};
use nia_lexer::TokenKind;
use nia_node_id::{NodeKey, NodeOriginTable, SyntaxKind as NodeSyntaxKind};
use nia_source::{SourceId, SourceRevision, SourceVersion};
use nia_span::Span;
use nia_syntax::{SyntaxToken, SyntaxTokenCursor, SyntaxTree};

mod expr;
mod items;
mod stmt;
mod types;

fn synthetic_source_version() -> SourceVersion {
    SourceVersion {
        id: SourceId(u32::MAX),
        revision: SourceRevision::INITIAL,
    }
}

pub fn parse_module(source: &str) -> (Module, Vec<ParseError>) {
    let syntax = nia_syntax::parse_source(source, Some(synthetic_source_version()));
    parse_module_syntax(&syntax)
}

pub fn parse_module_syntax(syntax: &SyntaxTree) -> (Module, Vec<ParseError>) {
    let (module, errors, _) = parse_module_syntax_with_origins(syntax);
    (module, errors)
}

pub fn parse_module_syntax_with_origins(
    syntax: &SyntaxTree,
) -> (Module, Vec<ParseError>, NodeOriginTable) {
    Parser::from_syntax(syntax).parse_module()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
    pub node_key: Option<NodeKey>,
}

pub struct Parser {
    source: String,
    tokens: SyntaxTokenCursor,
    errors: Vec<ParseError>,
    origins: NodeOriginTable,
}

impl Parser {
    pub fn new(source: &str) -> Self {
        let syntax = nia_syntax::parse_source(source, Some(synthetic_source_version()));
        Self::from_syntax(&syntax)
    }

    fn from_syntax(syntax: &SyntaxTree) -> Self {
        let tokens = SyntaxTokenCursor::new(syntax);
        let errors = tokens
            .tokens()
            .iter()
            .filter_map(|token| match &token.kind {
                TokenKind::Error(error) => Some(ParseError {
                    span: token.span,
                    message: format!("lex error: {error:?}"),
                    node_key: token.node_key(),
                }),
                _ => None,
            })
            .collect();
        Self {
            source: syntax.source().to_string(),
            tokens,
            errors,
            origins: NodeOriginTable::default(),
        }
    }

    pub fn parse_module(mut self) -> (Module, Vec<ParseError>, NodeOriginTable) {
        let mut items = Vec::new();
        while !self.at(TokenKind::Eof) {
            if let Some(item) = self.parse_item() {
                items.push(item);
            } else {
                self.recover_to_item_boundary();
            }
        }
        (Module { items }, self.errors, self.origins)
    }

    fn node_key(&mut self, kind: NodeSyntaxKind, span: Span) -> NodeKey {
        let start = self.tokens.token_at_or_after(span.start);
        let end = self.tokens.token_before_or_at(span.end);
        let (Some(start), Some(end)) = (start, end) else {
            panic!("parser produced {kind:?} AST node without syntax tokens at {span:?}");
        };
        let Some(version) = start.source_version() else {
            panic!("parser produced {kind:?} AST node from unversioned syntax at {span:?}");
        };
        assert_eq!(
            end.source_version(),
            Some(version),
            "parser produced {kind:?} AST node spanning multiple source versions at {span:?}"
        );
        let key = NodeKey::child_path_range(
            version,
            kind,
            start.child_path().clone(),
            end.child_path().clone(),
        );
        self.origins.insert(kind, span, key.clone());
        key
    }

    fn make_item(
        &mut self,
        span: Span,
        attributes: Vec<Attribute>,
        vis: Visibility,
        kind: ItemKind,
    ) -> Item {
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        Item {
            span,
            node_key,
            attributes,
            vis,
            kind,
        }
    }

    fn make_param(
        &mut self,
        span: Span,
        receiver: Option<ReceiverKind>,
        name: Option<String>,
        ty: Option<TypeRef>,
    ) -> Param {
        let node_key = self.node_key(NodeSyntaxKind::Param, span);
        Param {
            receiver,
            name,
            ty,
            span,
            node_key,
        }
    }

    fn make_function(
        &mut self,
        name: String,
        generics: Vec<String>,
        where_clause: WhereClause,
        params: Vec<Param>,
        return_type: Option<TypeRef>,
        body: Option<Block>,
        is_extern: bool,
        is_comptime: bool,
        is_variadic: bool,
        span: Span,
    ) -> FunctionItem {
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        FunctionItem {
            name,
            generics,
            where_clause,
            params,
            return_type,
            body,
            is_extern,
            is_comptime,
            is_variadic,
            span,
            node_key,
        }
    }

    fn make_field(
        &mut self,
        name: String,
        ty: TypeRef,
        attributes: Vec<Attribute>,
        span: Span,
    ) -> Field {
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        Field {
            name,
            ty,
            attributes,
            span,
            node_key,
        }
    }

    fn make_trait_associated_type(&mut self, name: String, span: Span) -> TraitAssociatedType {
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        TraitAssociatedType {
            name,
            span,
            node_key,
        }
    }

    fn make_extend_associated_type(
        &mut self,
        name: String,
        ty: TypeRef,
        span: Span,
    ) -> nia_ast::ExtendAssociatedType {
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        nia_ast::ExtendAssociatedType {
            name,
            ty,
            span,
            node_key,
        }
    }

    fn make_enum_variant(&mut self, name: String, value: Option<Expr>, span: Span) -> EnumVariant {
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        EnumVariant {
            name,
            value,
            span,
            node_key,
        }
    }

    fn make_binding(
        &mut self,
        name: String,
        ty: Option<TypeRef>,
        value: Option<Expr>,
        is_let: bool,
        is_comptime: bool,
        is_extern: bool,
        span: Span,
    ) -> BindingItem {
        let node_key = self.node_key(NodeSyntaxKind::Item, span);
        BindingItem {
            name,
            ty,
            value,
            is_let,
            is_comptime,
            is_extern,
            node_key,
        }
    }

    fn make_type_ref(&mut self, span: Span, kind: TypeKind) -> TypeRef {
        let node_key = self.node_key(NodeSyntaxKind::Type, span);
        TypeRef {
            span,
            node_key,
            text: self.source_text(span),
            kind,
        }
    }

    fn make_expr(&mut self, span: Span, kind: ExprKind) -> Expr {
        let node_key = self.node_key(NodeSyntaxKind::Expr, span);
        Expr {
            span,
            node_key,
            kind,
        }
    }

    fn make_stmt(&mut self, span: Span, kind: StmtKind) -> Stmt {
        let node_key = self.node_key(NodeSyntaxKind::Stmt, span);
        Stmt {
            span,
            node_key,
            kind,
        }
    }

    fn parse_expr_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let expr = self.parse_expr_until_tokens(stops);
        if expr.is_some() {
            return expr;
        }
        let span = self.collect_until(stops)?;
        Some(self.make_expr(span, ExprKind::Raw(self.source_text(span))))
    }

    fn collect_until(&mut self, stops: &[TokenKind]) -> Option<Span> {
        let start = self.peek().span.start;
        let mut depth = 0usize;
        while !self.at(TokenKind::Eof) {
            if depth == 0 && stops.iter().any(|kind| self.at(kind.clone())) {
                break;
            }
            match self.peek().kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            self.bump();
        }
        let end = self.previous_end();
        if start >= end {
            self.error_here("expected syntax");
            None
        } else {
            Some(Span::new(start, end))
        }
    }

    fn expect(&mut self, kind: TokenKind, message: &str) -> Option<Span> {
        if self.at(kind) {
            Some(self.bump().span)
        } else {
            self.error_here(message);
            None
        }
    }

    fn expect_semicolon_after(&mut self, anchor: Span, message: &str) -> Option<Span> {
        if self.at(TokenKind::Semicolon) {
            Some(self.bump().span)
        } else {
            self.error_at_end(anchor, message);
            None
        }
    }

    fn expect_text(&mut self, kind: TokenKind, message: &str) -> Option<String> {
        if self.at(kind) {
            let token = self.bump();
            Some(self.token_text(&token).to_string())
        } else {
            self.error_here(message);
            None
        }
    }

    fn eat(&mut self, kind: TokenKind) -> Option<SyntaxToken> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            None
        }
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.tokens.at(kind)
    }

    fn at_comptime_if(&self) -> bool {
        self.at(TokenKind::Comptime) && matches!(self.tokens.nth_kind(1), Some(TokenKind::If))
    }

    fn at_comptime_fn(&self) -> bool {
        self.at(TokenKind::Comptime) && matches!(self.tokens.nth_kind(1), Some(TokenKind::Fn))
    }

    fn bump(&mut self) -> SyntaxToken {
        self.tokens.bump()
    }

    fn peek(&self) -> &SyntaxToken {
        self.tokens.peek()
    }

    fn previous_end(&self) -> usize {
        self.tokens.previous_end()
    }

    fn token_text<'token>(&self, token: &'token SyntaxToken) -> &'token str {
        &token.text
    }

    fn source_text(&self, span: Span) -> String {
        self.source
            .get(span.start..span.end)
            .unwrap_or("")
            .trim()
            .to_string()
    }

    fn error_here(&mut self, message: impl Into<String>) {
        self.errors.push(ParseError {
            span: self.peek().span,
            message: message.into(),
            node_key: self.peek().node_key(),
        });
    }

    fn error_at(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(ParseError {
            span,
            message: message.into(),
            node_key: None,
        });
    }

    fn error_at_end(&mut self, span: Span, message: impl Into<String>) {
        let start = span.end.saturating_sub(1).max(span.start);
        self.error_at(Span::new(start, span.end), message);
    }

    fn recover_to_item_boundary(&mut self) {
        while !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Semicolon).is_some() {
                return;
            }
            if self.at(TokenKind::At)
                && matches!(self.tokens.nth_kind(1), Some(TokenKind::LBracket))
            {
                return;
            }
            if matches!(
                self.peek().kind,
                TokenKind::Import
                    | TokenKind::Extern
                    | TokenKind::Struct
                    | TokenKind::Enum
                    | TokenKind::Type
                    | TokenKind::Fn
                    | TokenKind::Let
                    | TokenKind::Var
                    | TokenKind::Pub
            ) {
                return;
            }
            self.bump();
        }
    }

    fn recover_to_member_boundary(&mut self) {
        while !self.at(TokenKind::Eof) && !self.at(TokenKind::RBrace) {
            if self.eat(TokenKind::Comma).is_some() || self.eat(TokenKind::Semicolon).is_some() {
                return;
            }
            if self.at(TokenKind::At)
                && matches!(self.tokens.nth_kind(1), Some(TokenKind::LBracket))
            {
                return;
            }
            self.bump();
        }
    }

    fn recover_to_stmt_boundary(&mut self) {
        while !self.at(TokenKind::Eof) && !self.at(TokenKind::RBrace) {
            if self.eat(TokenKind::Semicolon).is_some() {
                return;
            }
            self.bump();
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
