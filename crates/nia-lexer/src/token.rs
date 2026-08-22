// SPDX-License-Identifier: GPL-3.0-or-later
use nia_span::Span;

/// Significant lexical token with a UTF-8 byte span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Token classification.
    pub kind: TokenKind,
    /// Half-open byte span in source text.
    pub span: Span,
}

/// Significant token or trivia element with its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LosslessToken {
    /// Token or trivia classification.
    pub kind: LosslessTokenKind,
    /// Half-open byte span in source text.
    pub span: Span,
}

/// Elements retained by lossless tokenization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LosslessTokenKind {
    /// Significant language token.
    Token(TokenKind),
    /// Contiguous whitespace trivia.
    Whitespace,
    /// Line-comment trivia.
    LineComment,
}

/// Significant lexical token kinds in Nia source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// Identifier.
    Ident,
    /// Integer literal.
    Integer,
    /// Floating-point literal.
    Float,
    /// String literal.
    String,
    /// Byte-string literal.
    ByteString,
    /// Character literal.
    Char,
    /// Byte-character literal.
    ByteChar,

    /// `and` keyword.
    And,
    /// `as` keyword.
    As,
    /// `bool` keyword.
    Bool,
    /// `break` keyword.
    Break,
    /// `const` keyword.
    Const,
    /// `continue` keyword.
    Continue,
    /// `defer` keyword.
    Defer,
    /// `else` keyword.
    Else,
    /// `enum` keyword.
    Enum,
    /// `extern` keyword.
    Extern,
    /// `extend` keyword.
    Extend,
    /// `false` keyword.
    False,
    /// `fn` keyword.
    Fn,
    /// `for` keyword.
    For,
    /// `if` keyword.
    If,
    /// `in` keyword.
    In,
    /// `is` keyword.
    Is,
    /// `let` keyword.
    Let,
    /// `loop` keyword.
    Loop,
    /// `module` keyword.
    Module,
    /// `mut` keyword.
    Mut,
    /// `never` keyword.
    Never,
    /// `not` keyword.
    Not,
    /// `null` keyword.
    Null,
    /// `opaque` keyword.
    Opaque,
    /// `or` keyword.
    Or,
    /// `pub` keyword.
    Pub,
    /// `pkg` keyword.
    Pkg,
    /// `return` keyword.
    Return,
    /// `self` value keyword.
    SelfValue,
    /// `struct` keyword.
    Struct,
    /// `Self` type keyword.
    SelfType,
    /// `static` keyword.
    Static,
    /// `super` keyword.
    Super,
    /// `match` keyword.
    Match,
    /// `true` keyword.
    True,
    /// `trait` keyword.
    Trait,
    /// `type` keyword.
    Type,
    /// `union` keyword.
    Union,
    /// `using` keyword.
    Using,
    /// `var` keyword.
    Var,
    /// `where` keyword.
    Where,
    /// `while` keyword.
    While,

    /// `(` delimiter.
    LParen,
    /// `)` delimiter.
    RParen,
    /// `{` delimiter.
    LBrace,
    /// `}` delimiter.
    RBrace,
    /// `[` delimiter.
    LBracket,
    /// `]` delimiter.
    RBracket,
    /// `,` punctuation.
    Comma,
    /// `.` punctuation.
    Dot,
    /// `..` punctuation.
    DotDot,
    /// `..=` punctuation.
    DotDotEq,
    /// `...` punctuation.
    Ellipsis,
    /// `:` punctuation.
    Colon,
    /// `::` punctuation.
    ColonColon,
    /// `;` punctuation.
    Semicolon,
    /// `@` punctuation.
    At,
    /// Backslash punctuation.
    Backslash,
    /// `?` punctuation.
    Question,
    /// Standalone `_` token.
    Underscore,

    /// `+` operator.
    Plus,
    /// `-` operator.
    Minus,
    /// `*` operator.
    Star,
    /// `/` operator.
    Slash,
    /// `%` operator.
    Percent,
    /// `&` operator.
    Amp,
    /// `|` operator.
    Pipe,
    /// `^` operator.
    Caret,
    /// `~` operator.
    Tilde,
    /// `!` operator.
    Bang,
    /// `=` operator.
    Eq,
    /// `<` operator.
    Lt,
    /// `>` operator.
    Gt,
    /// `<<` operator.
    LtLt,
    /// `>>` operator.
    GtGt,

    /// `+=` operator.
    PlusEq,
    /// `-=` operator.
    MinusEq,
    /// `*=` operator.
    StarEq,
    /// `/=` operator.
    SlashEq,
    /// `%=` operator.
    PercentEq,
    /// `&=` operator.
    AmpEq,
    /// `|=` operator.
    PipeEq,
    /// `^=` operator.
    CaretEq,
    /// `==` operator.
    EqEq,
    /// `!=` operator.
    BangEq,
    /// `<=` operator.
    LtEq,
    /// `>=` operator.
    GtEq,
    /// `<<=` operator.
    LtLtEq,
    /// `>>=` operator.
    GtGtEq,
    /// `=>` punctuation.
    FatArrow,
    /// `->` punctuation.
    ThinArrow,

    /// Terminal end-of-file token.
    Eof,
    /// Recoverable lexical error token.
    Error(LexError),
}

/// Lexical failures represented in-band as error tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexError {
    /// Unexpected ASCII byte or leading byte of an unsupported Unicode scalar.
    UnexpectedByte(u8),
    /// String literal reached end of input before closing.
    UnterminatedString,
    /// Character literal reached end of input before closing.
    UnterminatedChar,
    /// Character literal contains no scalar value.
    EmptyChar,
    /// Byte-character literal contains a non-byte value.
    InvalidByteChar,
    /// Numeric literal has invalid syntax.
    InvalidNumber,
    /// String literal contains an invalid escape.
    InvalidStringEscape,
    /// Character literal contains an invalid escape.
    InvalidCharEscape,
}
