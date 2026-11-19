// SPDX-License-Identifier: GPL-3.0-or-later
//! Source positions and diagnostics after a proposed workspace edit.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
};

use nia_compiler_query::ProgramDiagnostic;
use nia_diagnostic::{Diagnostic, DiagnosticCategory, SpanSource};
use nia_span::Span;

use crate::analysis::normalized;
use crate::workspace::{Binding, Bindings, Cancellation, Site};

pub(crate) type Edits = BTreeMap<String, BTreeMap<Span, String>>;

pub(crate) fn preserves_bindings(
    before: Bindings,
    after: &Bindings,
    map_span: impl Fn(&str, Span) -> Span,
    cancel: &Cancellation,
) -> bool {
    let shift = |mut site: Site| {
        site.span = map_span(&site.source, site.span);
        site
    };
    before.into_iter().all(|(source, entries)| {
        cancel.check().is_ok()
            && entries.into_iter().all(|(span, candidates)| {
                let expected: BTreeSet<_> = candidates
                    .into_iter()
                    .filter(|binding| binding.declaration.is_some() || binding.alias.is_some())
                    .map(|binding| Binding {
                        declaration: binding.declaration.map(&shift),
                        alias: binding.alias.map(&shift),
                    })
                    .collect();
                expected.is_empty()
                    || after
                        .get(&source)
                        .and_then(|entries| entries.get(&map_span(&source, span)))
                        .is_some_and(|actual| *actual == expected)
            })
    })
}

pub(crate) fn relocated(source: &str, moves: &[(PathBuf, PathBuf)]) -> Option<PathBuf> {
    let (_, new, tail) = moves
        .iter()
        .filter_map(|(old, new)| {
            let tail = Path::new(source).strip_prefix(old).ok()?;
            Some((old, new, tail))
        })
        .max_by_key(|(old, _, _)| old.components().count())?;
    Some(if tail.as_os_str().is_empty() {
        new.clone()
    } else {
        new.join(tail)
    })
}

struct EditOffset {
    span: Span,
    before: isize,
    length: usize,
}

impl EditOffset {
    fn after(&self) -> isize {
        self.before + self.length as isize - self.span.len() as isize
    }
}

pub(crate) struct EditOffsets<'a>(BTreeMap<&'a str, Vec<EditOffset>>);

impl<'a> EditOffsets<'a> {
    pub(crate) fn new(edits: &'a Edits) -> Self {
        Self(
            edits
                .iter()
                .map(|(source, spans)| {
                    let mut delta = 0;
                    let offsets = spans
                        .iter()
                        .map(|(&span, replacement)| {
                            let edit = EditOffset {
                                span,
                                before: delta,
                                length: replacement.len(),
                            };
                            delta = edit.after();
                            edit
                        })
                        .collect();
                    (source.as_str(), offsets)
                })
                .collect(),
        )
    }

    pub(crate) fn binding_span(&self, source: &str, span: Span) -> Span {
        let Some(spans) = self.0.get(source) else {
            return span;
        };
        let (delta, length) = match spans.binary_search_by_key(&span, |edit| edit.span) {
            Ok(index) => (spans[index].before, spans[index].length),
            Err(0) => (0, span.len()),
            Err(index) => (spans[index - 1].after(), span.len()),
        };
        let start = span.start.saturating_add_signed(delta);
        Span::new(start, start + length)
    }

    fn unchanged_span(&self, source: &str, span: Span) -> Option<Span> {
        let Some(edits) = self.0.get(source) else {
            return Some(span);
        };
        let index = edits.partition_point(|edit| edit.span.end <= span.start);
        // An intact span is evidence that the diagnostic still concerns the same text.
        if edits
            .get(index)
            .is_some_and(|edit| edit.span.start < span.end)
        {
            return None;
        }
        let delta = index.checked_sub(1).map_or(0, |index| edits[index].after());
        Some(Span::new(
            span.start.checked_add_signed(delta)?,
            span.end.checked_add_signed(delta)?,
        ))
    }

    fn diagnostic(
        &self,
        item: &ProgramDiagnostic,
        moves: &[(PathBuf, PathBuf)],
    ) -> Option<(String, Diagnostic)> {
        let source = item.path.as_str();
        let relocate = |source: &str| {
            relocated(source, moves).map_or_else(|| source.to_owned(), |path| normalized(&path))
        };
        let mut diagnostic = item.diagnostic.clone();
        for label in diagnostic.labels.iter_mut() {
            if label.span_source == SpanSource::Source {
                label.span = self.unchanged_span(source, label.span)?;
            }
        }
        for related in diagnostic.related.iter_mut() {
            related.span = self.unchanged_span(
                related.source_path.as_deref().unwrap_or(source),
                related.span,
            )?;
            if let Some(path) = &mut related.source_path {
                *path = relocate(path);
            }
        }
        for suggestion in diagnostic.suggestions.iter_mut() {
            for edit in suggestion.edits.iter_mut() {
                edit.span = self.unchanged_span(source, edit.span)?;
            }
        }
        if let Some(cause) = &mut diagnostic.cause {
            cause.span = self.unchanged_span(&cause.source_path, cause.span)?;
            cause.source_path = relocate(&cause.source_path);
        }
        if let Some(path) = &mut diagnostic.source_path {
            *path = relocate(path);
        }
        Some((relocate(source), diagnostic))
    }

    pub(crate) fn new_error<'before, 'after>(
        &self,
        before: impl IntoIterator<Item = &'before ProgramDiagnostic>,
        mut after: impl Iterator<Item = &'after ProgramDiagnostic>,
        moves: &[(PathBuf, PathBuf)],
    ) -> Option<&'after ProgramDiagnostic> {
        let allowed: HashSet<_> = before
            .into_iter()
            .filter_map(|item| self.diagnostic(item, moves))
            .collect();
        after.find(|item| {
            item.diagnostic.category() == DiagnosticCategory::Internal
                || !allowed.contains(&(item.path.as_str().to_owned(), item.diagnostic.clone()))
        })
    }
}
