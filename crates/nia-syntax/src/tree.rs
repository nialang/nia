// SPDX-License-Identifier: GPL-3.0-or-later
//! Lossless green/red syntax storage.
//!
//! Green nodes are immutable and own exact token text; red views
//! ([`SyntaxNode`] and [`SyntaxToken`]) add structural child paths used for
//! stable node identity. Every tree owns a terminal EOF token so token views
//! are total at end of input.

use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind};
use nia_node_id::{NodeChildPath, SyntaxKind as NodeSyntaxKind, VersionedNodeKey};
use nia_source::SourceVersion;
use nia_span::Span;
use std::sync::{Arc, OnceLock};

use crate::SyntaxKind;

/// Lossless syntax tree for one source revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxTree {
    /// Original UTF-8 source text.
    source: String,
    /// Optional source identity attached to nodes and tokens.
    version: Option<SourceVersion>,
    /// Immutable green root containing trivia and malformed input.
    root: GreenNode,
}

/// Immutable syntax node in the lossless green tree.
#[derive(Debug, Clone)]
pub struct GreenNode {
    data: Arc<GreenNodeData>,
    start: usize,
    positioned_children: OnceLock<Arc<[GreenElement]>>,
}

/// Position-independent storage. Child positions are relative to this node.
#[derive(Debug)]
struct GreenNodeData {
    kind: SyntaxKind,
    width: usize,
    children: Box<[GreenElement]>,
}

impl PartialEq for GreenNode {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && (self.shares_structure(other)
                || (self.kind() == other.kind()
                    && self.span() == other.span()
                    && self.children() == other.children()))
    }
}

impl Eq for GreenNode {}

/// Either a nested green node or a terminal green token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreenElement {
    /// Nested structural node.
    Node(GreenNode),
    /// Terminal token, including trivia and EOF.
    Token(GreenToken),
}

/// Immutable terminal element in a green tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenToken {
    /// Token or trivia kind.
    kind: SyntaxKind,
    /// Byte span occupied by the token.
    span: Span,
    /// Exact source text for the token.
    text: Arc<str>,
}

/// Token view carrying source text, location, and optional node identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxToken {
    /// Lexical token kind (trivia is omitted from this view).
    pub kind: TokenKind,
    /// Byte span in the owning source text.
    pub span: Span,
    /// Exact token text.
    pub text: Arc<str>,
    path: NodeChildPath,
    version: Option<SourceVersion>,
}

/// Replacement applied to a source byte span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// UTF-8 byte span to replace.
    pub span: Span,
    /// Replacement text.
    pub replacement: String,
}

/// Event emitted by a grammar parser while constructing a green tree.
///
/// Events separate grammar recognition from tree allocation. A parser can
/// report malformed input with an error node or preserve a missing construct
/// without manufacturing a token, while the builder remains responsible for
/// immutable parent/child structure and source spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreenEvent {
    /// Starts a new green node that will contain subsequent events.
    Start(SyntaxKind),
    /// Adds one lossless token or trivia element to the current node.
    Token(LosslessToken),
    /// Finishes the most recently started node.
    Finish,
}

/// Invalid grammar event sequence or lossless source coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GreenBuildError {
    /// Events did not form exactly one root node.
    InvalidStructure,
    /// Emitted tokens did not reconstruct the covered source.
    InvalidSource,
}

/// Borrowed red-tree view of a [`GreenNode`] with source identity context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode<'a> {
    tree: &'a SyntaxTree,
    path: Vec<u32>,
    node: &'a GreenNode,
}

impl TextEdit {
    /// Creates an edit replacing `span` with `replacement` text.
    pub fn replace(span: Span, replacement: impl Into<String>) -> Self {
        Self {
            span,
            replacement: replacement.into(),
        }
    }

    /// Creates an insertion edit at a UTF-8 byte offset.
    pub fn insert(offset: usize, replacement: impl Into<String>) -> Self {
        Self::replace(Span::new(offset, offset), replacement)
    }

    /// Creates an edit deleting `span`.
    pub fn delete(span: Span) -> Self {
        Self::replace(span, "")
    }

    /// Applies the edit, rejecting spans outside `source` or off UTF-8 boundaries.
    pub fn apply(&self, source: &str) -> Option<String> {
        if self.span.start > self.span.end
            || self.span.end > source.len()
            || !source.is_char_boundary(self.span.start)
            || !source.is_char_boundary(self.span.end)
        {
            return None;
        }
        let mut edited =
            String::with_capacity(source.len() - self.span.len() + self.replacement.len());
        edited.push_str(&source[..self.span.start]);
        edited.push_str(&self.replacement);
        edited.push_str(&source[self.span.end..]);
        Some(edited)
    }

    /// Returns the signed byte-length change introduced by the edit.
    pub fn delta(&self) -> isize {
        self.replacement.len() as isize - self.span.len() as isize
    }
}

impl SyntaxToken {
    /// Returns the token's structural child path within the tree.
    pub fn child_path(&self) -> &NodeChildPath {
        &self.path
    }

    /// Returns the optional source version attached to this token.
    pub fn source_version(&self) -> Option<SourceVersion> {
        self.version
    }

    /// Builds a versioned node key for this token when a source version exists.
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
    pub(crate) fn from_root_children(
        source: String,
        version: Option<SourceVersion>,
        children: Vec<GreenElement>,
    ) -> Result<Self, GreenBuildError> {
        let root = GreenNode::new(SyntaxKind::SourceFile, Span::new(0, source.len()), children);
        let tree = Self {
            source,
            version,
            root,
        };
        if tree.full_text() != tree.source {
            return Err(GreenBuildError::InvalidSource);
        }
        Ok(tree)
    }

    /// Builds a lossless tree from events emitted by a grammar parser.
    ///
    /// The events must describe exactly one [`SyntaxKind::SourceFile`] root
    /// whose tokens reconstruct `source`, ending with the terminal EOF token.
    pub fn from_green_events(
        source: &str,
        version: Option<SourceVersion>,
        events: impl IntoIterator<Item = GreenEvent>,
    ) -> Result<Self, GreenBuildError> {
        let root = build_green(source, 0, events)?;
        if *root.kind() != SyntaxKind::SourceFile || root.span() != Span::new(0, source.len()) {
            return Err(GreenBuildError::InvalidStructure);
        }
        let tree = Self {
            source: source.to_owned(),
            version,
            root,
        };
        if tree.full_text() != source {
            return Err(GreenBuildError::InvalidSource);
        }
        Ok(tree)
    }

    /// Returns the original source text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the optional source version attached to this tree.
    pub fn version(&self) -> Option<SourceVersion> {
        self.version
    }

    /// Returns the borrowed red root node.
    pub fn root(&self) -> SyntaxNode<'_> {
        SyntaxNode {
            tree: self,
            path: Vec::new(),
            node: &self.root,
        }
    }

    /// Returns the immutable green root node.
    pub fn green_root(&self) -> &GreenNode {
        &self.root
    }

    /// Returns significant tokens in source order, including EOF.
    pub fn tokens(&self) -> Vec<SyntaxToken> {
        self.root().tokens()
    }

    /// Reconstructs the exact source text represented by the tree.
    pub fn full_text(&self) -> String {
        let mut text = String::new();
        self.root.push_text(&mut text);
        text
    }

    /// Replaces the child node at `path` and shifts every later span.
    ///
    /// `replacement` must cover `old_span + delta` of `source`, where the old
    /// node at `path` covered `old_span` before `edit`.
    pub fn splice_node(
        &self,
        path: &[u32],
        replacement: GreenNode,
        edit: &TextEdit,
        version: Option<SourceVersion>,
    ) -> Option<Self> {
        let source = edit.apply(&self.source)?;
        let root = splice_green(&self.root, path, replacement, edit, source.len())?;
        let tree = Self {
            source,
            version,
            root,
        };
        (tree.full_text() == tree.source).then_some(tree)
    }
}

impl GreenNode {
    fn new(kind: SyntaxKind, span: Span, children: Vec<GreenElement>) -> Self {
        let children = children
            .into_iter()
            .map(|child| relocate_element(&child, -(span.start as isize)))
            .collect();
        Self {
            data: Arc::new(GreenNodeData {
                kind,
                width: span.len(),
                children,
            }),
            start: span.start,
            positioned_children: OnceLock::new(),
        }
    }

    fn relocated(&self, delta: isize) -> Self {
        if delta == 0 {
            return self.clone();
        }
        Self {
            data: self.data.clone(),
            start: shift_offset(self.start, delta),
            positioned_children: OnceLock::new(),
        }
    }

    /// Reports whether both handles reuse the same immutable node storage.
    /// Positions and source revisions are deliberately excluded.
    pub fn shares_structure(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }

    pub(crate) fn shifted(&self, edit: &TextEdit) -> Self {
        self.relocated(if self.start >= edit.span.end {
            edit.delta()
        } else {
            0
        })
    }

    /// Builds one standalone green node from grammar events.
    ///
    /// The events must form exactly one root node of any kind whose tokens
    /// are contiguous and begin at `start`. Fragments carry alternative
    /// interpretations of source that the primary tree already covers.
    pub fn from_events(
        source: &str,
        start: usize,
        events: impl IntoIterator<Item = GreenEvent>,
    ) -> Result<Self, GreenBuildError> {
        build_green(source, start, events)
    }

    /// Returns this node's structural kind.
    pub fn kind(&self) -> &SyntaxKind {
        &self.data.kind
    }

    /// Returns the byte span covered by this node.
    pub fn span(&self) -> Span {
        Span::new(self.start, self.start + self.data.width)
    }

    /// Returns ordered child elements.
    pub fn children(&self) -> &[GreenElement] {
        if self.start == 0 {
            return &self.data.children;
        }
        self.positioned_children.get_or_init(|| {
            self.data
                .children
                .iter()
                .map(|child| relocate_element(child, self.start as isize))
                .collect()
        })
    }

    /// Returns direct child nodes in source order.
    pub fn child_nodes(&self) -> impl Iterator<Item = &GreenNode> {
        self.children().iter().filter_map(|child| match child {
            GreenElement::Node(node) => Some(node),
            GreenElement::Token(_) => None,
        })
    }

    /// Returns direct significant tokens in source order.
    pub fn child_tokens(&self) -> impl Iterator<Item = &GreenToken> {
        self.children().iter().filter_map(|child| match child {
            GreenElement::Token(token) if !token.kind.is_trivia() => Some(token),
            _ => None,
        })
    }

    /// Returns the first direct child node of `kind`.
    pub fn child(&self, kind: &SyntaxKind) -> Option<&GreenNode> {
        self.child_nodes().find(|node| node.kind() == kind)
    }

    /// Returns the first direct significant token of `kind`.
    pub fn token(&self, kind: &TokenKind) -> Option<&GreenToken> {
        self.child_tokens()
            .find(|token| token.kind.token_kind() == Some(kind))
    }

    /// Returns the first significant descendant token.
    pub fn first_token(&self) -> Option<&GreenToken> {
        self.children().iter().find_map(|child| match child {
            GreenElement::Node(node) => node.first_token(),
            GreenElement::Token(token) if !token.kind.is_trivia() => Some(token),
            GreenElement::Token(_) => None,
        })
    }

    /// Returns the last significant descendant token.
    pub fn last_token(&self) -> Option<&GreenToken> {
        self.children().iter().rev().find_map(|child| match child {
            GreenElement::Node(node) => node.last_token(),
            GreenElement::Token(token) if !token.kind.is_trivia() => Some(token),
            GreenElement::Token(_) => None,
        })
    }

    /// Returns the byte span from the first to the last significant token.
    pub fn token_span(&self) -> Option<Span> {
        Some(Span::new(
            self.first_token()?.span.start,
            self.last_token()?.span.end,
        ))
    }

    fn push_text(&self, text: &mut String) {
        for child in self.children() {
            match child {
                GreenElement::Node(node) => node.push_text(text),
                GreenElement::Token(token) => text.push_str(&token.text),
            }
        }
    }
}

impl GreenToken {
    /// Returns this token's syntax kind.
    pub fn kind(&self) -> &SyntaxKind {
        &self.kind
    }

    /// Returns the byte span covered by this token.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the exact token text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Reports whether this is a significant token of `kind`.
    pub fn is(&self, kind: &TokenKind) -> bool {
        self.kind.token_kind() == Some(kind)
    }
}

impl<'a> SyntaxNode<'a> {
    /// Returns this node's structural kind.
    pub fn kind(&self) -> &SyntaxKind {
        self.node.kind()
    }

    /// Returns the byte span covered by this node.
    pub fn span(&self) -> Span {
        self.node.span()
    }

    /// Returns the underlying green node.
    pub fn green(&self) -> &'a GreenNode {
        self.node
    }

    /// Returns this node's structural child path.
    pub fn path(&self) -> &[u32] {
        &self.path
    }

    /// Builds a versioned node key when the tree has a source version.
    pub fn node_key(&self) -> Option<VersionedNodeKey> {
        let version = self.tree.version?;
        Some(VersionedNodeKey::child_path(
            version,
            self.kind().node_key_kind(),
            NodeChildPath::from_steps(self.path.clone()),
        ))
    }

    /// Returns direct nested child nodes in source order.
    pub fn child_nodes(&self) -> Vec<SyntaxNode<'a>> {
        self.node
            .children()
            .iter()
            .enumerate()
            .filter_map(|(index, child)| match child {
                GreenElement::Node(node) => {
                    let mut path = self.path.clone();
                    path.push(u32::try_from(index).ok()?);
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

    /// Returns significant descendant tokens in source order, including EOF.
    pub fn tokens(&self) -> Vec<SyntaxToken> {
        let mut tokens = Vec::new();
        let mut path = self.path.clone();
        push_tokens(self.tree, self.node, &mut path, &mut tokens);
        tokens
    }
}

fn push_tokens(
    tree: &SyntaxTree,
    node: &GreenNode,
    path: &mut Vec<u32>,
    tokens: &mut Vec<SyntaxToken>,
) {
    for (index, child) in node.children().iter().enumerate() {
        let Some(index) = u32::try_from(index).ok() else {
            continue;
        };
        path.push(index);
        match child {
            GreenElement::Node(node) => push_tokens(tree, node, path, tokens),
            GreenElement::Token(token) => {
                if let SyntaxKind::Token(kind) = &token.kind {
                    tokens.push(SyntaxToken {
                        kind: kind.clone(),
                        span: token.span,
                        text: token.text.clone(),
                        path: NodeChildPath::from_slice(path),
                        version: tree.version,
                    });
                }
            }
        }
        path.pop();
    }
}

fn syntax_kind(kind: LosslessTokenKind) -> SyntaxKind {
    match kind {
        LosslessTokenKind::Token(kind) => SyntaxKind::Token(kind),
        LosslessTokenKind::Whitespace => SyntaxKind::Whitespace,
        LosslessTokenKind::LineComment => SyntaxKind::LineComment,
    }
}

/// Builds exactly one green node from grammar events.
fn build_green(
    source: &str,
    start: usize,
    events: impl IntoIterator<Item = GreenEvent>,
) -> Result<GreenNode, GreenBuildError> {
    let mut builder = GreenNodeBuilder {
        source,
        stack: Vec::new(),
        finished: None,
        offset: start,
    };
    for event in events {
        match event {
            GreenEvent::Start(kind) => builder.start(kind)?,
            GreenEvent::Token(token) => builder.token(token)?,
            GreenEvent::Finish => builder.finish()?,
        }
    }
    if !builder.stack.is_empty() {
        return Err(GreenBuildError::InvalidStructure);
    }
    builder.finished.ok_or(GreenBuildError::InvalidStructure)
}

/// Incremental green tree builder driven by grammar events.
#[derive(Debug)]
struct GreenNodeBuilder<'a> {
    source: &'a str,
    stack: Vec<NodeBuilder>,
    finished: Option<GreenNode>,
    offset: usize,
}

impl GreenNodeBuilder<'_> {
    fn start(&mut self, kind: SyntaxKind) -> Result<(), GreenBuildError> {
        if self.finished.is_some() || kind.token_kind().is_some() || kind.is_trivia() {
            return Err(GreenBuildError::InvalidStructure);
        }
        self.stack.push(NodeBuilder {
            kind,
            span: Span::new(self.offset, self.offset),
            children: Vec::new(),
        });
        Ok(())
    }

    fn token(&mut self, token: LosslessToken) -> Result<(), GreenBuildError> {
        let Some(node) = self.stack.last_mut() else {
            return Err(GreenBuildError::InvalidStructure);
        };
        if token.span.start != self.offset
            || token.span.end > self.source.len()
            || self.source.get(token.span.start..token.span.end).is_none()
        {
            return Err(GreenBuildError::InvalidSource);
        }
        let green_token = GreenToken {
            kind: syntax_kind(token.kind),
            span: token.span,
            text: Arc::from(&self.source[token.span.start..token.span.end]),
        };
        node.push_element(GreenElement::Token(green_token));
        self.offset = token.span.end;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), GreenBuildError> {
        let Some(node) = self.stack.pop() else {
            return Err(GreenBuildError::InvalidStructure);
        };
        let node = node.finish(self.offset);
        if let Some(parent) = self.stack.last_mut() {
            parent.push_element(GreenElement::Node(node));
        } else {
            self.finished = Some(node);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct NodeBuilder {
    kind: SyntaxKind,
    span: Span,
    children: Vec<GreenElement>,
    // Zero-width recovery children do not move the parent's span boundaries.
}

impl NodeBuilder {
    fn finish(mut self, offset: usize) -> GreenNode {
        if self.children.is_empty() {
            self.span = Span::new(offset, offset);
        }
        GreenNode::new(self.kind, self.span, self.children)
    }

    fn push_element(&mut self, element: GreenElement) {
        let element_span = match &element {
            GreenElement::Node(node) => node.span(),
            GreenElement::Token(token) => token.span,
        };
        let has_width = self.span.start != self.span.end;
        if element_span.start != element_span.end {
            if !has_width {
                self.span.start = element_span.start;
            }
            self.span.end = element_span.end;
        } else if !has_width {
            self.span = element_span;
        }
        self.children.push(element);
    }
}

fn element_width(element: &GreenElement) -> usize {
    match element {
        GreenElement::Node(node) => node.span().len(),
        GreenElement::Token(token) => token.span.len(),
    }
}

fn splice_green(
    node: &GreenNode,
    path: &[u32],
    replacement: GreenNode,
    edit: &TextEdit,
    source_len: usize,
) -> Option<GreenNode> {
    let (&index, rest) = path.split_first()?;
    let index = usize::try_from(index).ok()?;
    let mut replacement = Some(replacement);
    let mut children = Vec::with_capacity(node.children().len());
    for (position, child) in node.children().iter().enumerate() {
        let child = match (position.cmp(&index), child) {
            (std::cmp::Ordering::Less, child) => child.clone(),
            (std::cmp::Ordering::Equal, GreenElement::Node(child)) => {
                let replacement = replacement.take()?;
                GreenElement::Node(if rest.is_empty() {
                    replacement
                } else {
                    splice_green(child, rest, replacement, edit, source_len)?
                })
            }
            (std::cmp::Ordering::Equal, GreenElement::Token(_)) => return None,
            (std::cmp::Ordering::Greater, child) => shift_element(child, edit),
        };
        children.push(child);
    }
    replacement.is_none().then_some(())?;
    let span = if matches!(node.kind(), SyntaxKind::SourceFile) {
        Span::new(0, source_len)
    } else {
        rebuilt_span(node, &children, edit)
    };
    Some(GreenNode::new(node.kind().clone(), span, children))
}

fn rebuilt_span(node: &GreenNode, children: &[GreenElement], edit: &TextEdit) -> Span {
    let mut widths = children
        .iter()
        .filter(|child| element_width(child) != 0)
        .map(|child| match child {
            GreenElement::Node(node) => node.span(),
            GreenElement::Token(token) => token.span,
        });
    match widths.next() {
        Some(first) => Span::new(first.start, widths.next_back().unwrap_or(first).end),
        None => shift_span(node.span(), edit),
    }
}

pub(crate) fn shift_element(element: &GreenElement, edit: &TextEdit) -> GreenElement {
    match element {
        GreenElement::Node(node) => GreenElement::Node(node.shifted(edit)),
        GreenElement::Token(token) => GreenElement::Token(shift_token(token, edit)),
    }
}

fn relocate_element(element: &GreenElement, delta: isize) -> GreenElement {
    match element {
        GreenElement::Node(node) => GreenElement::Node(node.relocated(delta)),
        GreenElement::Token(token) => GreenElement::Token(GreenToken {
            kind: token.kind.clone(),
            span: Span::new(
                shift_offset(token.span.start, delta),
                shift_offset(token.span.end, delta),
            ),
            text: token.text.clone(),
        }),
    }
}

fn shift_token(token: &GreenToken, edit: &TextEdit) -> GreenToken {
    GreenToken {
        kind: token.kind().clone(),
        span: shift_span(token.span(), edit),
        text: Arc::clone(&token.text),
    }
}

/// Shifts a span that lies at or after the end of an edit.
pub fn shift_span(span: Span, edit: &TextEdit) -> Span {
    if span.start < edit.span.end {
        return span;
    }
    let delta = edit.delta();
    Span::new(
        shift_offset(span.start, delta),
        shift_offset(span.end, delta),
    )
}

fn shift_offset(offset: usize, delta: isize) -> usize {
    if delta.is_negative() {
        offset.saturating_sub(delta.unsigned_abs())
    } else {
        offset.saturating_add(delta as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_lexer::tokenize_lossless;
    use nia_node_id::NodePosition;
    use nia_source::{SourceId, SourceRevision};

    fn flat(source: &str, version: Option<SourceVersion>) -> SyntaxTree {
        let mut events = vec![GreenEvent::Start(SyntaxKind::SourceFile)];
        events.extend(tokenize_lossless(source).into_iter().map(GreenEvent::Token));
        events.push(GreenEvent::Finish);
        SyntaxTree::from_green_events(source, version, events).expect("flat token tree")
    }

    fn version(revision: u64) -> SourceVersion {
        SourceVersion {
            id: SourceId::isolated(),
            revision: SourceRevision(revision),
        }
    }

    #[test]
    fn syntax_tree_preserves_full_source_text() {
        let source = "pub fn main() i32 { // keep me\n  0\n}\n";
        let tree = flat(source, None);

        assert_eq!(tree.full_text(), source);
        assert!(
            tree.green_root()
                .children()
                .iter()
                .any(|child| matches!(child, GreenElement::Token(token) if token.kind() == &SyntaxKind::LineComment))
        );
    }

    #[test]
    fn tokens_filter_trivia_but_keep_source_text() {
        let version = version(1);
        let tree = flat("fn  main() // c\n{}", Some(version));
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
        assert_eq!(tokens[1].text.as_ref(), "main");
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
    fn token_views_share_green_text_storage() {
        let tree = flat("fn", None);
        let GreenElement::Token(green) = &tree.green_root().children()[0] else {
            panic!("expected source token");
        };
        assert!(Arc::ptr_eq(&green.text, &tree.tokens()[0].text));
    }

    #[test]
    fn unsupported_unicode_remains_lossless() {
        let source = "fn 中() {}";
        let tree = flat(source, None);

        assert_eq!(tree.full_text(), source);
        assert!(tree.tokens().iter().any(|token| {
            matches!(
                token.kind,
                TokenKind::Error(nia_lexer::LexError::UnexpectedByte(_))
            ) && token.text.as_ref() == "中"
        }));
    }

    #[test]
    fn offset_shifting_saturates_malformed_extremes() {
        assert_eq!(shift_offset(usize::MAX, 1), usize::MAX);
        assert_eq!(shift_offset(0, -1), 0);
    }

    #[test]
    fn red_root_has_child_path_node_identity() {
        let version = version(5);
        let tree = flat("fn main() () {}", Some(version));
        let key = tree.root().node_key().expect("root node key");

        assert_eq!(key.source_version(), version);
        assert_eq!(
            key.position(),
            &NodePosition::ChildPath(NodeChildPath::root())
        );
    }

    #[test]
    fn grammar_events_build_nodes_without_losing_source() {
        let source = "fn main() {}";
        let mut events = vec![GreenEvent::Start(SyntaxKind::SourceFile)];
        events.push(GreenEvent::Start(SyntaxKind::FunctionDecl));
        let mut tokens = tokenize_lossless(source);
        let eof = tokens.pop().expect("eof");
        events.extend(tokens.into_iter().map(GreenEvent::Token));
        events.push(GreenEvent::Finish);
        events.push(GreenEvent::Token(eof));
        events.push(GreenEvent::Finish);

        let tree = SyntaxTree::from_green_events(source, None, events).expect("valid events");
        assert_eq!(tree.full_text(), source);
        let children = tree.root().child_nodes();
        assert_eq!(children[0].kind(), &SyntaxKind::FunctionDecl);
        assert_eq!(children[0].span(), Span::new(0, source.len()));
        assert_eq!(
            children[0].node_key().map(|key| key.kind()),
            None,
            "unversioned trees have no node keys"
        );
    }

    #[test]
    fn grammar_events_reject_unbalanced_or_out_of_order_nodes() {
        let error = SyntaxTree::from_green_events(
            "fn",
            None,
            [GreenEvent::Start(SyntaxKind::FunctionDecl)],
        )
        .expect_err("missing source root and finish");
        assert_eq!(error, GreenBuildError::InvalidStructure);
    }

    #[test]
    fn grammar_events_retain_zero_width_missing_nodes() {
        let source = "fn";
        let mut events = vec![
            GreenEvent::Start(SyntaxKind::SourceFile),
            GreenEvent::Start(SyntaxKind::Missing),
            GreenEvent::Finish,
        ];
        events.extend(tokenize_lossless(source).into_iter().map(GreenEvent::Token));
        events.push(GreenEvent::Finish);

        let tree = SyntaxTree::from_green_events(source, None, events).expect("valid events");
        let children = tree.root().child_nodes();
        assert_eq!(children[0].kind(), &SyntaxKind::Missing);
        assert_eq!(children[0].span(), Span::new(0, 0));
        assert_eq!(tree.full_text(), source);
    }

    #[test]
    fn zero_width_recovery_does_not_shrink_parent_span() {
        let source = "module module next;";
        let mut tokens = tokenize_lossless(source);
        let eof = tokens.pop().expect("eof");
        let mut events = vec![
            GreenEvent::Start(SyntaxKind::SourceFile),
            GreenEvent::Start(SyntaxKind::ModuleDecl),
            GreenEvent::Start(SyntaxKind::Missing),
            GreenEvent::Finish,
        ];
        events.extend(tokens.into_iter().map(GreenEvent::Token));
        events.push(GreenEvent::Start(SyntaxKind::Missing));
        events.push(GreenEvent::Finish);
        events.push(GreenEvent::Finish);
        events.push(GreenEvent::Token(eof));
        events.push(GreenEvent::Finish);
        let tree = SyntaxTree::from_green_events(source, None, events).expect("valid event stream");
        let module = &tree.root().child_nodes()[0];
        assert_eq!(module.span(), Span::new(0, source.len()));
    }

    #[test]
    fn fragments_start_at_their_first_token() {
        let source = "a + b";
        let tokens = tokenize_lossless(source);
        let events = vec![
            GreenEvent::Start(SyntaxKind::NameExpr),
            GreenEvent::Token(tokens[4].clone()),
            GreenEvent::Finish,
        ];
        let node = GreenNode::from_events(source, 4, events).expect("fragment");
        assert_eq!(node.span(), Span::new(4, 5));
        assert_eq!(node.first_token().map(GreenToken::text), Some("b"));
    }

    #[test]
    fn edits_reject_non_utf8_boundaries() {
        assert!(
            TextEdit::replace(Span::new(1, 2), "x")
                .apply("中")
                .is_none()
        );
        assert_eq!(TextEdit::insert(1, "b").apply("ac").as_deref(), Some("abc"));
    }
}
