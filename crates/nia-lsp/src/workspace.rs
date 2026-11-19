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
use nia_lexer::TokenKind;
use nia_span::Span;
use nia_syntax::SyntaxKind;

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
    managed: BTreeSet<PathBuf>,
    references: OnceLock<HashMap<Site, BTreeSet<Site>>>,
}

enum EntryKind {
    Declared,
    BuildCandidate,
    Standalone,
}

fn declares_build_hook(text: &str) -> bool {
    let Ok(parsed) = nia_syntax::parse(text, None) else {
        return false;
    };
    parsed.tree.green_root().child_nodes().any(|item| {
        item.child(&SyntaxKind::FunctionDecl)
            .and_then(|function| function.token(&TokenKind::Ident))
            .is_some_and(|name| name.text() == "build")
    })
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
            managed: workspace.managed,
            references: OnceLock::new(),
        };
        for (request, kind) in workspace
            .entries
            .into_iter()
            .map(|request| (request, EntryKind::Declared))
            .chain(
                workspace
                    .build_candidates
                    .into_iter()
                    .map(|request| (request, EntryKind::BuildCandidate)),
            )
            .chain(
                workspace
                    .standalone
                    .into_iter()
                    .map(|request| (request, EntryKind::Standalone)),
            )
        {
            cancel.check()?;
            let path = request.check.entry_path.as_str();
            if let Some((_, text)) = index.files.get(path)
                && match kind {
                    EntryKind::Declared => false,
                    EntryKind::BuildCandidate => !declares_build_hook(text),
                    EntryKind::Standalone => true,
                }
            {
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
    pub(crate) fn contains_build_state(&self, path: &Path) -> bool {
        self.managed
            .iter()
            .any(|directory| directory.starts_with(path) && directory.exists())
    }

    pub(crate) fn editable_source(&self, path: &Path) -> bool {
        if self
            .managed
            .iter()
            .any(|directory| path.starts_with(directory))
        {
            return false;
        }
        let Some(root) = self
            .editable
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
        else {
            return false;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        !relative.starts_with("target")
            && path
                .ancestors()
                .skip(1)
                .take_while(|ancestor| *ancestor != root)
                .all(|ancestor| !ancestor.join("nia.toml").is_file())
    }

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
