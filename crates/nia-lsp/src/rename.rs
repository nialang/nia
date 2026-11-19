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
    rewrite::{EditOffsets, Edits, relocated},
    workspace::{Binding, Bindings, Cancellation, Coverage, Site, WorkspaceIndex, bindings},
};

type ShorthandFields = BTreeMap<String, BTreeMap<Span, (Site, usize)>>;

mod relocation;
use relocation::{NamespaceRewrite, collect_relocation};

#[derive(Default)]
struct RenamePlan {
    edits: Edits,
    moves: Moves,
    shorthand: ShorthandFields,
    namespaces: Vec<NamespaceRewrite>,
}

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

pub(crate) fn identifier(text: &str) -> bool {
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
        let moves = file_moves(files);
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
        let declared: Vec<_> = moves
            .iter()
            .filter_map(|(old, new)| {
                Some((
                    old,
                    new,
                    workspace.module_declarations.get(&normalized(old))?,
                ))
            })
            .collect();
        if declared.is_empty() {
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
        }
        let mut plan = RenamePlan::default();
        let mut analyses = BTreeMap::new();
        let directories: Moves = moves
            .iter()
            .filter(|(old, _)| old.is_dir())
            .cloned()
            .collect();
        for (old, new, declarations) in declared {
            cancel.check()?;
            if declarations.len() != 1 {
                return Err(format!(
                    "Multiple declarations resolve to {}",
                    old.display()
                ));
            }
            let new_name = new
                .file_stem()
                .and_then(|name| name.to_str())
                .filter(|_| is_nia(new))
                .ok_or("A module destination must be a Nia source file")?;
            let old_parent = old
                .parent()
                .ok_or("A module source must have a parent directory")?;
            let parent = relocated(&normalized(old_parent), &directories)
                .unwrap_or_else(|| old_parent.to_path_buf());
            let declaration = declarations.first().expect("one declaration");
            let source = Path::new(declaration.source.as_ref());
            let analysis = match analyses.entry(source) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(self.analyze_with_cancel(documents, source, Some(cancel))?)
                }
            };
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
            let (symbol, _) = target(analysis, source, position)?;
            if analysis.module_file(symbol) != Some(normalized(old).as_str()) {
                return Err("The module declaration changed during the file operation".into());
            }
            let change = if Some(parent.as_path()) == new.parent() {
                collect_rename(analysis, &workspace, source, symbol, new_name, cancel)?
            } else {
                collect_relocation(analysis, &workspace, symbol, old, new, cancel)?
            };
            plan.namespaces.extend(change.namespaces);
            for (source, spans) in change.edits {
                let merged = plan.edits.entry(source).or_default();
                for (span, text) in spans {
                    if merged.insert(span, text).is_some() {
                        return Err("The module declarations require overlapping edits".into());
                    }
                }
            }
            plan.moves
                .extend(change.moves.into_iter().map(|(old, new)| {
                    let destination = relocated(&normalized(&new), &directories).unwrap_or(new);
                    (old, destination)
                }));
        }
        if plan
            .moves
            .iter()
            .any(|operation| !moves.contains(operation))
            || moves
                .iter()
                .any(|(old, new)| relocated(&normalized(old), &plan.moves).as_ref() != Some(new))
        {
            return Err("The file operation must match each module source and its child directory, if present".into());
        }
        self.verify_rename(&workspace, documents, plan, versioned, false, cancel)
            .map(Some)
    }

    pub(crate) fn rename(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        request: Rename<'_>,
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
        validate_new_name(symbol, new_name)?;
        let (text, span) = analysis
            .declaration_source(symbol)
            .ok_or("The declaration is unavailable")?;
        if text[span.start..span.end] == *new_name {
            return Ok(WorkspaceEdit::default());
        }
        if matches!(symbol, Symbol::Module(_)) && !resource_rename {
            return Err("The editor must support rename file operations".into());
        }
        let workspace = self.workspace_index(path, documents, folders, cancel)?;
        let plan = collect_rename(analysis, &workspace, path, symbol, new_name, cancel)?;
        self.verify_rename(
            &workspace,
            documents,
            plan,
            versioned,
            resource_rename,
            cancel,
        )
    }

    fn verify_rename(
        &self,
        workspace: &WorkspaceIndex,
        documents: &Documents,
        plan: RenamePlan,
        versioned: bool,
        resource_rename: bool,
        cancel: &Cancellation,
    ) -> Result<WorkspaceEdit, String> {
        let RenamePlan {
            edits,
            moves,
            shorthand,
            namespaces,
        } = plan;
        for (index, (old, new)) in moves.iter().enumerate() {
            let conflicts = moves[..index].iter().any(|(other_old, other_new)| {
                (paths_overlap(old, other_old) || paths_overlap(new, other_new))
                    && !moves_inside(old, new, other_old, other_new)
                    && !moves_inside(other_old, other_new, old, new)
            });
            if conflicts {
                return Err(format!("Overlapping module moves at {}", old.display()));
            }
        }
        if let Some((package, error)) = workspace.unavailable_plans.first() {
            return Err(format!(
                "Cannot verify rename in package `{}` ({error})",
                package.display()
            ));
        }
        if workspace
            .entries
            .iter()
            .any(|entry| relocated(entry.request.check.entry_path.as_str(), &moves).is_some())
        {
            return Err("A module move would relocate a declared build entry".to_owned());
        }
        let files = &workspace.files;
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
            let mut previous_end = 0;
            for &span in spans.keys() {
                if span.start < previous_end || text.get(span.start..span.end).is_none() {
                    return Err(format!("Overlapping or invalid rename edits in {source}"));
                }
                previous_end = span.end;
            }
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
        let offsets = EditOffsets::new(&edits);
        let affected = workspace.entries.iter().filter(|entry| {
            edits.keys().any(|source| entry.depends_on(source))
                || moves.iter().any(|(from, to)| entry.observes_move(from, to))
        });
        for entry in affected {
            cancel.check()?;
            let request = &entry.request;
            let after = verification
                .analyze_request(request.clone(), cancel)
                .map_err(|error| format!("Cannot verify rename in {request} ({error})"))?;
            if let Some(error) =
                offsets.new_error(entry.coverage.source_errors(), after.errors(), &moves)
            {
                return Err(format!(
                    "Rename would break {request} ({} at {})",
                    error.diagnostic.summary,
                    error.path.as_str(),
                ));
            }
            let mut actual = bindings(&after, cancel)?;
            for rewrite in &namespaces {
                rewrite.remove_new_prefixes(&mut actual, &offsets, &moves);
            }
            let expected = shifted_bindings(
                entry.bindings.clone(),
                &offsets,
                &moves,
                &shorthand,
                &namespaces,
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
                return Err(format!("Rename would change name resolution in {request}"));
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
        for (old, new) in &moves {
            check_destination(old, new)?;
        }
        if resource_rename && !moves.is_empty() {
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

fn collect_rename(
    analysis: &Analysis,
    workspace: &WorkspaceIndex,
    path: &Path,
    symbol: Symbol,
    new_name: &str,
    cancel: &Cancellation,
) -> Result<RenamePlan, String> {
    validate_new_name(symbol, new_name)?;
    let (source, span) = analysis
        .definitions
        .get(&symbol)
        .ok_or("The declaration is unavailable")?;
    let old = &analysis.files[source].text[span.start..span.end];
    if old == new_name {
        return Ok(RenamePlan::default());
    }
    let target = Site {
        source: Arc::from(source.as_str()),
        span: *span,
    };
    let workspace_symbol = analysis.rename_workspace_symbol(symbol);
    let moves = if let Some(path) = analysis.module_file(symbol) {
        module_moves(Path::new(path), new_name, workspace)?
    } else {
        Vec::new()
    };
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
        if !workspace.entry_may_reach(entry, source) {
            continue;
        }
        if let Coverage::Unavailable(error) = &entry.coverage {
            return Err(format!(
                "Cannot check rename in {} ({error})",
                entry.request
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
                        && !(entry.inactive_at(source, token.span)
                            && workspace.has_resolved_binding(source, token.span))
                })
            {
                return Err(format!(
                    "Cannot determine every use of `{old}` in {source} under {}",
                    entry.request
                ));
            }
            for (&span, candidates) in occurrences {
                if &text[span.start..span.end] == old && candidates.iter().any(matches_target) {
                    let is_shorthand = field
                        && parsed
                            .entry(source)
                            .or_insert_with(|| nia_syntax::parse(text, None).ok())
                            .as_ref()
                            .is_some_and(|syntax| field_shorthand(syntax.tree.green_root(), span));
                    let (range, replacement) = if is_shorthand {
                        shorthand
                            .entry(source.clone())
                            .or_default()
                            .insert(span, (target.clone(), new_name.len()));
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
                    .is_some_and(|spans| spans.contains_key(span))
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
    Ok(RenamePlan {
        edits,
        moves,
        shorthand,
        namespaces: Vec::new(),
    })
}

fn validate_new_name(symbol: Symbol, name: &str) -> Result<(), String> {
    if !identifier(name) {
        return Err("The new name must be a Nia identifier".into());
    }
    if matches!(symbol, Symbol::Global(_)) && matches!(name, "main" | "build") {
        return Err("The name is reserved for entry points".into());
    }
    Ok(())
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

pub(crate) fn file_moves(files: Vec<lsp_types::FileRename>) -> BTreeSet<(PathBuf, PathBuf)> {
    let decode = |uri: &str| uri.parse().ok().and_then(|uri| path(&uri));
    files
        .iter()
        .filter_map(|file| Some((decode(&file.old_uri)?, decode(&file.new_uri)?)))
        .filter(|(old, new)| old != new)
        .collect()
}

fn paths_overlap(first: &Path, second: &Path) -> bool {
    let case_sensitive = !cfg!(windows);
    let matches = |path: &Path, prefix: &Path| {
        if case_sensitive {
            path.starts_with(prefix)
        } else {
            let mut components = path.components();
            prefix.components().all(|part| {
                components.next().is_some_and(|component| {
                    component
                        .as_os_str()
                        .as_encoded_bytes()
                        .eq_ignore_ascii_case(part.as_os_str().as_encoded_bytes())
                })
            })
        }
    };
    matches(first, second) || matches(second, first)
}

fn moves_inside(source: &Path, destination: &Path, from: &Path, to: &Path) -> bool {
    // The nested operation can change the name in its transported directory.
    source
        .parent()
        .and_then(|parent| parent.strip_prefix(from).ok())
        .is_some_and(|relative| destination.parent() == Some(to.join(relative).as_path()))
}

fn module_moves(
    source: &Path,
    new_name: &str,
    workspace: &WorkspaceIndex,
) -> Result<Moves, String> {
    let file = source.with_file_name(format!("{new_name}.nia"));
    module_move_to(source, file, workspace)
}

fn module_move_to(
    source: &Path,
    file: PathBuf,
    workspace: &WorkspaceIndex,
) -> Result<Moves, String> {
    let subtree = source.with_extension("");
    let target_subtree = file.with_extension("");
    let mut moves = vec![(source.to_path_buf(), file)];
    if subtree.exists() {
        if !subtree.is_dir() {
            return Err(format!(
                "Module subtree is not a directory ({})",
                subtree.display()
            ));
        }
        moves.push((subtree, target_subtree));
    }
    for (old, new) in &moves {
        if old.is_dir() && new.starts_with(old) {
            return Err("A module subtree cannot move inside itself".into());
        }
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
        if old.parent() != new.parent()
            && let Some(parent) = new
                .parent()
                .and_then(|parent| parent.ancestors().find(|path| path.exists()))
            && parent
                .canonicalize()
                .ok()
                .is_none_or(|path| normalized(&path) != normalized(parent))
        {
            return Err(format!(
                "Cannot move a module through a linked destination ({})",
                parent.display()
            ));
        }
        check_destination(old, new)?;
    }
    Ok(moves)
}

fn check_destination(source: &Path, destination: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata)
            if !metadata.file_type().is_symlink()
                && source != destination
                && source.parent() == destination.parent()
                && source
                    .file_name()
                    .zip(destination.file_name())
                    .is_some_and(|(old, new)| {
                        old.as_encoded_bytes()
                            .eq_ignore_ascii_case(new.as_encoded_bytes())
                    })
                && destination
                    .canonicalize()
                    .is_ok_and(|path| normalized(&path) == normalized(source)) =>
        {
            Ok(())
        }
        Ok(_) => Err(format!(
            "Module destination already exists ({})",
            destination.display()
        )),
        Err(error) => Err(format!(
            "Cannot inspect module destination ({}) ({error})",
            destination.display()
        )),
    }
}

fn shifted_bindings(
    before: Bindings,
    offsets: &EditOffsets<'_>,
    moves: &Moves,
    shorthand: &ShorthandFields,
    namespaces: &[NamespaceRewrite],
) -> Bindings {
    let mut rewrites = BTreeMap::<_, Vec<_>>::new();
    for rewrite in namespaces {
        rewrites
            .entry(rewrite.before.source.as_ref())
            .or_default()
            .push(rewrite);
    }
    for spans in rewrites.values_mut() {
        spans.sort_unstable_by_key(|rewrite| rewrite.old_range);
    }
    let rewrite_at = |source: &str, span: Span| {
        let locations = rewrites.get(source)?;
        let index = locations.partition_point(|rewrite| rewrite.old_range.start <= span.start);
        let rewrite = locations.get(index.checked_sub(1)?)?;
        relocation::contains(rewrite.old_range, span).then_some(*rewrite)
    };
    let shift_site = |site: Site| Site {
        span: offsets.binding_span(&site.source, site.span),
        source: relocated(&site.source, moves).map_or(site.source, |path| normalized(&path).into()),
    };
    let translate_site = |site: Site| {
        rewrite_at(&site.source, site.span)
            .and_then(|rewrite| rewrite.translated(site.span, offsets, moves))
            .unwrap_or_else(|| shift_site(site))
    };
    let mut updated = Bindings::new();
    for (source, occurrences) in before {
        let moved =
            relocated(&source, moves).map_or_else(|| source.clone(), |path| normalized(&path));
        updated.entry(moved.clone()).or_default();
        for (span, candidates) in occurrences {
            if let Some(rewrite) = rewrite_at(&source, span) {
                if let Some(projected) = rewrite.translated(span, offsets, moves) {
                    let bindings = updated.entry(projected.source.to_string()).or_default();
                    bindings
                        .entry(projected.span)
                        .or_default()
                        .extend(candidates.into_iter().map(|binding| Binding {
                            declaration: binding.declaration.map(translate_site),
                            alias: binding.alias.map(translate_site),
                        }));
                }
                continue;
            }
            let destination = updated.get_mut(&moved).expect("relocated binding source");
            let shifted = offsets.binding_span(&source, span);
            for binding in candidates {
                let binding_span = if let Some((target, name_len)) =
                    shorthand.get(&source).and_then(|spans| spans.get(&span))
                    && binding.alias.as_ref().or(binding.declaration.as_ref()) == Some(target)
                {
                    Span::new(shifted.start - name_len - 2, shifted.start - 2)
                } else {
                    shifted
                };
                destination
                    .entry(binding_span)
                    .or_default()
                    .insert(Binding {
                        declaration: binding.declaration.map(translate_site),
                        alias: binding.alias.map(translate_site),
                    });
            }
        }
    }
    updated
}
