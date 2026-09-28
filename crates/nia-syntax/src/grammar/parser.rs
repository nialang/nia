// SPDX-License-Identifier: GPL-3.0-or-later
//! Grammar parser state: token cursor, event stream, speculation, and errors.
//!
//! Productions build nodes through [`Parser::node`] and
//! [`Parser::node_preceding`]. A production that fails after consuming input
//! becomes an [`SyntaxKind::Error`] node; one that fails without consuming is
//! abandoned so its parent keeps any recovery markers. Speculative parses use
//! [`Parser::checkpoint`] and [`Parser::rewind`], which roll back tokens,
//! events, forward-parent links, and alternates as one transaction.

use std::cell::Cell;

use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind};
use nia_span::Span;

use crate::{
    Alternate, AlternateKind, GreenBuildError, GreenEvent, GreenNode, ParseErrorKind, SyntaxKind,
};

/// One grammar event before forward parents are resolved.
#[derive(Debug, Clone)]
pub(crate) enum Event {
    Start {
        kind: Option<SyntaxKind>,
        forward_parent: Option<usize>,
    },
    Finish,
    Token(usize),
}

/// A grammar error before its token identity is resolved against the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawError {
    pub(crate) span: Span,
    pub(crate) kind: ParseErrorKind,
    pub(crate) message: String,
    /// Token span, resolved against the finished tree after any incremental edit.
    pub(crate) token: Option<Span>,
}

/// Grammar products of one top-level source-file iteration.
///
/// An iteration's output is a function of the tokens from `start` through
/// `read_end`, which lets incremental reparse reuse unaffected iterations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Iteration {
    /// Start byte of the first significant token the iteration consumed.
    pub(crate) start: usize,
    /// End byte of the farthest significant token read so far.
    pub(crate) read_end: usize,
    /// Grammar errors reported so far, excluding lexical errors.
    pub(crate) errors_end: usize,
    /// Alternates captured so far.
    pub(crate) alternates_end: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockRegion {
    pub(crate) span: Span,
    /// Farthest enclosing read before entry, including abandoned speculation.
    pub(crate) prefix_read_end: usize,
}

/// Grammar facts about a completed production that later decisions need.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Shape(u8);

impl Shape {
    /// A type path whose segments carry no type arguments.
    pub(crate) const PATH_WITHOUT_ARGS: Self = Self(1);
    /// A type path with exactly one named segment.
    pub(crate) const SINGLE_NAME_PATH: Self = Self(1 << 1);
    /// An expression that is a name or a `::` chain rooted at a name.
    pub(crate) const VALUE_PATH: Self = Self(1 << 2);
    /// A range with a start bound.
    pub(crate) const RANGE_START: Self = Self(1 << 3);
    /// A range with an end bound.
    pub(crate) const RANGE_END: Self = Self(1 << 4);

    pub(crate) fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Result of a successful production.
#[derive(Debug, Clone)]
pub(crate) struct Parsed {
    /// Kind that grammar decisions observe; a parenthesized production
    /// reports the construct it lowers to.
    pub(crate) class: SyntaxKind,
    /// Span of the AST value this production lowers to.
    pub(crate) span: Span,
    pub(crate) shape: Shape,
    /// Start event of the outermost node, where a new parent is attached.
    start_event: usize,
    /// Start event of the innermost first node, where the production begins.
    first_event: usize,
}

/// Completion request returned by a production body.
pub(crate) struct Done {
    kind: SyntaxKind,
    class: SyntaxKind,
    span: Span,
    shape: Shape,
}

impl Done {
    pub(crate) fn new(kind: SyntaxKind, span: Span) -> Option<Self> {
        Self::shaped(kind, span, Shape::default())
    }

    pub(crate) fn shaped(kind: SyntaxKind, span: Span, shape: Shape) -> Option<Self> {
        Some(Self {
            class: kind.clone(),
            kind,
            span,
            shape,
        })
    }

    /// Completes a node that lowers to `inner` under a new span.
    pub(crate) fn wrapping(kind: SyntaxKind, inner: &Parsed, span: Span) -> Option<Self> {
        Some(Self {
            kind,
            class: inner.class.clone(),
            span,
            shape: inner.shape,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Checkpoint {
    pub(crate) pos: usize,
    emitted: usize,
    events: usize,
    forward_log: usize,
    alternates: usize,
    blocks: usize,
}

pub(crate) struct Parser<'s> {
    pub(crate) source: &'s str,
    tokens: Vec<LosslessToken>,
    significant: Vec<usize>,
    pos: usize,
    emitted: usize,
    /// Farthest significant-token index any production has read.
    max_read: Cell<usize>,
    events: Vec<Event>,
    pub(crate) errors: Vec<RawError>,
    forward_log: Vec<usize>,
    alternates: Vec<RawAlternate>,
    blocks: Vec<BlockRegion>,
}

/// Alternative interpretation captured from a speculative production.
#[derive(Clone)]
pub(crate) struct RawAlternate {
    kind: AlternateKind,
    span: Span,
    events: Vec<Event>,
}

/// Detached production and the alternate readings needed to lower its children.
pub(crate) struct Snapshot {
    events: Vec<Event>,
    alternates: Vec<RawAlternate>,
}

pub(crate) struct GrammarOutput {
    pub(crate) events: Vec<GreenEvent>,
    pub(crate) errors: Vec<RawError>,
    pub(crate) alternates: Vec<Alternate>,
    pub(crate) blocks: Vec<BlockRegion>,
}

impl<'s> Parser<'s> {
    pub(crate) fn new(source: &'s str, tokens: Vec<LosslessToken>) -> Self {
        let significant = tokens
            .iter()
            .enumerate()
            .filter_map(|(index, token)| match token.kind {
                LosslessTokenKind::Token(_) => Some(index),
                LosslessTokenKind::Whitespace | LosslessTokenKind::LineComment => None,
            })
            .collect();
        Self {
            source,
            tokens,
            significant,
            pos: 0,
            emitted: 0,
            max_read: Cell::new(0),
            events: Vec::new(),
            errors: Vec::new(),
            forward_log: Vec::new(),
            alternates: Vec::new(),
            blocks: Vec::new(),
        }
    }

    /// Positions the parser at the significant token starting at `start`,
    /// with every earlier lossless token treated as already emitted.
    pub(crate) fn seek(&mut self, start: usize) {
        let emitted = self
            .tokens
            .partition_point(|token| token.span.start < start);
        self.emitted = emitted;
        self.pos = self.significant.partition_point(|&index| index < emitted);
        self.max_read.set(self.pos);
    }

    /// Starts attributing token reads to a new top-level iteration.
    pub(crate) fn begin_iteration(&self) {
        self.max_read.set(self.pos);
    }

    pub(crate) fn read_end(&self) -> usize {
        self.tokens[self.significant[self.max_read.get()]].span.end
    }

    pub(crate) fn record_block(&mut self, span: Span, prefix_read_end: usize) {
        self.blocks.push(BlockRegion {
            span,
            prefix_read_end,
        });
    }

    /// Records the products of the top-level iteration that began at `start`.
    pub(crate) fn iteration(&self, start: usize) -> Iteration {
        Iteration {
            start: self.token(start).span.start,
            read_end: self.token(self.max_read.get()).span.end,
            errors_end: self.errors.len(),
            alternates_end: self.alternates.len(),
        }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Start byte of the current significant token.
    pub(crate) fn current_start(&self) -> usize {
        self.span().start
    }

    /// Lossless tokens that remain unemitted before the current token.
    pub(crate) fn emit_trivia(&mut self) {
        self.flush_trivia();
    }

    /// Opens the source-file root.
    pub(crate) fn start_root(&mut self) {
        self.events.push(Event::Start {
            kind: Some(SyntaxKind::SourceFile),
            forward_parent: None,
        });
    }

    /// Emits the remaining trivia and EOF, then closes the root.
    pub(crate) fn finish_root(&mut self) {
        self.emit_until(self.tokens.len());
        self.finish_fragment();
    }

    /// Closes a parsed region without consuming the untouched suffix.
    pub(crate) fn finish_fragment(&mut self) {
        self.events.push(Event::Finish);
    }

    pub(crate) fn into_parts(self) -> Result<GrammarOutput, GreenBuildError> {
        let events = resolve_events(&self.tokens, self.events);
        let alternates = self
            .alternates
            .into_iter()
            .map(|alternate| {
                let events = resolve_events(&self.tokens, alternate.events);
                let start = events
                    .iter()
                    .find_map(|event| match event {
                        GreenEvent::Token(token) => Some(token.span.start),
                        GreenEvent::Start(_) | GreenEvent::Finish => None,
                    })
                    .ok_or(GreenBuildError::InvalidStructure)?;
                let root = GreenNode::from_events(self.source, start, events)?;
                Ok(Alternate {
                    kind: alternate.kind,
                    span: alternate.span,
                    root,
                })
            })
            .collect::<Result<_, GreenBuildError>>()?;
        Ok(GrammarOutput {
            events,
            errors: self.errors,
            alternates,
            blocks: self.blocks,
        })
    }

    // ---- node construction ------------------------------------------------

    /// Runs a production inside a new node.
    pub(crate) fn node(&mut self, body: impl FnOnce(&mut Self) -> Option<Done>) -> Option<Parsed> {
        self.flush_trivia();
        let start_pos = self.pos;
        let start_event = self.events.len();
        self.events.push(Event::Start {
            kind: None,
            forward_parent: None,
        });
        match body(self) {
            Some(done) => {
                self.complete(start_event, done.kind.clone());
                Some(Parsed {
                    class: done.class,
                    span: done.span,
                    shape: done.shape,
                    start_event,
                    first_event: start_event,
                })
            }
            None => {
                if self.pos != start_pos {
                    self.complete(start_event, SyntaxKind::Error);
                }
                None
            }
        }
    }

    /// Runs a production inside a new node that also contains `lhs`.
    pub(crate) fn node_preceding(
        &mut self,
        lhs: &Parsed,
        body: impl FnOnce(&mut Self) -> Option<Done>,
    ) -> Option<Parsed> {
        let start_event = self.events.len();
        self.events.push(Event::Start {
            kind: None,
            forward_parent: None,
        });
        if let Event::Start { forward_parent, .. } = &mut self.events[lhs.start_event] {
            *forward_parent = Some(start_event - lhs.start_event);
            self.forward_log.push(lhs.start_event);
        }
        match body(self) {
            Some(done) => {
                self.complete(start_event, done.kind.clone());
                Some(Parsed {
                    class: done.class,
                    span: done.span,
                    shape: done.shape,
                    start_event,
                    first_event: lhs.first_event,
                })
            }
            None => {
                self.complete(start_event, SyntaxKind::Error);
                None
            }
        }
    }

    /// Wraps tokens consumed by `recover` in an error node.
    pub(crate) fn recover_in_error(&mut self, recover: impl FnOnce(&mut Self)) {
        let start_pos = self.pos;
        self.flush_trivia();
        let start_event = self.events.len();
        self.events.push(Event::Start {
            kind: None,
            forward_parent: None,
        });
        recover(self);
        if self.pos != start_pos {
            self.complete(start_event, SyntaxKind::Error);
        }
    }

    fn complete(&mut self, start_event: usize, kind: SyntaxKind) {
        if let Event::Start { kind: slot, .. } = &mut self.events[start_event] {
            *slot = Some(kind);
        }
        self.events.push(Event::Finish);
    }

    /// Marks a construct the grammar required but did not find.
    pub(crate) fn missing(&mut self) {
        self.events.push(Event::Start {
            kind: Some(SyntaxKind::Missing),
            forward_parent: None,
        });
        self.events.push(Event::Finish);
    }

    /// Copies the events of a just-completed production for later capture.
    pub(crate) fn snapshot(&self, parsed: &Parsed, checkpoint: Checkpoint) -> Snapshot {
        Snapshot {
            events: detach_events(self.events[parsed.first_event..].to_vec()),
            alternates: self.alternates[checkpoint.alternates..].to_vec(),
        }
    }

    /// Records a previously snapshotted production as an alternate reading.
    pub(crate) fn push_alternate(&mut self, kind: AlternateKind, span: Span, snapshot: Snapshot) {
        // Rewinding the speculative production removes its nested alternates.
        // Restore them along with the saved tree, including readings that the
        // selected expression interpretation did not visit.
        for alternate in snapshot.alternates {
            if !self
                .alternates
                .iter()
                .any(|existing| existing.kind == alternate.kind && existing.span == alternate.span)
            {
                self.alternates.push(alternate);
            }
        }
        self.alternates.push(RawAlternate {
            kind,
            span,
            events: snapshot.events,
        });
    }

    // ---- speculation ------------------------------------------------------

    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            pos: self.pos,
            emitted: self.emitted,
            events: self.events.len(),
            forward_log: self.forward_log.len(),
            alternates: self.alternates.len(),
            blocks: self.blocks.len(),
        }
    }

    pub(crate) fn rewind(&mut self, checkpoint: Checkpoint) {
        for &event in &self.forward_log[checkpoint.forward_log..] {
            if event < checkpoint.events
                && let Event::Start { forward_parent, .. } = &mut self.events[event]
            {
                *forward_parent = None;
            }
        }
        self.forward_log.truncate(checkpoint.forward_log);
        self.events.truncate(checkpoint.events);
        self.alternates.truncate(checkpoint.alternates);
        self.blocks.truncate(checkpoint.blocks);
        self.pos = checkpoint.pos;
        self.emitted = checkpoint.emitted;
    }

    pub(crate) fn errors_len(&self) -> usize {
        self.errors.len()
    }

    pub(crate) fn truncate_errors(&mut self, len: usize) {
        self.errors.truncate(len);
    }

    pub(crate) fn ensure_progress(&mut self, checkpoint: Checkpoint) {
        if self.pos == checkpoint.pos && !self.at(TokenKind::Eof) {
            self.bump();
        }
    }

    // ---- token cursor -----------------------------------------------------

    fn token(&self, pos: usize) -> &LosslessToken {
        let pos = pos.min(self.significant.len() - 1);
        if pos > self.max_read.get() {
            self.max_read.set(pos);
        }
        &self.tokens[self.significant[pos]]
    }

    pub(crate) fn kind(&self) -> &TokenKind {
        self.nth_kind(0).unwrap_or(&TokenKind::Eof)
    }

    pub(crate) fn nth_kind(&self, offset: usize) -> Option<&TokenKind> {
        let pos = self.pos.checked_add(offset)?;
        let &index = self.significant.get(pos)?;
        if pos > self.max_read.get() {
            self.max_read.set(pos);
        }
        match &self.tokens[index].kind {
            LosslessTokenKind::Token(kind) => Some(kind),
            LosslessTokenKind::Whitespace | LosslessTokenKind::LineComment => None,
        }
    }

    pub(crate) fn at(&self, kind: TokenKind) -> bool {
        *self.kind() == kind
    }

    pub(crate) fn at_any(&self, kinds: &[TokenKind]) -> bool {
        kinds.contains(self.kind())
    }

    pub(crate) fn span(&self) -> Span {
        self.token(self.pos).span
    }

    pub(crate) fn text(&self) -> &'s str {
        let span = self.span();
        &self.source[span.start..span.end]
    }

    pub(crate) fn previous_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.token(self.pos - 1).span.end
        }
    }

    /// Consumes the current token, stopping at EOF. Returns its span.
    pub(crate) fn bump(&mut self) -> Span {
        let span = self.span();
        if self.at(TokenKind::Eof) {
            return span;
        }
        let index = self.significant[self.pos];
        self.emit_until(index + 1);
        self.pos += 1;
        span
    }

    pub(crate) fn eat(&mut self, kind: TokenKind) -> Option<Span> {
        self.at(kind).then(|| self.bump())
    }

    fn flush_trivia(&mut self) {
        if let Some(&index) = self.significant.get(self.pos) {
            self.emit_until(index);
        }
    }

    fn emit_until(&mut self, end: usize) {
        while self.emitted < end {
            self.events.push(Event::Token(self.emitted));
            self.emitted += 1;
        }
    }

    pub(crate) fn has_line_break_between(&self, start: usize, end: usize) -> bool {
        self.source
            .get(start..end)
            .is_some_and(|text| text.bytes().any(|byte| byte == b'\n' || byte == b'\r'))
    }

    // ---- errors -----------------------------------------------------------

    pub(crate) fn at_lexical_error(&self) -> bool {
        matches!(self.kind(), TokenKind::Error(_))
    }

    pub(crate) fn error_here(&mut self, message: impl Into<String>) {
        self.error_here_as(ParseErrorKind::Grammar, message);
    }

    pub(crate) fn error_here_as(&mut self, kind: ParseErrorKind, message: impl Into<String>) {
        if self.at_lexical_error() {
            return;
        }
        self.errors.push(RawError {
            span: self.span(),
            kind,
            message: message.into(),
            token: Some(self.span()),
        });
    }

    /// Reports a missing construct at the current token, naming what was found.
    pub(crate) fn expected_here(&mut self, kind: ParseErrorKind, message: &str) {
        let message = format!("{message}; found {}", self.found_token_description());
        self.error_here_as(kind, message);
        self.missing();
    }

    pub(crate) fn error_at(&mut self, span: Span, message: impl Into<String>) {
        self.error_at_as(ParseErrorKind::Grammar, span, message);
    }

    pub(crate) fn error_at_as(
        &mut self,
        kind: ParseErrorKind,
        span: Span,
        message: impl Into<String>,
    ) {
        if self.at_lexical_error() && span == self.span() && kind != ParseErrorKind::Lexical {
            return;
        }
        self.errors.push(RawError {
            span,
            kind,
            message: message.into(),
            token: None,
        });
    }

    pub(crate) fn error_at_end_as(
        &mut self,
        kind: ParseErrorKind,
        span: Span,
        message: impl Into<String>,
    ) {
        if self.at_lexical_error() && kind != ParseErrorKind::Lexical {
            return;
        }
        let start = span.end.saturating_sub(1).max(span.start);
        self.error_at_as(kind, Span::new(start, span.end), message);
    }

    pub(crate) fn expect(&mut self, kind: TokenKind, message: &str) -> Option<Span> {
        if self.at(kind.clone()) {
            Some(self.bump())
        } else {
            self.expected_here(ParseErrorKind::for_expected_token(&kind), message);
            None
        }
    }

    pub(crate) fn expect_semicolon_after(&mut self, anchor: Span, message: &str) -> Option<Span> {
        if self.at(TokenKind::Semicolon) {
            Some(self.bump())
        } else {
            let message = format!("{message}; found {}", self.found_token_description());
            self.error_at_end_as(ParseErrorKind::MissingSemicolon, anchor, message);
            self.missing();
            None
        }
    }

    /// Expects an identifier and returns its span.
    pub(crate) fn expect_name(&mut self, message: &str) -> Option<Span> {
        if self.at(TokenKind::Ident) {
            Some(self.bump())
        } else {
            self.expected_here(ParseErrorKind::ExpectedName, message);
            None
        }
    }

    fn found_token_description(&self) -> String {
        if self.at(TokenKind::Eof) {
            return "end of file".to_string();
        }
        let text = self.text().trim();
        if text.is_empty() {
            return "an unexpected token".to_string();
        }
        let mut chars = text.chars();
        let prefix = chars.by_ref().take(32).collect::<String>();
        if chars.next().is_some() {
            format!("`{prefix}...`")
        } else {
            format!("`{prefix}`")
        }
    }
}

/// Converts forward-parent events into nested green events.
fn resolve_events(tokens: &[LosslessToken], mut events: Vec<Event>) -> Vec<GreenEvent> {
    let mut green = Vec::with_capacity(events.len());
    let mut chain = Vec::new();
    for index in 0..events.len() {
        match std::mem::replace(&mut events[index], Event::Finish) {
            Event::Start {
                kind,
                forward_parent,
            } => {
                chain.clear();
                chain.push(kind);
                let mut cursor = index;
                let mut next = forward_parent;
                while let Some(offset) = next {
                    cursor += offset;
                    match std::mem::replace(
                        &mut events[cursor],
                        Event::Start {
                            kind: None,
                            forward_parent: None,
                        },
                    ) {
                        Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            chain.push(kind);
                            next = forward_parent;
                        }
                        _ => break,
                    }
                }
                for kind in chain.drain(..).rev().flatten() {
                    green.push(GreenEvent::Start(kind));
                }
            }
            Event::Finish => green.push(GreenEvent::Finish),
            Event::Token(token) => green.push(GreenEvent::Token(tokens[token].clone())),
        }
    }
    green
}

/// Drops forward-parent links that leave an extracted event slice.
fn detach_events(events: Vec<Event>) -> Vec<Event> {
    // Forward-parent links are relative offsets, so an extracted contiguous
    // slice keeps every link whose target lies inside the slice.
    let len = events.len();
    events
        .into_iter()
        .enumerate()
        .map(|(index, event)| match event {
            Event::Start {
                kind,
                forward_parent,
            } => Event::Start {
                kind,
                forward_parent: forward_parent.filter(|offset| index + offset < len),
            },
            other => other,
        })
        .collect()
}
