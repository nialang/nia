// SPDX-License-Identifier: GPL-3.0-or-later
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LosslessToken {
    pub kind: LosslessTokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LosslessTokenKind {
    Token(TokenKind),
    Whitespace,
    LineComment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident,
    Integer,
    Float,
    String,
    ByteString,
    CString,
    Char,
    ByteChar,

    And,
    As,
    Bool,
    Break,
    Comptime,
    Const,
    Continue,
    Defer,
    Else,
    Enum,
    Extern,
    Extend,
    False,
    Fn,
    For,
    If,
    Import,
    In,
    Loop,
    Mut,
    Or,
    Pub,
    Return,
    Struct,
    SelfType,
    Switch,
    True,
    Trait,
    Type,
    Union,
    Using,
    Var,
    Void,
    Where,
    While,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    DotDot,
    DotDotEq,
    Ellipsis,
    Colon,
    ColonColon,
    Semicolon,
    At,
    Underscore,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Bang,
    Eq,
    Lt,
    Gt,
    LtLt,
    GtGt,

    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    EqEq,
    BangEq,
    LtEq,
    GtEq,
    LtLtEq,
    GtGtEq,
    FatArrow,

    Eof,
    Error(LexError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexError {
    UnexpectedByte(u8),
    UnterminatedString,
    UnterminatedChar,
    EmptyChar,
    InvalidByteChar,
    InvalidNumber,
    InvalidStringEscape,
    InvalidCharEscape,
}
