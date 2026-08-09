// SPDX-License-Identifier: GPL-3.0-or-later
use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_node_id::{NodeChildPath, SyntaxKind as NodeSyntaxKind, VersionedNodeKey};
use nia_source::SourceVersion;
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxTree {
    source: String,
    version: Option<SourceVersion>,
    root: GreenNode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenNode {
    kind: SyntaxKind,
    span: Span,
    children: Vec<GreenElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreenElement {
    Node(GreenNode),
    Token(GreenToken),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenToken {
    kind: SyntaxKind,
    span: Span,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxToken {
    pub kind: TokenKind,
    pub span: Span,
    pub text: String,
    path: NodeChildPath,
    version: Option<SourceVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxTokenCursor {
    tokens: Vec<SyntaxToken>,
    pos: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub span: Span,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reparse {
    pub tree: SyntaxTree,
    pub kind: ReparseKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReparseKind {
    Partial,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxKind {
    SourceFile,
    Delimited {
        open: TokenKind,
        close: Option<TokenKind>,
    },
    Token(TokenKind),
    Whitespace,
    LineComment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode<'a> {
    tree: &'a SyntaxTree,
    path: Vec<u32>,
    node: &'a GreenNode,
}

pub fn parse_source(source: &str, version: Option<SourceVersion>) -> SyntaxTree {
    SyntaxTree::parse(source, version)
}

impl TextEdit {
    pub fn replace(span: Span, replacement: impl Into<String>) -> Self {
        Self {
            span,
            replacement: replacement.into(),
        }
    }

    pub fn insert(offset: usize, replacement: impl Into<String>) -> Self {
        Self::replace(Span::new(offset, offset), replacement)
    }

    pub fn delete(span: Span) -> Self {
        Self::replace(span, "")
    }
}

impl SyntaxTokenCursor {
    pub fn new(tree: &SyntaxTree) -> Self {
        Self {
            tokens: tree.root().tokens(),
            pos: 0,
        }
    }

    pub fn tokens(&self) -> &[SyntaxToken] {
        &self.tokens
    }

    pub fn checkpoint(&self) -> usize {
        self.pos
    }

    pub fn rewind(&mut self, checkpoint: usize) {
        self.pos = checkpoint.min(self.tokens.len().saturating_sub(1));
    }

    pub fn peek(&self) -> &SyntaxToken {
        &self.tokens[self.pos]
    }

    pub fn bump(&mut self) -> SyntaxToken {
        let token = self.peek().clone();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    pub fn at(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    pub fn nth(&self, offset: usize) -> Option<&SyntaxToken> {
        self.tokens.get(self.pos + offset)
    }

    pub fn nth_kind(&self, offset: usize) -> Option<&TokenKind> {
        self.nth(offset).map(|token| &token.kind)
    }

    pub fn token_at_or_after(&self, offset: usize) -> Option<&SyntaxToken> {
        let index = self
            .tokens
            .partition_point(|token| token.kind != TokenKind::Eof && token.span.end <= offset);
        self.tokens[index..]
            .iter()
            .find(|token| token.kind != TokenKind::Eof)
    }

    pub fn token_before_or_at(&self, offset: usize) -> Option<&SyntaxToken> {
        let index = self
            .tokens
            .partition_point(|token| token.kind != TokenKind::Eof && token.span.start < offset);
        self.tokens[..index]
            .iter()
            .rev()
            .find(|token| token.kind != TokenKind::Eof)
    }

    pub fn previous_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        }
    }
}

impl SyntaxToken {
    pub fn child_path(&self) -> &NodeChildPath {
        &self.path
    }

    pub fn source_version(&self) -> Option<SourceVersion> {
        self.version
    }

    pub fn node_key(&self) -> Option<VersionedNodeKey> {
        let version = self.version?;
        Some(VersionedNodeKey::child_path(
            version,
            NodeSyntaxKind::Token,
            self.path.clone(),
        ))
    }
}

impl SyntaxTree {
    pub fn parse(source: &str, version: Option<SourceVersion>) -> Self {
        let tokens = tokenize_lossless(source);
        Self::from_lossless_tokens(source, version, tokens)
    }

    pub fn from_lossless_tokens(
        source: &str,
        version: Option<SourceVersion>,
        tokens: Vec<LosslessToken>,
    ) -> Self {
        let root = build_green_root(source, tokens);
        Self {
            source: source.to_string(),
            version,
            root,
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn version(&self) -> Option<SourceVersion> {
        self.version
    }

    pub fn root(&self) -> SyntaxNode<'_> {
        SyntaxNode {
            tree: self,
            path: Vec::new(),
            node: &self.root,
        }
    }

    pub fn green_root(&self) -> &GreenNode {
        &self.root
    }

    pub fn tokens(&self) -> Vec<SyntaxToken> {
        self.root().tokens()
    }

    pub fn full_text(&self) -> String {
        let mut text = String::new();
        self.root.push_text(&mut text);
        text
    }

    pub fn reparse(&self, edit: TextEdit, version: Option<SourceVersion>) -> Reparse {
        let Some(source) = apply_edit(&self.source, &edit) else {
            return Reparse {
                tree: Self::parse(&self.source, version),
                kind: ReparseKind::Full,
            };
        };
        match try_partial_reparse(&self.root, &source, &edit) {
            Some(root) => Reparse {
                tree: Self {
                    source,
                    version,
                    root,
                },
                kind: ReparseKind::Partial,
            },
            None => Reparse {
                tree: Self::parse(&source, version),
                kind: ReparseKind::Full,
            },
        }
    }
}

impl GreenNode {
    pub fn kind(&self) -> &SyntaxKind {
        &self.kind
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn children(&self) -> &[GreenElement] {
        &self.children
    }

    fn push_text(&self, text: &mut String) {
        for child in &self.children {
            match child {
                GreenElement::Node(node) => node.push_text(text),
                GreenElement::Token(token) => text.push_str(&token.text),
            }
        }
    }
}

impl GreenToken {
    pub fn kind(&self) -> &SyntaxKind {
        &self.kind
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'a> SyntaxNode<'a> {
    pub fn kind(&self) -> &SyntaxKind {
        &self.node.kind
    }

    pub fn span(&self) -> Span {
        self.node.span
    }

    pub fn node_key(&self) -> Option<VersionedNodeKey> {
        let version = self.tree.version?;
        Some(VersionedNodeKey::child_path(
            version,
            self.node_key_kind(),
            NodeChildPath::from_steps(self.path.clone()),
        ))
    }

    fn node_key_kind(&self) -> NodeSyntaxKind {
        match self.kind() {
            SyntaxKind::SourceFile => NodeSyntaxKind::Module,
            SyntaxKind::Delimited { .. } => NodeSyntaxKind::Syntax,
            SyntaxKind::Token(_) | SyntaxKind::Whitespace | SyntaxKind::LineComment => {
                NodeSyntaxKind::Token
            }
        }
    }

    pub fn child_nodes(&self) -> Vec<SyntaxNode<'a>> {
        self.node
            .children
            .iter()
            .enumerate()
            .filter_map(|(index, child)| match child {
                GreenElement::Node(node) => {
                    let mut path = self.path.clone();
                    path.push(index as u32);
                    Some(SyntaxNode {
                        tree: self.tree,
                        path,
                        node,
                    })
                }
                GreenElement::Token(_) => None,
            })
            .collect()
    }

    pub fn tokens(&self) -> Vec<SyntaxToken> {
        let mut tokens = Vec::new();
        self.push_tokens(&mut tokens);
        tokens
    }

    fn push_tokens(&self, tokens: &mut Vec<SyntaxToken>) {
        for (index, child) in self.node.children.iter().enumerate() {
            let mut path = self.path.clone();
            path.push(index as u32);
            match child {
                GreenElement::Node(node) => SyntaxNode {
                    tree: self.tree,
                    path,
                    node,
                }
                .push_tokens(tokens),
                GreenElement::Token(token) => {
                    if let SyntaxKind::Token(kind) = &token.kind {
                        tokens.push(SyntaxToken {
                            kind: kind.clone(),
                            span: token.span,
                            text: token.text.clone(),
                            path: NodeChildPath::from_steps(path),
                            version: self.tree.version,
                        });
                    }
                }
            }
        }
    }
}

fn syntax_kind(kind: LosslessTokenKind) -> SyntaxKind {
    match kind {
        LosslessTokenKind::Token(kind) => SyntaxKind::Token(kind),
        LosslessTokenKind::Whitespace => SyntaxKind::Whitespace,
        LosslessTokenKind::LineComment => SyntaxKind::LineComment,
    }
}

fn build_green_root(source: &str, tokens: Vec<LosslessToken>) -> GreenNode {
    let end = tokens.last().map(|token| token.span.end).unwrap_or(0);
    // The root builder stays at stack[0] for the whole construction. The
    // remaining stack entries are open delimiter nodes; this invariant is why
    // the stack unwraps below are structural assertions rather than parser
    // diagnostics.
    let mut stack = vec![NodeBuilder::new(SyntaxKind::SourceFile, Span::new(0, end))];
    for token in tokens {
        let kind = syntax_kind(token.kind);
        let green_token = green_token(source, kind, token.span);
        match delimiter_open(green_token.kind()) {
            Some(open) => {
                let span = green_token.span();
                stack.push(NodeBuilder::new(
                    SyntaxKind::Delimited { open, close: None },
                    span,
                ));
                stack
                    .last_mut()
                    .expect("delimiter node")
                    .children
                    .push(GreenElement::Token(green_token));
            }
            None if let Some(close) = delimiter_close(green_token.kind())
                && stack.len() > 1 =>
            {
                let mut node = stack.pop().expect("delimiter node");
                if let SyntaxKind::Delimited {
                    close: node_close, ..
                } = &mut node.kind
                {
                    *node_close = Some(close);
                }
                node.span.end = green_token.span().end;
                node.children.push(GreenElement::Token(green_token));
                stack
                    .last_mut()
                    .expect("parent node")
                    .children
                    .push(GreenElement::Node(node.finish()));
            }
            None => stack
                .last_mut()
                .expect("current node")
                .children
                .push(GreenElement::Token(green_token)),
        }
    }
    // The root builder is never popped inside the token loop. Any remaining
    // builders are unmatched delimiter nodes and are attached back under root
    // so parsing can recover while preserving the original text.
    while stack.len() > 1 {
        let node = stack.pop().expect("unclosed delimiter node").finish();
        stack
            .last_mut()
            .expect("parent node")
            .children
            .push(GreenElement::Node(node));
    }
    stack.pop().expect("root node").finish()
}

#[derive(Debug)]
struct NodeBuilder {
    kind: SyntaxKind,
    span: Span,
    children: Vec<GreenElement>,
}

impl NodeBuilder {
    fn new(kind: SyntaxKind, span: Span) -> Self {
        Self {
            kind,
            span,
            children: Vec::new(),
        }
    }

    fn finish(self) -> GreenNode {
        GreenNode {
            kind: self.kind,
            span: self.span,
            children: self.children,
        }
    }
}

fn green_token(source: &str, kind: SyntaxKind, span: Span) -> GreenToken {
    let text = source.get(span.start..span.end).unwrap_or("").to_string();
    GreenToken { kind, span, text }
}

fn delimiter_open(kind: &SyntaxKind) -> Option<TokenKind> {
    match kind {
        SyntaxKind::Token(TokenKind::LParen)
        | SyntaxKind::Token(TokenKind::LBrace)
        | SyntaxKind::Token(TokenKind::LBracket) => token_kind(kind),
        _ => None,
    }
}

fn delimiter_close(kind: &SyntaxKind) -> Option<TokenKind> {
    match kind {
        SyntaxKind::Token(TokenKind::RParen)
        | SyntaxKind::Token(TokenKind::RBrace)
        | SyntaxKind::Token(TokenKind::RBracket) => token_kind(kind),
        _ => None,
    }
}

fn token_kind(kind: &SyntaxKind) -> Option<TokenKind> {
    match kind {
        SyntaxKind::Token(kind) => Some(kind.clone()),
        _ => None,
    }
}

fn apply_edit(source: &str, edit: &TextEdit) -> Option<String> {
    if edit.span.start > edit.span.end
        || edit.span.end > source.len()
        || !source.is_char_boundary(edit.span.start)
        || !source.is_char_boundary(edit.span.end)
    {
        return None;
    }

    let mut edited = String::with_capacity(source.len() - edit.span.len() + edit.replacement.len());
    edited.push_str(&source[..edit.span.start]);
    edited.push_str(&edit.replacement);
    edited.push_str(&source[edit.span.end..]);
    Some(edited)
}

fn try_partial_reparse(root: &GreenNode, source: &str, edit: &TextEdit) -> Option<GreenNode> {
    let target = find_single_token_edit(root, edit)?;
    let replacement_text = edited_token_text(target.text(), target.span(), edit)?;
    token_kind_matches(target.kind(), &replacement_text)?;
    Some(rewrite_after_single_token_edit(
        root,
        target.span(),
        replacement_text,
        edit,
        source.len(),
    ))
}

fn find_single_token_edit<'a>(node: &'a GreenNode, edit: &TextEdit) -> Option<&'a GreenToken> {
    for child in node.children() {
        match child {
            GreenElement::Node(node) => {
                if let Some(token) = find_single_token_edit(node, edit) {
                    return Some(token);
                }
            }
            GreenElement::Token(token)
                if token.span().start <= edit.span.start
                    && edit.span.end <= token.span().end
                    && token.span().start < token.span().end =>
            {
                return Some(token);
            }
            GreenElement::Token(_) => {}
        }
    }
    None
}

fn edited_token_text(token_text: &str, token_span: Span, edit: &TextEdit) -> Option<String> {
    let relative_start = edit.span.start.checked_sub(token_span.start)?;
    let relative_end = edit.span.end.checked_sub(token_span.start)?;
    if relative_start > relative_end
        || relative_end > token_text.len()
        || !token_text.is_char_boundary(relative_start)
        || !token_text.is_char_boundary(relative_end)
    {
        return None;
    }

    let mut text = String::with_capacity(
        token_text.len() - (relative_end - relative_start) + edit.replacement.len(),
    );
    text.push_str(&token_text[..relative_start]);
    text.push_str(&edit.replacement);
    text.push_str(&token_text[relative_end..]);
    Some(text)
}

fn token_kind_matches(kind: &SyntaxKind, text: &str) -> Option<()> {
    let mut tokens = tokenize_lossless(text).into_iter();
    let token = tokens.next()?;
    let eof = tokens.next()?;
    if tokens.next().is_some() {
        return None;
    }
    if token.span != Span::new(0, text.len())
        || eof.span != Span::new(text.len(), text.len())
        || !matches!(eof.kind, LosslessTokenKind::Token(TokenKind::Eof))
        || syntax_kind(token.kind) != *kind
    {
        return None;
    }
    Some(())
}

fn rewrite_after_single_token_edit(
    node: &GreenNode,
    target_span: Span,
    replacement_text: String,
    edit: &TextEdit,
    source_len: usize,
) -> GreenNode {
    let children = node
        .children()
        .iter()
        .map(|child| match child {
            GreenElement::Node(node) => GreenElement::Node(rewrite_after_single_token_edit(
                node,
                target_span,
                replacement_text.clone(),
                edit,
                source_len,
            )),
            GreenElement::Token(token) => GreenElement::Token(rewrite_token_after_edit(
                token,
                target_span,
                replacement_text.clone(),
                edit,
            )),
        })
        .collect::<Vec<_>>();

    let span = if matches!(node.kind(), SyntaxKind::SourceFile) {
        Span::new(0, source_len)
    } else {
        // All green nodes originate from token spans. Partial reparse only
        // rewrites an existing token, so non-root nodes remain non-empty.
        element_span(
            children.first().expect("green nodes always contain tokens"),
            children.last().expect("green nodes always contain tokens"),
        )
    };

    GreenNode {
        kind: node.kind().clone(),
        span,
        children,
    }
}

fn rewrite_token_after_edit(
    token: &GreenToken,
    target_span: Span,
    replacement_text: String,
    edit: &TextEdit,
) -> GreenToken {
    if token.span() == target_span {
        return GreenToken {
            kind: token.kind().clone(),
            span: Span::new(
                token.span().start,
                token.span().start + replacement_text.len(),
            ),
            text: replacement_text,
        };
    }

    GreenToken {
        kind: token.kind().clone(),
        span: shift_span(token.span(), edit),
        text: token.text().to_string(),
    }
}

fn shift_span(span: Span, edit: &TextEdit) -> Span {
    if span.start < edit.span.end {
        return span;
    }
    let delta = edit.replacement.len() as isize - edit.span.len() as isize;
    Span::new(
        shift_offset(span.start, delta),
        shift_offset(span.end, delta),
    )
}

fn shift_offset(offset: usize, delta: isize) -> usize {
    if delta.is_negative() {
        offset - delta.unsigned_abs()
    } else {
        offset + delta as usize
    }
}

fn element_span(first: &GreenElement, last: &GreenElement) -> Span {
    Span::new(element_start(first), element_end(last))
}

fn element_start(element: &GreenElement) -> usize {
    match element {
        GreenElement::Node(node) => node.span().start,
        GreenElement::Token(token) => token.span().start,
    }
}

fn element_end(element: &GreenElement) -> usize {
    match element {
        GreenElement::Node(node) => node.span().end,
        GreenElement::Token(token) => token.span().end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_lexer::TokenKind;
    use nia_node_id::NodePosition;
    use nia_source::{SourceId, SourceRevision};

    #[test]
    fn syntax_tree_preserves_full_source_text() {
        let source = "pub fn main() i32 { // keep me\n  0\n}\n";
        let tree = parse_source(source, None);

        assert_eq!(tree.full_text(), source);
        assert!(contains_token_kind(
            tree.green_root(),
            &SyntaxKind::LineComment
        ));
    }

    #[test]
    fn tokens_filter_trivia_but_keep_source_text() {
        let version = SourceVersion {
            id: SourceId(5),
            revision: SourceRevision(1),
        };
        let tree = parse_source("fn  main() // c\n{}", Some(version));
        let tokens = tree.tokens();
        let kinds = tokens
            .iter()
            .map(|token| token.kind.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            kinds,
            vec![
                TokenKind::Fn,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
        assert_eq!(tokens[1].text, "main");
        assert!(matches!(
            tokens[1].node_key().map(|key| key.position().clone()),
            Some(NodePosition::ChildPath(path)) if !path.steps().is_empty()
        ));
        assert_eq!(
            tokens[1].node_key().map(|key| key.source_version()),
            Some(version)
        );
    }

    #[test]
    fn token_cursor_finds_tokens_around_offsets_by_span() {
        let version = SourceVersion {
            id: SourceId(7),
            revision: SourceRevision(1),
        };
        let tree = parse_source("fn  main() {}\n", Some(version));
        let cursor = SyntaxTokenCursor::new(&tree);

        assert_eq!(
            cursor.token_at_or_after(0).map(|token| token.kind.clone()),
            Some(TokenKind::Fn)
        );
        assert_eq!(
            cursor.token_at_or_after(2).map(|token| token.kind.clone()),
            Some(TokenKind::Ident)
        );
        assert_eq!(
            cursor.token_before_or_at(2).map(|token| token.kind.clone()),
            Some(TokenKind::Fn)
        );
        assert_eq!(
            cursor.token_before_or_at(4).map(|token| token.kind.clone()),
            Some(TokenKind::Fn)
        );
        assert_eq!(
            cursor.token_before_or_at(8).map(|token| token.kind.clone()),
            Some(TokenKind::Ident)
        );
        assert!(cursor.token_at_or_after(tree.full_text().len()).is_none());
    }

    #[test]
    fn red_root_has_child_path_node_identity() {
        let version = SourceVersion {
            id: SourceId(2),
            revision: SourceRevision(5),
        };
        let tree = parse_source("fn main() () {}", Some(version));
        let key = tree.root().node_key().expect("root node key");

        assert_eq!(key.source_version(), version);
        assert_eq!(
            key.position(),
            &NodePosition::ChildPath(NodeChildPath::root())
        );
    }

    #[test]
    fn delimiter_groups_create_nested_child_paths() {
        let version = SourceVersion {
            id: SourceId(3),
            revision: SourceRevision(1),
        };
        let tree = parse_source("fn main() i32 { (1 + 2) }", Some(version));
        let children = tree.root().child_nodes();

        assert!(!children.is_empty());
        assert!(children.iter().any(|child| {
            matches!(
                child.kind(),
                SyntaxKind::Delimited {
                    open: TokenKind::LParen,
                    close: Some(TokenKind::RParen)
                }
            ) && matches!(
                child.node_key().map(|key| key.position().clone()),
                Some(NodePosition::ChildPath(path)) if !path.steps().is_empty()
            )
        }));
    }

    #[test]
    fn reparses_single_token_edit_partially() {
        let original_version = SourceVersion {
            id: SourceId(4),
            revision: SourceRevision(1),
        };
        let edited_version = SourceVersion {
            id: SourceId(4),
            revision: SourceRevision(2),
        };
        let source = "fn main() i32 { 1 }";
        let tree = parse_source(source, Some(original_version));
        let number = source.find('1').expect("number literal");
        let reparse = tree.reparse(
            TextEdit::replace(Span::new(number, number + 1), "2"),
            Some(edited_version),
        );

        assert_eq!(reparse.kind, ReparseKind::Partial);
        assert_eq!(reparse.tree.version(), Some(edited_version));
        assert_eq!(reparse.tree.full_text(), "fn main() i32 { 2 }");
        assert_token_kinds(
            &reparse.tree,
            &[TokenKind::Fn, TokenKind::Ident, TokenKind::LParen],
        );
        assert!(reparse.tree.tokens().iter().any(|token| token.text == "2"));
    }

    #[test]
    fn reparses_trivia_edit_partially() {
        let source = "fn main() i32 { // old\n  1\n}";
        let tree = parse_source(source, None);
        let old = source.find("old").expect("comment text");
        let reparse = tree.reparse(TextEdit::replace(Span::new(old, old + 3), "new"), None);

        assert_eq!(reparse.kind, ReparseKind::Partial);
        assert_eq!(reparse.tree.full_text(), "fn main() i32 { // new\n  1\n}");
    }

    #[test]
    fn reparse_falls_back_when_edit_changes_token_boundaries() {
        let source = "fn main() i32 { 1 }";
        let tree = parse_source(source, None);
        let number = source.find('1').expect("number literal");
        let reparse = tree.reparse(
            TextEdit::replace(Span::new(number, number + 1), "1 + 2"),
            None,
        );

        assert_eq!(reparse.kind, ReparseKind::Full);
        assert_eq!(reparse.tree.full_text(), "fn main() i32 { 1 + 2 }");
        assert!(
            reparse
                .tree
                .tokens()
                .iter()
                .any(|token| token.kind == TokenKind::Plus)
        );
    }

    fn assert_token_kinds(tree: &SyntaxTree, prefix: &[TokenKind]) {
        let actual = tree
            .tokens()
            .into_iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>();
        assert!(actual.starts_with(prefix), "{actual:?}");
    }

    fn contains_token_kind(node: &GreenNode, kind: &SyntaxKind) -> bool {
        node.children().iter().any(|child| match child {
            GreenElement::Node(node) => contains_token_kind(node, kind),
            GreenElement::Token(token) => token.kind() == kind,
        })
    }
}
