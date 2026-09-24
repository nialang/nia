// SPDX-License-Identifier: GPL-3.0-or-later
//! Lossless token trees and revision-aware incremental reparsing.
//!
//! Every tree owns a terminal EOF token, including trees reconstructed from a
//! caller-provided token stream. This keeps cursors total at end-of-input while
//! delimiter nodes preserve trivia and malformed source for parser recovery.

use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_node_id::{NodeChildPath, SyntaxKind as NodeSyntaxKind, VersionedNodeKey};
use nia_source::SourceVersion;
use nia_span::Span;
use std::sync::Arc;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenNode {
    /// Structural kind of this node.
    kind: SyntaxKind,
    /// Byte span covered by the node.
    span: Span,
    /// Ordered child nodes and tokens.
    children: Vec<GreenElement>,
}

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

/// Cursor over the significant token view of a [`SyntaxTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxTokenCursor {
    tokens: Vec<SyntaxToken>,
    pos: usize,
}

/// Replacement applied to a source byte span during reparsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// UTF-8 byte span to replace.
    pub span: Span,
    /// Replacement text.
    pub replacement: String,
}

/// Result of applying an edit to a syntax tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reparse {
    /// Tree built from the edited source.
    pub tree: SyntaxTree,
    /// Whether the existing tree was rewritten partially or rebuilt fully.
    pub kind: ReparseKind,
}

/// Strategy used to construct a reparsed tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReparseKind {
    /// A single token was rewritten while preserving the surrounding tree.
    Partial,
    /// The edited source required a complete lex and tree rebuild.
    Full,
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
    /// Events did not form exactly one source-file root.
    InvalidStructure,
    /// Emitted tokens did not reconstruct the complete source.
    InvalidSource,
}

/// Structural or lexical kind represented by a green tree element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyntaxKind {
    /// Root node covering the complete source.
    SourceFile,
    /// Syntax retained losslessly until its grammar production is migrated.
    Unparsed,
    /// Complete module grammar node.
    Module,
    /// Module using/import declaration grammar node.
    Using,
    /// Top-level or nested declaration grammar node.
    Item,
    /// Attribute grammar node.
    Attribute,
    /// Function declaration grammar node.
    Function,
    /// Struct declaration grammar node.
    Struct,
    /// Union declaration grammar node.
    Union,
    /// Enum declaration grammar node.
    Enum,
    /// Trait declaration grammar node.
    Trait,
    /// Extension declaration grammar node.
    Extend,
    /// Type grammar node.
    Type,
    /// Expression grammar node.
    Expr,
    /// Statement grammar node.
    Stmt,
    /// Pattern grammar node.
    Pattern,
    /// Parameter grammar node.
    Param,
    /// Aggregate field grammar node.
    Field,
    /// Enum variant grammar node.
    Variant,
    /// Trait or extension member grammar node.
    Member,
    /// Explicit missing construct inserted by recovery.
    Missing,
    /// Malformed source region retained by recovery.
    Error,
    /// Node grouped by an opening delimiter and optional matching close.
    Delimited {
        /// Opening delimiter token.
        open: TokenKind,
        /// Matching close token, or `None` for an unmatched opener.
        close: Option<TokenKind>,
    },
    /// Significant lexical token.
    Token(TokenKind),
    /// Whitespace trivia.
    Whitespace,
    /// Line-comment trivia.
    LineComment,
}

/// Borrowed red-tree view of a [`GreenNode`] with source identity context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode<'a> {
    tree: &'a SyntaxTree,
    path: Vec<u32>,
    node: &'a GreenNode,
}

/// Parses source text into a lossless syntax tree.
pub fn parse_source(source: &str, version: Option<SourceVersion>) -> SyntaxTree {
    SyntaxTree::parse(source, version)
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
}

impl SyntaxTokenCursor {
    /// Creates a cursor positioned at the first token.
    pub fn new(tree: &SyntaxTree) -> Self {
        Self {
            tokens: tree.root().tokens(),
            pos: 0,
        }
    }

    /// Returns all significant tokens, including the terminal EOF token.
    pub fn tokens(&self) -> &[SyntaxToken] {
        &self.tokens
    }

    /// Returns the current cursor position for later [`Self::rewind`].
    pub fn checkpoint(&self) -> usize {
        self.pos
    }

    /// Restores a checkpoint, clamping it to the terminal EOF position.
    pub fn rewind(&mut self, checkpoint: usize) {
        self.pos = checkpoint.min(self.tokens.len().saturating_sub(1));
    }

    /// Returns the current token; the cursor is always non-empty due to EOF.
    pub fn peek(&self) -> &SyntaxToken {
        &self.tokens[self.pos]
    }

    /// Returns and advances one token, stopping at EOF.
    pub fn bump(&mut self) -> SyntaxToken {
        let token = self.peek().clone();
        if token.kind != TokenKind::Eof {
            self.pos += 1;
        }
        token
    }

    /// Tests whether the current token has `kind`.
    pub fn at(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    /// Returns the token `offset` positions after the current token.
    pub fn nth(&self, offset: usize) -> Option<&SyntaxToken> {
        self.pos
            .checked_add(offset)
            .and_then(|index| self.tokens.get(index))
    }

    /// Returns the kind `offset` positions after the current token.
    pub fn nth_kind(&self, offset: usize) -> Option<&TokenKind> {
        self.nth(offset).map(|token| &token.kind)
    }

    /// Finds the first significant token whose end is at or after `offset`.
    pub fn token_at_or_after(&self, offset: usize) -> Option<&SyntaxToken> {
        let index = self
            .tokens
            .partition_point(|token| token.kind != TokenKind::Eof && token.span.end <= offset);
        self.tokens[index..]
            .iter()
            .find(|token| token.kind != TokenKind::Eof)
    }

    /// Finds the last significant token whose start is before `offset`.
    pub fn token_before_or_at(&self, offset: usize) -> Option<&SyntaxToken> {
        let index = self
            .tokens
            .partition_point(|token| token.kind != TokenKind::Eof && token.span.start < offset);
        self.tokens[..index]
            .iter()
            .rev()
            .find(|token| token.kind != TokenKind::Eof)
    }

    /// Returns the end offset of the token immediately before the cursor.
    pub fn previous_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        }
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
    /// Lexes and parses source text into a lossless syntax tree.
    pub fn parse(source: &str, version: Option<SourceVersion>) -> Self {
        let tokens = tokenize_lossless(source);
        Self::from_lossless_tokens(source, version, tokens)
    }

    /// Builds a tree from an already lossless token stream.
    pub fn from_lossless_tokens(
        source: &str,
        version: Option<SourceVersion>,
        mut tokens: Vec<LosslessToken>,
    ) -> Self {
        // This constructor is public for clients that already lexed a source.
        // Normalize the stream boundary so malformed/custom input cannot make
        // `SyntaxTokenCursor::peek` index an empty vector or expose tokens after
        // the first EOF marker.
        if let Some(eof) = tokens
            .iter()
            .position(|token| matches!(token.kind, LosslessTokenKind::Token(TokenKind::Eof)))
        {
            tokens.truncate(eof + 1);
            // EOF is a stream boundary, not a caller-owned source span. A
            // stale `0..0` (or out-of-range) EOF would otherwise shorten or
            // extend the root span and break offset-based consumers.
            if let Some(eof) = tokens.last_mut() {
                eof.span = Span::new(source.len(), source.len());
            }
        } else {
            tokens.push(LosslessToken {
                kind: LosslessTokenKind::Token(TokenKind::Eof),
                span: Span::new(source.len(), source.len()),
            });
        }
        let root = build_green_root(source, tokens);
        Self {
            source: source.to_string(),
            version,
            root,
        }
    }

    /// Builds a lossless tree from events emitted by a grammar parser.
    pub fn from_green_events(
        source: &str,
        version: Option<SourceVersion>,
        events: impl IntoIterator<Item = GreenEvent>,
    ) -> Result<Self, GreenBuildError> {
        let root = build_green_from_events(source, events)?;
        if root.kind != SyntaxKind::SourceFile {
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

    /// Applies an edit and chooses partial rewriting when token boundaries remain valid.
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

/// Builds a green tree from grammar parser events.
fn build_green_from_events(
    source: &str,
    events: impl IntoIterator<Item = GreenEvent>,
) -> Result<GreenNode, GreenBuildError> {
    let mut builder = GreenNodeBuilder::new(source);
    for event in events {
        match event {
            GreenEvent::Start(kind) => builder.start(kind)?,
            GreenEvent::Token(token) => builder.token(token)?,
            GreenEvent::Finish => builder.finish()?,
        }
    }
    builder.finish_root()
}

/// Incremental green tree builder driven by grammar events.
#[derive(Debug)]
struct GreenNodeBuilder<'a> {
    source: &'a str,
    stack: Vec<NodeBuilder>,
    finished: Option<GreenNode>,
    offset: usize,
}

impl<'a> GreenNodeBuilder<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            stack: Vec::new(),
            finished: None,
            offset: 0,
        }
    }

    fn start(&mut self, kind: SyntaxKind) -> Result<(), GreenBuildError> {
        if self.finished.is_some()
            || (self.stack.is_empty() && kind != SyntaxKind::SourceFile)
            || (!self.stack.is_empty() && kind == SyntaxKind::SourceFile)
        {
            return Err(GreenBuildError::InvalidStructure);
        }
        self.stack
            .push(NodeBuilder::new(kind, Span::new(self.offset, self.offset)));
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
        let kind = syntax_kind(token.kind);
        let green_token = green_token(self.source, kind, token.span);
        if node.children.is_empty() {
            node.span.start = token.span.start;
        }
        node.span.end = token.span.end;
        node.children.push(GreenElement::Token(green_token));
        self.offset = token.span.end;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), GreenBuildError> {
        let Some(node) = self.stack.pop() else {
            return Err(GreenBuildError::InvalidStructure);
        };
        let element = GreenElement::Node(node.finish());
        if let Some(parent) = self.stack.last_mut() {
            parent.push_element(element);
        } else if let GreenElement::Node(node) = element {
            self.finished = Some(node);
        }
        Ok(())
    }

    fn finish_root(self) -> Result<GreenNode, GreenBuildError> {
        if !self.stack.is_empty() || self.offset != self.source.len() {
            return Err(GreenBuildError::InvalidStructure);
        }
        self.finished.ok_or(GreenBuildError::InvalidStructure)
    }
}

impl GreenNode {
    /// Returns this node's structural kind.
    pub fn kind(&self) -> &SyntaxKind {
        &self.kind
    }

    /// Returns the byte span covered by this node.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns ordered child elements.
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
}

impl<'a> SyntaxNode<'a> {
    /// Returns this node's structural kind.
    pub fn kind(&self) -> &SyntaxKind {
        &self.node.kind
    }

    /// Returns the byte span covered by this node.
    pub fn span(&self) -> Span {
        self.node.span
    }

    /// Builds a versioned node key when the tree has a source version.
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
            SyntaxKind::Module => NodeSyntaxKind::Module,
            SyntaxKind::Using => NodeSyntaxKind::Item,
            SyntaxKind::Item
            | SyntaxKind::Attribute
            | SyntaxKind::Function
            | SyntaxKind::Struct
            | SyntaxKind::Union
            | SyntaxKind::Enum
            | SyntaxKind::Trait
            | SyntaxKind::Extend => NodeSyntaxKind::Item,
            SyntaxKind::Type => NodeSyntaxKind::Type,
            SyntaxKind::Expr => NodeSyntaxKind::Expr,
            SyntaxKind::Stmt => NodeSyntaxKind::Stmt,
            SyntaxKind::Pattern => NodeSyntaxKind::Pattern,
            SyntaxKind::Param => NodeSyntaxKind::Param,
            SyntaxKind::Field | SyntaxKind::Variant | SyntaxKind::Member => NodeSyntaxKind::Syntax,
            SyntaxKind::Unparsed
            | SyntaxKind::Missing
            | SyntaxKind::Error
            | SyntaxKind::Delimited { .. } => NodeSyntaxKind::Syntax,
            SyntaxKind::Token(_) | SyntaxKind::Whitespace | SyntaxKind::LineComment => {
                NodeSyntaxKind::Token
            }
        }
    }

    /// Returns direct nested child nodes in source order.
    pub fn child_nodes(&self) -> Vec<SyntaxNode<'a>> {
        self.node
            .children
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
    for (index, child) in node.children.iter().enumerate() {
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

fn build_green_root(source: &str, tokens: Vec<LosslessToken>) -> GreenNode {
    let mut events = Vec::with_capacity(tokens.len() * 2 + 2);
    events.push(GreenEvent::Start(SyntaxKind::SourceFile));
    let mut stack = Vec::new();
    for token in tokens {
        let kind = syntax_kind(token.kind.clone());
        match delimiter_open(&kind) {
            Some(open) => {
                events.push(GreenEvent::Start(SyntaxKind::Delimited {
                    open: open.clone(),
                    close: None,
                }));
                events.push(GreenEvent::Token(token));
                stack.push(open);
            }
            None if let Some(close) = delimiter_close(&kind)
                && !stack.is_empty()
                && stack
                    .last()
                    .is_some_and(|open| delimiter_matches(open, &close)) =>
            {
                events.push(GreenEvent::Token(token));
                events.push(GreenEvent::Finish);
                stack.pop();
            }
            None => {
                events.push(GreenEvent::Token(token));
            }
        }
    }
    while stack.pop().is_some() {
        events.push(GreenEvent::Finish);
    }
    events.push(GreenEvent::Finish);
    build_green_from_events(source, events)
        .expect("normalized lossless tokens form one source tree")
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

    fn finish(mut self) -> GreenNode {
        if let SyntaxKind::Delimited { open, close } = &mut self.kind {
            *close = self.children.last().and_then(|child| match child {
                GreenElement::Token(token) => match token.kind() {
                    SyntaxKind::Token(candidate) if delimiter_matches(open, candidate) => {
                        Some(candidate.clone())
                    }
                    _ => None,
                },
                GreenElement::Node(_) => None,
            });
        }
        GreenNode {
            kind: self.kind,
            span: self.span,
            children: self.children,
        }
    }

    fn push_element(&mut self, element: GreenElement) {
        let element_span = match &element {
            GreenElement::Node(node) => node.span,
            GreenElement::Token(token) => token.span,
        };
        if element_span.start != element_span.end {
            if self.children.is_empty() {
                self.span.start = element_span.start;
            }
            self.span.end = element_span.end;
        }
        self.children.push(element);
    }
}

fn green_token(source: &str, kind: SyntaxKind, span: Span) -> GreenToken {
    let text = Arc::from(source.get(span.start..span.end).unwrap_or(""));
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

fn delimiter_matches(kind: &TokenKind, close: &TokenKind) -> bool {
    matches!(
        (kind, close),
        (&TokenKind::LParen, &TokenKind::RParen)
            | (&TokenKind::LBrace, &TokenKind::RBrace)
            | (&TokenKind::LBracket, &TokenKind::RBracket)
    )
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
        &replacement_text,
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
    replacement_text: &str,
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
                replacement_text,
                edit,
                source_len,
            )),
            GreenElement::Token(token) => GreenElement::Token(rewrite_token_after_edit(
                token,
                target_span,
                replacement_text,
                edit,
            )),
        })
        .collect::<Vec<_>>();

    let span = if matches!(node.kind(), SyntaxKind::SourceFile) {
        Span::new(0, source_len)
    } else {
        // All green nodes originate from token spans. Partial reparse only
        // rewrites an existing token, so non-root nodes remain non-empty.
        match children.first().zip(children.last()) {
            Some((first, last)) => element_span(first, last),
            None => node.span(),
        }
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
    replacement_text: &str,
    edit: &TextEdit,
) -> GreenToken {
    if token.span() == target_span {
        return GreenToken {
            kind: token.kind().clone(),
            span: Span::new(
                token.span().start,
                token.span().start + replacement_text.len(),
            ),
            text: Arc::from(replacement_text),
        };
    }

    GreenToken {
        kind: token.kind().clone(),
        span: shift_span(token.span(), edit),
        text: Arc::clone(&token.text),
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
        offset.saturating_sub(delta.unsigned_abs())
    } else {
        offset.saturating_add(delta as usize)
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
            id: SourceId::isolated(),
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
        let tree = parse_source("fn", None);
        let green_text = match &tree.green_root().children()[0] {
            GreenElement::Token(token) => &token.text,
            GreenElement::Node(_) => panic!("expected source token"),
        };
        let mut cursor = SyntaxTokenCursor::new(&tree);

        assert!(Arc::ptr_eq(green_text, &cursor.tokens()[0].text));
        let bumped = cursor.bump();
        assert!(Arc::ptr_eq(green_text, &bumped.text));
    }

    #[test]
    fn token_cursor_finds_tokens_around_offsets_by_span() {
        let version = SourceVersion {
            id: SourceId::isolated(),
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
        assert!(cursor.nth(usize::MAX).is_none());
    }

    #[test]
    fn caller_supplied_token_stream_is_normalized_to_one_terminal_eof() {
        let tree = SyntaxTree::from_lossless_tokens("", None, Vec::new());
        let cursor = SyntaxTokenCursor::new(&tree);
        assert_eq!(cursor.tokens().len(), 1);
        assert_eq!(cursor.peek().kind, TokenKind::Eof);

        let tokens = vec![
            LosslessToken {
                kind: LosslessTokenKind::Token(TokenKind::Eof),
                span: Span::new(0, 0),
            },
            LosslessToken {
                kind: LosslessTokenKind::Token(TokenKind::Ident),
                span: Span::new(0, 0),
            },
        ];
        let tree = SyntaxTree::from_lossless_tokens("", None, tokens);
        assert_eq!(SyntaxTokenCursor::new(&tree).tokens().len(), 1);
    }

    #[test]
    fn caller_supplied_eof_is_relocated_to_source_end() {
        let tokens = vec![
            LosslessToken {
                kind: LosslessTokenKind::Token(TokenKind::Ident),
                span: Span::new(0, 3),
            },
            LosslessToken {
                kind: LosslessTokenKind::Token(TokenKind::Eof),
                span: Span::new(0, 0),
            },
        ];
        let tree = SyntaxTree::from_lossless_tokens("foo", None, tokens);

        assert_eq!(tree.green_root().span(), Span::new(0, 3));
        assert_eq!(
            tree.tokens().last().expect("terminal EOF").span,
            Span::new(3, 3)
        );
        assert_eq!(tree.full_text(), "foo");
    }

    #[test]
    fn unsupported_unicode_remains_lossless_during_error_recovery() {
        let source = "fn 中() {}";
        let tree = SyntaxTree::parse(source, None);

        assert_eq!(tree.full_text(), source);
        assert!(SyntaxTokenCursor::new(&tree).tokens().iter().any(|token| {
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
        let version = SourceVersion {
            id: SourceId::isolated(),
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
            id: SourceId::isolated(),
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
    fn grammar_events_build_nodes_without_losing_source() {
        let source = "fn main() {}";
        let tokens = tokenize_lossless(source);
        let mut events = vec![GreenEvent::Start(SyntaxKind::SourceFile)];
        events.push(GreenEvent::Start(SyntaxKind::Function));
        events.extend(tokens.into_iter().map(GreenEvent::Token));
        events.push(GreenEvent::Finish);
        events.push(GreenEvent::Finish);

        let tree = SyntaxTree::from_green_events(source, None, events).expect("valid events");
        assert_eq!(tree.full_text(), source);
        assert!(matches!(
            tree.root().child_nodes().first().map(SyntaxNode::kind),
            Some(SyntaxKind::Function)
        ));
    }

    #[test]
    fn grammar_events_reject_unbalanced_or_out_of_order_nodes() {
        let source = "fn";
        let error =
            SyntaxTree::from_green_events(source, None, [GreenEvent::Start(SyntaxKind::Function)])
                .expect_err("missing source root and finish");
        assert_eq!(error, GreenBuildError::InvalidStructure);
    }

    #[test]
    fn grammar_events_retain_zero_width_missing_nodes() {
        let source = "fn";
        let tokens = tokenize_lossless(source);
        let mut events = vec![
            GreenEvent::Start(SyntaxKind::SourceFile),
            GreenEvent::Start(SyntaxKind::Missing),
            GreenEvent::Finish,
        ];
        events.extend(tokens.into_iter().map(GreenEvent::Token));
        events.push(GreenEvent::Finish);

        let tree = SyntaxTree::from_green_events(source, None, events).expect("valid events");
        let children = tree.root().child_nodes();
        let missing = children.first().expect("missing node");
        assert_eq!(missing.kind(), &SyntaxKind::Missing);
        assert_eq!(missing.span(), Span::new(0, 0));
        assert_eq!(tree.full_text(), source);
    }

    #[test]
    fn zero_width_recovery_does_not_shrink_parent_span() {
        let source = "module module next;";
        let tokens = tokenize_lossless(source);
        let mut events = vec![GreenEvent::Start(SyntaxKind::SourceFile)];
        events.push(GreenEvent::Start(SyntaxKind::Module));
        events.push(GreenEvent::Start(SyntaxKind::Missing));
        events.push(GreenEvent::Finish);
        events.extend(tokens.into_iter().map(GreenEvent::Token));
        events.push(GreenEvent::Finish);
        events.push(GreenEvent::Finish);
        let tree = SyntaxTree::from_green_events(source, None, events).expect("valid event stream");
        let children = tree.root().child_nodes();
        let module = children.first().expect("module node");
        assert_eq!(module.span(), Span::new(0, source.len()));
    }

    #[test]
    fn mismatched_closers_do_not_close_the_wrong_delimiter() {
        let tree = parse_source("([)]", None);
        let root_children = tree.green_root().children();
        let Some(GreenElement::Node(paren)) = root_children.first() else {
            panic!("expected unmatched parenthesis node");
        };
        assert!(matches!(
            paren.kind(),
            SyntaxKind::Delimited {
                open: TokenKind::LParen,
                close: None
            }
        ));
        let Some(GreenElement::Node(bracket)) = paren.children().get(1) else {
            panic!("expected nested bracket node");
        };
        assert!(matches!(
            bracket.kind(),
            SyntaxKind::Delimited {
                open: TokenKind::LBracket,
                close: Some(TokenKind::RBracket)
            }
        ));
        assert!(bracket.children().iter().any(|child| {
            matches!(
                child,
                GreenElement::Token(token)
                    if matches!(token.kind(), SyntaxKind::Token(TokenKind::RParen))
            )
        }));
        assert_eq!(tree.full_text(), "([)]");
    }

    #[test]
    fn reparses_single_token_edit_partially() {
        let source_id = SourceId::isolated();
        let original_version = SourceVersion {
            id: source_id,
            revision: SourceRevision(1),
        };
        let edited_version = SourceVersion {
            id: source_id,
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
        assert!(
            reparse
                .tree
                .tokens()
                .iter()
                .any(|token| token.text.as_ref() == "2")
        );
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
