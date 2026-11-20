// SPDX-License-Identifier: GPL-3.0-or-later
//! Organizes import directives and named selectors with binding verification.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lsp_types::{
    CodeAction, CodeActionKind, OneOf, OptionalVersionedTextDocumentIdentifier, TextDocumentEdit,
};
use nia_span::Span;
use nia_syntax::{GreenNode, SyntaxKind, TextEdit};

use super::{Analysis, AnalysisMode, Documents, Session, normalized};
use crate::document::{Document, text_workspace_edit};
use crate::rewrite::{EditOffsets, Edits, preserves_bindings};
use crate::workspace::{Cancellation, bindings};

impl Session {
    pub(super) fn organize_imports(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        path: &Path,
        versioned: bool,
        cancel: &Cancellation,
    ) -> Option<CodeAction> {
        let open = documents.get(path)?;
        let file = analysis.file(path)?;
        if file.text != open.document.text {
            return None;
        }
        let parsed = file.syntax();
        if !parsed.errors.is_empty() {
            return None;
        }
        let source = &file.text;
        let root = parsed.tree.green_root();
        let mut selectors = ImportOrder::default();
        selectors.sort_selectors(source, root);
        selectors.edits.sort_by_key(|edit| edit.span);
        let mut text = source.to_string();
        selectors.apply(&mut text);
        let directives = ImportOrder::directives(&text, root);
        let removed = unused_directives(analysis, path, root, source);
        if selectors.edits.is_empty() && directives.edits.is_empty() && removed.is_empty() {
            return None;
        }
        directives.apply(&mut text);
        let mut ranges: Vec<_> = directives.edits.iter().map(|edit| edit.span).collect();
        ranges.extend(selectors.edits.iter().filter_map(|edit| {
            let index = directives
                .edits
                .partition_point(|outer| outer.span.end <= edit.span.start);
            directives
                .edits
                .get(index)
                .is_none_or(|outer| edit.span.end <= outer.span.start)
                .then_some(edit.span)
        }));
        ranges.sort_unstable();
        let mut replacements: BTreeMap<_, _> = ranges
            .into_iter()
            .map(|span| (span, text[span.start..span.end].to_owned()))
            .collect();
        let source_path = normalized(path);
        let mut deletions = BTreeMap::new();
        for &span in &removed {
            let moved = directives.translate(span);
            let start = text[..moved.start].rfind('\n').map_or(0, |at| at + 1);
            let end = text[moved.end..]
                .find('\n')
                .map_or(text.len(), |at| moved.end + at + 1);
            deletions.insert(Span::new(start, end), String::new());
        }
        for span in deletions.keys().rev() {
            text.replace_range(span.start..span.end, "");
        }
        if !deletions.is_empty() {
            let edit = TextEdit::between(source, &text);
            replacements = BTreeMap::from([(edit.span, edit.replacement)]);
        }
        let deletions = Edits::from([(source_path.clone(), deletions)]);
        let shifts = EditOffsets::new(&deletions);
        let mut before = bindings(analysis, cancel).ok()?;
        if let Some(entries) = before.get_mut(&source_path) {
            entries.retain(|span, _| {
                !removed
                    .iter()
                    .any(|item| item.start <= span.start && span.end <= item.end)
            });
        }
        let mut documents = documents.clone();
        documents.get_mut(path)?.document = Document::new(open.document.version, text);
        let after = self
            .analyze_snapshot(&documents, &analysis.request, cancel, AnalysisMode::Full)
            .ok()?;
        let changes = Edits::from([(source_path.clone(), replacements)]);
        if EditOffsets::new(&changes)
            .new_error(analysis.errors(), after.errors(), &[])
            .is_some()
            || !preserves_bindings(
                before,
                &bindings(&after, cancel).ok()?,
                |file, span| {
                    if file == source_path {
                        shifts.binding_span(file, directives.translate(selectors.translate(span)))
                    } else {
                        span
                    }
                },
                cancel,
            )
        {
            return None;
        }
        let edits = changes
            .into_values()
            .flatten()
            .map(|(span, replacement)| {
                OneOf::Left(lsp_types::TextEdit::new(
                    open.document.lines.range(source, span),
                    replacement,
                ))
            })
            .collect();
        Some(CodeAction {
            kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
            title: "Organize imports".into(),
            edit: Some(text_workspace_edit(
                vec![TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: open.uri.clone(),
                        version: Some(open.document.version),
                    },
                    edits,
                }],
                versioned,
            )),
            ..CodeAction::default()
        })
    }
}

fn unused_directives(
    analysis: &Analysis,
    path: &Path,
    root: &GreenNode,
    source: &str,
) -> Vec<Span> {
    let unused: BTreeSet<_> = analysis
        .source_diagnostics(path)
        .filter(|item| item.diagnostic.code.as_str() == nia_diagnostic::codes::UNUSED_IMPORT.code)
        .filter_map(|item| item.diagnostic.primary_span())
        .collect();
    if unused.is_empty() {
        return Vec::new();
    }
    // A warning only covers the active configuration. A disabled body can
    // need a module for its extension methods without spelling the module name.
    if analysis.file(path).is_none_or(|file| {
        analysis
            .db
            .source_inactive_spans(file.module)
            .map_or(true, |spans| !spans.is_empty())
    }) {
        return Vec::new();
    }
    root.child_nodes()
        .filter_map(|item| {
            let span = standalone_import(source, item)?;
            let directive = &source[span.start..span.end];
            // A group or wildcard can introduce several names at one site.
            if directive.contains(['{', '*', '\n', '\r']) || directive.contains("//") {
                return None;
            }
            unused
                .range(Span::new(span.start, span.start)..)
                .next()
                .filter(|name| name.end <= span.end)
                .map(|_| span)
        })
        .collect()
}

#[derive(Default)]
struct ImportOrder {
    edits: Vec<TextEdit>,
    destinations: BTreeMap<Span, usize>,
}

impl ImportOrder {
    fn directives(source: &str, root: &GreenNode) -> Self {
        let mut result = Self::default();
        let mut pending = vec![root];
        while let Some(scope) = pending.pop() {
            pending.extend(scope.child_nodes());
            if !matches!(scope.kind(), SyntaxKind::SourceFile | SyntaxKind::Block) {
                continue;
            }
            let mut group = Vec::new();
            for item in scope.child_nodes() {
                let directive = standalone_import(source, item);
                if let Some(last) = group.last()
                    && directive.is_none_or(|next| !adjacent_directives(source, *last, next))
                {
                    result.sort(source, &group);
                    group.clear();
                }
                group.extend(directive);
            }
            result.sort(source, &group);
        }
        result.edits.sort_by_key(|edit| edit.span);
        result
    }

    fn sort_selectors(&mut self, source: &str, node: &GreenNode) {
        if node.kind() == &SyntaxKind::UsingGroup {
            let mut group: Vec<Span> = Vec::new();
            for child in node.child_nodes() {
                let selector = (child.kind() == &SyntaxKind::UsingName)
                    .then(|| child.token_span())
                    .flatten();
                if let Some(&previous) = group.last()
                    && selector
                        .is_none_or(|next: Span| source[previous.end..next.start].trim() != ",")
                {
                    self.sort(source, &group);
                    group.clear();
                }
                group.extend(selector);
            }
            self.sort(source, &group);
        }
        for child in node.child_nodes() {
            self.sort_selectors(source, child);
        }
    }

    fn apply(&self, source: &mut String) {
        for edit in self.edits.iter().rev() {
            source.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        }
    }

    fn sort(&mut self, source: &str, group: &[Span]) {
        let mut sorted = group.to_vec();
        sorted.sort_by_key(|span| &source[span.start..span.end]);
        if sorted == group {
            return;
        }
        let start = group[0].start;
        let end = group[group.len() - 1].end;
        let mut replacement = String::with_capacity(end - start);
        let mut previous_end = start;
        for (slot, original) in group.iter().zip(sorted) {
            replacement.push_str(&source[previous_end..slot.start]);
            self.destinations
                .insert(original, start + replacement.len());
            replacement.push_str(&source[original.start..original.end]);
            previous_end = slot.end;
        }
        self.edits
            .push(TextEdit::replace(Span::new(start, end), replacement));
    }

    fn translate(&self, span: Span) -> Span {
        let Some((original, destination)) = self
            .destinations
            .range(..=Span::new(span.start, usize::MAX))
            .next_back()
            .filter(|(original, _)| span.end <= original.end)
        else {
            return span;
        };
        let start = destination + span.start - original.start;
        Span::new(start, start + span.len())
    }
}

fn standalone_import(source: &str, item: &GreenNode) -> Option<Span> {
    if (item.kind() != &SyntaxKind::UsingStmt && item.child(&SyntaxKind::UsingDecl).is_none())
        || item.child(&SyntaxKind::Visibility).is_some()
        || item.child(&SyntaxKind::Attribute).is_some()
    {
        return None;
    }
    let span = item.token_span()?;
    let line_start = source[..span.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let line_end = source[span.end..]
        .find('\n')
        .map_or(source.len(), |index| span.end + index);
    if !source[line_start..span.start].trim().is_empty()
        || !source[span.end..line_end].trim().is_empty()
        || source[..line_start]
            .lines()
            .next_back()
            .is_some_and(|line| line.trim_start().starts_with("//"))
    {
        return None;
    }
    Some(span)
}

fn adjacent_directives(source: &str, left: Span, right: Span) -> bool {
    let between = &source[left.end..right.start];
    between.trim().is_empty() && between.bytes().filter(|&byte| byte == b'\n').count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{OpenDocument, tests::test_session, uri};

    #[test]
    fn organize_removes_unused_imports_and_keeps_extension_providers() {
        let _resources =
            nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
        let root = nia_test_support::test_dir("lsp-prune-imports");
        let entry = root.join("root.nia");
        let source = "using std::slice;\nusing std::iter;\nusing std::mem;\n\nfun run(values: &[i32]) usize { values.iter().count() }\n";
        std::fs::write(&entry, source).unwrap();
        let documents = Documents::from([(
            entry.clone(),
            OpenDocument {
                uri: uri(&entry).unwrap(),
                document: Document::new(1, source.to_owned()),
            },
        )]);
        let checked = test_session().analyze(&documents, &entry).unwrap();
        let action = test_session()
            .organize_imports(
                &checked,
                &documents,
                &entry,
                false,
                &Cancellation::default(),
            )
            .unwrap();
        let edits = &action.edit.unwrap().changes.unwrap()[&uri(&entry).unwrap()];
        let lines = &documents[&entry].document.lines;
        let mut edited = source.to_owned();
        for edit in edits.iter().rev() {
            let start = lines.offset(source, edit.range.start).unwrap();
            let end = lines.offset(source, edit.range.end).unwrap();
            edited.replace_range(start..end, &edit.new_text);
        }
        assert!(!edited.contains("using std::mem;"), "{edited}");
        assert!(edited.contains("using std::iter;"), "{edited}");
        assert!(edited.contains("using std::slice;"), "{edited}");

        let conditional = format!("{source}\n@[release] fun release_only() {{}}\n");
        let mut documents = documents;
        documents.get_mut(&entry).unwrap().document = Document::new(2, conditional.clone());
        let mut session = test_session();
        let checked = session.analyze(&documents, &entry).unwrap();
        assert!(
            unused_directives(
                &checked,
                &entry,
                checked.file(&entry).unwrap().syntax().tree.green_root(),
                &conditional,
            )
            .is_empty()
        );
    }
}
