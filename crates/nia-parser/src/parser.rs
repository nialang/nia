// SPDX-License-Identifier: GPL-3.0-or-later
use nia_ast::{
    ArrayElements, ArrayLen, AssignOp, BinaryOp, BindingItem, BindingStmt, Block, BracketArg,
    EnumItem, EnumVariant, Expr, ExprKind, ExprStub, ExtendItem, ExtendMethod, Field, FieldInit,
    ForHeader, ForInit, ForStmt, FunctionItem, ImportItem, ImportPath, ImportPathKind, Item,
    ItemKind, Module, Param, ReceiverKind, Stmt, StmtKind, StringLiteral, StructItem, SwitchArm,
    SwitchArmBody, SwitchPattern, SwitchStmt, TypeAliasItem, TypeArg, TypeKind, TypePathSegment,
    TypeRef, UnaryOp, UnionItem, UsingGroupItem, UsingHostSegment, UsingItem, UsingName,
    UsingSelector, Visibility,
};
use nia_lexer::{Token, TokenKind};
use nia_span::Span;
use nia_syntax::SyntaxTree;

mod expr;
mod items;
mod stmt;
mod types;

pub fn parse_module(source: &str) -> (Module, Vec<ParseError>) {
    let syntax = nia_syntax::parse_source(source, None);
    parse_module_syntax(&syntax)
}

pub fn parse_module_syntax(syntax: &SyntaxTree) -> (Module, Vec<ParseError>) {
    Parser::from_tokens(syntax.source(), syntax.semantic_tokens()).parse_module()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
}

pub struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Self {
        let syntax = nia_syntax::parse_source(source, None);
        Self::from_tokens(source, syntax.semantic_tokens())
    }

    fn from_tokens(source: &'a str, tokens: Vec<Token>) -> Self {
        let errors = tokens
            .iter()
            .filter_map(|token| match &token.kind {
                TokenKind::Error(error) => Some(ParseError {
                    span: token.span,
                    message: format!("lex error: {error:?}"),
                }),
                _ => None,
            })
            .collect();
        Self {
            source,
            tokens,
            pos: 0,
            errors,
        }
    }

    pub fn parse_module(mut self) -> (Module, Vec<ParseError>) {
        let mut items = Vec::new();
        while !self.at(TokenKind::Eof) {
            if let Some(item) = self.parse_item() {
                items.push(item);
            } else {
                self.recover_to_item_boundary();
            }
        }
        (Module { items }, self.errors)
    }

    fn has_top_level_semicolon_before_lbrace(&self) -> bool {
        let mut depth = 0usize;
        let mut index = self.pos;
        while let Some(token) = self.tokens.get(index) {
            match token.kind {
                TokenKind::LBrace if depth == 0 => return false,
                TokenKind::Semicolon if depth == 0 => return true,
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            index += 1;
        }
        false
    }

    fn parse_expr_until(&mut self, stops: &[TokenKind]) -> Option<Expr> {
        let expr = self.parse_expr();
        if expr.is_some() {
            return expr;
        }
        let span = self.collect_until(stops)?;
        Some(Expr {
            span,
            kind: ExprKind::Raw(self.source_text(span)),
        })
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

    fn eat(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            None
        }
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    fn bump(&mut self) -> Token {
        let token = self.peek().clone();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn previous_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        }
    }

    fn token_text(&self, token: &Token) -> &str {
        self.source
            .get(token.span.start..token.span.end)
            .unwrap_or("")
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
        });
    }

    fn error_at(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(ParseError {
            span,
            message: message.into(),
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
            if matches!(
                self.peek().kind,
                TokenKind::Import
                    | TokenKind::Extern
                    | TokenKind::Struct
                    | TokenKind::Enum
                    | TokenKind::Type
                    | TokenKind::Fn
                    | TokenKind::Const
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
