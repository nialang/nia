// SPDX-License-Identifier: GPL-3.0-or-later
//! Reparse clean function blocks whose enclosing grammar did not read their interior.

use super::*;
use crate::SyntaxKind;
use crate::grammar::parser::BlockRegion;

impl Parse {
    pub(super) fn reparse_block(
        &self,
        edit: &TextEdit,
        version: Option<SourceVersion>,
        source: &str,
        tokens: &[LosslessToken],
    ) -> Result<Option<Self>, GreenBuildError> {
        if !self.errors.is_empty()
            || tokens.iter().any(|token| {
                matches!(
                    token.kind,
                    LosslessTokenKind::Token(nia_lexer::TokenKind::Error(_))
                )
            })
        {
            return Ok(None);
        }
        let mut candidates = Vec::new();
        find_blocks(
            self.tree.green_root(),
            edit.span,
            false,
            &mut Vec::new(),
            &mut candidates,
        );
        for (span, path) in candidates.into_iter().rev() {
            let Some(region) = self.blocks.iter().find(|region| region.span == span) else {
                continue;
            };
            if region.prefix_read_end > span.start + 1 {
                continue;
            }
            let iteration_index = self
                .iterations
                .partition_point(|iteration| iteration.start <= span.start)
                .saturating_sub(1);
            if self.iterations[..iteration_index]
                .iter()
                .any(|iteration| iteration.read_end > span.start + 1)
            {
                continue;
            }
            let inside = |other: Span| span.start <= other.start && other.end <= span.end;
            if self.alternates.iter().any(|alternate| {
                alternate.span.start < span.end
                    && span.start < alternate.span.end
                    && !inside(alternate.span)
            }) {
                continue;
            }
            let mut parser = Parser::new(source, tokens.to_vec());
            parser.seek(span.start);
            let Some(parsed) = grammar::block(&mut parser) else {
                continue;
            };
            if parsed.span != Span::new(span.start, shift_offset(span.end, edit))
                || !parser.errors.is_empty()
            {
                continue;
            }
            let read_end = parser.read_end();
            let GrammarOutput {
                events,
                errors,
                alternates,
                blocks,
            } = parser.into_parts()?;
            let replacement = GreenNode::from_events(source, span.start, events)?;
            let Some(tree) = self.tree.splice_node(&path, replacement, edit, version) else {
                continue;
            };
            let first = self
                .alternates
                .partition_point(|alternate| alternate.span.end <= span.start);
            let removed = self
                .alternates
                .iter()
                .filter(|alternate| inside(alternate.span))
                .count();
            let added = alternates.len();
            let mut all_alternates = self.alternates[..first].to_vec();
            all_alternates.extend(alternates);
            all_alternates.extend(self.alternates[first + removed..].iter().map(|alternate| {
                Alternate {
                    span: shift_span(alternate.span, edit),
                    root: alternate.root.shifted(edit),
                    ..alternate.clone()
                }
            }));
            let mut iterations = self.iterations.clone();
            for (index, iteration) in iterations.iter_mut().enumerate().skip(iteration_index) {
                iteration.start = shift_offset(iteration.start, edit);
                iteration.read_end = shift_offset(iteration.read_end, edit);
                if index == iteration_index {
                    iteration.read_end = iteration.read_end.max(read_end);
                }
                iteration.alternates_end = iteration.alternates_end - removed + added;
            }
            let mut all_blocks = self
                .blocks
                .iter()
                .filter(|region| !inside(region.span))
                .map(|region| {
                    let mut region = *region;
                    if region.span.start < span.start && region.span.end >= span.end {
                        region.span.end = shift_offset(region.span.end, edit);
                    } else {
                        region.span = shift_span(region.span, edit);
                    }
                    region.prefix_read_end = if region.prefix_read_end > edit.span.start
                        && region.prefix_read_end < edit.span.end
                    {
                        edit.span.start + edit.replacement.len()
                    } else {
                        shift_offset(region.prefix_read_end, edit)
                    };
                    region
                })
                .collect::<Vec<BlockRegion>>();
            // Regions are recorded in post-order; enclosing blocks follow the
            // reparsed interior exactly as they would in a clean parse.
            let at = all_blocks.partition_point(|region| region.span.end <= span.start);
            all_blocks.splice(at..at, blocks);
            return Ok(Some(finish_parse(
                tree,
                errors,
                all_alternates,
                iterations,
                all_blocks,
            )));
        }
        Ok(None)
    }
}

fn find_blocks(
    node: &GreenNode,
    edit: Span,
    in_body: bool,
    path: &mut Vec<u32>,
    candidates: &mut Vec<(Span, Vec<u32>)>,
) {
    let kind = node.kind();
    if kind.is_type()
        || matches!(
            kind,
            SyntaxKind::Attribute
                | SyntaxKind::Error
                | SyntaxKind::TypeArgList
                | SyntaxKind::BracketArgList
        )
    {
        return;
    }
    if in_body
        && *kind == SyntaxKind::Block
        && node.span().start < edit.start
        && edit.end < node.span().end
    {
        candidates.push((node.span(), path.clone()));
    }
    for (index, child) in node.children().iter().enumerate() {
        if let GreenElement::Node(child) = child
            && child.span().start <= edit.start
            && edit.end <= child.span().end
        {
            path.push(index as u32);
            find_blocks(
                child,
                edit,
                in_body
                    || (*kind == SyntaxKind::FunctionDecl && *child.kind() == SyntaxKind::Block),
                path,
                candidates,
            );
            path.pop();
        }
    }
}
