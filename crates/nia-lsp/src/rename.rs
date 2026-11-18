// SPDX-License-Identifier: GPL-3.0-or-later
//! Validates workspace edits against fresh compiler analyses.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use lsp_types::{
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, Position,
    PrepareRenameResponse, TextDocumentEdit, TextEdit, WorkspaceEdit,
};
use nia_lexer::{TokenKind, tokenize};
use nia_span::Span;

use crate::{
    analysis::{Analysis, Documents, OpenDocument, Session, Symbol, normalized},
    document::Document,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Site {
    source: Arc<str>,
    span: Span,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Binding {
    declaration: Option<Site>,
    alias: Option<Site>,
}

type Bindings = BTreeMap<String, BTreeMap<Span, BTreeSet<Binding>>>;
type Edits = BTreeMap<String, BTreeSet<Span>>;

impl Analysis {
    pub(crate) fn prepare_rename(
        &self,
        path: &Path,
        position: Position,
    ) -> Option<PrepareRenameResponse> {
        let (_, span) = target(self, path, position).ok()?;
        let file = self.file(path)?;
        Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: file.lines.range(&file.text, span),
            placeholder: file.text[span.start..span.end].to_owned(),
        })
    }
}

fn target(analysis: &Analysis, path: &Path, position: Position) -> Result<(Symbol, Span), String> {
    let file = analysis
        .file(path)
        .ok_or("Source is outside the current analysis")?;
    let offset = file
        .lines
        .offset(&file.text, position)
        .ok_or("Invalid rename position")?;
    let (symbol, span) = analysis
        .rename_symbol_at(file, offset)
        .ok_or("No resolved name at this position")?;
    let name = &file.text[span.start..span.end];
    if !analysis.rename_supported(symbol) || !identifier(name) {
        return Err("This kind of symbol cannot be renamed safely".to_owned());
    }
    Ok((symbol, span))
}

fn identifier(text: &str) -> bool {
    let tokens = tokenize(text);
    matches!(tokens.as_slice(), [name, eof]
        if name.kind == TokenKind::Ident && name.span == Span::new(0, text.len()) && eof.kind == TokenKind::Eof)
}

pub(crate) struct Rename<'a> {
    pub(crate) path: &'a Path,
    pub(crate) position: Position,
    pub(crate) new_name: &'a str,
    pub(crate) folders: &'a [PathBuf],
    pub(crate) versioned: bool,
}

impl Session {
    pub(crate) fn rename(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        request: Rename<'_>,
    ) -> Result<WorkspaceEdit, String> {
        let Rename {
            path,
            position,
            new_name,
            folders,
            versioned,
        } = request;
        let (symbol, _) = target(analysis, path, position)?;
        if !identifier(new_name) {
            return Err("The new name must be a Nia identifier".to_owned());
        }
        if matches!(symbol, Symbol::Global(_)) && matches!(new_name, "main" | "build") {
            return Err("The name is reserved for entry points".to_owned());
        }
        let (source, span) = analysis
            .definitions
            .get(&symbol)
            .ok_or("The declaration is unavailable")?;
        let old = &analysis.files[source].text[span.start..span.end];
        if old == new_name {
            return Ok(WorkspaceEdit::default());
        }
        let target = Site {
            source: Arc::from(source.as_str()),
            span: *span,
        };
        let workspace_symbol = matches!(symbol, Symbol::Global(_));
        let workspace =
            self.projects
                .workspace_requests(path, documents.keys().cloned(), folders)?;
        let scope = analysis.rename_scope(symbol);
        let requested_source = normalized(path);
        self.install_documents(documents)?;
        let mut snapshots = Vec::new();
        let mut files = BTreeMap::new();
        let mut edits = Edits::new();
        let matches_target = |binding: &Binding| {
            binding.alias.as_ref().or(binding.declaration.as_ref()) == Some(&target)
        };
        for (request, standalone) in workspace
            .entries
            .into_iter()
            .map(|entry| (entry, false))
            .chain(workspace.standalone.into_iter().map(|entry| (entry, true)))
        {
            if standalone && files.contains_key(request.entry_path.as_str()) {
                continue;
            }
            let current = self.analyze_request(request.clone())?;
            for source in [target.source.as_ref(), requested_source.as_str()] {
                if let Some(file) = current.files.get(source)
                    && file.text != analysis.files[source].text
                {
                    return Err(format!("Source changed during rename ({source})"));
                }
            }
            if let Some(error) = current.first_error() {
                return Err(format!(
                    "Cannot check rename in {} ({error})",
                    request.entry_path.as_str()
                ));
            }
            let bindings = bindings(&current);
            for (source, file) in current.files {
                if (workspace_symbol || source.as_str() == target.source.as_ref())
                    && editable_source(Path::new(&source), &workspace.editable)
                {
                    let occurrences = bindings.get(&source);
                    if tokenize(&file.text).into_iter().any(|token| {
                        token.kind == TokenKind::Ident
                            && scope.is_none_or(|scope| {
                                scope.start <= token.span.start && token.span.end <= scope.end
                            })
                            && &file.text[token.span.start..token.span.end] == old
                            && !occurrences
                                .is_some_and(|occurrences| occurrences.contains_key(&token.span))
                    }) {
                        return Err(format!("Cannot determine every use of `{old}` in {source}"));
                    }
                }
                if let Some(occurrences) = bindings.get(&source) {
                    for (&span, candidates) in occurrences {
                        if &file.text[span.start..span.end] == old
                            && candidates.iter().any(matches_target)
                        {
                            edits.entry(source.clone()).or_default().insert(span);
                        }
                    }
                }
                files.entry(source).or_insert((file.uri, file.text));
            }
            snapshots.push((request, bindings));
        }
        if !edits
            .get(target.source.as_ref())
            .is_some_and(|spans| spans.contains(&target.span))
        {
            return Err("The declaration is outside the checked workspace".to_owned());
        }
        for (_, snapshot) in &snapshots {
            for (source, occurrences) in snapshot {
                let Some(spans) = edits.get(source) else {
                    continue;
                };
                if occurrences.iter().any(|(span, candidates)| {
                    spans.contains(span) && !candidates.iter().all(matches_target)
                }) {
                    return Err(format!(
                        "A name resolves differently between entries in {source}"
                    ));
                }
            }
        }
        let mut updated = documents.clone();
        let mut changes = Vec::new();
        for (source, spans) in &edits {
            let path = PathBuf::from(source);
            let physical = path.canonicalize().map(|path| normalized(&path));
            if !editable_source(&path, &workspace.editable)
                || physical.as_ref().is_ok_and(|physical| physical != source)
                || (path.exists() && physical.is_err())
            {
                return Err(format!(
                    "A rename would edit a file outside the workspace ({source})"
                ));
            }
            let (uri, text) = files.get(source).ok_or("Rename source disappeared")?;
            let open = documents
                .iter()
                .find(|(path, _)| normalized(path) == *source);
            if open.is_none() && std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
                return Err(format!("Source changed on disk ({source})"));
            }
            let uri = open.map_or(uri, |(_, open)| &open.uri);
            let version = open.map(|(_, open)| open.document.version);
            let original = crate::document::Lines::new(text);
            let mut changed = text.to_string();
            for span in spans.iter().rev() {
                changed.replace_range(span.start..span.end, new_name);
            }
            updated.insert(
                open.map_or(path, |(path, _)| path.clone()),
                OpenDocument {
                    uri: uri.clone(),
                    document: Document::new(version.unwrap_or_default(), changed),
                },
            );
            changes.push(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                edits: spans
                    .iter()
                    .map(|&span| {
                        OneOf::Left(TextEdit::new(
                            original.range(text, span),
                            new_name.to_owned(),
                        ))
                    })
                    .collect(),
            });
        }
        self.install_documents(&updated)?;
        for (request, before) in snapshots {
            let after = self.analyze_request(request.clone())?;
            if let Some(error) = after.first_error() {
                return Err(format!(
                    "Rename would break {} ({error})",
                    request.entry_path.as_str()
                ));
            }
            let actual = bindings(&after);
            let expected = shifted_bindings(before, &edits, new_name.len());
            if actual != expected {
                return Err(format!(
                    "Rename would change name resolution in {}",
                    request.entry_path.as_str()
                ));
            }
        }
        let open: BTreeSet<_> = documents.keys().map(|path| normalized(path)).collect();
        for (source, (_, text)) in files {
            if !open.contains(&source)
                && std::fs::read_to_string(&source).ok().as_deref() != Some(&text)
            {
                return Err(format!("Source changed on disk ({source})"));
            }
        }
        if versioned {
            Ok(WorkspaceEdit {
                document_changes: Some(DocumentChanges::Edits(changes)),
                ..WorkspaceEdit::default()
            })
        } else {
            Ok(WorkspaceEdit::new(
                changes
                    .into_iter()
                    .map(|change| {
                        (
                            change.text_document.uri,
                            change
                                .edits
                                .into_iter()
                                .filter_map(|item| match item {
                                    OneOf::Left(edit) => Some(edit),
                                    OneOf::Right(_) => None,
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            ))
        }
    }
}

fn editable_source(path: &Path, roots: &BTreeSet<PathBuf>) -> bool {
    let Some(root) = roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
    else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    !relative.starts_with("build")
        && !relative.starts_with("target")
        && path
            .ancestors()
            .skip(1)
            .take_while(|ancestor| *ancestor != root)
            .all(|ancestor| !ancestor.join("nia.toml").is_file())
}

fn bindings(analysis: &Analysis) -> Bindings {
    let mut result = Bindings::new();
    let sources: HashMap<_, Arc<str>> = analysis
        .files
        .keys()
        .map(|source| (source, source.as_str().into()))
        .collect();
    let declaration = |symbol| {
        let (source, span) = analysis.definitions.get(&symbol)?;
        Some(Site {
            source: sources.get(source)?.clone(),
            span: *span,
        })
    };
    for (source, file) in &analysis.files {
        let mut occurrences = BTreeMap::<Span, BTreeSet<Binding>>::new();
        for (span, symbol, alias) in analysis.rename_occurrences(file) {
            occurrences.entry(span).or_default().insert(Binding {
                declaration: declaration(symbol),
                alias: alias.and_then(declaration),
            });
        }
        result.insert(source.clone(), occurrences);
    }
    result
}

fn shifted_bindings(before: Bindings, edits: &Edits, len: usize) -> Bindings {
    let offsets: BTreeMap<_, Vec<_>> = edits
        .iter()
        .map(|(source, spans)| {
            let mut delta = 0;
            (
                source.as_str(),
                spans
                    .iter()
                    .map(|&span| {
                        let before = delta;
                        delta += len as isize - span.len() as isize;
                        (span, before)
                    })
                    .collect(),
            )
        })
        .collect();
    let shift = |source: &str, span: Span| {
        let Some(spans) = offsets.get(source) else {
            return span;
        };
        let (delta, length) = match spans.binary_search_by_key(&span, |&(edit, _)| edit) {
            Ok(index) => (spans[index].1, len),
            Err(0) => (0, span.len()),
            Err(index) => {
                let (edit, before) = spans[index - 1];
                (before + len as isize - edit.len() as isize, span.len())
            }
        };
        let start = span.start.saturating_add_signed(delta);
        Span::new(start, start + length)
    };
    let shift_site = |site: Site| Site {
        span: shift(&site.source, site.span),
        ..site
    };
    before
        .into_iter()
        .map(|(source, occurrences)| {
            let updated = occurrences
                .into_iter()
                .map(|(span, candidates)| {
                    (
                        shift(&source, span),
                        candidates
                            .into_iter()
                            .map(|binding| Binding {
                                declaration: binding.declaration.map(shift_site),
                                alias: binding.alias.map(shift_site),
                            })
                            .collect(),
                    )
                })
                .collect();
            (source, updated)
        })
        .collect()
}
