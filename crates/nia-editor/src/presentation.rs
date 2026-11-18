// SPDX-License-Identifier: GPL-3.0-or-later

use nia_ids::BuiltinTypeAnchor;
use nia_lexer::TokenKind;
use nia_span::Span;
use nia_syntax::{GreenElement, GreenNode, GreenToken, SyntaxKind, SyntaxTree};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Highlight {
    Keyword,
    Type,
    Primitive,
    Interface,
    TypeParameter,
    Namespace,
    Function,
    Method,
    Variable,
    Parameter,
    Property,
    EnumMember,
    Constant,
    Decorator,
    Number,
    String,
    Comment,
    Operator,
    Propagation,
    Dereference,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Token(usize, usize, usize, usize, usize, Highlight);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum FoldKind {
    Region,
    Imports,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Fold(usize, usize, FoldKind);

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Presentation {
    tokens: Vec<Token>,
    folds: Vec<Fold>,
}

impl Presentation {
    pub(crate) fn new(tree: &SyntaxTree) -> Self {
        let source = tree.source();
        let mut walk = Walk {
            position: Position::default(),
            line_starts: std::iter::once(0)
                .chain(
                    source
                        .bytes()
                        .enumerate()
                        .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
                )
                .collect(),
            source,
            result: Self::default(),
        };
        let root = tree.green_root();
        walk.imports(root);
        walk.node(root, None, None);
        // Each editor accepts line ranges. Keep the outer range when two
        // opening delimiters are on the same line.
        walk.result
            .folds
            .sort_unstable_by_key(|Fold(start, end, _)| (*start, std::cmp::Reverse(*end)));
        walk.result.folds.dedup_by_key(|fold| fold.0);
        make_folds_nested(&mut walk.result.folds);
        walk.result
    }
}

fn make_folds_nested(folds: &mut Vec<Fold>) {
    let mut active: Vec<usize> = Vec::new();
    for index in 0..folds.len() {
        let Fold(start, end, _) = folds[index];
        while active.last().is_some_and(|outer| folds[*outer].1 < start) {
            active.pop();
        }
        while let Some(&outer) = active.last() {
            if folds[outer].1 >= end {
                break;
            }
            // Adjacent blocks can close and open on the same line.
            folds[outer].1 = folds[outer].1.min(start - 1);
            active.pop();
        }
        active.push(index);
    }
    folds.retain(|Fold(start, end, _)| start < end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(source: &str) -> Presentation {
        Presentation::new(&nia_syntax::parse(source, None).unwrap().tree)
    }

    #[test]
    fn highlights_signature_roles_and_preserves_multiline_headers() {
        let source = concat!(
            "using std::iter;\n",
            "using std::slice;\n",
            "extend[T] Box[T] : Trait\n",
            "where T: Other\n",
            "{\n",
            "    fun run(value: usize) i32 {\n",
            "        return match self.read().? {\n",
            "            0 => value.*,\n",
            "            _ => 0,\n",
            "        };\n",
            "    }\n",
            "}\n",
        );
        let data = present(source);
        let lines: Vec<_> = source.lines().collect();
        for (line, text, kind) in [
            (2, "Box", Highlight::Type),
            (2, "Trait", Highlight::Interface),
            (3, "Other", Highlight::Interface),
            (5, "run", Highlight::Method),
            (5, "usize", Highlight::Primitive),
            (6, "read", Highlight::Method),
            (6, "?", Highlight::Propagation),
            (7, "value", Highlight::Variable),
            (7, "*", Highlight::Dereference),
        ] {
            assert!(
                data.tokens.iter().any(|token| {
                    token.0 == line
                        && &lines[line][token.1..token.1 + token.2] == text
                        && token.5 == kind
                }),
                "missing {kind:?} for {text}"
            );
        }
        assert_eq!(
            data.folds,
            [
                Fold(0, 1, FoldKind::Imports),
                Fold(4, 11, FoldKind::Region),
                Fold(5, 10, FoldKind::Region),
                Fold(6, 9, FoldKind::Region),
            ]
        );
    }

    #[test]
    fn unfinished_blocks_preserve_following_items_and_unicode_columns() {
        let data =
            present("fun unfinished() {\r\nfun next() {\r\n    _ = \"\u{1f600}\"; _ = 2;\r\n}\r\n");
        assert_eq!(data.folds, [Fold(1, 3, FoldKind::Region)]);
        assert!(
            data.tokens
                .contains(&Token(2, 20, 1, 18, 1, Highlight::Number))
        );
    }

    #[test]
    fn blocks_with_a_shared_delimiter_line_produce_nested_or_disjoint_ranges() {
        let data = present("fun a(c: bool) i32 {\n if c {\n  1\n } else {\n  2\n }\n}\n");
        assert_eq!(
            data.folds,
            [
                Fold(0, 6, FoldKind::Region),
                Fold(1, 2, FoldKind::Region),
                Fold(3, 5, FoldKind::Region),
            ]
        );
    }
}

#[derive(Default)]
struct Position {
    offset: usize,
    line: usize,
    line_start: usize,
    utf16_column: usize,
}

impl Position {
    fn advance(&mut self, source: &str, end: usize) {
        let text = &source[self.offset..end];
        if let Some(newline) = text.rfind('\n') {
            self.line += text.bytes().filter(|byte| *byte == b'\n').count();
            self.line_start = self.offset + newline + 1;
            self.utf16_column = utf16_len(&text[newline + 1..]);
        } else {
            self.utf16_column += utf16_len(text);
        }
        self.offset = end;
    }
}

fn utf16_len(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.encode_utf16().count()
    }
}

struct Walk<'a> {
    source: &'a str,
    position: Position,
    line_starts: Vec<usize>,
    result: Presentation,
}

impl Walk<'_> {
    fn line(&self, byte: usize) -> usize {
        self.line_starts.partition_point(|start| *start <= byte) - 1
    }

    fn fold(&mut self, start: usize, end: usize, kind: FoldKind) {
        let start = self.line(start);
        let end = self.line(end);
        if start < end {
            self.result.folds.push(Fold(start, end, kind));
        }
    }

    fn imports(&mut self, root: &GreenNode) {
        let mut run: Option<(Span, Span)> = None;
        for child in root.child_nodes() {
            let import = child
                .child_nodes()
                .find(|node| *node.kind() == SyntaxKind::UsingDecl)
                .and_then(GreenNode::token_span);
            if let Some(span) = import
                && let Some((_, last)) = run.as_mut()
                && self.line(span.start) <= self.line(last.end - 1) + 1
            {
                *last = span;
                continue;
            }
            if let Some((first, last)) = run.take()
                && first != last
            {
                self.fold(first.start, last.end - 1, FoldKind::Imports);
            }
            run = import.map(|span| (span, span));
        }
        if let Some((first, last)) = run
            && first != last
        {
            self.fold(first.start, last.end - 1, FoldKind::Imports);
        }
    }

    fn node(
        &mut self,
        node: &GreenNode,
        parent: Option<&SyntaxKind>,
        inherited: Option<Highlight>,
    ) {
        use SyntaxKind::*;
        let kind = node.kind();
        let delimiters = match kind {
            Block | MatchExpr | FieldList | VariantList | MemberList | FieldInitList
            | UsingGroup => Some((TokenKind::LBrace, TokenKind::RBrace)),
            TupleFieldList => Some((TokenKind::LParen, TokenKind::RParen)),
            _ => None,
        };
        if let Some((open, close)) = delimiters
            && let Some(start) = node.child_tokens().find(|token| token.is(&open))
            && let Some(end) = node.child_tokens().filter(|token| token.is(&close)).last()
        {
            self.fold(
                start.span().start,
                end.span().start,
                if *kind == UsingGroup {
                    FoldKind::Imports
                } else {
                    FoldKind::Region
                },
            );
        }

        let call_target = match kind {
            CallExpr => Some(Highlight::Function),
            BracketExpr | ParenExpr => inherited.filter(|role| *role == Highlight::Function),
            _ => None,
        };
        let mut first_child = true;
        let mut first_name = true;
        let mut qualified = false;
        for child in node.children() {
            match child {
                GreenElement::Node(child) => {
                    let context = match kind {
                        CallExpr | BracketExpr | ParenExpr if first_child => call_target,
                        WherePredicate if first_child => Some(Highlight::Type),
                        SupertraitList | WherePredicate | TraitTargetExpr => {
                            Some(Highlight::Interface)
                        }
                        ExtendDecl if qualified && child.kind().is_type() => {
                            Some(Highlight::Interface)
                        }
                        FieldExpr | QualifiedExpr if inherited == Some(Highlight::Function) => None,
                        PathType => inherited,
                        _ => None,
                    };
                    self.node(child, Some(kind), context);
                    first_child = false;
                }
                GreenElement::Token(token) => {
                    let role = if token.is(&TokenKind::Ident) {
                        let role = identifier(node, token, parent, inherited, first_name);
                        first_name = false;
                        role
                    } else {
                        qualified |= token.is(&TokenKind::Colon);
                        lexical(token, kind)
                    };
                    if let Some(role) = role {
                        self.highlight(token.span(), role);
                    }
                }
            }
        }
    }

    fn highlight(&mut self, span: Span, role: Highlight) {
        let pos = &mut self.position;
        for line in self.source[span.start..span.end].split_inclusive('\n') {
            let start = pos.offset.max(span.start);
            let text = line.strip_suffix('\n').unwrap_or(line);
            let text = text.strip_suffix('\r').unwrap_or(text);
            pos.advance(self.source, start);
            if !text.is_empty() {
                self.result.tokens.push(Token(
                    pos.line,
                    start - pos.line_start,
                    text.len(),
                    pos.utf16_column,
                    utf16_len(text),
                    role,
                ));
            }
            pos.advance(self.source, start + line.len());
        }
    }
}

fn identifier(
    node: &GreenNode,
    token: &GreenToken,
    parent: Option<&SyntaxKind>,
    inherited: Option<Highlight>,
    first: bool,
) -> Option<Highlight> {
    use Highlight as H;
    use SyntaxKind::*;
    Some(match node.kind() {
        Attribute | ConditionCall => H::Decorator,
        ModuleDecl | UsingTree | UsingName => H::Namespace,
        StructDecl | UnionDecl | EnumDecl | TypeAliasDecl | AssocTypeDecl => H::Type,
        TraitDecl => H::Interface,
        FunctionDecl if parent == Some(&Member) => H::Method,
        FunctionDecl => H::Function,
        GenericParam if first => H::TypeParameter,
        Param if first => H::Parameter,
        Field | FieldInit | NamedPatternField | AssocBindingArg => H::Property,
        Variant | OmittedMemberExpr => H::EnumMember,
        AssocConstDecl => H::Constant,
        BindingDecl | LetStmt if node.child_tokens().any(|token| token.is(&TokenKind::Const)) => {
            H::Constant
        }
        BindingDecl | StaticStmt | BindPattern | Capture | ConditionAtom => H::Variable,
        PathSegment if BuiltinTypeAnchor::from_name(token.text()).is_some() => H::Primitive,
        PathSegment if parent == Some(&PathType) => inherited.unwrap_or(H::Type),
        PathSegment | NameExpr | QualifiedExpr => inherited.unwrap_or(H::Variable),
        FieldExpr if inherited == Some(H::Function) => H::Method,
        FieldExpr => H::Property,
        ProjectionType | NominalPattern | TypedStructLiteral | QualifiedStructLiteral => H::Type,
        CallableType => H::Primitive,
        _ => return None,
    })
}

fn lexical(token: &GreenToken, parent: &SyntaxKind) -> Option<Highlight> {
    use Highlight as H;
    use TokenKind::*;
    let kind = match token.kind() {
        SyntaxKind::LineComment => return Some(H::Comment),
        SyntaxKind::Token(kind) => kind,
        _ => return None,
    };
    Some(match kind {
        Integer | Float => H::Number,
        String | ByteString | Char | ByteChar => H::String,
        True | False | Null => H::Constant,
        Bool | Never | Opaque | SelfType => H::Primitive,
        At => H::Decorator,
        Dot | Question if *parent == SyntaxKind::TryExpr => H::Propagation,
        Dot | Star if *parent == SyntaxKind::DerefExpr => H::Dereference,
        And | Or | Not | DotDot | DotDotEq | Backslash | Question | Plus | Minus | Star | Slash
        | Percent | Amp | Pipe | Caret | Tilde | Bang | Eq | Lt | Gt | LtLt | GtGt | PlusEq
        | MinusEq | StarEq | SlashEq | PercentEq | AmpEq | PipeEq | CaretEq | EqEq | BangEq
        | LtEq | GtEq | LtLtEq | GtGtEq | FatArrow | ThinArrow => H::Operator,
        As | Break | Const | Continue | Defer | Else | Enum | Extern | Extend | Fun | For | If
        | In | Is | Let | Loop | Module | Mut | Pub | Root | Return | SelfValue | Struct
        | Static | Super | Match | Trait | Type | Union | Using | Var | Where | While => H::Keyword,
        Ident | LParen | RParen | LBrace | RBrace | LBracket | RBracket | Comma | Dot
        | Ellipsis | Colon | ColonColon | Semicolon | Underscore | Eof | Error(_) => return None,
    })
}
