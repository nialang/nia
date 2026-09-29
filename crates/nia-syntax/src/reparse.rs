// SPDX-License-Identifier: GPL-3.0-or-later
//! Reparse a safe enclosing block or declaration regions affected by an edit.
//!
//! Lexing still covers the whole new source. Grammar work starts at the first
//! affected iteration and ends at an unchanged iteration boundary. Recovery
//! may consume further declarations, so synchronization is decided by the
//! grammar cursor, never merely by balancing delimiters in edited text.

use nia_lexer::{LosslessToken, LosslessTokenKind, tokenize_lossless};
use nia_source::SourceVersion;
use nia_span::Span;

use crate::grammar::parser::{GrammarOutput, Iteration, Parser, RawError};
use crate::tree::shift_element;
use crate::{
    Alternate, GreenBuildError, GreenElement, GreenNode, GreenToken, Parse, SyntaxTree, TextEdit,
    finish_parse, grammar, parse, shift_span,
};

mod block;

impl Parse {
    /// Applies a UTF-8 edit and reparses a safe block or affected declarations.
    ///
    /// Unchanged declaration products are reused after accounting for grammar
    /// lookahead. Diagnostics and AST-origin token paths belong to `version`.
    /// Invalid edit ranges are rejected without changing this parse.
    pub fn reparse(
        &self,
        edit: &TextEdit,
        version: Option<SourceVersion>,
    ) -> Result<Self, GreenBuildError> {
        let source = edit
            .apply(self.tree.source())
            .ok_or(GreenBuildError::InvalidSource)?;
        if self.iterations.is_empty() {
            return parse(&source, version);
        }
        let tokens = tokenize_lossless(&source);
        let mut old_tokens = Vec::new();
        collect_tokens(self.tree.green_root(), &mut old_tokens);
        let equal = |old: &&GreenToken, new: &LosslessToken| token_matches(old, new, &source);
        let prefix = old_tokens
            .iter()
            .zip(&tokens)
            .take_while(|(old, new)| equal(old, new))
            .count();
        if prefix == old_tokens.len() && prefix == tokens.len() {
            let tree = SyntaxTree::from_root_children(
                source,
                version,
                self.tree.green_root().children().to_vec(),
            )?;
            return Ok(finish_parse(
                tree,
                self.grammar_errors.clone(),
                self.alternates.clone(),
                self.iterations.clone(),
                self.blocks.clone(),
            ));
        }
        let suffix = old_tokens[prefix..]
            .iter()
            .rev()
            .zip(tokens[prefix..].iter().rev())
            .take_while(|(old, new)| equal(old, new))
            .count();
        let changed_start = old_tokens
            .get(prefix)
            .map_or(edit.span.start, |token| token.span().start)
            .min(edit.span.start);
        let suffix_start = old_tokens
            .get(old_tokens.len() - suffix)
            .map(|token| token.span().start);
        if let Some(reparsed) = self.reparse_block(edit, version, &source, &tokens)? {
            return Ok(reparsed);
        }
        let mut first = self
            .iterations
            .iter()
            .position(|iteration| iteration.read_end >= changed_start)
            .unwrap_or(self.iterations.len() - 1);
        // Trivia between declarations belongs to the preceding region, even
        // when neither production inspected its text.
        if first > 0 && self.iterations[first].start > changed_start {
            first -= 1;
        }
        let start = if first == 0 {
            0
        } else {
            self.iterations[first].start
        };
        let error_prefix = first
            .checked_sub(1)
            .map_or(0, |index| self.iterations[index].errors_end);
        let alternate_prefix = first
            .checked_sub(1)
            .map_or(0, |index| self.iterations[index].alternates_end);

        let mut parser = Parser::new(&source, tokens);
        parser.seek(start);
        parser.start_root();
        let mut iterations = Vec::new();
        let mut reused = None;
        let candidates = self
            .iterations
            .iter()
            .enumerate()
            .skip(first + 1)
            .filter(|(_, iteration)| {
                iteration.start >= edit.span.end
                    && suffix_start.is_some_and(|suffix| iteration.start >= suffix)
            })
            .map(|(index, iteration)| (shift_offset(iteration.start, edit), index))
            .collect::<Vec<_>>();
        let synchronized = grammar::source_file(&mut parser, &mut iterations, |offset| {
            reused = candidates
                .binary_search_by_key(&offset, |&(start, _)| start)
                .ok()
                .map(|index| candidates[index].1);
            reused.is_some()
        });
        if synchronized {
            parser.emit_trivia();
            parser.finish_fragment();
        } else {
            parser.finish_root();
        }
        let GrammarOutput {
            events,
            errors,
            alternates,
            blocks,
        } = parser.into_parts()?;
        let fragment = GreenNode::from_events(&source, start, events)?;
        let old_children = self.tree.green_root().children();
        let mut children = old_children
            .iter()
            .take_while(|child| element_span(child).start < start)
            .cloned()
            .collect::<Vec<_>>();
        children.extend_from_slice(fragment.children());
        if let Some(index) = reused {
            let suffix_start = self.iterations[index].start;
            children.extend(
                old_children
                    .iter()
                    .filter(|child| element_span(child).start >= suffix_start)
                    .map(|child| shift_element(child, edit)),
            );
        }
        let tree = SyntaxTree::from_root_children(source, version, children)?;
        let mut grammar_errors = self.grammar_errors[..error_prefix].to_vec();
        grammar_errors.extend(errors);
        let mut all_alternates = self.alternates[..alternate_prefix].to_vec();
        all_alternates.extend(alternates);
        let mut all_blocks = self
            .blocks
            .iter()
            .filter(|block| block.span.end <= start)
            .copied()
            .collect::<Vec<_>>();
        all_blocks.extend(blocks);
        let mut all_iterations = self.iterations[..first].to_vec();
        all_iterations.extend(iterations.into_iter().map(|iteration| Iteration {
            errors_end: iteration.errors_end + error_prefix,
            alternates_end: iteration.alternates_end + alternate_prefix,
            ..iteration
        }));
        if let Some(index) = reused {
            all_blocks.extend(
                self.blocks
                    .iter()
                    .filter(|block| block.span.start >= self.iterations[index].start)
                    .map(|block| crate::grammar::parser::BlockRegion {
                        span: shift_span(block.span, edit),
                        prefix_read_end: shift_offset(block.prefix_read_end, edit),
                    }),
            );
            let old_errors = self.iterations[index - 1].errors_end;
            let old_alternates = self.iterations[index - 1].alternates_end;
            let new_errors = grammar_errors.len();
            let new_alternates = all_alternates.len();
            grammar_errors.extend(
                self.grammar_errors[old_errors..]
                    .iter()
                    .map(|error| RawError {
                        span: shift_span(error.span, edit),
                        token: error.token.map(|span| shift_span(span, edit)),
                        ..error.clone()
                    }),
            );
            all_alternates.extend(self.alternates[old_alternates..].iter().map(|alternate| {
                Alternate {
                    kind: alternate.kind,
                    span: shift_span(alternate.span, edit),
                    root: alternate.root.shifted(edit),
                }
            }));
            all_iterations.extend(self.iterations[index..].iter().map(|iteration| Iteration {
                start: shift_offset(iteration.start, edit),
                read_end: shift_offset(iteration.read_end, edit),
                errors_end: iteration.errors_end - old_errors + new_errors,
                alternates_end: iteration.alternates_end - old_alternates + new_alternates,
            }));
        }
        Ok(finish_parse(
            tree,
            grammar_errors,
            all_alternates,
            all_iterations,
            all_blocks,
        ))
    }
}

fn shift_offset(offset: usize, edit: &TextEdit) -> usize {
    shift_span(Span::new(offset, offset), edit).start
}

fn element_span(element: &GreenElement) -> Span {
    match element {
        GreenElement::Node(node) => node.span(),
        GreenElement::Token(token) => token.span(),
    }
}

fn collect_tokens<'a>(node: &'a GreenNode, tokens: &mut Vec<&'a GreenToken>) {
    for child in node.children() {
        match child {
            GreenElement::Node(node) => collect_tokens(node, tokens),
            GreenElement::Token(token) => tokens.push(token),
        }
    }
}

fn token_matches(old: &GreenToken, new: &LosslessToken, source: &str) -> bool {
    let kind_matches = match &new.kind {
        LosslessTokenKind::Token(kind) => old.kind().token_kind() == Some(kind),
        LosslessTokenKind::Whitespace => *old.kind() == crate::SyntaxKind::Whitespace,
        LosslessTokenKind::LineComment => *old.kind() == crate::SyntaxKind::LineComment,
    };
    kind_matches && old.text() == &source[new.span.start..new.span.end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_source::{SourceId, SourceRevision};

    fn version(revision: u64) -> SourceVersion {
        SourceVersion {
            id: SourceId::isolated(),
            revision: SourceRevision(revision),
        }
    }

    /// `Parse` equality ignores incremental bookkeeping, so drift there would
    /// only surface as a wrong reuse several edits later. Bounds may be more
    /// conservative than a clean parse, never narrower.
    fn assert_bookkeeping_sound(incremental: &Parse, clean: &Parse, context: &str) {
        assert_eq!(
            incremental.grammar_errors, clean.grammar_errors,
            "{context}: raw grammar errors"
        );
        let starts = |parse: &Parse| {
            parse
                .iterations
                .iter()
                .map(|it| it.start)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            starts(incremental),
            starts(clean),
            "{context}: iteration starts"
        );
        for (reused, fresh) in incremental.iterations.iter().zip(&clean.iterations) {
            assert!(
                reused.read_end >= fresh.read_end,
                "{context}: {reused:?} < {fresh:?}"
            );
            assert_eq!(reused.errors_end, fresh.errors_end, "{context}: {reused:?}");
            assert_eq!(
                reused.alternates_end, fresh.alternates_end,
                "{context}: {reused:?}"
            );
        }
        let spans = |parse: &Parse| {
            parse
                .blocks
                .iter()
                .map(|block| block.span)
                .collect::<Vec<_>>()
        };
        assert_eq!(spans(incremental), spans(clean), "{context}: block regions");
        for (reused, fresh) in incremental.blocks.iter().zip(&clean.blocks) {
            assert!(
                reused.prefix_read_end >= fresh.prefix_read_end,
                "{context}: {reused:?} < {fresh:?}"
            );
        }
    }

    fn node_with_text<'a>(node: &'a GreenNode, source: &str, text: &str) -> Option<&'a GreenNode> {
        if &source[node.span().start..node.span().end] == text {
            return Some(node);
        }
        node.children().iter().find_map(|child| match child {
            GreenElement::Node(child) => node_with_text(child, source, text),
            GreenElement::Token(_) => None,
        })
    }

    #[test]
    fn nested_block_edits_share_moved_siblings_and_keep_old_snapshots() {
        let source = "fn main() { before(); if flag { value[index]; } after(); } fn last() {}";
        let original = parse(source, None).unwrap();
        assert!(original.errors.is_empty(), "{:?}", original.errors);
        let mut current = original.clone();
        for replacement in ["longer[Box[N]]", "x", "value[index]", "a + b"] {
            let start = current.tree.source().find("{ ").unwrap();
            let inner = current.tree.source()[start + 2..].find("{ ").unwrap() + start + 4;
            let end = current.tree.source()[inner..].find(';').unwrap() + inner;
            let edit = TextEdit::replace(Span::new(inner, end), replacement);
            current = current.reparse(&edit, None).unwrap();
            assert_eq!(current, parse(current.tree.source(), None).unwrap());
            for text in ["before();", "after();", "fn last() {}"] {
                let before = node_with_text(original.tree.green_root(), source, text).unwrap();
                let after =
                    node_with_text(current.tree.green_root(), current.tree.source(), text).unwrap();
                assert!(
                    before.shares_structure(after),
                    "{text} must share node storage"
                );
            }
            assert_eq!(original.tree.full_text(), source);
        }
    }

    #[test]
    fn edits_match_clean_parsing_across_grammar_and_recovery_boundaries() {
        let sources = [
            "// header\nmodule first;\nfn main[T](x: T) T { x }\nmodule last;\n",
            "fn first() { a[T](x); }\nfn second() { b[index]; }\nfn last() {}",
            "type A = Box[N];\nfn broken(x) { let y = ; }\nmodule last;",
            "@[test]\nfn first() { \"text\"; }\n// next\nfn last() {}",
            "fn broken[T(x: T) T;\nmodule next;\nfn last() { $; }",
            "struct S { x: i32, y: }\ntrait T { fn f(&self); }\nmodule last;",
            "fn a() { if x { f(); } else { g(); } }\nfn b() {}",
            "fn a() { match x { ?v => v, null => 0, } }\nfn b() {}",
            "fn a() { before(); if flag { value[Box[N]]; } after(); } fn b() {}",
            "fn a() { call[{ value; }]; let x: [T; { n }] = y; } fn b() {}",
            "fn a() { let x = |v| { v }; loop { if flag { x(); } } } fn b() {}",
            "// UTF-8: \u{4e2d}\u{6587}\nmodule a;\nmodule b;\n",
            "",
            "// comment",
            "module a; module a; module a;",
            "$ } ; fn f( {",
        ];
        let edits = [
            "", " ", "\n", "//", "/*", "x", "Fn", "{", "}", "]", ";", "\"", "\u{4e2d}",
        ];
        for source in sources {
            let initial_version = version(1);
            let next_version = SourceVersion {
                revision: SourceRevision(2),
                ..initial_version
            };
            let original = parse(source, Some(initial_version)).expect("initial parse");
            let boundaries = (0..=source.len())
                .filter(|&offset| source.is_char_boundary(offset))
                .collect::<Vec<_>>();
            for (index, &start) in boundaries.iter().enumerate() {
                for end in [start, boundaries.get(index + 1).copied().unwrap_or(start)] {
                    for replacement in edits {
                        let edit = TextEdit::replace(Span::new(start, end), replacement);
                        let edited = edit.apply(source).expect("valid edit");
                        let incremental = original
                            .reparse(&edit, Some(next_version))
                            .unwrap_or_else(|error| panic!("{source:?}, {edit:?}: {error:?}"));
                        let clean = parse(&edited, Some(next_version)).expect("clean parse");
                        assert_eq!(incremental, clean, "source {source:?}, edit {edit:?}");
                        assert_bookkeeping_sound(
                            &incremental,
                            &clean,
                            &format!("source {source:?}, edit {edit:?}"),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn reused_tokens_keep_shared_text_and_diagnostics_get_new_origins() {
        let source = "module first;\nfn changed() { value[index]; }\nfn broken(x) {}\nmodule last;";
        let first_version = version(1);
        let next_version = SourceVersion {
            revision: SourceRevision(2),
            ..first_version
        };
        let original = parse(source, Some(first_version)).expect("initial parse");
        let start = source.find("value").unwrap();
        let edited = original
            .reparse(
                &TextEdit::replace(Span::new(start, start + 5), "longer"),
                Some(next_version),
            )
            .unwrap();
        let before = original.tree.tokens();
        let after = edited.tree.tokens();
        for name in ["first", "last"] {
            let before = before
                .iter()
                .find(|token| token.text.as_ref() == name)
                .unwrap();
            let after = after
                .iter()
                .find(|token| token.text.as_ref() == name)
                .unwrap();
            assert!(
                std::sync::Arc::ptr_eq(&before.text, &after.text),
                "{name} should be reused"
            );
        }
        assert!(
            edited
                .errors
                .iter()
                .filter_map(|error| error.node_key.as_ref())
                .all(|key| key.source_version() == next_version)
        );
    }

    #[test]
    fn invalid_utf8_edits_leave_original_parse_untouched() {
        let original = parse("// \u{4e2d}\nmodule a;", None).unwrap();
        assert_eq!(
            original.reparse(&TextEdit::delete(Span::new(4, 5)), None),
            Err(GreenBuildError::InvalidSource)
        );
        assert_eq!(original.tree.full_text(), "// \u{4e2d}\nmodule a;");
    }

    #[test]
    fn replacement_suffix_cannot_resynchronize_inside_the_replaced_range() {
        let original = parse("val};]]", None).unwrap();
        let edit = TextEdit::replace(Span::new(5, 6), "@[test]");
        let incremental = original.reparse(&edit, None).unwrap();
        assert_eq!(incremental, parse("val};@[test]]", None).unwrap());
    }

    #[test]
    fn wide_edits_and_repeated_recovery_match_clean_parsing() {
        let mut current = parse(
            "module a; fn f[T](x: T) { call[Box[N]](x); } fn g(x) {} module z;",
            None,
        )
        .unwrap();
        let replacements = [
            "",
            "fn f() {}",
            "module new;",
            "//",
            "\n",
            "{",
            "}",
            "@[test]",
            "\"text\"",
            "value[Box[N]]",
        ];
        let mut random = 0x1234_5678u32;
        for step in 0..1500 {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let start = random as usize % (current.tree.source().len() + 1);
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let end = start + random as usize % (current.tree.source().len() - start + 1);
            let edit = TextEdit::replace(
                Span::new(start, end),
                replacements[step % replacements.len()],
            );
            let edited_source = edit.apply(current.tree.source()).unwrap();
            let clean = parse(&edited_source, None)
                .unwrap_or_else(|error| panic!("clean parse {edited_source:?}: {error:?}"));
            current = current.reparse(&edit, None).unwrap_or_else(|error| {
                panic!(
                    "step {step}, source {:?}, {edit:?}: {error:?}",
                    current.tree.source()
                )
            });
            assert_eq!(current, clean, "step {step}, {edit:?}");
            assert_bookkeeping_sound(&current, &clean, &format!("step {step}, {edit:?}"));
        }
    }
}
