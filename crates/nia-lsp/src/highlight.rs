// SPDX-License-Identifier: GPL-3.0-or-later
//! Semantic token classification and UTF-16 encoding.

use std::collections::HashMap;

use lsp_types::{SemanticToken, SemanticTokens, SemanticTokensEdit, SemanticTokensLegend};
use nia_defs::DefKind;
use nia_lexer::{LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_span::Span;
use nia_syntax::{GreenElement, GreenNode, SyntaxKind};
use nia_ty::PrimitiveTypeSpelling;

use crate::document::Document;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Namespace,
    Type,
    Struct,
    Enum,
    Interface,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    EnumMember,
    Function,
    Method,
    Keyword,
    Comment,
    String,
    Number,
    Operator,
    Decorator,
}

impl From<DefKind> for Kind {
    fn from(kind: DefKind) -> Self {
        match kind {
            DefKind::Module => Self::Namespace,
            DefKind::Struct | DefKind::Union => Self::Struct,
            DefKind::Enum => Self::Enum,
            DefKind::Trait => Self::Interface,
            DefKind::TraitAssociatedType | DefKind::TypeAlias => Self::Type,
            DefKind::Function => Self::Function,
            DefKind::TraitMethod | DefKind::Method => Self::Method,
            DefKind::Global | DefKind::Const => Self::Variable,
            DefKind::StructField | DefKind::UnionField | DefKind::EnumVariantField => {
                Self::Property
            }
            DefKind::EnumVariant => Self::EnumMember,
        }
    }
}

const DECLARATION: u32 = 1;
const READONLY: u32 = 2;
const DEFAULT_LIBRARY: u32 = 4;

pub(crate) fn legend() -> SemanticTokensLegend {
    use lsp_types::{SemanticTokenModifier as Modifier, SemanticTokenType as Type};

    SemanticTokensLegend {
        token_types: vec![
            Type::NAMESPACE,
            Type::TYPE,
            Type::STRUCT,
            Type::ENUM,
            Type::INTERFACE,
            Type::TYPE_PARAMETER,
            Type::PARAMETER,
            Type::VARIABLE,
            Type::PROPERTY,
            Type::ENUM_MEMBER,
            Type::FUNCTION,
            Type::METHOD,
            Type::KEYWORD,
            Type::COMMENT,
            Type::STRING,
            Type::NUMBER,
            Type::OPERATOR,
            Type::DECORATOR,
        ],
        token_modifiers: vec![
            Modifier::DECLARATION,
            Modifier::READONLY,
            Modifier::DEFAULT_LIBRARY,
        ],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Class {
    kind: Kind,
    modifiers: u32,
}

impl Class {
    pub(crate) fn new(kind: Kind) -> Self {
        Self { kind, modifiers: 0 }
    }

    pub(crate) fn builtin(kind: Kind) -> Self {
        Self {
            kind,
            modifiers: DEFAULT_LIBRARY,
        }
    }

    pub(crate) fn symbol(kind: Kind, declaration: bool, readonly: bool) -> Self {
        Self {
            kind,
            modifiers: (u32::from(declaration) * DECLARATION) | (u32::from(readonly) * READONLY),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Highlight {
    span: Span,
    class: Class,
}

pub(crate) type Resolved = HashMap<Span, Class>;

impl Document {
    fn syntax_highlights(&self) -> &[Highlight] {
        self.highlights.get_or_init(|| {
            let mut highlights = Vec::new();
            if let Some(syntax) = self.syntax() {
                visit(
                    syntax.tree.green_root(),
                    None,
                    None,
                    &self.text,
                    &mut highlights,
                );
            } else {
                for token in tokenize_lossless(&self.text) {
                    let text = &self.text[token.span.start..token.span.end];
                    let class = match token.kind {
                        LosslessTokenKind::Token(kind) => lexical(&kind, text),
                        LosslessTokenKind::LineComment => Some(Class::new(Kind::Comment)),
                        LosslessTokenKind::Whitespace => None,
                    };
                    if let Some(class) = class {
                        highlights.push(Highlight {
                            span: token.span,
                            class,
                        });
                    }
                }
            }
            highlights
        })
    }

    pub(crate) fn has_refinements(&self, resolved: &Resolved) -> bool {
        self.syntax_highlights()
            .iter()
            .any(|token| refine(*token, Some(resolved)) != token.class)
    }

    pub(crate) fn semantic_tokens(&self, resolved: Option<&Resolved>) -> SemanticTokens {
        let highlights = self.syntax_highlights();
        let mut data = Vec::with_capacity(highlights.len());
        let (mut previous_line, mut previous_start, mut previous_column, mut previous_end) =
            (0, 0, 0, 0);
        for &token in highlights {
            let class = refine(token, resolved);
            let mut start = token.span.start;
            let mut line = self.lines.line_index(start);
            while start < token.span.end {
                let line_start = self.lines.line_start(line);
                let next_line = self.lines.next_line_start(line).unwrap_or(self.text.len());
                let end = token.span.end.min(next_line);
                let segment = self.text[start..end].trim_end_matches(['\r', '\n']);
                if !segment.is_empty() {
                    let same_line = line == previous_line;
                    let column = if same_line {
                        previous_column + utf16_len(&self.text[previous_end..start])
                    } else {
                        utf16_len(&self.text[line_start..start])
                    };
                    let length = utf16_len(segment);
                    data.push(SemanticToken {
                        delta_line: (line - previous_line) as u32,
                        delta_start: if same_line {
                            column - previous_start
                        } else {
                            column
                        },
                        length,
                        token_type: class.kind as u32,
                        token_modifiers_bitset: class.modifiers,
                    });
                    previous_line = line;
                    previous_start = column;
                    previous_column = column + length;
                    previous_end = start + segment.len();
                }
                start = end;
                line += 1;
            }
        }
        SemanticTokens {
            result_id: None,
            data,
        }
    }
}

pub(crate) fn token_edits(
    before: &[SemanticToken],
    after: &[SemanticToken],
) -> Option<Vec<SemanticTokensEdit>> {
    let prefix = before
        .iter()
        .zip(after)
        .take_while(|(left, right)| left == right)
        .count();
    let before = &before[prefix..];
    let after = &after[prefix..];
    if before.is_empty() && after.is_empty() {
        return Some(Vec::new());
    }
    let suffix = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let removed = before.len() - suffix;
    let inserted = after.len() - suffix;
    let start = u32::try_from(prefix.checked_mul(5)?).ok()?;
    let delete_count = u32::try_from(removed.checked_mul(5)?).ok()?;
    Some(vec![SemanticTokensEdit {
        start,
        delete_count,
        data: (inserted != 0).then(|| after[..inserted].to_vec()),
    }])
}

fn refine(token: Highlight, resolved: Option<&Resolved>) -> Class {
    if matches!(
        token.class.kind,
        Kind::Keyword
            | Kind::Comment
            | Kind::String
            | Kind::Number
            | Kind::Operator
            | Kind::Decorator
    ) || token.class.modifiers & DEFAULT_LIBRARY != 0
    {
        return token.class;
    }
    resolved
        .and_then(|resolved| resolved.get(&token.span))
        .copied()
        .filter(|class| {
            token.class.modifiers & DECLARATION == 0 || class.modifiers & DECLARATION != 0
        })
        .unwrap_or(token.class)
}

fn utf16_len(text: &str) -> u32 {
    if text.is_ascii() {
        text.len() as u32
    } else {
        text.encode_utf16().count() as u32
    }
}

fn visit(
    node: &GreenNode,
    parent: Option<&SyntaxKind>,
    role: Option<Kind>,
    source: &str,
    output: &mut Vec<Highlight>,
) {
    use SyntaxKind::*;

    let last_path_segment = if node.kind() == &PathType {
        node.child_nodes()
            .filter(|child| child.kind() == &PathSegment)
            .last()
    } else {
        None
    };
    let first_expr = node.child_nodes().find(|child| child.kind().is_expr());
    let name = node
        .child_tokens()
        .find(|token| token.is(&TokenKind::Ident) || token.is(&TokenKind::SelfValue));
    let name_class = named_class(node, parent, role);
    for element in node.children() {
        match element {
            GreenElement::Node(child) => {
                let is_expr = first_expr.is_some_and(|expr| std::ptr::eq(expr, child));
                let child_role = match node.kind() {
                    PathType if child.kind() == &PathSegment => Some(
                        if last_path_segment.is_some_and(|last| std::ptr::eq(last, child)) {
                            role.unwrap_or(Kind::Type)
                        } else {
                            Kind::Namespace
                        },
                    ),
                    CallExpr if is_expr => Some(if child.kind() == &FieldExpr {
                        Kind::Method
                    } else {
                        Kind::Function
                    }),
                    QualifiedExpr if is_expr => Some(Kind::Namespace),
                    BracketExpr if is_expr => role,
                    TypedStructLiteral | QualifiedStructLiteral if is_expr => Some(Kind::Type),
                    SupertraitList if child.kind() == &PathType => Some(Kind::Interface),
                    _ => None,
                };
                visit(child, Some(node.kind()), child_role, source, output);
            }
            GreenElement::Token(token) => {
                let span = token.span();
                let text = &source[span.start..span.end];
                let class = if token.is(&TokenKind::Ident) && node.kind() == &UsingTree {
                    Some(Class::new(Kind::Namespace))
                } else if token.is(&TokenKind::Ident) && node.kind() == &Attribute {
                    Some(Class::new(Kind::Decorator))
                } else if name.is_some_and(|name| std::ptr::eq(name, token)) {
                    name_class
                        .map(|mut class| {
                            if class.kind == Kind::Type
                                && class.modifiers == 0
                                && PrimitiveTypeSpelling::from_name(text).is_some()
                            {
                                class.modifiers = DEFAULT_LIBRARY;
                            }
                            class
                        })
                        .or_else(|| {
                            token
                                .kind()
                                .token_kind()
                                .and_then(|kind| lexical(kind, text))
                        })
                } else if token.kind() == &LineComment {
                    Some(Class::new(Kind::Comment))
                } else {
                    token
                        .kind()
                        .token_kind()
                        .and_then(|kind| lexical(kind, text))
                };
                if let Some(class) = class {
                    output.push(Highlight { span, class });
                }
            }
        }
    }
}

fn named_class(node: &GreenNode, parent: Option<&SyntaxKind>, role: Option<Kind>) -> Option<Class> {
    use SyntaxKind::*;

    let declaration = match node.kind() {
        ModuleDecl => Kind::Namespace,
        StructDecl | UnionDecl => Kind::Struct,
        EnumDecl => Kind::Enum,
        Variant => Kind::EnumMember,
        TraitDecl => Kind::Interface,
        FunctionDecl if parent == Some(&Member) => Kind::Method,
        FunctionDecl => Kind::Function,
        TypeAliasDecl | AssocTypeDecl => Kind::Type,
        GenericParam => Kind::TypeParameter,
        Param => Kind::Parameter,
        BindPattern | BindingDecl | AssocConstDecl => Kind::Variable,
        Field => Kind::Property,
        _ => {
            return Some(Class::new(match node.kind() {
                PathSegment => role.unwrap_or(Kind::Type),
                NameExpr | QualifiedExpr => role.unwrap_or(Kind::Variable),
                FieldExpr => role.unwrap_or(Kind::Property),
                FieldInit | NamedPatternField => Kind::Property,
                Capture => Kind::Variable,
                OmittedMemberExpr => Kind::EnumMember,
                _ => return None,
            }));
        }
    };
    let readonly = node.kind() == &AssocConstDecl
        || node.kind() == &BindingDecl && node.token(&TokenKind::Const).is_some();
    Some(Class::symbol(declaration, true, readonly))
}

fn lexical(token: &TokenKind, text: &str) -> Option<Class> {
    use TokenKind::*;

    Some(match token {
        Ident => Class::new(Kind::Variable),
        Bool | Never => Class::builtin(Kind::Type),
        SelfType | Opaque => Class::new(Kind::Type),
        Root | Super => Class::new(Kind::Namespace),
        Integer | Float => Class::new(Kind::Number),
        String | ByteString | Char | ByteChar => Class::new(Kind::String),
        Error(_)
            if text.starts_with(['"', '\''])
                || text.starts_with("b\"")
                || text.starts_with("b'") =>
        {
            Class::new(Kind::String)
        }
        And | As | Break | Const | Continue | Defer | Else | Enum | Extern | Extend | False
        | Fun | For | If | In | Is | Let | Loop | Match | Module | Mut | Not | Null | Or | Pub
        | Return | SelfValue | Static | Struct | Trait | True | Type | Union | Using | Var
        | Where | While => Class::new(Kind::Keyword),
        Plus | Minus | Star | Slash | Percent | Amp | Pipe | Caret | Tilde | Bang | Eq | Lt
        | Gt | LtLt | GtGt | PlusEq | MinusEq | StarEq | SlashEq | PercentEq | AmpEq | PipeEq
        | CaretEq | EqEq | BangEq | LtEq | GtEq | LtLtEq | GtGtEq | DotDot | DotDotEq
        | Question | FatArrow | ThinArrow | Backslash => Class::new(Kind::Operator),
        At => Class::new(Kind::Decorator),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_highlights_declarations_and_encodes_utf16_through_recovery() {
        let source = concat!(
            "// A shape.\r\n",
            "trait Shape {\r\n",
            "    fun measure(&self) i32;\r\n",
            "}\r\n",
            "fun read(value: i32) i32 {\r\n",
            "    _ = \"\u{1f97e}\"; value\r\n",
            "}\r\n",
            "fun unfinished(\r\n",
        );
        let document = Document::new(1, source.into());
        let highlights = document.syntax_highlights();
        let expected = [
            ("// A shape.", Kind::Comment, 0),
            ("Shape", Kind::Interface, DECLARATION),
            ("measure", Kind::Method, DECLARATION),
            ("value", Kind::Parameter, DECLARATION),
            ("i32", Kind::Type, DEFAULT_LIBRARY),
        ];
        for (text, kind, modifiers) in expected {
            assert!(
                highlights.iter().any(|token| {
                    source[token.span.start..token.span.end].trim_end_matches('\r') == text
                        && token.class == Class { kind, modifiers }
                }),
                "{text}"
            );
        }

        let tokens = document.semantic_tokens(None);
        assert_eq!(tokens.data[0].length, "// A shape.".len() as u32);
        let (mut line, mut column) = (0, 0);
        let mut string = None;
        let mut value = None;
        for token in tokens.data {
            if token.delta_line > 0 {
                line += token.delta_line;
                column = 0;
            }
            column += token.delta_start;
            if line == 5 && token.token_type == Kind::String as u32 {
                string = Some((column, token.length));
            }
            if line == 5 && token.token_type == Kind::Variable as u32 {
                value = Some((column, token.length));
            }
        }
        assert_eq!(string, Some((8, 4)));
        assert_eq!(value, Some((14, 5)));
        assert_eq!(
            legend().token_types[Kind::Decorator as usize].as_str(),
            "decorator"
        );
    }
}
