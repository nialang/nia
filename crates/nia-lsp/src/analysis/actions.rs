// SPDX-License-Identifier: GPL-3.0-or-later
//! Projects source-owned compiler suggestions onto the current editor snapshot.

use std::{collections::BTreeMap, path::Path};

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionParams, CodeActionTriggerKind, CompletionItem, OneOf,
    OptionalVersionedTextDocumentIdentifier, TextDocumentEdit, TextEdit,
};
use nia_diagnostic::{DiagnosticSuggestion, SuggestionApplicability, codes};
use nia_lexer::{TokenKind, tokenize};
use nia_span::Span;

use super::{
    Analysis, Documents, File, OpenDocument, Session, VisibleName, editor_diagnostic, normalized,
};
use crate::document::{Document, text_workspace_edit};
use crate::rewrite::{EditOffsets, Edits, preserves_bindings};
use crate::workspace::{Bindings, Cancellation, Site, bindings, symbol_site};

impl Session {
    pub(crate) fn code_actions(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        path: &Path,
        params: &CodeActionParams,
        versioned: bool,
        cancel: &Cancellation,
    ) -> Vec<CodeAction> {
        let open = &documents[path];
        let mut actions = analysis.code_actions(path, open, params, versioned);
        if params.context.only.as_ref().is_some_and(|kinds| {
            kinds.iter().any(|kind| {
                *kind == CodeActionKind::SOURCE_ORGANIZE_IMPORTS || *kind == CodeActionKind::SOURCE
            })
        }) {
            actions.extend(self.organize_imports(analysis, documents, path, versioned, cancel));
        }
        if !requests_quick_fixes(params)
            || params.context.trigger_kind == Some(CodeActionTriggerKind::AUTOMATIC)
        {
            return actions;
        }
        let Some(file) = analysis
            .file(path)
            .filter(|file| file.text == open.document.text)
        else {
            return actions;
        };
        for item in analysis.source_diagnostics(path) {
            if cancel.check().is_err() {
                break;
            }
            if item.diagnostic.code.as_str() != codes::NAME_RESOLUTION.code
                || !item.diagnostic.suggestions.is_empty()
            {
                continue;
            }
            let diagnostic = editor_diagnostic(item, &open.document);
            if diagnostic.range.start > params.range.end
                || params.range.start > diagnostic.range.end
            {
                continue;
            }
            let Some(span) = item.diagnostic.primary_span() else {
                continue;
            };
            let Some(name) = file.text.get(span.start..span.end) else {
                continue;
            };
            if !matches!(tokenize(name).as_slice(), [token, _] if token.kind == TokenKind::Ident)
                || !["unknown value", "unknown type"]
                    .iter()
                    .any(|prefix| item.diagnostic.summary == format!("{prefix} `{name}`"))
                || analysis.symbol_at_offset(file, span.start).is_some()
            {
                continue;
            }
            let Some(candidates) =
                analysis.import_candidates(file, |item| item.name.as_ref() == name, cancel)
            else {
                continue;
            };
            for candidate in candidates {
                let Some(import) =
                    self.verified_import_edit(analysis, documents, path, span, &candidate, cancel)
                else {
                    continue;
                };
                let edit = TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: open.uri.clone(),
                        version: Some(open.document.version),
                    },
                    edits: vec![OneOf::Left(import)],
                };
                let action = CodeAction {
                    title: format!("Import `{}`", candidate.path),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(text_workspace_edit(vec![edit], versioned)),
                    ..CodeAction::default()
                };
                if !actions.contains(&action) {
                    actions.push(action);
                }
            }
        }
        actions
    }

    pub(super) fn complete_import(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        path: &Path,
        position: lsp_types::Position,
        cancel: &Cancellation,
    ) -> Option<CompletionItem> {
        let file = analysis.file(path)?;
        let (span, candidate) = nia_query::with_cancellation(Some(cancel.token()), || {
            Ok(analysis.import_completion(path, position, cancel))
        })
        .ok()
        .flatten()?;
        if analysis.symbol_at_offset(file, span.start).is_some() {
            return None;
        }
        let import =
            self.verified_import_edit(analysis, documents, path, span, &candidate, cancel)?;
        let range = file.lines.range(&file.text, span);
        let mut item = crate::editing::completion_item(
            candidate.name.to_string(),
            candidate.kind.map(crate::editing::completion_kind),
            range,
        );
        item.label_details = Some(lsp_types::CompletionItemLabelDetails {
            detail: None,
            description: Some(candidate.path),
        });
        if let Some(callable) = candidate
            .symbol
            .and_then(|symbol| analysis.callable_declaration(symbol))
        {
            callable.describe_completion(&mut item);
        }
        item.additional_text_edits = Some(vec![import]);
        Some(item)
    }

    fn verified_import_edit(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        path: &Path,
        span: Span,
        candidate: &ImportCandidate,
        cancel: &Cancellation,
    ) -> Option<TextEdit> {
        let open = documents.get(path)?;
        let file = analysis
            .file(path)
            .filter(|file| file.text == open.document.text)?;
        let newline = if file.text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let text = format!("using {};{newline}{newline}", candidate.path);
        let source = normalized(path);
        let mut changes = BTreeMap::from([(Span::default(), text.clone())]);
        let mut edited = file.text.to_string();
        if file.text[span.start..span.end] != *candidate.name {
            changes.insert(span, candidate.name.to_string());
            edited.replace_range(span.start..span.end, &candidate.name);
        }
        edited.insert_str(0, &text);
        let edits = Edits::from([(source.clone(), changes)]);
        let mut updated = documents.clone();
        updated.get_mut(path)?.document = Document::new(open.document.version, edited);
        let after = self
            .analyze_with_cancel(&updated, path, Some(cancel))
            .ok()?;
        verified_import(
            analysis,
            &after,
            &source,
            span,
            &candidate.target,
            &edits,
            cancel,
        )
        .then(|| TextEdit::new(Default::default(), text))
    }
}

pub(super) struct ImportCandidate {
    target: Site,
    path: String,
    name: std::sync::Arc<str>,
    kind: Option<nia_defs::DefKind>,
    symbol: Option<super::Symbol>,
}

impl Analysis {
    pub(super) fn import_completion(
        &self,
        path: &Path,
        position: lsp_types::Position,
        cancel: &Cancellation,
    ) -> Option<(Span, ImportCandidate)> {
        use nia_defs::DefKind;
        let file = self.file(path)?;
        let (span, prefix, type_only) = crate::editing::unqualified_completion(file, position)?;
        let candidates = self.import_candidates(
            file,
            |item| {
                item.name.starts_with(prefix)
                    && item.kind.is_some_and(|kind| {
                        if type_only {
                            matches!(
                                kind,
                                DefKind::Struct
                                    | DefKind::Enum
                                    | DefKind::Union
                                    | DefKind::Trait
                                    | DefKind::TypeAlias
                            )
                        } else {
                            matches!(kind, DefKind::Function | DefKind::Const | DefKind::Global)
                        }
                    })
            },
            cancel,
        )?;
        let [candidate] = candidates.try_into().ok()?;
        Some((span, candidate))
    }

    fn code_actions(
        &self,
        path: &Path,
        open: &OpenDocument,
        params: &CodeActionParams,
        versioned: bool,
    ) -> Vec<CodeAction> {
        let document = &open.document;
        if !requests_quick_fixes(params)
            || self
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

    fn import_candidates(
        &self,
        file: &File,
        matches: impl Fn(&VisibleName) -> bool,
        cancel: &Cancellation,
    ) -> Option<Vec<ImportCandidate>> {
        let imports = self.db.source_imports(file.module, 0).ok()?;
        let root = self.graph.current_package_root(file.module)?;
        let mut paths: BTreeMap<Vec<String>, _> = imports
            .modules
            .iter()
            .filter_map(|(alias, module)| {
                Some((
                    vec![self.db.source_symbol_text(*alias)?.to_string()],
                    *module,
                ))
            })
            .collect();
        let mut packages = BTreeMap::<_, Vec<_>>::new();
        packages.entry(root).or_default().push("root".to_owned());
        for (alias, package) in self.graph.package_aliases(file.module) {
            let Some(name) = self.db.source_symbol_text(alias) else {
                continue;
            };
            if matches!(tokenize(&name).as_slice(), [token, _] if token.kind == TokenKind::Ident) {
                packages.entry(package).or_default().push(name.to_string());
            }
        }
        for module in self.graph.modules() {
            cancel.check().ok()?;
            if module.id == file.module {
                continue;
            }
            let Some(roots) = self
                .graph
                .current_package_root(module.id)
                .and_then(|root| packages.get(&root))
            else {
                continue;
            };
            let Some(segments) = module
                .module_path
                .segments
                .iter()
                .map(|segment| {
                    self.db
                        .source_symbol_text(*segment)
                        .map(|name| name.to_string())
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            for root in roots {
                let qualifier: Vec<_> = std::iter::once(root.as_str())
                    .chain(segments.iter().map(String::as_str))
                    .collect();
                if self
                    .resolve_namespace(file.module, &imports, &qualifier)
                    .ok()?
                    == Some(module.id)
                {
                    paths.insert(
                        qualifier.into_iter().map(str::to_owned).collect(),
                        module.id,
                    );
                }
            }
        }
        let mut found = BTreeMap::<_, ImportCandidate>::new();
        for (path, module) in paths {
            cancel.check().ok()?;
            let available = self.namespace_names(file.module, module).ok()?;
            for item in available.into_iter().filter(&matches) {
                let Some(target) = item.symbol.and_then(|symbol| symbol_site(self, symbol)) else {
                    continue;
                };
                let import = format!("{}::{}", path.join("::"), item.name);
                let key = (target.clone(), item.name.clone());
                if let Some(current) = found.get_mut(&key) {
                    if (import.len(), &import) < (current.path.len(), &current.path) {
                        current.path = import;
                    }
                } else {
                    found.insert(
                        key,
                        ImportCandidate {
                            target,
                            path: import,
                            name: item.name,
                            kind: item.kind,
                            symbol: item.symbol,
                        },
                    );
                }
            }
        }
        let mut candidates: Vec<_> = found.into_values().collect();
        candidates.sort_by(|left, right| left.path.cmp(&right.path));
        Some(candidates)
    }
}

fn requests_quick_fixes(params: &CodeActionParams) -> bool {
    params.context.only.as_ref().is_none_or(|kinds| {
        kinds
            .iter()
            .any(|kind| kind.as_str().is_empty() || *kind == CodeActionKind::QUICKFIX)
    })
}

fn verified_import(
    before: &Analysis,
    after: &Analysis,
    source: &str,
    use_span: Span,
    target: &Site,
    edits: &Edits,
    cancel: &Cancellation,
) -> bool {
    let offsets = EditOffsets::new(edits);
    let shift = |site: &Site| Site {
        span: offsets.binding_span(&site.source, site.span),
        source: site.source.clone(),
    };
    let (Ok(previous), Ok(current)) = (bindings(before, cancel), bindings(after, cancel)) else {
        return false;
    };
    if !matches_import_target(source, use_span, &shift(target), &offsets, &current) {
        return false;
    }
    if offsets
        .new_error(before.errors(), after.errors(), &[])
        .is_some()
    {
        return false;
    }
    preserves_bindings(
        previous,
        &current,
        |source, span| offsets.binding_span(source, span),
        cancel,
    )
}

fn matches_import_target(
    source: &str,
    use_span: Span,
    target: &Site,
    offsets: &EditOffsets<'_>,
    current: &Bindings,
) -> bool {
    current
        .get(source)
        .and_then(|entries| entries.get(&offsets.binding_span(source, use_span)))
        .is_some_and(|candidates| {
            candidates
                .iter()
                .all(|binding| binding.declaration.as_ref() == Some(target))
        })
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
