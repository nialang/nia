// SPDX-License-Identifier: GPL-3.0-or-later
//! Document outline, folding, and selection from the current lossless syntax tree.

use lsp_types::{
    DocumentSymbol, DocumentSymbolResponse, FoldingRange, FoldingRangeKind, Location, Position,
    SelectionRange, SymbolInformation, SymbolKind, Uri,
};
use nia_lexer::TokenKind;
use nia_span::Span;
use nia_syntax::{GreenElement, GreenNode, SyntaxKind};

use crate::document::Document;

impl Document {
    pub(crate) fn selection_ranges(&self, positions: &[Position]) -> Option<Vec<SelectionRange>> {
        let root = self.syntax().map(|syntax| syntax.tree.green_root());
        positions
            .iter()
            .map(|&position| {
                let offset = if position.line as usize > self.lines.line_index(self.text.len()) {
                    self.text.len()
                } else {
                    self.lines.offset(&self.text, position)?
                };
                let mut spans = vec![Span::new(0, self.text.len())];
                if let Some(root) = root {
                    selection_spans(root, offset, &mut spans);
                }
                let mut selection = None;
                for span in spans {
                    selection = Some(Box::new(SelectionRange {
                        range: self.lines.range(&self.text, span),
                        parent: selection,
                    }));
                }
                selection.map(|range| *range)
            })
            .collect()
    }

    pub(crate) fn symbols(&self, uri: &Uri, hierarchical: bool) -> DocumentSymbolResponse {
        let mut symbols = Vec::new();
        if let Some(syntax) = self.syntax() {
            self.outline_children(syntax.tree.green_root(), None, &mut symbols);
        }
        if hierarchical {
            DocumentSymbolResponse::Nested(symbols)
        } else {
            let mut flat = Vec::new();
            flatten(symbols, uri, None, &mut flat);
            DocumentSymbolResponse::Flat(flat)
        }
    }

    fn outline_children(
        &self,
        parent: &GreenNode,
        wrapper: Option<Span>,
        output: &mut Vec<DocumentSymbol>,
    ) {
        for node in parent.child_nodes() {
            match node.kind() {
                SyntaxKind::Item | SyntaxKind::Member => {
                    self.outline_children(node, node.token_span(), output);
                    continue;
                }
                SyntaxKind::MemberList | SyntaxKind::FieldList | SyntaxKind::VariantList => {
                    self.outline_children(node, None, output);
                    continue;
                }
                _ => {}
            }
            let kind = if node.kind() == &SyntaxKind::FunctionDecl {
                Some(if parent.kind() == &SyntaxKind::Member {
                    SymbolKind::METHOD
                } else {
                    SymbolKind::FUNCTION
                })
            } else {
                outline_kind(node)
            };
            if let Some(kind) = kind
                && let Some(symbol) = self.outline(node, kind, wrapper)
            {
                output.push(symbol);
            }
        }
    }

    #[allow(deprecated)]
    fn outline(
        &self,
        node: &GreenNode,
        kind: SymbolKind,
        wrapper: Option<Span>,
    ) -> Option<DocumentSymbol> {
        let span = wrapper.or_else(|| node.token_span())?;
        let selection = if node.kind() == &SyntaxKind::ExtendDecl {
            let keyword = node.token(&TokenKind::Extend)?.span();
            let target = node.child_nodes().find(|child| child.kind().is_type())?;
            let header_end = node
                .child(&SyntaxKind::WhereClause)
                .or_else(|| node.child(&SyntaxKind::MemberList))?
                .first_token()?
                .span()
                .start;
            (Span::new(keyword.start, header_end), target.token_span()?)
        } else {
            let name = node.token(&TokenKind::Ident)?.span();
            (name, name)
        };
        let mut children = Vec::new();
        self.outline_children(node, None, &mut children);
        Some(DocumentSymbol {
            name: self.text[selection.0.start..selection.0.end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range: self.lines.range(&self.text, span),
            selection_range: self.lines.range(&self.text, selection.1),
            children: (!children.is_empty()).then_some(children),
        })
    }

    pub(crate) fn folding_ranges(
        &self,
        line_only: bool,
        limit: Option<usize>,
    ) -> Vec<FoldingRange> {
        let mut folds = Folds {
            document: self,
            line_only,
            ranges: Vec::new(),
            comments: None,
        };
        if let Some(syntax) = self.syntax() {
            folds.visit(syntax.tree.green_root());
            folds.finish_comments();
        }
        folds.ranges.sort_by_key(|range| {
            (
                range.start_line,
                range.start_character,
                std::cmp::Reverse((range.end_line, range.end_character)),
                range.kind.is_none(),
            )
        });
        folds
            .ranges
            .dedup_by_key(|range| (range.start_line, range.start_character));
        if let Some(limit) = limit {
            folds.ranges.truncate(limit);
        }
        folds.ranges
    }
}

fn selection_spans(mut node: &GreenNode, offset: usize, spans: &mut Vec<Span>) {
    let contains = |span: Span| span.start <= offset && offset < span.end;
    loop {
        if let Some(span) = node.token_span().filter(|&span| contains(span))
            && spans.last() != Some(&span)
        {
            spans.push(span);
        }
        match node.children().iter().find(|child| match child {
            GreenElement::Node(node) => contains(node.span()),
            GreenElement::Token(token) => contains(token.span()),
        }) {
            Some(GreenElement::Node(child)) => node = child,
            Some(GreenElement::Token(token)) => {
                let span = token.span();
                if token.kind() != &SyntaxKind::Whitespace && spans.last() != Some(&span) {
                    spans.push(span);
                }
                return;
            }
            None => return,
        }
    }
}

fn outline_kind(node: &GreenNode) -> Option<SymbolKind> {
    Some(match node.kind() {
        SyntaxKind::ModuleDecl => SymbolKind::MODULE,
        SyntaxKind::StructDecl | SyntaxKind::UnionDecl => SymbolKind::STRUCT,
        SyntaxKind::TraitDecl => SymbolKind::INTERFACE,
        SyntaxKind::ExtendDecl => SymbolKind::OBJECT,
        SyntaxKind::EnumDecl => SymbolKind::ENUM,
        SyntaxKind::Variant => SymbolKind::ENUM_MEMBER,
        SyntaxKind::Field => SymbolKind::FIELD,
        SyntaxKind::TypeAliasDecl | SyntaxKind::AssocTypeDecl => SymbolKind::TYPE_PARAMETER,
        SyntaxKind::AssocConstDecl => SymbolKind::CONSTANT,
        SyntaxKind::BindingDecl if node.token(&TokenKind::Const).is_some() => SymbolKind::CONSTANT,
        SyntaxKind::BindingDecl => SymbolKind::VARIABLE,
        _ => return None,
    })
}

#[allow(deprecated)]
fn flatten(
    symbols: Vec<DocumentSymbol>,
    uri: &Uri,
    parent: Option<&str>,
    output: &mut Vec<SymbolInformation>,
) {
    for symbol in symbols {
        let index = output.len();
        output.push(SymbolInformation {
            name: symbol.name,
            kind: symbol.kind,
            tags: symbol.tags,
            deprecated: None,
            location: Location::new(uri.clone(), symbol.selection_range),
            container_name: parent.map(str::to_owned),
        });
        if let Some(children) = symbol.children {
            let parent = output[index].name.clone();
            flatten(children, uri, Some(&parent), output);
        }
    }
}

struct Folds<'a> {
    document: &'a Document,
    line_only: bool,
    ranges: Vec<FoldingRange>,
    comments: Option<(u32, u32)>,
}

impl Folds<'_> {
    fn visit(&mut self, node: &GreenNode) {
        use SyntaxKind::*;
        let brackets = match node.kind() {
            Block | FieldList | FieldInitList | MatchExpr | MemberList | NominalNamedFields
            | VariantList | UsingGroup => Some((TokenKind::LBrace, TokenKind::RBrace)),
            ArgList | CallableType | FnPointerType | NominalTupleFields | ParamList | ParenExpr
            | ParenPattern | ParenType | TupleExpr | TupleFieldList | TuplePattern | TupleType => {
                Some((TokenKind::LParen, TokenKind::RParen))
            }
            ArrayExpr | ArrayType | BracketArgList | CaptureList | GenericParamList
            | ProjectionType | SlicePointeeType | SliceType | TraitTargetExpr | TypeArgList
            | TypeTargetExpr => Some((TokenKind::LBracket, TokenKind::RBracket)),
            _ => None,
        };
        if let Some((open, close)) = brackets
            && let Some(open) = node.token(&open)
        {
            let closing = node.token(&close);
            let end = closing.map_or(node.span().end, |token| token.span().start);
            let kind = (node.kind() == &UsingGroup).then_some(FoldingRangeKind::Imports);
            self.push(open.span().end, end, kind, closing.is_some());
        } else if node.kind() == &WhereClause
            && let Some(keyword) = node.token(&TokenKind::Where)
            && let Some(last) = node.last_token()
        {
            self.push(keyword.span().end, last.span().end, None, false);
        }

        for child in node.children() {
            match child {
                GreenElement::Node(child) => self.visit(child),
                GreenElement::Token(token) if token.kind() == &LineComment => {
                    let start = token.span().start;
                    let line = self.document.lines.line_index(start);
                    if self.line_prefix_is_whitespace(line, start) {
                        let line = line as u32;
                        if let Some((first, last)) = self.comments
                            && last + 1 == line
                        {
                            self.comments = Some((first, line));
                        } else {
                            self.finish_comments();
                            self.comments = Some((line, line));
                        }
                    }
                }
                GreenElement::Token(token) if !token.kind().is_trivia() => self.finish_comments(),
                GreenElement::Token(_) => {}
            }
        }
    }

    fn finish_comments(&mut self) {
        if let Some((first, last)) = self.comments.take()
            && first < last
        {
            self.ranges.push(FoldingRange {
                start_line: first,
                end_line: last,
                kind: Some(FoldingRangeKind::Comment),
                ..FoldingRange::default()
            });
        }
    }

    fn line_prefix_is_whitespace(&self, line: usize, end: usize) -> bool {
        self.document.text[self.document.lines.line_start(line)..end]
            .trim()
            .is_empty()
    }

    fn push(&mut self, start: usize, end: usize, kind: Option<FoldingRangeKind>, delimited: bool) {
        let lines = &self.document.lines;
        let text = &self.document.text;
        let start_line = lines.line_index(start);
        let mut end_line = lines.line_index(end);
        // Keep a closing delimiter on its own line visible in line-only clients.
        if self.line_only
            && delimited
            && end_line > start_line
            && self.line_prefix_is_whitespace(end_line, end)
        {
            end_line -= 1;
        }
        if start_line < end_line {
            self.ranges.push(FoldingRange {
                start_line: start_line as u32,
                end_line: end_line as u32,
                start_character: (!self.line_only).then(|| lines.position(text, start).character),
                end_character: (!self.line_only).then(|| lines.position(text, end).character),
                kind,
                ..FoldingRange::default()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use lsp_types::Range;

    use super::*;

    #[test]
    fn selection_uses_recovered_tokens_and_preserves_position_order() {
        let source = "// notes\r\nfun incomplete(";
        let document = Document::new(1, source.into());
        let eof = Position::new(1, 15);
        let comment = Position::new(0, 4);
        let name = Position::new(1, 6);
        let file = Range::new(Position::new(0, 0), eof);
        let ranges = document.selection_ranges(&[eof, comment, name]).unwrap();
        assert_eq!(
            ranges[0],
            SelectionRange {
                range: file,
                parent: None
            }
        );
        assert_eq!(
            ranges[1].range,
            Range::new(Position::new(0, 0), Position::new(0, 8))
        );
        assert_eq!(
            ranges[2].range,
            Range::new(Position::new(1, 4), Position::new(1, 14))
        );
        for mut range in &ranges {
            while let Some(parent) = &range.parent {
                assert!(parent.range.start <= range.range.start);
                assert!(range.range.end <= parent.range.end);
                assert_ne!(range.range, parent.range);
                range = parent;
            }
            assert_eq!(range.range, file);
        }
        let mixed = document
            .selection_ranges(&[Position::new(2, 0), name])
            .unwrap();
        assert_eq!(mixed, [ranges[0].clone(), ranges[2].clone()]);
        let empty = Document::new(1, String::new());
        assert_eq!(
            empty.selection_ranges(&[Position::new(0, 0)]).unwrap(),
            [SelectionRange::default()]
        );
    }

    #[test]
    fn outline_keeps_extension_and_enum_members_in_their_declaring_blocks() {
        let document = Document::new(
            1,
            concat!(
                "module math;\n",
                "pub enum Shape {\n",
                "    Pair { left: i32, right: i32 },\n",
                "}\n",
                "trait Measure {\n",
                "    type Unit;\n",
                "    fun size(&self) i32;\n",
                "}\n",
                "extend Shape : Measure {\n",
                "    type Unit = i32;\n",
                "    fun size(&self) i32 { 0 }\n",
                "}\n",
                "fun incomplete(\n",
            )
            .into(),
        );
        let uri = "file:///outline.nia".parse().unwrap();
        let DocumentSymbolResponse::Nested(symbols) = document.symbols(&uri, true) else {
            panic!("expected hierarchical outline");
        };
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            ["math", "Shape", "Measure", "extend Shape : Measure"]
        );
        assert_eq!(symbols[1].range.start, Position::new(1, 0));
        let variant = &symbols[1].children.as_ref().unwrap()[0];
        assert_eq!(variant.kind, SymbolKind::ENUM_MEMBER);
        assert_eq!(variant.children.as_ref().unwrap()[1].name, "right");
        assert_eq!(
            symbols[2].children.as_ref().unwrap()[1].kind,
            SymbolKind::METHOD
        );
        assert_eq!(
            symbols[3].selection_range,
            Range::new(Position::new(8, 7), Position::new(8, 12))
        );
        let DocumentSymbolResponse::Flat(flat) = document.symbols(&uri, false) else {
            panic!("expected flat outline");
        };
        assert!(flat.iter().any(|item| item.name == "size"
            && item.container_name.as_deref() == Some("extend Shape : Measure")));
    }

    #[test]
    fn folding_uses_grammar_delimiters_and_client_position_support() {
        let document = Document::new(
            1,
            concat!(
                "// first\r\n",
                "// second\r\n",
                "fun compute(\r\n",
                "    value: i32,\r\n",
                ") i32 {\r\n",
                "    _ = \"{ 🥾 }\"; if true {\r\n",
                "        return value;\r\n",
                "    }\r\n",
                "    value\r\n",
                "}\r\n",
            )
            .into(),
        );
        let folds = document.folding_ranges(true, None);
        assert_eq!(
            folds
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 1), (2, 3), (4, 8), (5, 6)]
        );
        assert_eq!(folds[0].kind, Some(FoldingRangeKind::Comment));
        assert!(
            folds
                .iter()
                .all(|range| range.start_character.is_none() && range.end_character.is_none())
        );
        let folds = document.folding_ranges(false, None);
        assert_eq!(
            (folds[3].start_line, folds[3].start_character),
            (5, Some(27))
        );
        assert_eq!((folds[3].end_line, folds[3].end_character), (7, Some(4)));
        assert_eq!(document.folding_ranges(true, Some(2)).len(), 2);
    }

    #[test]
    fn folding_keeps_grouped_imports_and_where_predicates() {
        let document = Document::new(
            1,
            concat!(
                "using root::math::{\n",
                "    sqrt,\n",
                "    sin,\n",
                "};\n",
                "extend [T] Box[T] where\n",
                "    T: Measure,\n",
                "{\n",
                "    fun size(&self) i32 { 0 }\n",
                "}\n",
            )
            .into(),
        );
        let folds = document.folding_ranges(true, None);
        assert_eq!(
            folds
                .into_iter()
                .map(|range| (range.start_line, range.end_line, range.kind))
                .collect::<Vec<_>>(),
            [
                (0, 2, Some(FoldingRangeKind::Imports)),
                (4, 5, None),
                (6, 7, None)
            ]
        );
    }

    #[test]
    fn folding_covers_nested_types_in_signatures_and_tuple_expressions() {
        let document = Document::new(
            1,
            concat!(
                "type Callback = &fun(\n",
                "    [\n",
                "        i32; 4\n",
                "    ],\n",
                "    (\n",
                "        i32, bool\n",
                "    ),\n",
                ") (\n",
                "    i32, bool\n",
                ");\n",
                "fun pair() (i32, bool) {\n",
                "    (\n",
                "        1, true\n",
                "    )\n",
                "}\n",
            )
            .into(),
        );
        assert_eq!(
            document
                .folding_ranges(true, None)
                .into_iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            [(0, 6), (1, 2), (4, 5), (7, 8), (10, 13), (11, 12)]
        );
        let ranges = document.folding_ranges(false, None);
        assert_eq!(ranges[0].end_line, 7);
        assert_eq!(ranges[0].end_character, Some(0));
        assert_eq!(ranges[3].start_line, 7);
        assert_eq!(ranges[3].start_character, Some(3));
    }
}
