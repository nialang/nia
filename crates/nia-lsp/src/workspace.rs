// SPDX-License-Identifier: GPL-3.0-or-later
//! Compiler bindings with source identities that survive separate entry analyses.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use crate::structure::FlatSymbol;
use lsp_types::{CallHierarchyItem, Location, Position, Range, SymbolKind, Uri};
use nia_compiler_query::ProgramDiagnostic;
use nia_span::Span;

use crate::{
    analysis::{Analysis, Documents, Session, Symbol, normalized},
    document::Lines,
    editing::CallableDeclaration,
    project::{EditorRequest, Projects, WorkspaceRequests, declares_build_hook},
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
    references: OnceLock<HashMap<Site, Vec<Site>>>,
    pub(crate) coverage: Coverage,
    pub(crate) embedded_resources: std::collections::HashSet<nia_source::EmbeddedResource>,
    inactive: BTreeMap<String, Arc<[Span]>>,
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
    pub(crate) configuration: Arc<WorkspaceRequests>,
    toolchain_sources: PathBuf,
    symbol_packages: OnceLock<HashMap<String, Option<String>>>,
    implementations: BTreeMap<Site, BTreeSet<Site>>,
    pub(crate) module_declarations: BTreeMap<String, BTreeSet<Site>>,
    calls: OnceLock<CallIndex>,
}

pub(crate) struct WorkspaceSnapshot {
    state: WorkspaceIndexState,
    configuration: Arc<WorkspaceRequests>,
    folders: Vec<PathBuf>,
    anchor: Option<String>,
    overlays: BTreeMap<String, Arc<str>>,
}

enum WorkspaceIndexState {
    Complete(Arc<WorkspaceIndex>),
    Interrupted(Vec<Arc<Entry>>),
}

impl WorkspaceIndexState {
    fn index(&self) -> Option<&Arc<WorkspaceIndex>> {
        match self {
            Self::Complete(index) => Some(index),
            Self::Interrupted(_) => None,
        }
    }

    fn entries(&self) -> &[Arc<Entry>] {
        match self {
            Self::Complete(index) => &index.entries,
            Self::Interrupted(entries) => entries,
        }
    }
}

struct WorkspaceDeclaration {
    name: String,
    kind: SymbolKind,
    container: String,
}

impl WorkspaceDeclaration {
    fn container_name<'a>(&'a self, package: Option<&'a str>) -> Cow<'a, str> {
        match package {
            Some(package) if self.container.is_empty() => Cow::Borrowed(package),
            Some(package) => Cow::Owned(format!("{package}::{}", self.container)),
            None => Cow::Borrowed(&self.container),
        }
    }
}

enum EntryKind {
    Declared,
    BuildCandidate,
    Standalone,
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

    pub(crate) fn verify_workspace_configuration(
        &mut self,
        documents: &Documents,
        cancel: &Cancellation,
    ) -> Result<(), String> {
        cancel.check()?;
        let snapshot = self
            .workspace_index
            .as_ref()
            .ok_or("Workspace index is unavailable")?;
        let current = Projects::default().workspace_requests(
            snapshot.anchor.as_deref().map(Path::new),
            documents.keys().cloned(),
            &snapshot.folders,
            &self.toolchain,
            cancel,
        );
        cancel.check()?;
        let problem = match current {
            Ok(current) if current == *snapshot.configuration => return Ok(()),
            Ok(current) => current.unavailable_plans.first().map_or_else(
                || "Workspace build configuration changed during rename".to_owned(),
                |(package, error)| {
                    format!(
                        "Cannot verify rename in package `{}` ({error})",
                        package.display()
                    )
                },
            ),
            Err(error) => format!("Cannot verify workspace build configuration ({error})"),
        };
        self.invalidate_workspace_configuration();
        Err(problem)
    }

    pub(crate) fn index_workspace(
        &mut self,
        path: Option<&Path>,
        documents: &Documents,
        folders: &[PathBuf],
        cancel: &Cancellation,
    ) -> Result<Arc<WorkspaceIndex>, String> {
        self.index_workspace_with(path, documents, folders, cancel, Self::analyze_request)
    }

    fn index_workspace_with(
        &mut self,
        path: Option<&Path>,
        documents: &Documents,
        folders: &[PathBuf],
        cancel: &Cancellation,
        mut analyze: impl FnMut(&Self, EditorRequest, &Cancellation) -> Result<Analysis, String>,
    ) -> Result<Arc<WorkspaceIndex>, String> {
        cancel.check()?;
        self.install_documents(documents)?;
        // Open documents already contribute their packages and standalone entries.
        let path = path.filter(|path| !documents.contains_key(*path));
        let anchor = path.map(normalized);
        let overlays: BTreeMap<_, _> = documents
            .iter()
            .map(|(path, open)| (normalized(path), open.document.text.clone()))
            .collect();
        let previous = self.workspace_index.as_ref();
        if let Some(previous) = previous
            && let Some(index) = previous.state.index()
            && previous.folders == folders
            && previous.anchor == anchor
            && previous.overlays == overlays
            && self.changed_disk_sources.is_empty()
        {
            return Ok(index.clone());
        }
        let mut changed: BTreeSet<_> = self
            .changed_disk_sources
            .iter()
            .map(String::as_str)
            .collect();
        let mut previous_entries = HashMap::<_, Vec<_>>::new();
        if let Some(previous) = previous {
            for (path, old) in &previous.overlays {
                let unchanged = overlays.get(path).map_or_else(
                    || {
                        nia_source::read_source_text(Path::new(path))
                            .is_ok_and(|disk| disk == **old)
                    },
                    |open| open == old,
                );
                if !unchanged {
                    changed.insert(path.as_str());
                }
            }
            for (path, text) in &overlays {
                if !previous.overlays.contains_key(path)
                    && !previous
                        .state
                        .entries()
                        .iter()
                        .find_map(|entry| entry.files.get(path))
                        .is_some_and(|(_, indexed)| indexed == text)
                {
                    changed.insert(path.as_str());
                }
            }
            for entry in previous.state.entries() {
                previous_entries
                    .entry(entry.request.check.entry_path.as_str())
                    .or_default()
                    .push(entry);
            }
        }
        let workspace = Arc::new(self.projects.workspace_requests(
            path,
            documents.keys().cloned(),
            folders,
            &self.toolchain,
            cancel,
        )?);
        let mut entries: Vec<Arc<Entry>> = Vec::new();
        let mut reused = 0;
        let mut analyzed = 0;
        let mut requests = workspace
            .entries
            .iter()
            .map(|request| (request, EntryKind::Declared))
            .chain(
                workspace
                    .build_candidates
                    .iter()
                    .map(|request| (request, EntryKind::BuildCandidate)),
            )
            .chain(
                workspace
                    .standalone
                    .iter()
                    .map(|request| (request, EntryKind::Standalone)),
            );
        let reusable = |request: &EditorRequest| {
            previous_entries
                .get(request.check.entry_path.as_str())
                .into_iter()
                .flatten()
                .copied()
                .find(|entry| {
                    entry.request == *request
                        && entry.consulted_sources.is_some()
                        && changed.iter().all(|path| !entry.depends_on(path))
                })
        };
        let collected = (|| {
            loop {
                cancel.check()?;
                let Some((request, kind)) = requests.next() else {
                    break;
                };
                let path = request.check.entry_path.as_str();
                if !matches!(kind, EntryKind::Declared)
                    && let Some((_, text)) = entries.iter().find_map(|entry| entry.files.get(path))
                    && (matches!(kind, EntryKind::Standalone) || !declares_build_hook(text))
                {
                    continue;
                }
                let entry = if let Some(entry) = reusable(request) {
                    reused += 1;
                    Arc::clone(entry)
                } else {
                    analyzed += 1;
                    let analysis = analyze(self, request.clone(), cancel);
                    cancel.check()?;
                    Arc::new(Entry::new(request.clone(), analysis, cancel)?)
                };
                entries.push(entry);
            }
            Ok(())
        })();
        if crate::timing_mode().enabled() {
            nia_timing::emit_counter("lsp.workspace_entries_reused", reused);
            nia_timing::emit_counter("lsp.workspace_entries_analyzed", analyzed);
        }
        let remaining = collected.is_err().then(|| {
            let mut retained: HashSet<_> = entries.iter().map(Arc::as_ptr).collect();
            requests
                .filter_map(|(request, _)| {
                    reusable(request)
                        .filter(|entry| retained.insert(Arc::as_ptr(entry)))
                        .cloned()
                })
                .collect::<Vec<_>>()
        });
        let merged = collected.and_then(|()| {
            if let Some(index) = previous.and_then(|previous| previous.state.index())
                && index.configuration.editable == workspace.editable
                && index.configuration.managed == workspace.managed
                && index.configuration.unavailable_plans == workspace.unavailable_plans
                && index.configuration.package_errors == workspace.package_errors
                && index.configuration.incomplete_inputs == workspace.incomplete_inputs
                && index.configuration.package_names == workspace.package_names
                && index.configuration.declared_entries == workspace.declared_entries
                && index.entries.len() == entries.len()
                && index
                    .entries
                    .iter()
                    .zip(&entries)
                    .all(|(old, new)| Arc::ptr_eq(old, new))
            {
                if crate::timing_mode().enabled() {
                    nia_timing::emit_counter("lsp.workspace_index_reused", 1);
                }
                cancel.check()?;
                return Ok(index.clone());
            }
            let mut index = WorkspaceIndex {
                entries: Vec::new(),
                files: BTreeMap::new(),
                configuration: workspace.clone(),
                toolchain_sources: PathBuf::from(normalized(self.toolchain.resource_root())),
                symbol_packages: OnceLock::new(),
                implementations: BTreeMap::new(),
                module_declarations: BTreeMap::new(),
                calls: OnceLock::new(),
            };
            for entry in &entries {
                index.merge(entry, cancel)?;
            }
            cancel.check()?;
            index.entries = std::mem::take(&mut entries);
            Ok(Arc::new(index))
        });
        let state = match &merged {
            Ok(index) => WorkspaceIndexState::Complete(index.clone()),
            Err(_) if cancel.check().is_err() => {
                // Keep current entries that the canceled scan had yet to reach.
                // Only retain requests from this scan's workspace plan.
                entries.extend(remaining.into_iter().flatten());
                if entries.is_empty() {
                    return merged;
                }
                WorkspaceIndexState::Interrupted(entries)
            }
            Err(_) => return merged,
        };
        self.workspace_index = Some(WorkspaceSnapshot {
            state,
            configuration: workspace,
            folders: folders.to_vec(),
            anchor,
            overlays,
        });
        self.changed_disk_sources.clear();
        merged
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
                    embedded_resources: Default::default(),
                    inactive: BTreeMap::new(),
                    bindings: Bindings::new(),
                    references: OnceLock::new(),
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
            references: OnceLock::new(),
            inactive: analysis
                .files
                .iter()
                .filter_map(|(source, file)| Some((source.clone(), file.inactive.clone()?)))
                .collect(),
            consulted_sources: Some(analysis.consulted_sources),
            embedded_resources: analysis.embedded_resources,
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

    fn reference_sites(&self, target: &Site, cancel: &Cancellation) -> Result<&[Site], String> {
        let references = if let Some(references) = self.references.get() {
            references
        } else {
            let mut references = HashMap::<Site, Vec<Site>>::new();
            for (source, occurrences) in &self.bindings {
                cancel.check()?;
                let source: Arc<str> = Arc::from(source.as_str());
                for (&span, candidates) in occurrences {
                    let site = Site {
                        source: source.clone(),
                        span,
                    };
                    for binding in candidates {
                        for target in binding.declaration.iter().chain(&binding.alias) {
                            let sites = references.entry(target.clone()).or_default();
                            // Bindings visit sites in source order. The same
                            // site can reach a target through several bindings.
                            if sites.last() != Some(&site) {
                                sites.push(site.clone());
                            }
                        }
                    }
                }
            }
            cancel.check()?;
            if crate::timing_mode().enabled() {
                nia_timing::emit_counter("lsp.workspace_reference_entries_indexed", 1);
            }
            self.references.get_or_init(|| references)
        };
        Ok(references.get(target).map_or(&[], Vec::as_slice))
    }

    pub(crate) fn depends_on(&self, source: &str) -> bool {
        self.files.contains_key(source)
            || self
                .consulted_sources
                .as_ref()
                .is_some_and(|paths| paths.contains(source))
    }

    pub(crate) fn inactive_at(&self, source: &str, span: Span) -> bool {
        self.inactive
            .get(source)
            .is_some_and(|ranges| inactive_at(ranges, span))
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
    pub(crate) fn entry_may_reach(&self, entry: &Entry, source: &str) -> bool {
        let path = Path::new(source);
        if entry.depends_on(source) || path.starts_with(&self.toolchain_sources) {
            return true;
        }
        // A damaged entry may omit an import from the observed graph. Its
        // explicit roots bound which source trees could have been reached.
        std::iter::once(&entry.request.check.entry_path)
            .chain(
                entry
                    .request
                    .check
                    .module_map
                    .all_entries()
                    .map(|(_, root)| root),
            )
            .any(|root| {
                Path::new(root.as_str())
                    .parent()
                    .is_some_and(|directory| path.starts_with(directory))
            })
    }

    pub(crate) fn has_resolved_binding(&self, source: &str, span: Span) -> bool {
        self.entries.iter().any(|entry| {
            entry
                .bindings
                .get(source)
                .and_then(|bindings| bindings.get(&span))
                .is_some_and(|bindings| {
                    bindings
                        .iter()
                        .any(|binding| binding.declaration.is_some() || binding.alias.is_some())
                })
        })
    }

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

    fn merge(&mut self, entry: &Entry, cancel: &Cancellation) -> Result<(), String> {
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
        Ok(())
    }

    fn symbol_packages(
        &self,
        cancel: &Cancellation,
    ) -> Result<&HashMap<String, Option<String>>, String> {
        if let Some(packages) = self.symbol_packages.get() {
            return Ok(packages);
        }
        let mut packages = HashMap::new();
        for source in self.files.keys() {
            cancel.check()?;
            let path = Path::new(source);
            if self.editable_source(path) {
                let package = path
                    .ancestors()
                    .find_map(|directory| self.configuration.package_names.get(directory));
                packages.insert(source.clone(), package.cloned());
            }
        }
        cancel.check()?;
        Ok(self.symbol_packages.get_or_init(|| packages))
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

    pub(crate) fn symbols(
        &self,
        query: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<FlatSymbol>, String> {
        let query = query.trim().to_lowercase();
        let qualified = query.contains("::");
        let packages = self.symbol_packages(cancel)?;
        let mut matches = Vec::new();
        let mut seen = HashSet::new();
        for (site, declaration) in self.entries.iter().flat_map(|entry| &entry.symbols) {
            cancel.check()?;
            let Some(package) = packages.get(site.source.as_ref()) else {
                continue;
            };
            // The name is fixed by the source range. A qualified search also
            // uses the first entry's module path, which can differ by entry.
            if qualified && !seen.insert(site) {
                continue;
            }
            let name = declaration.name.to_lowercase();
            let candidate = if qualified {
                format!(
                    "{}::{name}",
                    declaration
                        .container_name(package.as_deref())
                        .to_lowercase()
                )
            } else {
                name
            };
            if let Some(offset) = candidate.find(&query)
                && (qualified || seen.insert(site))
            {
                matches.push((
                    candidate != query,
                    offset,
                    &declaration.name,
                    site,
                    declaration,
                    package,
                ));
            }
        }
        matches.sort_unstable_by_key(|&(mismatch, offset, name, site, _, _)| {
            (mismatch, offset, name, site)
        });
        let mut lines = HashMap::new();
        matches
            .into_iter()
            .map(|(_, _, _, site, declaration, package)| {
                cancel.check()?;
                let source = site.source.as_ref();
                let (uri, text) = &self.files[source];
                let lines = lines.entry(source).or_insert_with(|| Lines::new(text));
                let container = declaration.container_name(package.as_deref());
                Ok(FlatSymbol {
                    name: declaration.name.clone(),
                    kind: declaration.kind,
                    location: Location::new(uri.clone(), lines.range(text, site.span)),
                    container_name: (!container.is_empty()).then(|| container.into_owned()),
                })
            })
            .collect()
    }

    pub(crate) fn reference_coverage_warning(&self, source: &str) -> Option<String> {
        let problem =
            if let Some((package, error)) = self.configuration.package_errors.first_key_value() {
                format!("{}: {error}", package.display())
            } else if let Some((package, error)) = self.configuration_problem_for(source) {
                format!("`{}`. {error}", package.display())
            } else {
                let (entry, error) = self
                    .entries
                    .iter()
                    .filter(|entry| self.entry_may_reach(entry, source))
                    .find_map(|entry| Some((entry, entry.coverage.problem()?)))?;
                format!("{}. {error}", entry.request)
            };
        Some(format!(
            "Workspace references may be incomplete for {problem}"
        ))
    }

    pub(crate) fn configuration_problem_for(&self, source: &str) -> Option<(&PathBuf, &String)> {
        let path = Path::new(source);
        self.configuration
            .package_errors
            .iter()
            .chain(
                self.configuration
                    .unavailable_plans
                    .iter()
                    .map(|(root, error)| (root, error)),
            )
            .find(|(root, _)| {
                path.starts_with(root)
                    || self
                        .configuration
                        .incomplete_inputs
                        .get(*root)
                        .is_some_and(|inputs| inputs.iter().any(|input| path.starts_with(input)))
                    || self.entries.iter().any(|entry| {
                        Path::new(entry.request.check.entry_path.as_str()).starts_with(root)
                            && self.entry_may_reach(entry, source)
                    })
            })
    }

    pub(crate) fn contains_build_state(&self, path: &Path) -> bool {
        self.configuration
            .managed
            .iter()
            .any(|directory| directory.starts_with(path) && directory.exists())
    }

    pub(crate) fn editable_source(&self, path: &Path) -> bool {
        if self.managed_source(path) {
            return false;
        }
        let Some(root) = self
            .configuration
            .editable
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
        else {
            return false;
        };
        path.ancestors()
            .skip(1)
            .take_while(|ancestor| *ancestor != root)
            .all(|ancestor| !ancestor.join("nia.toml").is_file())
    }

    pub(crate) fn managed_source(&self, path: &Path) -> bool {
        self.configuration
            .managed
            .iter()
            .any(|directory| path.starts_with(directory))
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
        let mut sites: BTreeSet<&Site> = BTreeSet::new();
        for entry in &self.entries {
            cancel.check()?;
            sites.extend(entry.reference_sites(&target, cancel)?);
        }
        let mut results = Vec::new();
        let mut lines = HashMap::new();
        for site in sites {
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
            if inactive_at(file.inactive.as_deref().unwrap_or_default(), span) {
                continue;
            }
            occurrences.entry(span).or_default().insert(Binding {
                declaration: declaration(symbol),
                alias: alias.and_then(declaration),
            });
        }
        result.insert(source.clone(), occurrences);
    }
    Ok(result)
}

fn inactive_at(ranges: &[Span], span: Span) -> bool {
    // The compiler prunes a subtree before visiting its children.
    let index = ranges.partition_point(|range| range.start <= span.start);
    index > 0 && span.end <= ranges[index - 1].end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{OpenDocument, uri};
    use crate::document::Document;
    use nia_test_support::{TestWorkload, acquire_test_resources, test_dir, workspace_root};
    use nia_toolchain::{ToolchainLayout, ToolchainLayoutRequest};

    #[test]
    fn canceled_workspace_scan_reuses_current_completed_entries() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let toolchain = ToolchainLayout::resolve(ToolchainLayoutRequest::explicit(
            std::env::current_exe().unwrap(),
            workspace_root().join("lib"),
        ))
        .unwrap();
        let root = test_dir("lsp-canceled-workspace-scan");
        let mut documents: Documents = [
            ("first.nia", "firstName"),
            ("second.nia", "secondName"),
            ("third.nia", "thirdName"),
        ]
        .into_iter()
        .map(|(filename, name)| {
            let path = root.join(filename);
            let text = format!("pub fun {name}() i32 {{ 42 }}\n");
            std::fs::write(&path, &text).unwrap();
            let uri = uri(&path).unwrap();
            (
                path,
                OpenDocument {
                    uri,
                    document: Document::new(1, text),
                },
            )
        })
        .collect();
        let mut session = Session::new(Arc::new(toolchain));
        for edit_before_retry in [false, true] {
            session.reload();
            let cancel = Cancellation::default();
            let mut analyzed = 0;
            let interrupted = session.index_workspace_with(
                None,
                &documents,
                &[],
                &cancel,
                |session, request, cancel| {
                    analyzed += 1;
                    if analyzed == 2 {
                        cancel.cancel();
                    }
                    session.analyze_request(request, cancel)
                },
            );
            assert!(interrupted.is_err());
            let WorkspaceIndexState::Interrupted(entries) =
                &session.workspace_index.as_ref().unwrap().state
            else {
                panic!("canceled scan must retain completed entries");
            };
            let [first] = entries.as_slice() else {
                panic!("only the first entry completed");
            };
            let first = first.clone();
            if edit_before_retry {
                documents.get_mut(&root.join("first.nia")).unwrap().document =
                    Document::new(2, "pub fun editedName() i32 { 42 }\n".into());
            }
            let retry = Cancellation::default();
            let index = session
                .index_workspace(None, &documents, &[], &retry)
                .unwrap();
            assert_eq!(index.entries.len(), 3);
            assert_eq!(Arc::ptr_eq(&first, &index.entries[0]), !edit_before_retry);
            let expected = if edit_before_retry {
                "editedName"
            } else {
                "firstName"
            };
            assert_eq!(index.symbols(expected, &retry).unwrap().len(), 1);
            assert_eq!(index.symbols("secondName", &retry).unwrap().len(), 1);
        }

        let cancel = Cancellation::default();
        let original = session
            .index_workspace(None, &documents, &[], &cancel)
            .unwrap();
        documents
            .get_mut(&root.join("second.nia"))
            .unwrap()
            .document = Document::new(2, "pub fun updatedSecondName() i32 { 42 }\n".into());
        let interrupted =
            session.index_workspace_with(None, &documents, &[], &cancel, |_, request, cancel| {
                assert_eq!(
                    request.check.entry_path.as_str(),
                    normalized(&root.join("second.nia"))
                );
                cancel.cancel();
                cancel.check()?;
                unreachable!("the scan was canceled")
            });
        assert!(interrupted.is_err());
        let retry = Cancellation::default();
        let index = session
            .index_workspace(None, &documents, &[], &retry)
            .unwrap();
        assert!(Arc::ptr_eq(&original.entries[0], &index.entries[0]));
        assert!(!Arc::ptr_eq(&original.entries[1], &index.entries[1]));
        assert!(Arc::ptr_eq(&original.entries[2], &index.entries[2]));
        assert_eq!(index.symbols("updatedSecondName", &retry).unwrap().len(), 1);
    }
}
