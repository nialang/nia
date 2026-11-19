// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects source-owned compiler suggestions onto the current editor snapshot.

use std::path::Path;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionParams, OneOf, OptionalVersionedTextDocumentIdentifier,
    TextDocumentEdit, TextEdit,
};
use nia_diagnostic::{DiagnosticSuggestion, SuggestionApplicability};

use super::{Analysis, OpenDocument, editor_diagnostic};
use crate::document::{Document, text_workspace_edit};

impl Analysis {
    pub(crate) fn code_actions(
        &self,
        path: &Path,
        open: &OpenDocument,
        params: &CodeActionParams,
        versioned: bool,
    ) -> Vec<CodeAction> {
        let document = &open.document;
        if params.context.only.as_ref().is_some_and(|kinds| {
            !kinds
                .iter()
                .any(|kind| kind.as_str().is_empty() || *kind == CodeActionKind::QUICKFIX)
        }) || self
            .file(path)
            .is_none_or(|file| file.text != document.text)
        {
            return Vec::new();
        }
        let mut actions = Vec::new();
        for item in self.source_diagnostics(path) {
            let suggestions = &item.diagnostic.suggestions;
            if suggestions.is_empty() {
                continue;
            }
            let diagnostic = editor_diagnostic(item, document);
            if diagnostic.range.start > params.range.end
                || params.range.start > diagnostic.range.end
            {
                continue;
            }
            let mut preferred = suggestions.iter().filter(|suggestion| {
                suggestion.applicability == SuggestionApplicability::MachineApplicable
            });
            let unique_preferred = preferred.next().is_some() && preferred.next().is_none();
            for suggestion in suggestions.iter() {
                let Some(edits) = suggestion_edits(document, suggestion) else {
                    continue;
                };
                let edit = TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: open.uri.clone(),
                        version: Some(document.version),
                    },
                    edits,
                };
                let action = CodeAction {
                    title: suggestion.message.clone(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(text_workspace_edit(vec![edit], versioned)),
                    is_preferred: (unique_preferred
                        && suggestion.applicability == SuggestionApplicability::MachineApplicable)
                        .then_some(true),
                    ..CodeAction::default()
                };
                if !actions.contains(&action) {
                    actions.push(action);
                }
            }
        }
        actions
    }
}

fn suggestion_edits(
    document: &Document,
    suggestion: &DiagnosticSuggestion,
) -> Option<Vec<OneOf<TextEdit, lsp_types::AnnotatedTextEdit>>> {
    let mut edits: Vec<_> = suggestion.edits.iter().collect();
    edits.sort_by_key(|edit| edit.span);
    if edits.is_empty()
        || edits.windows(2).any(|pair| {
            pair[0].span.end > pair[1].span.start || pair[0].span.start == pair[1].span.start
        })
    {
        return None;
    }
    edits
        .into_iter()
        .map(|edit| {
            document.text.get(edit.span.start..edit.span.end)?;
            Some(OneOf::Left(TextEdit::new(
                document.lines.range(&document.text, edit.span),
                edit.replacement.clone(),
            )))
        })
        .collect()
}
