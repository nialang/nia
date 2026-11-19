// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler bindings with source identities that survive separate entry analyses.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use lsp_types::{Location, Position, Uri};
use nia_span::Span;

use crate::{
    analysis::{Analysis, Documents, Session, Symbol, normalized},
    document::Lines,
    project::EditorRequest,
};

#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        if self.0.load(Ordering::Relaxed) {
            Err("Analysis canceled".into())
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct Site {
    pub(crate) source: Arc<str>,
    pub(crate) span: Span,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Binding {
    pub(crate) declaration: Option<Site>,
    pub(crate) alias: Option<Site>,
}

pub(crate) type Bindings = BTreeMap<String, BTreeMap<Span, BTreeSet<Binding>>>;

pub(crate) struct Entry {
    pub(crate) request: EditorRequest,
    pub(crate) bindings: Bindings,
    pub(crate) error: Option<String>,
}

pub(crate) struct WorkspaceIndex {
    pub(crate) entries: Vec<Entry>,
    pub(crate) files: BTreeMap<String, (Uri, Arc<str>)>,
    pub(crate) editable: BTreeSet<PathBuf>,
    references: OnceLock<HashMap<Site, BTreeSet<Site>>>,
}

impl Session {
    pub(crate) fn workspace_index(
        &mut self,
        path: &Path,
        documents: &Documents,
        folders: &[PathBuf],
        cancel: &Cancellation,
    ) -> Result<Arc<WorkspaceIndex>, String> {
        cancel.check()?;
        self.install_documents(documents)?;
        if let Some(index) = &self.workspace_index
            && index.files.contains_key(&normalized(path))
        {
            return Ok(index.clone());
        }
        let workspace =
            self.projects
                .workspace_requests(path, documents.keys().cloned(), folders)?;
        let mut index = WorkspaceIndex {
            entries: Vec::new(),
            files: BTreeMap::new(),
            editable: workspace.editable,
            references: OnceLock::new(),
        };
        for (request, standalone) in workspace
            .entries
            .into_iter()
            .map(|request| (request, false))
            .chain(
                workspace
                    .standalone
                    .into_iter()
                    .map(|request| (request, true)),
            )
        {
            cancel.check()?;
            if standalone && index.files.contains_key(request.check.entry_path.as_str()) {
                continue;
            }
            let entry = match self.analyze_request(request.clone()) {
                Ok(analysis) => {
                    let error = analysis.first_error().map(str::to_owned);
                    let bindings = bindings(&analysis, cancel)?;
                    for (source, file) in analysis.files {
                        match index.files.entry(source) {
                            std::collections::btree_map::Entry::Occupied(current) => {
                                if current.get().1 != file.text {
                                    return Err(format!(
                                        "Source changed during workspace analysis ({})",
                                        current.key()
                                    ));
                                }
                            }
                            std::collections::btree_map::Entry::Vacant(current) => {
                                current.insert((file.uri, file.text));
                            }
                        }
                    }
                    Entry {
                        request,
                        bindings,
                        error,
                    }
                }
                Err(error) => Entry {
                    request,
                    bindings: Bindings::new(),
                    error: Some(error),
                },
            };
            index.entries.push(entry);
        }
        cancel.check()?;
        let index = Arc::new(index);
        self.workspace_index = Some(index.clone());
        Ok(index)
    }
}

impl WorkspaceIndex {
    pub(crate) fn references(
        &self,
        analysis: &Analysis,
        path: &Path,
        position: Position,
        include_declaration: bool,
        cancel: &Cancellation,
    ) -> Result<Vec<Location>, String> {
        let Some(file) = analysis.file(path) else {
            return Ok(Vec::new());
        };
        if self
            .files
            .get(&normalized(path))
            .is_some_and(|(_, text)| *text != file.text)
        {
            return Err("Source changed during reference lookup".into());
        }
        let Some(offset) = file.lines.offset(&file.text, position) else {
            return Ok(Vec::new());
        };
        let Some((symbol, _)) = analysis.rename_symbol_at(file, offset) else {
            return Ok(Vec::new());
        };
        let Some(target) = symbol_site(analysis, symbol) else {
            return Ok(Vec::new());
        };
        let references = if let Some(references) = self.references.get() {
            references
        } else {
            let mut references = HashMap::<Site, BTreeSet<Site>>::new();
            for entry in &self.entries {
                for (source, occurrences) in &entry.bindings {
                    cancel.check()?;
                    let source: Arc<str> = Arc::from(source.as_str());
                    for (&span, candidates) in occurrences {
                        for binding in candidates {
                            for target in binding.declaration.iter().chain(&binding.alias) {
                                references.entry(target.clone()).or_default().insert(Site {
                                    source: source.clone(),
                                    span,
                                });
                            }
                        }
                    }
                }
            }
            self.references.get_or_init(|| references)
        };
        let mut results = Vec::new();
        let mut lines = HashMap::new();
        for site in references.get(&target).into_iter().flatten() {
            cancel.check()?;
            if !include_declaration && site == &target {
                continue;
            }
            if let Some((uri, text)) = self.files.get(site.source.as_ref()) {
                let lines = lines
                    .entry(site.source.as_ref())
                    .or_insert_with(|| Lines::new(text));
                results.push(Location::new(uri.clone(), lines.range(text, site.span)));
            }
        }
        Ok(results)
    }
}

fn symbol_site(analysis: &Analysis, symbol: Symbol) -> Option<Site> {
    let (source, span) = analysis.definitions.get(&symbol)?;
    Some(Site {
        source: Arc::from(source.as_str()),
        span: *span,
    })
}

pub(crate) fn bindings(analysis: &Analysis, cancel: &Cancellation) -> Result<Bindings, String> {
    let sources: HashMap<_, Arc<str>> = analysis
        .files
        .keys()
        .map(|source| (source, Arc::from(source.as_str())))
        .collect();
    let declaration = |symbol| {
        let (source, span) = analysis.definitions.get(&symbol)?;
        Some(Site {
            source: sources.get(source)?.clone(),
            span: *span,
        })
    };
    let mut result = Bindings::new();
    for (source, file) in &analysis.files {
        cancel.check()?;
        let mut occurrences = BTreeMap::<Span, BTreeSet<Binding>>::new();
        for (span, symbol, alias) in analysis.rename_occurrences(file) {
            occurrences.entry(span).or_default().insert(Binding {
                declaration: declaration(symbol),
                alias: alias.and_then(declaration),
            });
        }
        result.insert(source.clone(), occurrences);
    }
    Ok(result)
}
