// SPDX-License-Identifier: GPL-3.0-or-later
//! Validates workspace edits against fresh compiler analyses.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use lsp_types::{
    DocumentChangeOperation, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, PrepareRenameResponse, RenameFile, ResourceOp, TextDocumentEdit, TextEdit,
    WorkspaceEdit,
};
use nia_lexer::{TokenKind, tokenize};
use nia_span::Span;

use crate::{
    analysis::{Analysis, Documents, OpenDocument, Session, Symbol, normalized, path, uri},
    document::{Document, text_workspace_edit},
    workspace::{Binding, Bindings, Cancellation, Site, WorkspaceIndex, bindings},
};

type Edits = BTreeMap<String, BTreeMap<Span, String>>;
type ShorthandFields = BTreeMap<String, BTreeSet<Span>>;

impl Analysis {
    pub(crate) fn prepare_rename(
        &self,
        path: &Path,
        position: Position,
        resource_rename: bool,
    ) -> Option<PrepareRenameResponse> {
        let (symbol, span) = target(self, path, position).ok()?;
        if matches!(symbol, Symbol::Module(_)) && !resource_rename {
            return None;
        }
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
    pub(crate) resource_rename: bool,
}

impl Session {
    pub(crate) fn will_rename_files(
        &mut self,
        files: Vec<lsp_types::FileRename>,
        documents: &Documents,
        folders: &[PathBuf],
        versioned: bool,
        cancel: &Cancellation,
    ) -> Result<Option<WorkspaceEdit>, String> {
        let decode = |uri: &str| uri.parse().ok().and_then(|uri| path(&uri));
        let moves: BTreeSet<_> = files
            .iter()
            .filter_map(|file| Some((decode(&file.old_uri)?, decode(&file.new_uri)?)))
            .filter(|(old, new)| old != new)
            .collect();
        let is_nia = |path: &Path| path.extension().is_some_and(|ext| ext == "nia");
        let Some(anchor) = moves.iter().find_map(|(old, _)| {
            if is_nia(old) {
                return Some(old.clone());
            }
            // Module subtrees may have dots in an ancestor path. Append the extension
            // to the final component, which is the module name.
            let sibling = old.with_file_name(format!("{}.nia", old.file_name()?.to_str()?));
            (old.is_dir() && sibling.is_file()).then_some(sibling)
        }) else {
            return Ok(None);
        };
        let workspace = self.workspace_index(&anchor, documents, folders, cancel)?;
        let mut declared = moves.iter().filter_map(|(old, new)| {
            Some((
                old,
                new,
                workspace.module_declarations.get(&normalized(old))?,
            ))
        });
        let Some((old, new, declarations)) = declared.next() else {
            if moves.iter().any(|(old, _)| {
                old.is_dir()
                    && workspace.module_declarations.keys().any(|module| {
                        Path::new(module).starts_with(old)
                            || Path::new(module).with_extension("") == *old
                    })
            }) {
                return Err("A module subtree must move with its source file".into());
            }
            return Ok(None);
        };
        if declared.next().is_some() || declarations.len() != 1 {
            return Err("The file operation has multiple module declarations to update".into());
        }
        let new_name = new
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|_| is_nia(new))
            .ok_or("A module destination must be a Nia source file")?;
        if old.parent() != new.parent() {
            return Err("A module source must stay in its declaring module's directory".into());
        }
        let declaration = declarations.first().expect("one declaration");
        let source = Path::new(declaration.source.as_ref());
        let analysis = self.analyze_with_cancel(documents, source, Some(cancel))?;
        let file = analysis
            .file(source)
            .ok_or("Module declaration is unavailable")?;
        if workspace
            .files
            .get(declaration.source.as_ref())
            .map(|(_, text)| text)
            != Some(&file.text)
        {
            return Err("The module declaration changed during the file operation".into());
        }
        let position = file.lines.range(&file.text, declaration.span).start;
        self.rename_checked(
            &analysis,
            documents,
            Rename {
                path: source,
                position,
                new_name,
                folders,
                versioned,
                resource_rename: false,
            },
            Some(&moves),
            cancel,
        )
        .map(Some)
    }

    pub(crate) fn rename(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        request: Rename<'_>,
        cancel: &Cancellation,
    ) -> Result<WorkspaceEdit, String> {
        self.rename_checked(analysis, documents, request, None, cancel)
    }

    fn rename_checked(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        request: Rename<'_>,
        client_moves: Option<&BTreeSet<(PathBuf, PathBuf)>>,
        cancel: &Cancellation,
    ) -> Result<WorkspaceEdit, String> {
        let Rename {
            path,
            position,
            new_name,
            folders,
            versioned,
            resource_rename,
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
            if client_moves.is_some() {
                return Err(
                    "The module declaration does not match the requested file rename".into(),
                );
            }
            return Ok(WorkspaceEdit::default());
        }
        let target = Site {
            source: Arc::from(source.as_str()),
            span: *span,
        };
        let workspace_symbol = analysis.rename_workspace_symbol(symbol);
        let workspace = self.workspace_index(path, documents, folders, cancel)?;
        if let Some((package, error)) = workspace.unavailable_plans.first() {
            return Err(format!(
                "Cannot verify rename in package `{}` ({error})",
                package.display()
            ));
        }
        let moves = if let Some(path) = analysis.module_file(symbol) {
            if !resource_rename && client_moves.is_none() {
                return Err("The editor must support rename file operations".to_owned());
            }
            module_moves(Path::new(path), new_name, &workspace)?
        } else {
            Vec::new()
        };
        if let Some(client_moves) = client_moves
            && (moves.len() != client_moves.len()
                || moves
                    .iter()
                    .any(|operation| !client_moves.contains(operation)))
        {
            return Err("The file operation must match the module source and its child directory, if present".into());
        }
        if workspace
            .entries
            .iter()
            .any(|entry| relocated(entry.request.check.entry_path.as_str(), &moves).is_some())
        {
            return Err("A module move would relocate a declared build entry".to_owned());
        }
        let scope = analysis.rename_scope(symbol);
        let requested_source = normalized(path);
        let files = &workspace.files;
        for source in [target.source.as_ref(), requested_source.as_str()] {
            if files
                .get(source)
                .is_some_and(|(_, text)| *text != analysis.files[source].text)
            {
                return Err(format!("Source changed during rename ({source})"));
            }
        }
        let mut edits = Edits::new();
        let mut shorthand = ShorthandFields::new();
        let mut parsed = HashMap::new();
        let field = analysis.is_field(symbol);
        let matches_target = |binding: &Binding| {
            binding.alias.as_ref().or(binding.declaration.as_ref()) == Some(&target)
        };
        for entry in &workspace.entries {
            cancel.check()?;
            if let Some(error) = &entry.error {
                return Err(format!(
                    "Cannot check rename in {} ({error})",
                    entry.request.check.entry_path.as_str()
                ));
            }
            for (source, occurrences) in &entry.bindings {
                cancel.check()?;
                let (_, text) = &files[source];
                if (workspace_symbol || source.as_str() == target.source.as_ref())
                    && workspace.editable_source(Path::new(source))
                    && tokenize(text).into_iter().any(|token| {
                        token.kind == TokenKind::Ident
                            && scope.is_none_or(|scope| {
                                scope.start <= token.span.start && token.span.end <= scope.end
                            })
                            && &text[token.span.start..token.span.end] == old
                            && !occurrences.contains_key(&token.span)
                    })
                {
                    return Err(format!("Cannot determine every use of `{old}` in {source}"));
                }
                for (&span, candidates) in occurrences {
                    if &text[span.start..span.end] == old && candidates.iter().any(matches_target) {
                        let is_shorthand = field
                            && parsed
                                .entry(source)
                                .or_insert_with(|| nia_syntax::parse(text, None).ok())
                                .as_ref()
                                .is_some_and(|syntax| {
                                    field_shorthand(syntax.tree.green_root(), span)
                                });
                        let (range, replacement) = if is_shorthand {
                            shorthand.entry(source.clone()).or_default().insert(span);
                            (Span::new(span.start, span.start), format!("{new_name}: "))
                        } else {
                            (span, new_name.to_owned())
                        };
                        edits
                            .entry(source.clone())
                            .or_default()
                            .insert(range, replacement);
                    }
                }
            }
        }
        if !edits
            .get(target.source.as_ref())
            .is_some_and(|spans| spans.contains_key(&target.span))
        {
            return Err("The declaration is outside the checked workspace".to_owned());
        }
        for entry in &workspace.entries {
            cancel.check()?;
            for (source, occurrences) in &entry.bindings {
                let Some(spans) = edits.get(source) else {
                    continue;
                };
                if occurrences.iter().any(|(span, candidates)| {
                    if shorthand
                        .get(source)
                        .is_some_and(|spans| spans.contains(span))
                    {
                        !candidates.iter().any(matches_target)
                    } else {
                        spans.contains_key(span) && !candidates.iter().all(matches_target)
                    }
                }) {
                    return Err(format!(
                        "A name resolves differently between entries in {source}"
                    ));
                }
            }
        }
        if let Some((span, attribute)) = analysis.foreign_link_attribute(symbol) {
            edits
                .entry(source.clone())
                .or_default()
                .insert(span, format!("@[{attribute}(\"{old}\")] "));
        }
        let mut updated = documents.clone();
        let mut changes = Vec::new();
        for (source, spans) in &edits {
            cancel.check()?;
            let path = PathBuf::from(source);
            let physical = path.canonicalize().map(|path| normalized(&path));
            if !workspace.editable_source(&path)
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
            for (span, replacement) in spans.iter().rev() {
                changed.replace_range(span.start..span.end, replacement);
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
                    .map(|(&span, replacement)| {
                        OneOf::Left(TextEdit::new(
                            original.range(text, span),
                            replacement.clone(),
                        ))
                    })
                    .collect(),
            });
        }
        for (source, (file_uri, text)) in files {
            if let Some(destination) = relocated(source, &moves) {
                let mut document = updated
                    .iter()
                    .find(|(path, _)| normalized(path) == *source)
                    .map(|(_, document)| document.clone())
                    .unwrap_or_else(|| OpenDocument {
                        uri: file_uri.clone(),
                        document: Document::new(0, text.to_string()),
                    });
                document.uri = uri(&destination).ok_or("Cannot form a destination URI")?;
                updated.insert(destination, document);
            }
        }
        let mut verification = Session::new(self.toolchain.clone());
        verification.install_documents(&updated)?;
        for entry in &workspace.entries {
            cancel.check()?;
            let request = &entry.request;
            let after = verification.analyze_request(request.clone(), cancel)?;
            if let Some(error) = after.first_error() {
                return Err(format!(
                    "Rename would break {} ({error})",
                    request.check.entry_path.as_str()
                ));
            }
            let actual = bindings(&after, cancel)?;
            let expected = shifted_bindings(
                entry.bindings.clone(),
                &edits,
                &moves,
                &shorthand,
                &target,
                new_name.len(),
            );
            let workspace_file = |source: &str| {
                workspace
                    .editable
                    .iter()
                    .any(|root| Path::new(source).starts_with(root))
            };
            // A new method spelling can discover extra external providers.
            // Check every shared file and require the same workspace sources.
            if expected.iter().any(|(source, before)| {
                actual
                    .get(source)
                    .map_or_else(|| workspace_file(source), |after| after != before)
            }) || actual
                .keys()
                .any(|source| workspace_file(source) && !expected.contains_key(source))
            {
                return Err(format!(
                    "Rename would change name resolution in {}",
                    request.check.entry_path.as_str()
                ));
            }
        }
        let open: BTreeSet<_> = documents.keys().map(|path| normalized(path)).collect();
        for (source, (_, text)) in files {
            cancel.check()?;
            if !open.contains(source)
                && std::fs::read_to_string(source).ok().as_deref() != Some(text)
            {
                return Err(format!("Source changed on disk ({source})"));
            }
        }
        for (_, new) in &moves {
            check_destination(new)?;
        }
        if !moves.is_empty() && client_moves.is_none() {
            let mut operations: Vec<_> = changes
                .into_iter()
                .map(DocumentChangeOperation::Edit)
                .collect();
            for (old, new) in moves {
                operations.push(DocumentChangeOperation::Op(ResourceOp::Rename(
                    RenameFile {
                        old_uri: uri(&old).ok_or("Cannot form a source URI")?,
                        new_uri: uri(&new).ok_or("Cannot form a destination URI")?,
                        options: None,
                        annotation_id: None,
                    },
                )));
            }
            Ok(WorkspaceEdit {
                document_changes: Some(DocumentChanges::Operations(operations)),
                ..WorkspaceEdit::default()
            })
        } else {
            Ok(text_workspace_edit(changes, versioned))
        }
    }
}

fn field_shorthand(node: &nia_syntax::GreenNode, span: Span) -> bool {
    if node.span().start > span.start || span.end > node.span().end {
        return false;
    }
    if matches!(
        node.kind(),
        nia_syntax::SyntaxKind::FieldInit | nia_syntax::SyntaxKind::NamedPatternField
    ) {
        return node.token_span() == Some(span);
    }
    node.child_nodes().any(|child| field_shorthand(child, span))
}

type Moves = Vec<(PathBuf, PathBuf)>;

fn module_moves(
    source: &Path,
    new_name: &str,
    workspace: &WorkspaceIndex,
) -> Result<Moves, String> {
    let file = source.with_file_name(format!("{new_name}.nia"));
    let subtree = source.with_extension("");
    let mut moves = vec![(source.to_path_buf(), file)];
    if subtree.exists() {
        if !subtree.is_dir() {
            return Err(format!(
                "Module subtree is not a directory ({})",
                subtree.display()
            ));
        }
        moves.push((subtree.clone(), subtree.with_file_name(new_name)));
    }
    for (old, new) in &moves {
        if !workspace.editable_source(old)
            || !workspace.editable_source(new)
            || workspace.contains_build_state(old)
            || old
                .canonicalize()
                .ok()
                .is_none_or(|path| normalized(&path) != normalized(old))
        {
            return Err(format!(
                "Cannot move a module outside the workspace ({})",
                old.display()
            ));
        }
        check_destination(new)?;
    }
    Ok(moves)
}

fn check_destination(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "Module destination already exists ({})",
            path.display()
        )),
        Err(error) => Err(format!(
            "Cannot inspect module destination ({}) ({error})",
            path.display()
        )),
    }
}

fn relocated(source: &str, moves: &Moves) -> Option<PathBuf> {
    moves.iter().find_map(|(old, new)| {
        let tail = Path::new(source).strip_prefix(old).ok()?;
        Some(if tail.as_os_str().is_empty() {
            new.clone()
        } else {
            new.join(tail)
        })
    })
}

fn shifted_bindings(
    before: Bindings,
    edits: &Edits,
    moves: &Moves,
    shorthand: &ShorthandFields,
    target: &Site,
    name_len: usize,
) -> Bindings {
    let offsets: BTreeMap<_, Vec<_>> = edits
        .iter()
        .map(|(source, spans)| {
            let mut delta = 0;
            (
                source.as_str(),
                spans
                    .iter()
                    .map(|(&span, replacement)| {
                        let before = delta;
                        let length = replacement.len();
                        delta += length as isize - span.len() as isize;
                        (span, before, length)
                    })
                    .collect(),
            )
        })
        .collect();
    let shift = |source: &str, span: Span| {
        let Some(spans) = offsets.get(source) else {
            return span;
        };
        let (delta, length) = match spans.binary_search_by_key(&span, |&(edit, _, _)| edit) {
            Ok(index) => (spans[index].1, spans[index].2),
            Err(0) => (0, span.len()),
            Err(index) => {
                let (edit, before, length) = spans[index - 1];
                (before + length as isize - edit.len() as isize, span.len())
            }
        };
        let start = span.start.saturating_add_signed(delta);
        Span::new(start, start + length)
    };
    let shift_site = |site: Site| Site {
        span: shift(&site.source, site.span),
        source: relocated(&site.source, moves).map_or(site.source, |path| normalized(&path).into()),
    };
    before
        .into_iter()
        .map(|(source, occurrences)| {
            let mut updated = BTreeMap::<_, BTreeSet<_>>::new();
            for (span, candidates) in occurrences {
                let shifted = shift(&source, span);
                for binding in candidates {
                    let binding_span = if shorthand
                        .get(&source)
                        .is_some_and(|spans| spans.contains(&span))
                        && binding.alias.as_ref().or(binding.declaration.as_ref()) == Some(target)
                    {
                        Span::new(shifted.start - name_len - 2, shifted.start - 2)
                    } else {
                        shifted
                    };
                    updated.entry(binding_span).or_default().insert(Binding {
                        declaration: binding.declaration.map(shift_site),
                        alias: binding.alias.map(shift_site),
                    });
                }
            }
            (
                relocated(&source, moves).map_or(source, |path| normalized(&path)),
                updated,
            )
        })
        .collect()
}
