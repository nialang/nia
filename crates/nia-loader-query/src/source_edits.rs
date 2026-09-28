// SPDX-License-Identifier: GPL-3.0-or-later
//! Prepare incremental syntax before retiring the old source under session admission.

use super::*;
use crate::queries::SyntaxModuleQuery;
use nia_span::Span;
use nia_syntax::TextEdit;

impl LoaderDatabase {
    /// Applies a UTF-8 edit to the specified current source revision.
    /// Stale revisions and invalid ranges leave the source and its queries intact.
    pub fn edit_source(&self, version: SourceVersion, edit: &TextEdit) -> QueryResult<SourceFile> {
        let invalid = || {
            self.db.invalid_input(
                &SourceTextQuery(version.id),
                "source edit requires a current revision and a valid UTF-8 range",
            )
        };
        let path = self.sources.path_for_id(version.id).ok_or_else(invalid)?;
        self.update_source((*path).clone(), |previous| {
            let previous = previous
                .filter(|file| file.version() == version)
                .ok_or_else(invalid)?;
            let text = edit.apply(&previous.text).ok_or_else(invalid)?;
            Ok((Arc::from(text), Some(edit.clone())))
        })
    }

    pub(super) fn update_source(
        &self,
        path: SourcePath,
        prepare: impl FnOnce(Option<&SourceFile>) -> QueryResult<(Arc<str>, Option<TextEdit>)>,
    ) -> QueryResult<SourceFile> {
        let source_id = self
            .sources
            .id_for_path(&path)
            .map_err(|error| QueryError::internal(error.to_string()))?;
        self.db.with_retirement(|retirement| {
            let previous = self.sources.source_for_id(source_id);
            let (text, edit) = prepare(previous.as_ref())?;
            let previous_version = previous.as_ref().map_or(
                SourceVersion {
                    id: source_id,
                    revision: SourceRevision::INITIAL,
                },
                SourceFile::version,
            );
            let revision = previous
                .as_ref()
                .map_or(Ok(SourceRevision::INITIAL), |file| {
                    file.revision
                        .next()
                        .ok_or_else(|| QueryError::internal("source revision space exhausted"))
                })?;
            let version = SourceVersion {
                id: source_id,
                revision,
            };
            let syntax = match (
                &previous,
                edit,
                retirement.cached(&SyntaxModuleQuery(previous_version))?,
            ) {
                (Some(previous), Some(edit), Some(syntax))
                    if syntax.tree.source() == previous.text.as_ref() =>
                {
                    Some(syntax.reparse(&edit, Some(version)).map_err(|error| {
                        self.db.invalid_input(
                            &SyntaxModuleQuery(version),
                            format!("failed to reparse grammar tree: {error:?}"),
                        )
                    })?)
                }
                _ => None,
            };
            // Parsing and edit validation finish before any old resource is retired.
            self.reset_provider_facts(retirement)?;
            retirement.invalidate(SourceTextQuery(source_id))?;
            queries::retire_source_revision_queries(retirement, previous_version)?;
            self.db
                .context()
                .node_store
                .retire_revision(previous_version);
            let published = syntax.is_some();
            if let Some(syntax) = syntax {
                retirement.publish_shared(
                    SyntaxModuleQuery(version),
                    syntax,
                    &SourceTextQuery(source_id),
                )?;
            }
            // Source replacement commits last; no query can observe the prepared
            // syntax until session admission reopens.
            match self.sources.set_source(path, text) {
                Ok(file) => Ok(file),
                Err(error) => {
                    if published
                        && let Err(cleanup) = retirement.retire(&SyntaxModuleQuery(version))
                    {
                        return Err(QueryError::internal(format!(
                            "{error}; failed to retire prepared syntax: {cleanup}"
                        )));
                    }
                    Err(QueryError::internal(error.to_string()))
                }
            }
        })
    }
}

pub(super) fn replacement_edit(before: &str, after: &str) -> TextEdit {
    let mut start = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
        start -= 1;
    }
    let mut suffix = before[start..]
        .bytes()
        .rev()
        .zip(after[start..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    TextEdit::replace(
        Span::new(start, before.len() - suffix),
        &after[start..after.len() - suffix],
    )
}
