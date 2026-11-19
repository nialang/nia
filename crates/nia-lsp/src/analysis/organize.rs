// SPDX-License-Identifier: GPL-3.0-or-later
//! Organizes adjacent import directives while proving their bindings survived.

use std::{collections::BTreeMap, path::Path};

use lsp_types::{
    CodeAction, CodeActionKind, OneOf, OptionalVersionedTextDocumentIdentifier, TextDocumentEdit,
};
use nia_span::Span;
use nia_syntax::{GreenNode, SyntaxKind, TextEdit};

use super::{Analysis, Documents, Session, normalized};
use crate::document::{Document, text_workspace_edit};
use crate::rewrite::preserves_bindings;
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
        if file.text != open.document.text || analysis.errors().next().is_some() {
            return None;
        }
        let parsed = file.syntax()?;
        if !parsed.errors.is_empty() {
            return None;
        }
        let source = &file.text;
        let imports = ImportOrder::new(source, parsed.tree.green_root());
        if imports.edits.is_empty() {
            return None;
        }
        let mut text = source.to_string();
        for edit in imports.edits.iter().rev() {
            text.replace_range(edit.span.start..edit.span.end, &edit.replacement);
        }
        let mut documents = documents.clone();
        documents.get_mut(path)?.document = Document::new(open.document.version, text);
        let after = self
            .analyze_with_cancel(&documents, path, Some(cancel))
            .ok()?;
        let source_path = normalized(path);
        if after.errors().next().is_some()
            || !preserves_bindings(
                bindings(analysis, cancel).ok()?,
                &bindings(&after, cancel).ok()?,
                |file, span| {
                    if file == source_path {
                        imports.translate(span)
                    } else {
                        span
                    }
                },
                cancel,
            )
        {
            return None;
        }
        let edits = imports
            .edits
            .into_iter()
            .map(|edit| {
                OneOf::Left(lsp_types::TextEdit::new(
                    open.document.lines.range(source, edit.span),
                    edit.replacement,
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

#[derive(Default)]
struct ImportOrder {
    edits: Vec<TextEdit>,
    destinations: BTreeMap<Span, usize>,
}

impl ImportOrder {
    fn new(source: &str, root: &GreenNode) -> Self {
        let mut result = Self::default();
        let mut group = Vec::new();
        for item in root.child_nodes() {
            let directive = standalone_import(source, item);
            if !group.is_empty()
                && directive.is_none_or(|next| {
                    let last: &Span = group.last().expect("nonempty import group");
                    let between = &source[last.end..next.start];
                    !between.trim().is_empty()
                        || between.bytes().filter(|&byte| byte == b'\n').count() != 1
                })
            {
                result.sort(source, &group);
                group.clear();
            }
            group.extend(directive);
        }
        result.sort(source, &group);
        result
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
    if item.child(&SyntaxKind::UsingDecl).is_none()
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
