// SPDX-License-Identifier: GPL-3.0-or-later
//! Prepare incremental syntax before retiring the old source under session admission.

use super::*;
use crate::queries::SyntaxModuleQuery;
use nia_syntax::TextEdit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SourceGrammarRevisionQuery;

impl nia_query::QueryKey<LoaderContext> for SourceGrammarRevisionQuery {
    type Value = u64;

    fn name() -> &'static str {
        "source_grammar_revision"
    }

    fn execute_result(&self, db: &QueryDb<LoaderContext>) -> QueryResult<Self::Value> {
        Ok(*db.context().source_grammar_revision.lock())
    }
}

impl LoaderDatabase {
    pub(super) fn advance_source_grammar_revision(
        &self,
        retirement: &QueryRetirement<'_, LoaderContext>,
    ) -> QueryResult<()> {
        let mut revision = self.db.context().source_grammar_revision.lock();
        let next = revision
            .checked_add(1)
            .ok_or_else(|| QueryError::internal("source grammar revision space exhausted"))?;
        retirement.invalidate_without_report(SourceGrammarRevisionQuery)?;
        *revision = next;
        Ok(())
    }

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
            let source_query = SourceTextQuery(source_id);
            let observed = retirement.has_slot(&source_query)?;
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
            let classify =
                |before: &nia_syntax::Parse, after: &nia_syntax::Parse, edit: &TextEdit| {
                    if before.tree.source() == after.tree.source() {
                        return false;
                    }
                    before.same_grammar(after)
                        || (edit.span.len() == edit.replacement.len()
                            && before.changes_runtime_integer_return(after, edit))
                };
            let (syntax, preserves_external_facts) = match (
                &previous,
                edit.as_ref(),
                retirement.cached(&SyntaxModuleQuery(previous_version))?,
            ) {
                (Some(previous), Some(edit), Some(syntax))
                    if syntax.tree.source() == previous.text.as_ref() =>
                {
                    let edited = syntax.reparse(edit, Some(version)).map_err(|error| {
                        self.db.invalid_input(
                            &SyntaxModuleQuery(version),
                            format!("failed to reparse grammar tree: {error:?}"),
                        )
                    })?;
                    let change = classify(&syntax, &edited, edit);
                    (Some(edited), change)
                }
                (Some(previous), Some(edit), None)
                    if observed && previous.text.as_ref() != text.as_ref() =>
                {
                    // A relocated source can have an overlay under an
                    // additional identity with no cached parse of its own.
                    let parsed = nia_syntax::parse(&text, Some(version)).ok();
                    let change = parsed
                        .as_ref()
                        .and_then(|after| {
                            nia_syntax::parse(&previous.text, Some(previous_version))
                                .ok()
                                .map(|before| classify(&before, after, edit))
                        })
                        .unwrap_or_default();
                    (parsed, change)
                }
                _ => (None, false),
            };
            let same_path = previous
                .as_ref()
                .is_some_and(|previous| previous.path.as_str() == path.as_str());
            // Parsing and edit validation finish before any old resource is retired.
            // A runtime return changes the edited module's source version.
            // Keeping its width also preserves downstream caller coordinates.
            // Unedited modules can keep their checked bodies and provider facts.
            // A persisted plan may have read sources outside the query database.
            if (same_path && preserves_external_facts) || !observed {
                *self.db.context().provider_demand_plan_candidate.lock() = None;
            } else {
                self.advance_source_grammar_revision(retirement)?;
                self.reset_provider_facts(retirement)?;
            }
            retirement.invalidate_without_report(source_query)?;
            queries::retire_source_revision_queries(retirement, previous_version)?;
            self.db
                .context()
                .node_store
                .retire_revision(previous_version);
            let published = syntax.is_some();
            if let Some(syntax) = syntax {
                retirement.publish_shared(SyntaxModuleQuery(version), syntax, &source_query)?;
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
