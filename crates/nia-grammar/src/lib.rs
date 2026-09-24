// SPDX-License-Identifier: GPL-3.0-or-later
//! Grammar recognition and recovery events for Nia source.
//!
//! Grammar owns parse decisions; `nia-syntax` owns green/red storage, while
//! `nia-parser` owns AST lowering. Migrated productions currently include
//! attributes, module declarations, and function declaration boundaries.

use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_source::SourceVersion;
use nia_span::Span;
use nia_syntax::{GreenBuildError, GreenEvent, SyntaxKind, SyntaxTree};

/// One error emitted while building the grammar tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarError {
    /// Location of the missing or malformed construct.
    pub span: Span,
    /// Grammar rule that rejected the input.
    pub message: String,
}

/// A lossless grammar tree and its grammar errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrammarTree {
    /// Green tree with borrowed red views.
    pub tree: SyntaxTree,
    /// Errors from productions already migrated to this crate.
    pub errors: Vec<GrammarError>,
}

/// Parses the supported grammar productions into one lossless source tree.
pub fn parse(source: &str, version: Option<SourceVersion>) -> Result<GrammarTree, GreenBuildError> {
    let tokens = tokenize_lossless(source);
    let significant = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| match token.kind {
            LosslessTokenKind::Token(_) => Some(index),
            LosslessTokenKind::Whitespace | LosslessTokenKind::LineComment => None,
        })
        .collect();
    let mut parser = GrammarParser {
        tokens,
        significant,
        position: 0,
        emitted: 0,
        events: vec![GreenEvent::Start(SyntaxKind::SourceFile)],
        errors: Vec::new(),
    };
    parser.parse_source();
    let tree = SyntaxTree::from_green_events(source, version, parser.events)?;
    Ok(GrammarTree {
        tree,
        errors: parser.errors,
    })
}

struct GrammarParser {
    tokens: Vec<LosslessToken>,
    significant: Vec<usize>,
    position: usize,
    emitted: usize,
    events: Vec<GreenEvent>,
    errors: Vec<GrammarError>,
}

impl GrammarParser {
    fn parse_source(&mut self) {
        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::At)
                && matches!(self.significant.get(self.position + 1), Some(&index) if matches!(self.tokens[index].kind, LosslessTokenKind::Token(TokenKind::LBracket)))
            {
                self.parse_attributes();
            } else if self.at(TokenKind::Module) {
                self.emit_trivia_before_current();
                self.parse_module_item();
            } else if self.at(TokenKind::Using) {
                self.parse_using_item();
            } else if self.is_function_start() {
                self.parse_function_item();
            } else if self.is_aggregate_start() {
                self.parse_aggregate_item();
            } else if self.is_type_alias_start() {
                self.parse_type_alias_item();
            } else if self.is_binding_start() {
                self.parse_binding_item();
            } else {
                self.parse_unmigrated_item();
            }
        }
        self.emit_until(self.tokens.len());
        self.events.push(GreenEvent::Finish);
    }

    fn parse_attributes(&mut self) {
        while self.at(TokenKind::At)
            && matches!(self.significant.get(self.position + 1), Some(&index) if matches!(self.tokens[index].kind, LosslessTokenKind::Token(TokenKind::LBracket)))
        {
            self.emit_trivia_before_current();
            self.events.push(GreenEvent::Start(SyntaxKind::Attribute));
            self.bump();
            self.bump();

            let mut delimiters = Vec::new();
            let mut closed = false;
            while !self.at(TokenKind::Eof) {
                let kind = self.current_kind();
                if delimiters.is_empty() && is_item_start(&kind) {
                    break;
                }
                match kind {
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                        delimiters.push(kind);
                        self.bump();
                    }
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                        if delimiters.last().is_some_and(|open| closes(open, &kind)) {
                            delimiters.pop();
                            self.bump();
                        } else if kind == TokenKind::RBracket && delimiters.is_empty() {
                            self.bump();
                            self.events.push(GreenEvent::Finish);
                            closed = true;
                            break;
                        } else {
                            self.bump();
                        }
                    }
                    _ => self.bump(),
                }
            }

            if closed {
                continue;
            }
            self.errors.push(GrammarError {
                span: self.current_span(),
                message: "expected `]` after attribute".into(),
            });
            self.events.push(GreenEvent::Start(SyntaxKind::Missing));
            self.events.push(GreenEvent::Finish);
            self.events.push(GreenEvent::Finish);
        }
    }

    fn parse_module_item(&mut self) {
        self.events.push(GreenEvent::Start(SyntaxKind::Module));
        self.bump();
        if self.at(TokenKind::Ident) {
            self.bump();
        } else {
            self.missing("expected module name");
        }
        if self.at(TokenKind::Semicolon) {
            self.bump();
        } else if self.at(TokenKind::Eof) || is_item_start(&self.current_kind()) {
            self.missing("expected `;` after module declaration");
        } else {
            self.events.push(GreenEvent::Start(SyntaxKind::Error));
            while !self.at(TokenKind::Semicolon)
                && !self.at(TokenKind::Eof)
                && !is_item_start(&self.current_kind())
            {
                self.bump();
            }
            self.events.push(GreenEvent::Finish);
            self.missing("expected `;` after module declaration");
            if self.at(TokenKind::Semicolon) {
                self.bump();
            }
        }
        self.events.push(GreenEvent::Finish);
    }

    fn parse_using_item(&mut self) {
        self.emit_trivia_before_current();
        self.events.push(GreenEvent::Start(SyntaxKind::Using));
        self.bump();
        let mut delimiters = Vec::new();
        while !self.at(TokenKind::Eof) {
            let kind = self.current_kind();
            if delimiters.is_empty() && kind == TokenKind::Semicolon {
                self.bump();
                self.events.push(GreenEvent::Finish);
                return;
            }
            if is_top_level_recovery_start(&kind) {
                self.missing("expected `;` after using declaration");
                while delimiters.pop().is_some() {
                    self.events.push(GreenEvent::Start(SyntaxKind::Missing));
                    self.events.push(GreenEvent::Finish);
                    self.events.push(GreenEvent::Finish);
                }
                self.events.push(GreenEvent::Finish);
                return;
            }
            match kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    self.events.push(GreenEvent::Start(SyntaxKind::Delimited {
                        open: kind.clone(),
                        close: None,
                    }));
                    delimiters.push(kind);
                }
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                    if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                {
                    delimiters.pop();
                    self.bump();
                    self.events.push(GreenEvent::Finish);
                    continue;
                }
                _ => {}
            }
            self.bump();
        }
        self.missing("expected `;` after using declaration");
        while delimiters.pop().is_some() {
            self.events.push(GreenEvent::Start(SyntaxKind::Missing));
            self.events.push(GreenEvent::Finish);
            self.events.push(GreenEvent::Finish);
        }
        self.events.push(GreenEvent::Finish);
    }

    fn parse_function_item(&mut self) {
        self.emit_trivia_before_current();
        self.events.push(GreenEvent::Start(SyntaxKind::Function));
        while matches!(
            self.current_kind(),
            TokenKind::Pub | TokenKind::Extern | TokenKind::Const
        ) {
            self.bump();
        }
        self.bump();
        if self.at(TokenKind::Ident) {
            self.bump();
        } else {
            self.missing("expected function name");
        }
        self.consume_balanced_until(&[TokenKind::LParen, TokenKind::LBrace, TokenKind::Semicolon]);
        if self.at(TokenKind::LParen) {
            self.parse_parameter_list();
        } else {
            self.missing("expected `(` after function name");
        }
        self.consume_balanced_until(&[TokenKind::LBrace, TokenKind::Semicolon]);
        if self.at(TokenKind::LBrace) {
            if !self.consume_balanced_group() {
                self.missing("expected `}` after function body");
            }
        } else if self.at(TokenKind::Semicolon) {
            self.bump();
        } else {
            self.missing("expected function body or `;`");
        }
        self.events.push(GreenEvent::Finish);
    }

    fn parse_type_alias_item(&mut self) {
        self.emit_trivia_before_current();
        self.events.push(GreenEvent::Start(SyntaxKind::TypeAlias));
        if self.at(TokenKind::Pub) {
            self.bump();
        }
        self.bump();
        if self.at(TokenKind::Ident) {
            self.bump();
        } else {
            self.missing("expected type alias name");
        }
        self.consume_angle_group();
        self.consume_until_terminator(TokenKind::Semicolon);
        self.events.push(GreenEvent::Finish);
    }

    fn parse_binding_item(&mut self) {
        self.emit_trivia_before_current();
        self.events.push(GreenEvent::Start(SyntaxKind::Binding));
        while matches!(
            self.current_kind(),
            TokenKind::Pub | TokenKind::Extern | TokenKind::Const | TokenKind::Static
        ) {
            self.bump();
        }
        self.consume_until_terminator(TokenKind::Semicolon);
        self.events.push(GreenEvent::Finish);
    }

    fn consume_until_terminator(&mut self, terminator: TokenKind) {
        let mut delimiters = Vec::new();
        while !self.at(TokenKind::Eof) {
            let kind = self.current_kind();
            if delimiters.is_empty() && kind == terminator {
                self.bump();
                return;
            }
            if delimiters.is_empty() && is_top_level_recovery_start(&kind) {
                self.missing("expected declaration terminator");
                return;
            }
            match kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    delimiters.push(kind);
                }
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                    if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                {
                    delimiters.pop();
                }
                _ => {}
            }
            self.bump();
        }
        self.missing("expected declaration terminator");
    }

    fn parse_aggregate_item(&mut self) {
        self.emit_trivia_before_current();
        let kind = match self.declaration_keyword() {
            Some(TokenKind::Struct) => SyntaxKind::Struct,
            Some(TokenKind::Union) => SyntaxKind::Union,
            Some(TokenKind::Enum) => SyntaxKind::Enum,
            Some(TokenKind::Trait) => SyntaxKind::Trait,
            Some(TokenKind::Extend) => SyntaxKind::Extend,
            _ => return,
        };
        self.events.push(GreenEvent::Start(kind.clone()));
        while matches!(self.current_kind(), TokenKind::Pub | TokenKind::Extern) {
            self.bump();
        }
        self.bump();
        if kind == SyntaxKind::Extend {
            // `extend` names its target in the header rather than declaring a
            // new nominal item name.
        } else if self.at(TokenKind::Ident) {
            self.bump();
        } else {
            self.missing("expected declaration name");
        }
        self.consume_angle_group();
        self.consume_header_until_body();
        match self.current_kind() {
            TokenKind::LBrace
                if matches!(
                    &kind,
                    SyntaxKind::Struct | SyntaxKind::Union | SyntaxKind::Enum
                ) =>
            {
                if !self.parse_braced_members(if kind == SyntaxKind::Enum {
                    SyntaxKind::Variant
                } else {
                    SyntaxKind::Field
                }) {
                    self.missing("expected `}` after declaration body");
                }
            }
            TokenKind::LBrace if matches!(&kind, SyntaxKind::Trait | SyntaxKind::Extend) => {
                if !self.parse_trait_members() {
                    self.missing("expected `}` after declaration body");
                }
            }
            TokenKind::LBrace | TokenKind::LParen => {
                if !self.consume_balanced_group() {
                    self.missing("expected closing declaration delimiter");
                }
            }
            TokenKind::Semicolon => self.bump(),
            TokenKind::Eof => self.missing("expected declaration body"),
            _ => {
                self.missing("expected declaration body");
                if is_item_start(&self.current_kind()) {
                    self.events.push(GreenEvent::Start(SyntaxKind::Error));
                    self.events.push(GreenEvent::Finish);
                }
            }
        }
        self.events.push(GreenEvent::Finish);
    }

    fn parse_braced_members(&mut self, member_kind: SyntaxKind) -> bool {
        self.events.push(GreenEvent::Start(SyntaxKind::Delimited {
            open: TokenKind::LBrace,
            close: None,
        }));
        self.bump();
        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::RBrace) {
                self.bump();
                self.events.push(GreenEvent::Finish);
                return true;
            }
            if is_outer_recovery_start(&self.current_kind()) {
                self.events.push(GreenEvent::Start(SyntaxKind::Missing));
                self.events.push(GreenEvent::Finish);
                self.events.push(GreenEvent::Finish);
                return false;
            }
            self.emit_trivia_before_current();
            self.events.push(GreenEvent::Start(member_kind.clone()));
            let mut delimiters = Vec::new();
            while !self.at(TokenKind::Eof) {
                let kind = self.current_kind();
                if delimiters.is_empty()
                    && (matches!(kind, TokenKind::Comma | TokenKind::RBrace)
                        || is_item_start(&kind))
                {
                    break;
                }
                match kind {
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                        delimiters.push(kind);
                    }
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                        if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                    {
                        delimiters.pop();
                    }
                    _ => {}
                }
                self.bump();
            }
            self.events.push(GreenEvent::Finish);
            if self.at(TokenKind::Comma) {
                self.bump();
            } else if self.at(TokenKind::RBrace) {
                continue;
            } else if self.at(TokenKind::Eof) {
                self.events.push(GreenEvent::Start(SyntaxKind::Missing));
                self.events.push(GreenEvent::Finish);
                self.events.push(GreenEvent::Finish);
                return false;
            }
        }
        self.events.push(GreenEvent::Start(SyntaxKind::Missing));
        self.events.push(GreenEvent::Finish);
        self.events.push(GreenEvent::Finish);
        false
    }

    fn parse_trait_members(&mut self) -> bool {
        self.events.push(GreenEvent::Start(SyntaxKind::Delimited {
            open: TokenKind::LBrace,
            close: None,
        }));
        self.bump();
        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::RBrace) {
                self.bump();
                self.events.push(GreenEvent::Finish);
                return true;
            }
            if is_outer_recovery_start(&self.current_kind()) {
                self.events.push(GreenEvent::Start(SyntaxKind::Missing));
                self.events.push(GreenEvent::Finish);
                self.events.push(GreenEvent::Finish);
                return false;
            }
            self.emit_trivia_before_current();
            self.events.push(GreenEvent::Start(SyntaxKind::Member));
            let mut delimiters = Vec::new();
            let mut consumed = false;
            while !self.at(TokenKind::Eof) {
                let kind = self.current_kind();
                if delimiters.is_empty()
                    && (matches!(kind, TokenKind::Semicolon | TokenKind::RBrace)
                        || (consumed && is_trait_member_start(&kind)))
                {
                    break;
                }
                match kind {
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                        delimiters.push(kind);
                    }
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                        if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                    {
                        delimiters.pop();
                    }
                    _ => {}
                }
                self.bump();
                consumed = true;
            }
            self.events.push(GreenEvent::Finish);
            if self.at(TokenKind::Semicolon) {
                self.bump();
            } else if self.at(TokenKind::Eof) {
                self.events.push(GreenEvent::Start(SyntaxKind::Missing));
                self.events.push(GreenEvent::Finish);
                self.events.push(GreenEvent::Finish);
                return false;
            }
        }
        self.events.push(GreenEvent::Start(SyntaxKind::Missing));
        self.events.push(GreenEvent::Finish);
        self.events.push(GreenEvent::Finish);
        false
    }

    fn consume_angle_group(&mut self) {
        if !self.at(TokenKind::Lt) {
            return;
        }
        self.bump();
        let mut depth = 1usize;
        while !self.at(TokenKind::Eof) && depth != 0 {
            match self.current_kind() {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.bump();
        }
    }

    fn consume_header_until_body(&mut self) {
        let mut angle_depth = 0usize;
        while !self.at(TokenKind::Eof) {
            let kind = self.current_kind();
            if angle_depth == 0
                && matches!(
                    kind,
                    TokenKind::LBrace | TokenKind::LParen | TokenKind::Semicolon
                )
            {
                return;
            }
            match kind {
                TokenKind::Lt => angle_depth += 1,
                TokenKind::Gt if angle_depth > 0 => angle_depth -= 1,
                _ => {}
            }
            self.bump();
        }
    }

    fn parse_parameter_list(&mut self) {
        self.events.push(GreenEvent::Start(SyntaxKind::Delimited {
            open: TokenKind::LParen,
            close: None,
        }));
        self.bump();
        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::RParen) {
                self.bump();
                self.events.push(GreenEvent::Finish);
                return;
            }
            if self.at(TokenKind::LBrace) {
                break;
            }
            if self.at(TokenKind::Comma) {
                self.bump();
                continue;
            }
            self.events.push(GreenEvent::Start(SyntaxKind::Param));
            let before = self.position;
            let mut delimiters = Vec::new();
            while !self.at(TokenKind::Eof) {
                let kind = self.current_kind();
                if delimiters.is_empty()
                    && matches!(
                        kind,
                        TokenKind::Comma | TokenKind::RParen | TokenKind::LBrace
                    )
                {
                    break;
                }
                match kind {
                    TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                        delimiters.push(kind);
                    }
                    TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                        if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                    {
                        delimiters.pop();
                    }
                    _ => {}
                }
                self.bump();
            }
            if self.position == before {
                self.missing("expected parameter");
            }
            self.events.push(GreenEvent::Finish);
        }
        self.missing("expected `)` after parameters");
        self.events.push(GreenEvent::Finish);
    }

    fn consume_balanced_until(&mut self, stops: &[TokenKind]) {
        while !self.at(TokenKind::Eof) && !stops.contains(&self.current_kind()) {
            self.bump();
        }
    }

    fn consume_balanced_group(&mut self) -> bool {
        let open = self.current_kind();
        self.bump();
        let mut delimiters = vec![open];
        while !self.at(TokenKind::Eof) && !delimiters.is_empty() {
            let kind = self.current_kind();
            if delimiters.len() == 1 && is_item_start(&kind) {
                return false;
            }
            match kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    delimiters.push(kind);
                }
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                    if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                {
                    delimiters.pop();
                }
                _ => {}
            }
            self.bump();
        }
        delimiters.is_empty()
    }

    fn parse_unmigrated_item(&mut self) {
        self.emit_trivia_before_current();
        self.events.push(GreenEvent::Start(SyntaxKind::Unparsed));
        let mut delimiters = Vec::new();
        while !self.at(TokenKind::Eof) {
            if delimiters.is_empty() && self.at(TokenKind::Module) {
                break;
            }
            let kind = self.current_kind();
            let mut closes_item = delimiters.is_empty() && kind == TokenKind::Semicolon;
            match kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => {
                    delimiters.push(kind);
                }
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
                    if delimiters.last().is_some_and(|open| closes(open, &kind)) =>
                {
                    delimiters.pop();
                    closes_item = delimiters.is_empty() && kind == TokenKind::RBrace;
                }
                _ => {}
            }
            self.bump();
            if closes_item {
                break;
            }
        }
        self.events.push(GreenEvent::Finish);
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.current_kind() == kind
    }

    fn is_function_start(&self) -> bool {
        let mut offset = 0;
        while matches!(
            self.kind_at(offset),
            Some(TokenKind::Pub | TokenKind::Extern | TokenKind::Const)
        ) {
            if self.kind_at(offset) == Some(TokenKind::Pub)
                && self.kind_at(offset + 1) == Some(TokenKind::LParen)
            {
                return false;
            }
            offset += 1;
        }
        self.kind_at(offset) == Some(TokenKind::Fn)
    }

    fn is_aggregate_start(&self) -> bool {
        matches!(
            self.declaration_keyword(),
            Some(
                TokenKind::Struct
                    | TokenKind::Union
                    | TokenKind::Enum
                    | TokenKind::Trait
                    | TokenKind::Extend,
            )
        )
    }

    fn is_type_alias_start(&self) -> bool {
        self.kind_at(0) == Some(TokenKind::Type)
            || (self.kind_at(0) == Some(TokenKind::Pub) && self.kind_at(1) == Some(TokenKind::Type))
    }

    fn is_binding_start(&self) -> bool {
        let mut offset = 0;
        while matches!(
            self.kind_at(offset),
            Some(TokenKind::Pub | TokenKind::Extern)
        ) {
            offset += 1;
        }
        match self.kind_at(offset) {
            Some(TokenKind::Static) => true,
            Some(TokenKind::Const) => self.kind_at(offset + 1) != Some(TokenKind::Fn),
            _ => false,
        }
    }

    fn declaration_keyword(&self) -> Option<TokenKind> {
        let mut offset = 0;
        while matches!(
            self.kind_at(offset),
            Some(TokenKind::Pub | TokenKind::Extern)
        ) {
            offset += 1;
        }
        self.kind_at(offset)
    }

    fn kind_at(&self, offset: usize) -> Option<TokenKind> {
        self.significant
            .get(self.position + offset)
            .and_then(|&index| match &self.tokens[index].kind {
                LosslessTokenKind::Token(kind) => Some(kind.clone()),
                LosslessTokenKind::Whitespace | LosslessTokenKind::LineComment => None,
            })
    }

    fn current_kind(&self) -> TokenKind {
        self.significant
            .get(self.position)
            .and_then(|&index| match &self.tokens[index].kind {
                LosslessTokenKind::Token(kind) => Some(kind.clone()),
                LosslessTokenKind::Whitespace | LosslessTokenKind::LineComment => None,
            })
            .unwrap_or(TokenKind::Eof)
    }

    fn current_span(&self) -> Span {
        self.significant
            .get(self.position)
            .map_or_else(|| Span::new(0, 0), |&index| self.tokens[index].span)
    }

    fn bump(&mut self) {
        let Some(&index) = self.significant.get(self.position) else {
            return;
        };
        self.emit_until(index + 1);
        self.position += 1;
    }

    fn emit_trivia_before_current(&mut self) {
        if let Some(&index) = self.significant.get(self.position) {
            self.emit_until(index);
        }
    }

    fn emit_until(&mut self, end: usize) {
        self.events.extend(
            self.tokens[self.emitted..end]
                .iter()
                .cloned()
                .map(GreenEvent::Token),
        );
        self.emitted = end;
    }

    fn missing(&mut self, message: &str) {
        self.errors.push(GrammarError {
            span: self.current_span(),
            message: message.into(),
        });
        self.events.push(GreenEvent::Start(SyntaxKind::Missing));
        self.events.push(GreenEvent::Finish);
    }
}

fn closes(open: &TokenKind, close: &TokenKind) -> bool {
    matches!(
        (open, close),
        (TokenKind::LParen, TokenKind::RParen)
            | (TokenKind::LBracket, TokenKind::RBracket)
            | (TokenKind::LBrace, TokenKind::RBrace)
    )
}

fn is_item_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Module
            | TokenKind::Using
            | TokenKind::Extern
            | TokenKind::Struct
            | TokenKind::Union
            | TokenKind::Trait
            | TokenKind::Extend
            | TokenKind::Enum
            | TokenKind::Type
            | TokenKind::Fn
            | TokenKind::Const
            | TokenKind::Static
            | TokenKind::Pub
    )
}

fn is_outer_recovery_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Module
            | TokenKind::Using
            | TokenKind::Extern
            | TokenKind::Struct
            | TokenKind::Union
            | TokenKind::Trait
            | TokenKind::Extend
            | TokenKind::Enum
    )
}

fn is_top_level_recovery_start(kind: &TokenKind) -> bool {
    is_outer_recovery_start(kind)
        || matches!(
            kind,
            TokenKind::Type | TokenKind::Fn | TokenKind::Const | TokenKind::Static | TokenKind::Pub
        )
}

fn is_trait_member_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Type | TokenKind::Const | TokenKind::Fn | TokenKind::Pub
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_items_have_grammar_nodes_and_trivia_remains_lossless() {
        let source = "// header\nmodule math;\nmodule util;\n";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.tree.full_text(), source);
        let items = parsed.tree.root().child_nodes();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.kind() == &SyntaxKind::Module));
    }

    #[test]
    fn missing_module_name_and_semicolon_create_missing_nodes() {
        let source = "module module next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.tree.full_text(), source);
        assert_eq!(parsed.errors.len(), 2);
        let items = parsed.tree.root().child_nodes();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0]
                .child_nodes()
                .iter()
                .filter(|child| child.kind() == &SyntaxKind::Missing)
                .count(),
            2
        );
    }

    #[test]
    fn function_items_have_grammar_nodes_without_false_errors() {
        let source = "pub fn main() () { module_name(); }\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.tree.full_text(), source);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Function);
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
        assert!(children[0].child_nodes().iter().any(|child| child.kind()
            == &SyntaxKind::Delimited {
                open: TokenKind::LParen,
                close: Some(TokenKind::RParen),
            }));
    }

    #[test]
    fn attributes_are_grammar_nodes_and_do_not_hide_following_items() {
        let source = "@[test]\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.tree.full_text(), source);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Attribute);
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn unterminated_attribute_has_missing_node_and_recovers_at_item() {
        let source = "@[test\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.errors.len(), 1);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Attribute);
        assert!(
            children[0]
                .child_nodes()
                .iter()
                .any(|child| child.kind() == &SyntaxKind::Missing)
        );
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn function_items_are_kept_at_top_level_boundaries() {
        let source = "fn first() {}\nfn second();\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 3);
        assert!(
            children[..2]
                .iter()
                .all(|child| child.kind() == &SyntaxKind::Function)
        );
        assert_eq!(children[2].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn function_recovery_keeps_later_items_visible() {
        let source = "fn broken(value { 0 }\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.errors.len(), 1);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Function);
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
        assert!(children[0].child_nodes().iter().any(|child| {
            child.kind()
                == &SyntaxKind::Delimited {
                    open: TokenKind::LParen,
                    close: None,
                }
                && child
                    .child_nodes()
                    .iter()
                    .any(|nested| nested.kind() == &SyntaxKind::Missing)
        }));
    }

    #[test]
    fn aggregate_headers_have_distinct_nodes_and_preserve_following_items() {
        let source = "struct Point<T> { x: T }\nunion Value { i: i32 }\nenum Color: u8 { Red }\ntrait Display: Show { }\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 5);
        assert_eq!(children[0].kind(), &SyntaxKind::Struct);
        assert_eq!(children[1].kind(), &SyntaxKind::Union);
        assert_eq!(children[2].kind(), &SyntaxKind::Enum);
        assert_eq!(children[3].kind(), &SyntaxKind::Trait);
        assert_eq!(children[4].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn aggregate_header_recovery_keeps_later_items_visible() {
        let source = "struct { x: i32 }\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.errors.len(), 1);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Struct);
        assert!(
            children[0]
                .child_nodes()
                .iter()
                .any(|child| child.kind() == &SyntaxKind::Missing)
        );
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn unterminated_aggregate_body_recovers_at_following_item() {
        let source = "struct Point { x: i32\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.errors.len(), 1);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Struct);
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn aggregate_bodies_have_field_and_variant_nodes() {
        let source = "struct Point { x: i32, y: i32 }\nenum Color { Red, Green }";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let children = parsed.tree.root().child_nodes();
        let struct_body = children[0]
            .child_nodes()
            .into_iter()
            .find(|child| {
                matches!(
                    child.kind(),
                    SyntaxKind::Delimited {
                        open: TokenKind::LBrace,
                        ..
                    }
                )
            })
            .expect("struct body");
        assert_eq!(
            struct_body
                .child_nodes()
                .iter()
                .filter(|child| child.kind() == &SyntaxKind::Field)
                .count(),
            2
        );
        let enum_body = children[1]
            .child_nodes()
            .into_iter()
            .find(|child| {
                matches!(
                    child.kind(),
                    SyntaxKind::Delimited {
                        open: TokenKind::LBrace,
                        ..
                    }
                )
            })
            .expect("enum body");
        assert_eq!(
            enum_body
                .child_nodes()
                .iter()
                .filter(|child| child.kind() == &SyntaxKind::Variant)
                .count(),
            2
        );
    }

    #[test]
    fn trait_and_extend_bodies_have_member_nodes() {
        let source = "trait Display { type Output; const FLAG: bool; fn show(); }\nextend Point { fn show() {} }\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 3);
        for declaration in &children[..2] {
            let body = declaration
                .child_nodes()
                .into_iter()
                .find(|child| {
                    matches!(
                        child.kind(),
                        SyntaxKind::Delimited {
                            open: TokenKind::LBrace,
                            ..
                        }
                    )
                })
                .expect("member body");
            assert!(
                !body
                    .child_nodes()
                    .iter()
                    .filter(|child| child.kind() == &SyntaxKind::Member)
                    .collect::<Vec<_>>()
                    .is_empty()
            );
        }
        assert_eq!(children[2].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn unterminated_trait_body_recovers_at_following_item() {
        let source = "trait Display { fn show();\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.errors.len(), 1);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Trait);
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn using_declarations_have_grammar_nodes_and_preserve_groups() {
        let source = "using math::{Vec, Result as R};\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Using);
        assert!(children[0].child_nodes().iter().any(|child| matches!(
            child.kind(),
            SyntaxKind::Delimited {
                open: TokenKind::LBrace,
                ..
            }
        )));
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn unterminated_using_recovers_at_following_item() {
        let source = "using math::{Vec, Result\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.errors.len(), 1);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Using);
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn type_aliases_and_bindings_have_declaration_nodes() {
        let source = "type Word = u32;\nconst LIMIT: usize = 4;\nstatic ready: bool;\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 4);
        assert_eq!(children[0].kind(), &SyntaxKind::TypeAlias);
        assert_eq!(children[1].kind(), &SyntaxKind::Binding);
        assert_eq!(children[2].kind(), &SyntaxKind::Binding);
        assert_eq!(children[3].kind(), &SyntaxKind::Module);
    }

    #[test]
    fn unterminated_binding_recovers_at_following_item() {
        let source = "const LIMIT: usize = 4\nmodule next;";
        let parsed = parse(source, None).expect("valid event stream");
        assert_eq!(parsed.errors.len(), 1);
        let children = parsed.tree.root().child_nodes();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].kind(), &SyntaxKind::Binding);
        assert_eq!(children[1].kind(), &SyntaxKind::Module);
    }
}
