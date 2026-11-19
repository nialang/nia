// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler bindings with source identities that survive separate entry analyses.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use lsp_types::{CallHierarchyItem, Location, Position, Range, SymbolInformation, SymbolKind, Uri};
use nia_compiler_query::ProgramDiagnostic;
use nia_lexer::TokenKind;
use nia_span::Span;
use nia_syntax::SyntaxKind;

use crate::{
    analysis::{Analysis, Documents, Session, Symbol, normalized},
    document::Lines,
    editing::CallableDeclaration,
    project::EditorRequest,
};

#[derive(Clone, Default)]
pub(crate) struct Cancellation(nia_query::CancellationToken);

impl Cancellation {
    pub(crate) fn cancel(&self) {
        self.0.cancel();
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        self.0.check().map_err(|error| error.to_string())
    }

    pub(crate) fn token(&self) -> &nia_query::CancellationToken {
        &self.0
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

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct CallHierarchyContext {
    pub(crate) anchor: Uri,
    pub(crate) revision: u64,
}

struct CallEdge {
    caller: Site,
    callee: Site,
    span: Span,
}

type CallRelations = HashMap<Site, BTreeMap<Site, BTreeSet<Span>>>;

#[derive(Default)]
struct CallIndex {
    incoming: CallRelations,
    outgoing: CallRelations,
}

pub(crate) struct Entry {
    pub(crate) request: EditorRequest,
    pub(crate) bindings: Bindings,
    pub(crate) coverage: Coverage,
    consulted_sources: Option<Arc<BTreeSet<String>>>,
    files: BTreeMap<String, (Uri, Arc<str>)>,
    symbols: Vec<(Site, WorkspaceDeclaration)>,
    implementations: Vec<(Site, Site)>,
    module_declarations: Vec<(String, Site)>,
    calls: Vec<CallEdge>,
}

pub(crate) enum Coverage {
    Complete,
    Partial(Vec<ProgramDiagnostic>),
    Unavailable(String),
}

impl Coverage {
    pub(crate) fn source_errors(&self) -> &[ProgramDiagnostic] {
        match self {
            Self::Partial(errors) => errors,
            _ => &[],
        }
    }

    fn problem(&self) -> Option<&str> {
        match self {
            Self::Complete => None,
            Self::Partial(errors) => errors.first().map(|item| item.diagnostic.summary.as_str()),
            Self::Unavailable(error) => Some(error),
        }
    }
}

pub(crate) struct WorkspaceIndex {
    pub(crate) entries: Vec<Arc<Entry>>,
    pub(crate) files: BTreeMap<String, (Uri, Arc<str>)>,
    pub(crate) editable: BTreeSet<PathBuf>,
    managed: BTreeSet<PathBuf>,
    pub(crate) unavailable_plans: Vec<(PathBuf, String)>,
    folders: Vec<PathBuf>,
    anchor: Option<String>,
    overlays: BTreeMap<String, Arc<str>>,
    symbols: BTreeMap<(String, Span), WorkspaceDeclaration>,
    implementations: BTreeMap<Site, BTreeSet<Site>>,
    pub(crate) module_declarations: BTreeMap<String, BTreeSet<Site>>,
    references: OnceLock<HashMap<Site, BTreeSet<Site>>>,
    calls: OnceLock<CallIndex>,
}

#[derive(Clone)]
struct WorkspaceDeclaration {
    name: String,
    kind: SymbolKind,
    container: String,
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
        self.index_workspace(Some(path), documents, folders, cancel)
    }

    pub(crate) fn index_workspace(
        &mut self,
        path: Option<&Path>,
        documents: &Documents,
        folders: &[PathBuf],
        cancel: &Cancellation,
    ) -> Result<Arc<WorkspaceIndex>, String> {
        cancel.check()?;
        self.install_documents(documents)?;
        let anchor = path.map(normalized);
        let overlays: BTreeMap<_, _> = documents
            .iter()
            .map(|(path, open)| (normalized(path), open.document.text.clone()))
            .collect();
        let previous = self.workspace_index.as_ref();
        if let Some(index) = previous
            && index.folders == folders
            && index.anchor == anchor
            && index.overlays == overlays
        {
            return Ok(index.clone());
        }
        let mut changed = BTreeSet::new();
        let mut previous_entries = HashMap::<_, Vec<_>>::new();
        if let Some(previous) = previous {
            for (path, old) in &previous.overlays {
                if overlays.get(path) != Some(old) {
                    changed.insert(path.as_str());
                }
            }
            changed.extend(overlays.keys().filter_map(|path| {
                (!previous.overlays.contains_key(path)).then_some(path.as_str())
            }));
            for entry in &previous.entries {
                previous_entries
                    .entry(entry.request.check.entry_path.as_str())
                    .or_default()
                    .push(entry);
            }
        }
        let workspace = self.projects.workspace_requests(
            path,
            documents.keys().cloned(),
            folders,
            &self.toolchain,
        )?;
        let mut index = WorkspaceIndex {
            entries: Vec::new(),
            files: BTreeMap::new(),
            editable: workspace.editable,
            managed: workspace.managed,
            unavailable_plans: workspace.unavailable_plans,
            folders: folders.to_vec(),
            anchor,
            overlays: BTreeMap::new(),
            symbols: BTreeMap::new(),
            implementations: BTreeMap::new(),
            module_declarations: BTreeMap::new(),
            references: OnceLock::new(),
            calls: OnceLock::new(),
        };
        let mut reused = 0;
        let mut analyzed = 0;
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
            let cached = previous_entries
                .get(path)
                .into_iter()
                .flatten()
                .find(|entry| {
                    entry.request == request
                        && entry.consulted_sources.as_ref().is_some_and(|sources| {
                            changed.iter().all(|path| {
                                !sources.contains(*path) && !entry.files.contains_key(*path)
                            })
                        })
                });
            let entry = if let Some(entry) = cached {
                reused += 1;
                Arc::clone(entry)
            } else {
                analyzed += 1;
                let analysis = self.analyze_request(request.clone(), cancel);
                cancel.check()?;
                Arc::new(Entry::new(request, analysis, cancel)?)
            };
            index.merge(&entry, &workspace.package_names, cancel)?;
            index.entries.push(entry);
        }
        cancel.check()?;
        if crate::timing_mode().enabled() {
            nia_timing::emit_counter("lsp.workspace_entries_reused", reused);
            nia_timing::emit_counter("lsp.workspace_entries_analyzed", analyzed);
        }
        index.overlays = overlays;
        let index = Arc::new(index);
        self.workspace_index = Some(index.clone());
        Ok(index)
    }
}

impl Entry {
    fn new(
        request: EditorRequest,
        analysis: Result<Analysis, String>,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        let analysis = match analysis {
            Ok(analysis) => analysis,
            Err(error) => {
                return Ok(Self {
                    request,
                    coverage: Coverage::Unavailable(error),
                    bindings: Bindings::new(),
                    consulted_sources: None,
                    files: BTreeMap::new(),
                    symbols: Vec::new(),
                    implementations: Vec::new(),
                    module_declarations: Vec::new(),
                    calls: Vec::new(),
                });
            }
        };
        let diagnostics: Vec<_> = analysis.errors().cloned().collect();
        let coverage = if let Some(error) = diagnostics
            .iter()
            .find(|item| item.diagnostic.category() == nia_diagnostic::DiagnosticCategory::Internal)
        {
            Coverage::Unavailable(error.diagnostic.summary.clone())
        } else if diagnostics.is_empty() {
            Coverage::Complete
        } else {
            Coverage::Partial(diagnostics)
        };
        let bindings = bindings(&analysis, cancel)?;
        let mut symbols = Vec::new();
        for (source, span, name, kind, container) in analysis.workspace_declarations() {
            cancel.check()?;
            symbols.push((
                Site {
                    source: Arc::from(source),
                    span,
                },
                WorkspaceDeclaration {
                    name,
                    kind,
                    container,
                },
            ));
        }
        let mut implementations = Vec::new();
        for (symbol, source, span) in analysis.workspace_implementations() {
            cancel.check()?;
            if let Some(target) = symbol_site(&analysis, symbol) {
                implementations.push((
                    target,
                    Site {
                        source: Arc::from(source),
                        span,
                    },
                ));
            }
        }
        let module_declarations = analysis
            .module_declarations()
            .map(|(module, source, span)| {
                (
                    module.to_owned(),
                    Site {
                        source: Arc::from(source),
                        span,
                    },
                )
            })
            .collect();
        let mut calls = Vec::new();
        for (caller, callee, span) in analysis.workspace_calls() {
            cancel.check()?;
            if let (Some(caller), Some(callee)) = (
                symbol_site(&analysis, caller),
                symbol_site(&analysis, callee),
            ) {
                calls.push(CallEdge {
                    caller,
                    callee,
                    span,
                });
            }
        }
        Ok(Self {
            request,
            coverage,
            bindings,
            consulted_sources: Some(analysis.consulted_sources),
            symbols,
            implementations,
            module_declarations,
            calls,
            files: analysis
                .files
                .into_iter()
                .map(|(source, file)| (source, (file.uri, file.text)))
                .collect(),
        })
    }

    pub(crate) fn depends_on(&self, source: &str) -> bool {
        self.files.contains_key(source)
            || self
                .consulted_sources
                .as_ref()
                .is_some_and(|paths| paths.contains(source))
    }

    pub(crate) fn observes_move(&self, from: &Path, to: &Path) -> bool {
        self.files
            .keys()
            .chain(self.consulted_sources.iter().flat_map(|paths| paths.iter()))
            .any(|source| {
                let source = Path::new(source);
                source.starts_with(from) || source.starts_with(to)
            })
    }
}

impl WorkspaceIndex {
    pub(crate) fn call_relations(
        &self,
        item: &CallHierarchyItem,
        context: &CallHierarchyContext,
        incoming: bool,
        cancel: &Cancellation,
    ) -> Result<Vec<(CallHierarchyItem, Vec<Range>)>, String> {
        let path = crate::analysis::path(&item.uri).ok_or("Invalid callable URI")?;
        let source = normalized(&path);
        let (_, text) = self
            .files
            .get(&source)
            .ok_or("Callable source is outside the indexed workspace")?;
        let lines = Lines::new(text);
        let range = item.selection_range;
        let start = lines
            .offset(text, range.start)
            .ok_or("Invalid callable range")?;
        let end = lines
            .offset(text, range.end)
            .ok_or("Invalid callable range")?;
        if text.get(start..end) != Some(&item.name) {
            return Err("Callable selection changed".into());
        }
        let target = Site {
            source: source.into(),
            span: Span::new(start, end),
        };
        let calls = if let Some(calls) = self.calls.get() {
            calls
        } else {
            let mut calls = CallIndex::default();
            for entry in &self.entries {
                for edge in &entry.calls {
                    cancel.check()?;
                    calls
                        .incoming
                        .entry(edge.callee.clone())
                        .or_default()
                        .entry(edge.caller.clone())
                        .or_default()
                        .insert(edge.span);
                    calls
                        .outgoing
                        .entry(edge.caller.clone())
                        .or_default()
                        .entry(edge.callee.clone())
                        .or_default()
                        .insert(edge.span);
                }
            }
            self.calls.get_or_init(|| calls)
        };
        let related = if incoming {
            &calls.incoming
        } else {
            &calls.outgoing
        };
        let mut snapshots = HashMap::new();
        let mut results = Vec::new();
        for (site, spans) in related.get(&target).into_iter().flatten() {
            cancel.check()?;
            let Some((uri, other_text)) = self.files.get(site.source.as_ref()) else {
                continue;
            };
            let (other_lines, parsed) =
                snapshots.entry(site.source.as_ref()).or_insert_with(|| {
                    (
                        Lines::new(other_text),
                        nia_syntax::parse(other_text, None).ok(),
                    )
                });
            let Some(other) = parsed
                .as_ref()
                .and_then(|parsed| {
                    CallableDeclaration::in_tree(parsed.tree.green_root(), other_text, site.span)
                })
                .and_then(|callable| callable.hierarchy_item(other_lines, uri, site.span, context))
            else {
                continue;
            };
            let (caller_lines, caller_text) = if incoming {
                (&*other_lines, other_text)
            } else {
                (&lines, text)
            };
            let ranges = spans
                .iter()
                .map(|&span| caller_lines.range(caller_text, span))
                .collect();
            results.push((other, ranges));
        }
        Ok(results)
    }

    fn merge(
        &mut self,
        entry: &Entry,
        package_names: &BTreeMap<PathBuf, String>,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        for (source, file) in &entry.files {
            cancel.check()?;
            match self.files.entry(source.clone()) {
                std::collections::btree_map::Entry::Occupied(current) => {
                    if current.get().1 != file.1 {
                        return Err(format!(
                            "Source changed during workspace analysis ({source})"
                        ));
                    }
                }
                std::collections::btree_map::Entry::Vacant(current) => {
                    current.insert(file.clone());
                }
            }
        }
        for (module, site) in &entry.module_declarations {
            self.module_declarations
                .entry(module.clone())
                .or_default()
                .insert(site.clone());
        }
        for (target, site) in &entry.implementations {
            self.implementations
                .entry(target.clone())
                .or_default()
                .insert(site.clone());
        }
        for (site, symbol) in &entry.symbols {
            cancel.check()?;
            let source = Path::new(site.source.as_ref());
            if !self.editable_source(source) {
                continue;
            }
            let key = (site.source.to_string(), site.span);
            self.symbols.entry(key).or_insert_with(|| {
                let package = source
                    .ancestors()
                    .find_map(|directory| package_names.get(directory));
                let mut symbol = symbol.clone();
                if let Some(package) = package {
                    symbol.container = if symbol.container.is_empty() {
                        package.clone()
                    } else {
                        format!("{package}::{}", symbol.container)
                    };
                }
                symbol
            });
        }
        Ok(())
    }

    pub(crate) fn implementations(
        &self,
        analysis: &Analysis,
        path: &Path,
        position: Position,
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
            return Err("Source changed during implementation lookup".into());
        }
        let Some(target) = analysis
            .reference_symbol(path, position)
            .and_then(|symbol| symbol_site(analysis, symbol))
        else {
            return Ok(Vec::new());
        };
        let mut lines = HashMap::new();
        let mut locations = Vec::new();
        for site in self.implementations.get(&target).into_iter().flatten() {
            cancel.check()?;
            if let Some((uri, text)) = self.files.get(site.source.as_ref()) {
                let lines = lines
                    .entry(site.source.as_ref())
                    .or_insert_with(|| Lines::new(text));
                locations.push(Location::new(uri.clone(), lines.range(text, site.span)));
            }
        }
        Ok(locations)
    }

    #[allow(deprecated)]
    pub(crate) fn symbols(
        &self,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<SymbolInformation>, String> {
        let query = query.trim().to_lowercase();
        let mut matches = Vec::new();
        for ((source, span), declaration) in &self.symbols {
            cancel.check()?;
            let name = declaration.name.to_lowercase();
            let candidate = if query.contains("::") {
                format!("{}::{name}", declaration.container.to_lowercase())
            } else {
                name
            };
            let Some(offset) = candidate.find(&query) else {
                continue;
            };
            matches.push((candidate != query, offset, &declaration.name, source, span));
        }
        matches.sort_unstable();
        let mut lines = HashMap::new();
        matches
            .into_iter()
            .map(|(_, _, _, source, span)| {
                cancel.check()?;
                let declaration = &self.symbols[&(source.clone(), *span)];
                let (uri, text) = &self.files[source];
                let lines = lines.entry(source).or_insert_with(|| Lines::new(text));
                Ok(SymbolInformation {
                    name: declaration.name.clone(),
                    kind: declaration.kind,
                    tags: None,
                    deprecated: None,
                    location: Location::new(uri.clone(), lines.range(text, *span)),
                    container_name: (!declaration.container.is_empty())
                        .then(|| declaration.container.clone()),
                })
            })
            .collect()
    }

    pub(crate) fn reference_coverage_warning(&self) -> Option<String> {
        let problem = if let Some((package, error)) = self.unavailable_plans.first() {
            format!("`{}`. {error}", package.display())
        } else {
            let (entry, error) = self
                .entries
                .iter()
                .find_map(|entry| Some((entry, entry.coverage.problem()?)))?;
            format!("{}. {error}", entry.request)
        };
        Some(format!(
            "Workspace references may be incomplete for {problem}"
        ))
    }

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

pub(crate) fn symbol_site(analysis: &Analysis, symbol: Symbol) -> Option<Site> {
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
