// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{LexError, LosslessToken, LosslessTokenKind, Token, TokenKind};
use nia_span::Span;

pub fn tokenize(source: &str) -> Vec<Token> {
    Tokenizer::new(source).tokenize()
}

pub fn tokenize_lossless(source: &str) -> Vec<LosslessToken> {
    Tokenizer::new(source).tokenize_lossless()
}

pub struct Tokenizer<'a> {
    source: &'a [u8],
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
        }
    }

    pub fn tokenize(mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            let token = self.next_token();
            let is_eof = token.kind == TokenKind::Eof;
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        tokens
    }

    pub fn tokenize_lossless(mut self) -> Vec<LosslessToken> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_lossless_token();
            let is_eof = matches!(token.kind, LosslessTokenKind::Token(TokenKind::Eof));
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        tokens
    }

    fn next_lossless_token(&mut self) -> LosslessToken {
        let start = self.pos;
        let Some(byte) = self.peek() else {
            return self.lossless_token(LosslessTokenKind::Token(TokenKind::Eof), start, start);
        };
        if byte.is_ascii_whitespace() {
            self.consume_whitespace();
            return self.lossless_token(LosslessTokenKind::Whitespace, start, self.pos);
        }
        if byte == b'/' && self.peek_next() == Some(b'/') {
            self.consume_line_comment();
            return self.lossless_token(LosslessTokenKind::LineComment, start, self.pos);
        }
        let token = self.next_token();
        LosslessToken {
            kind: LosslessTokenKind::Token(token.kind),
            span: token.span,
        }
    }

    fn next_token(&mut self) -> Token {
        let start = self.pos;
        let Some(byte) = self.bump() else {
            return self.token(TokenKind::Eof, start, start);
        };

        match byte {
            b'b' if self.peek() == Some(b'"') => {
                self.bump();
                self.string(start, TokenKind::ByteString)
            }
            b'b' if self.peek() == Some(b'\\') && self.peek_next() == Some(b'\\') => {
                self.bump();
                self.multiline_string(start, TokenKind::ByteString)
            }
            b'b' if self.peek() == Some(b'\'') => {
                self.bump();
                self.char_lit(start, true)
            }
            b'a'..=b'z' | b'A'..=b'Z' => self.ident_or_keyword(start),
            b'_' => {
                if self.peek().is_some_and(is_ident_continue) {
                    self.ident_or_keyword(start)
                } else {
                    self.token(TokenKind::Underscore, start, self.pos)
                }
            }
            b'0'..=b'9' => self.number(start),
            b'"' => self.string(start, TokenKind::String),
            b'\\' if self.peek() == Some(b'\\') => self.multiline_string(start, TokenKind::String),
            b'\'' => self.char_lit(start, false),
            b'(' => self.token(TokenKind::LParen, start, self.pos),
            b')' => self.token(TokenKind::RParen, start, self.pos),
            b'{' => self.token(TokenKind::LBrace, start, self.pos),
            b'}' => self.token(TokenKind::RBrace, start, self.pos),
            b'[' => self.token(TokenKind::LBracket, start, self.pos),
            b']' => self.token(TokenKind::RBracket, start, self.pos),
            b',' => self.token(TokenKind::Comma, start, self.pos),
            b'.' => {
                if self.peek() == Some(b'.') && self.peek_next() == Some(b'.') {
                    self.bump();
                    self.bump();
                    self.token(TokenKind::Ellipsis, start, self.pos)
                } else if self.eat(b'.') {
                    if self.eat(b'=') {
                        self.token(TokenKind::DotDotEq, start, self.pos)
                    } else {
                        self.token(TokenKind::DotDot, start, self.pos)
                    }
                } else {
                    self.token(TokenKind::Dot, start, self.pos)
                }
            }
            b';' => self.token(TokenKind::Semicolon, start, self.pos),
            b'@' => self.token(TokenKind::At, start, self.pos),
            b'?' => self.token(TokenKind::Question, start, self.pos),
            b':' => {
                if self.eat(b':') {
                    self.token(TokenKind::ColonColon, start, self.pos)
                } else {
                    self.token(TokenKind::Colon, start, self.pos)
                }
            }
            b'+' => self.maybe_eq(start, TokenKind::Plus, TokenKind::PlusEq),
            b'-' => self.maybe_eq(start, TokenKind::Minus, TokenKind::MinusEq),
            b'*' => self.maybe_eq(start, TokenKind::Star, TokenKind::StarEq),
            b'/' => self.maybe_eq(start, TokenKind::Slash, TokenKind::SlashEq),
            b'%' => self.maybe_eq(start, TokenKind::Percent, TokenKind::PercentEq),
            b'^' => self.maybe_eq(start, TokenKind::Caret, TokenKind::CaretEq),
            b'~' => self.token(TokenKind::Tilde, start, self.pos),
            b'!' => self.maybe_eq(start, TokenKind::Bang, TokenKind::BangEq),
            b'=' => {
                if self.eat(b'=') {
                    self.token(TokenKind::EqEq, start, self.pos)
                } else if self.eat(b'>') {
                    self.token(TokenKind::FatArrow, start, self.pos)
                } else {
                    self.token(TokenKind::Eq, start, self.pos)
                }
            }
            b'<' => {
                if self.eat(b'<') {
                    self.maybe_eq(start, TokenKind::LtLt, TokenKind::LtLtEq)
                } else if self.eat(b'=') {
                    self.token(TokenKind::LtEq, start, self.pos)
                } else {
                    self.token(TokenKind::Lt, start, self.pos)
                }
            }
            b'>' => {
                if self.eat(b'>') {
                    self.maybe_eq(start, TokenKind::GtGt, TokenKind::GtGtEq)
                } else if self.eat(b'=') {
                    self.token(TokenKind::GtEq, start, self.pos)
                } else {
                    self.token(TokenKind::Gt, start, self.pos)
                }
            }
            b'&' => {
                if self.eat(b'=') {
                    self.token(TokenKind::AmpEq, start, self.pos)
                } else {
                    self.token(TokenKind::Amp, start, self.pos)
                }
            }
            b'|' => {
                if self.eat(b'=') {
                    self.token(TokenKind::PipeEq, start, self.pos)
                } else {
                    self.token(TokenKind::Pipe, start, self.pos)
                }
            }
            other => self.token(
                TokenKind::Error(LexError::UnexpectedByte(other)),
                start,
                self.pos,
            ),
        }
    }

    fn ident_or_keyword(&mut self, start: usize) -> Token {
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        let text = std::str::from_utf8(&self.source[start..self.pos]).unwrap_or("");
        let kind = match text {
            "and" => TokenKind::And,
            "as" => TokenKind::As,
            "bool" => TokenKind::Bool,
            "break" => TokenKind::Break,
            "comptime" => TokenKind::Comptime,
            "continue" => TokenKind::Continue,
            "defer" => TokenKind::Defer,
            "else" => TokenKind::Else,
            "enum" => TokenKind::Enum,
            "extern" => TokenKind::Extern,
            "extend" => TokenKind::Extend,
            "false" => TokenKind::False,
            "fn" => TokenKind::Fn,
            "for" => TokenKind::For,
            "if" => TokenKind::If,
            "in" => TokenKind::In,
            "let" => TokenKind::Let,
            "loop" => TokenKind::Loop,
            "module" => TokenKind::Module,
            "mut" => TokenKind::Mut,
            "never" => TokenKind::Never,
            "not" => TokenKind::Not,
            "null" => TokenKind::Null,
            "or" => TokenKind::Or,
            "pkg" => TokenKind::Pkg,
            "pub" => TokenKind::Pub,
            "return" => TokenKind::Return,
            "Self" => TokenKind::SelfType,
            "static" => TokenKind::Static,
            "struct" => TokenKind::Struct,
            "switch" => TokenKind::Switch,
            "trait" => TokenKind::Trait,
            "true" => TokenKind::True,
            "type" => TokenKind::Type,
            "union" => TokenKind::Union,
            "using" => TokenKind::Using,
            "void" => TokenKind::Void,
            "where" => TokenKind::Where,
            "while" => TokenKind::While,
            _ => TokenKind::Ident,
        };
        self.token(kind, start, self.pos)
    }

    fn number(&mut self, start: usize) -> Token {
        if self.source.get(start) == Some(&b'0') && matches!(self.peek(), Some(b'x' | b'X')) {
            self.bump();
            let (digits, invalid) = self.consume_digits(16);
            if digits == 0 || invalid {
                return self.token(TokenKind::Error(LexError::InvalidNumber), start, self.pos);
            }
            self.consume_numeric_suffix();
            return self.token(TokenKind::Integer, start, self.pos);
        }
        if self.source.get(start) == Some(&b'0') && matches!(self.peek(), Some(b'b' | b'B')) {
            self.bump();
            let (digits, invalid) = self.consume_digits(2);
            if digits == 0 || invalid {
                return self.token(TokenKind::Error(LexError::InvalidNumber), start, self.pos);
            }
            self.consume_numeric_suffix();
            return self.token(TokenKind::Integer, start, self.pos);
        }
        if self.source.get(start) == Some(&b'0') && matches!(self.peek(), Some(b'o' | b'O')) {
            self.bump();
            let (digits, invalid) = self.consume_digits(8);
            if digits == 0 || invalid {
                return self.token(TokenKind::Error(LexError::InvalidNumber), start, self.pos);
            }
            self.consume_numeric_suffix();
            return self.token(TokenKind::Integer, start, self.pos);
        }
        self.consume_digits(10);
        if self.peek() == Some(b'.') && self.peek_next().is_some_and(|b| b.is_ascii_digit()) {
            self.bump();
            self.consume_digits(10);
            if self.try_scan_exponent() == Some(false) {
                return self.token(TokenKind::Error(LexError::InvalidNumber), start, self.pos);
            }
            self.consume_numeric_suffix();
            return self.token(TokenKind::Float, start, self.pos);
        }
        if let Some(valid) = self.try_scan_exponent() {
            if !valid {
                return self.token(TokenKind::Error(LexError::InvalidNumber), start, self.pos);
            }
            self.consume_numeric_suffix();
            return self.token(TokenKind::Float, start, self.pos);
        }
        self.consume_numeric_suffix();
        self.token(TokenKind::Integer, start, self.pos)
    }

    fn try_scan_exponent(&mut self) -> Option<bool> {
        if !matches!(self.peek(), Some(b'e' | b'E')) {
            return None;
        }
        self.bump();
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.bump();
        }
        let (digits, invalid) = self.consume_digits(10);
        Some(digits > 0 && !invalid)
    }

    fn consume_digits(&mut self, radix: u32) -> (usize, bool) {
        let mut digits = 0usize;
        let mut invalid = false;
        while let Some(byte) = self.peek() {
            if byte == b'_' {
                self.bump();
            } else if digit_value(byte).is_some_and(|value| value < radix) {
                digits += 1;
                self.bump();
            } else if radix != 10 && digit_value(byte).is_some() {
                invalid = true;
                self.bump();
            } else {
                break;
            }
        }
        (digits, invalid)
    }

    fn consume_numeric_suffix(&mut self) {
        if !self.peek().is_some_and(is_ident_start) {
            return;
        }
        self.bump();
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
    }

    fn string(&mut self, start: usize, success_kind: TokenKind) -> Token {
        let mut invalid_escape = false;
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.bump();
                    let kind = if invalid_escape {
                        TokenKind::Error(LexError::InvalidStringEscape)
                    } else {
                        success_kind
                    };
                    return self.token(kind, start, self.pos);
                }
                b'\n' | b'\r' => {
                    return self.token(
                        TokenKind::Error(LexError::UnterminatedString),
                        start,
                        self.pos,
                    );
                }
                b'\\' => {
                    self.bump();
                    if self.scan_escape().flatten().is_none() {
                        invalid_escape = true;
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
        self.token(
            TokenKind::Error(LexError::UnterminatedString),
            start,
            self.pos,
        )
    }

    fn multiline_string(&mut self, start: usize, success_kind: TokenKind) -> Token {
        self.bump();
        loop {
            while self
                .peek()
                .is_some_and(|byte| byte != b'\n' && byte != b'\r')
            {
                self.bump();
            }

            let line_end = self.pos;
            let Some(newline_end) = self.consume_newline() else {
                return self.token(success_kind, start, line_end);
            };

            let mut next_line = newline_end;
            while matches!(self.source.get(next_line), Some(b' ' | b'\t')) {
                next_line += 1;
            }

            if self.source.get(next_line) == Some(&b'\\')
                && self.source.get(next_line + 1) == Some(&b'\\')
            {
                self.pos = next_line + 2;
            } else {
                self.pos = line_end;
                return self.token(success_kind, start, line_end);
            }
        }
    }

    fn char_lit(&mut self, start: usize, is_byte: bool) -> Token {
        let value = match self.scan_char_value() {
            CharScan::Value(value) => value,
            CharScan::Empty => {
                self.bump();
                return self.token(TokenKind::Error(LexError::EmptyChar), start, self.pos);
            }
            CharScan::Unterminated => {
                return self.token(
                    TokenKind::Error(LexError::UnterminatedChar),
                    start,
                    self.pos,
                );
            }
            CharScan::InvalidEscape => {
                self.recover_char_literal();
                return self.token(
                    TokenKind::Error(LexError::InvalidCharEscape),
                    start,
                    self.pos,
                );
            }
        };

        if self.bump() != Some(b'\'') {
            self.recover_char_literal();
            return self.token(
                TokenKind::Error(LexError::UnterminatedChar),
                start,
                self.pos,
            );
        }

        let kind = if is_byte {
            if value.byte_len != 1 || value.value > u8::MAX as u32 {
                TokenKind::Error(LexError::InvalidByteChar)
            } else {
                TokenKind::ByteChar
            }
        } else if char::from_u32(value.value).is_some() {
            TokenKind::Char
        } else {
            TokenKind::Error(LexError::InvalidCharEscape)
        };
        self.token(kind, start, self.pos)
    }

    fn scan_char_value(&mut self) -> CharScan {
        let Some(first) = self.peek() else {
            return CharScan::Unterminated;
        };
        match first {
            b'\'' => CharScan::Empty,
            b'\n' | b'\r' => CharScan::Unterminated,
            b'\\' => {
                self.bump();
                match self.scan_escape() {
                    Some(Some(value)) => CharScan::Value(value),
                    Some(None) => CharScan::InvalidEscape,
                    None => CharScan::Unterminated,
                }
            }
            _ => self.scan_utf8_codepoint(),
        }
    }

    fn scan_utf8_codepoint(&mut self) -> CharScan {
        let start = self.pos;
        let Some(first) = self.bump() else {
            return CharScan::Unterminated;
        };
        let width = utf8_width(first);
        if width == 0 || self.pos + width.saturating_sub(1) > self.source.len() {
            return CharScan::InvalidEscape;
        }
        for _ in 1..width {
            let Some(byte) = self.bump() else {
                return CharScan::Unterminated;
            };
            if (byte & 0b1100_0000) != 0b1000_0000 {
                return CharScan::InvalidEscape;
            }
        }
        let text = std::str::from_utf8(&self.source[start..self.pos]);
        let Ok(text) = text else {
            return CharScan::InvalidEscape;
        };
        let mut chars = text.chars();
        let Some(ch) = chars.next() else {
            return CharScan::InvalidEscape;
        };
        if chars.next().is_some() {
            return CharScan::InvalidEscape;
        }
        CharScan::Value(CharValue {
            value: ch as u32,
            byte_len: width,
        })
    }

    fn scan_escape(&mut self) -> Option<Option<CharValue>> {
        let escaped = self.bump()?;
        let value = match escaped {
            b'n' => Some(CharValue::one_byte(b'\n' as u32)),
            b'r' => Some(CharValue::one_byte(b'\r' as u32)),
            b't' => Some(CharValue::one_byte(b'\t' as u32)),
            b'\\' => Some(CharValue::one_byte(b'\\' as u32)),
            b'\'' => Some(CharValue::one_byte(b'\'' as u32)),
            b'"' => Some(CharValue::one_byte(b'"' as u32)),
            b'0' => Some(CharValue::one_byte(0)),
            b'x' => self.consume_exact_hex_digits(2).map(CharValue::one_byte),
            b'u' => self.scan_unicode_escape(),
            _ => None,
        };
        Some(value)
    }

    fn scan_unicode_escape(&mut self) -> Option<CharValue> {
        if self.bump() != Some(b'{') {
            return None;
        }
        let digits_start = self.pos;
        let mut digits = 0usize;
        while self.peek().is_some_and(|b| digit_value(b).is_some()) {
            self.bump();
            digits += 1;
        }
        if digits == 0 || digits > 6 {
            return None;
        }
        if self.bump() != Some(b'}') {
            return None;
        }
        let text = std::str::from_utf8(&self.source[digits_start..self.pos - 1]).ok()?;
        let value = u32::from_str_radix(text, 16).ok()?;
        char::from_u32(value).map(|ch| CharValue {
            value: ch as u32,
            byte_len: ch.len_utf8(),
        })
    }

    fn consume_exact_hex_digits(&mut self, count: usize) -> Option<u32> {
        let start = self.pos;
        for _ in 0..count {
            let byte = self.peek()?;
            if digit_value(byte).is_some_and(|value| value < 16) {
                self.bump();
            } else {
                return None;
            }
        }
        let text = std::str::from_utf8(&self.source[start..self.pos]).ok()?;
        u32::from_str_radix(text, 16).ok()
    }

    fn recover_char_literal(&mut self) {
        while let Some(byte) = self.peek() {
            if byte == b'\'' {
                self.bump();
                break;
            }
            if byte == b'\n' || byte == b'\r' {
                break;
            }
            self.bump();
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            if self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
                self.consume_whitespace();
                continue;
            }
            if self.peek() == Some(b'/') && self.peek_next() == Some(b'/') {
                self.consume_line_comment();
                continue;
            }
            break;
        }
    }

    fn consume_whitespace(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.bump();
        }
    }

    fn consume_line_comment(&mut self) {
        while self.peek().is_some_and(|b| b != b'\n') {
            self.bump();
        }
    }

    fn maybe_eq(&mut self, start: usize, plain: TokenKind, eq: TokenKind) -> Token {
        if self.eat(b'=') {
            self.token(eq, start, self.pos)
        } else {
            self.token(plain, start, self.pos)
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn consume_newline(&mut self) -> Option<usize> {
        match self.peek()? {
            b'\n' => {
                self.bump();
                Some(self.pos)
            }
            b'\r' => {
                self.bump();
                if self.peek() == Some(b'\n') {
                    self.bump();
                }
                Some(self.pos)
            }
            _ => None,
        }
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.source.get(self.pos).copied()?;
        self.pos += 1;
        Some(byte)
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<u8> {
        self.source.get(self.pos + 1).copied()
    }

    fn token(&self, kind: TokenKind, start: usize, end: usize) -> Token {
        Token {
            kind,
            span: Span::new(start, end),
        }
    }

    fn lossless_token(&self, kind: LosslessTokenKind, start: usize, end: usize) -> LosslessToken {
        LosslessToken {
            kind,
            span: Span::new(start, end),
        }
    }
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 0,
    }
}

struct CharValue {
    value: u32,
    byte_len: usize,
}

impl CharValue {
    const fn one_byte(value: u32) -> Self {
        Self { value, byte_len: 1 }
    }
}

enum CharScan {
    Value(CharValue),
    Empty,
    Unterminated,
    InvalidEscape,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        tokenize(source)
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn tokenizes_core_syntax() {
        assert_eq!(
            kinds("pub fn main() i32 { let mut x = ~1; x += 2; x <<= 1; y >> 2; math::add(); }"),
            vec![
                TokenKind::Pub,
                TokenKind::Fn,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Ident,
                TokenKind::LBrace,
                TokenKind::Let,
                TokenKind::Mut,
                TokenKind::Ident,
                TokenKind::Eq,
                TokenKind::Tilde,
                TokenKind::Integer,
                TokenKind::Semicolon,
                TokenKind::Ident,
                TokenKind::PlusEq,
                TokenKind::Integer,
                TokenKind::Semicolon,
                TokenKind::Ident,
                TokenKind::LtLtEq,
                TokenKind::Integer,
                TokenKind::Semicolon,
                TokenKind::Ident,
                TokenKind::GtGt,
                TokenKind::Integer,
                TokenKind::Semicolon,
                TokenKind::Ident,
                TokenKind::ColonColon,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Semicolon,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_literals_and_comments() {
        assert_eq!(
            kinds("\"nia\\0\" b\"nia\" b'a' '中' // comment\n@size[usize]()"),
            vec![
                TokenKind::String,
                TokenKind::ByteString,
                TokenKind::ByteChar,
                TokenKind::Char,
                TokenKind::At,
                TokenKind::Ident,
                TokenKind::LBracket,
                TokenKind::Ident,
                TokenKind::RBracket,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_multiline_string_literals() {
        assert_eq!(
            kinds("\\\\text\nb\\\\bytes\nlet mut x = 1;"),
            vec![
                TokenKind::String,
                TokenKind::ByteString,
                TokenKind::Let,
                TokenKind::Mut,
                TokenKind::Ident,
                TokenKind::Eq,
                TokenKind::Integer,
                TokenKind::Semicolon,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn tokenizes_numeric_forms_and_ellipsis() {
        assert_eq!(
            kinds("0xff 0b1010 0o755 1.5 1e-3 10usize 1.0f32 ... .. ..="),
            vec![
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::Float,
                TokenKind::Float,
                TokenKind::Integer,
                TokenKind::Float,
                TokenKind::Ellipsis,
                TokenKind::DotDot,
                TokenKind::DotDotEq,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn reports_invalid_numeric_forms_as_single_tokens() {
        assert_eq!(
            kinds("0x 0b102 0o89 1e+"),
            vec![
                TokenKind::Error(LexError::InvalidNumber),
                TokenKind::Error(LexError::InvalidNumber),
                TokenKind::Error(LexError::InvalidNumber),
                TokenKind::Error(LexError::InvalidNumber),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn validates_string_and_char_escapes() {
        assert_eq!(
            kinds(r#""\n\x41\u{4e2d}" b"\xff" '\u{4e2d}' b'\xff'"#),
            vec![
                TokenKind::String,
                TokenKind::ByteString,
                TokenKind::Char,
                TokenKind::ByteChar,
                TokenKind::Eof,
            ]
        );

        assert_eq!(
            kinds(r#""\q" '\q' b'中' b'\u{80}' ''"#),
            vec![
                TokenKind::Error(LexError::InvalidStringEscape),
                TokenKind::Error(LexError::InvalidCharEscape),
                TokenKind::Error(LexError::InvalidByteChar),
                TokenKind::Error(LexError::InvalidByteChar),
                TokenKind::Error(LexError::EmptyChar),
                TokenKind::Eof,
            ]
        );
    }
}
