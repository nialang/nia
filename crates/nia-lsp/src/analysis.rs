// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use lsp_types::{Diagnostic, DiagnosticSeverity, Location, NumberOrString, Position, Uri};
use nia_compiler_query::{CheckedProgramAnalysis, CompilerDatabase};
use nia_defs::{DefCollection, DefKind, ModuleUsingScope};
use nia_diagnostic::LabelStyle;
use nia_driver::{CheckRequest, Driver};
use nia_ids::{GlobalDefId, LocalId, ModuleId};
use nia_imports::{ModuleGraphSnapshot, ModuleMap, ModuleRootSegment};
use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_package_metadata::{ArtifactKind, PackageManifest};
use nia_sema_ir::{ResolvedCall, SemanticValueUse};
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_target_config::CompilationMode;
use nia_toolchain::ToolchainLayout;
use nia_type_resolve::TypeNameResolution;
use url::Url;

use crate::document::{Document, Lines};
use crate::highlight::{Class, Kind, Resolved};

pub(crate) type Documents = BTreeMap<PathBuf, OpenDocument>;

#[derive(Clone)]
pub(crate) struct OpenDocument {
    pub(crate) uri: Uri,
    pub(crate) document: Document,
}

pub(crate) fn path(uri: &Uri) -> Option<PathBuf> {
    Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

fn uri(path: &Path) -> Option<Uri> {
    Url::from_file_path(path).ok()?.as_str().parse().ok()
}

fn normalized(path: &Path) -> String {
    nia_source::normalize_path(&path.to_string_lossy())
}

pub(crate) struct Session {
    toolchain: Arc<ToolchainLayout>,
    driver: Driver,
    overlays: BTreeMap<PathBuf, Arc<str>>,
}

impl Session {
    pub(crate) fn new(toolchain: Arc<ToolchainLayout>) -> Self {
        Self {
            driver: Driver::new(toolchain.clone()),
            toolchain,
            overlays: BTreeMap::new(),
        }
    }

    pub(crate) fn analyze(
        &mut self,
        documents: &Documents,
        entry: &Path,
    ) -> Result<Analysis, String> {
        // A fresh source database lets closed files return to disk ownership.
        if self
            .overlays
            .keys()
            .any(|path| !documents.contains_key(path))
        {
            self.driver = Driver::new(self.toolchain.clone());
            self.overlays.clear();
        }
        for (path, open) in documents {
            let text = &open.document.text;
            if self.overlays.get(path).is_some_and(|old| old == text) {
                continue;
            }
            self.driver
                .set_source(normalized(path), text.clone())
                .map_err(|error| match error {
                    nia_driver::DriverError::InternalDiagnostic(diagnostic) => diagnostic.summary,
                    other => format!("{other:?}"),
                })?;
            self.overlays.insert(path.clone(), text.clone());
        }
        let (request, fallback) = request_for(entry)?;
        let analyze = |request| {
            self.driver
                .analyze_for_editor(request, |db, program| {
                    Analysis::from_program(&self.driver, db, program)
                })
                .map_err(|error| error.to_string())
        };
        let analysis = analyze(request)?;
        if let Some(fallback) = fallback
            && !analysis.files.contains_key(&normalized(entry))
        {
            return analyze(fallback);
        }
        Ok(analysis)
    }
}

fn request_for(entry: &Path) -> Result<(CheckRequest, Option<CheckRequest>), String> {
    let Some(directory) = entry
        .ancestors()
        .skip(1)
        .find(|directory| directory.join("nia.toml").is_file())
    else {
        return Ok((CheckRequest::new(normalized(entry)), None));
    };
    let manifest = PackageManifest::read(directory).map_err(|error| error.to_string())?;
    let library = manifest
        .library_path
        .as_deref()
        .map(|path| directory.join(path));
    let artifact = manifest
        .artifacts
        .iter()
        .find(|artifact| directory.join(&artifact.path) == entry);
    let mode = if artifact.is_some_and(|artifact| artifact.kind == ArtifactKind::Test) {
        CompilationMode::Test
    } else {
        CompilationMode::Normal
    };
    let mut modules = ModuleMap::new();
    if let Some(library) = &library
        && library != entry
    {
        modules.insert(
            manifest.package.name.clone(),
            SourcePath::new(normalized(library)),
        )?;
    }
    let request = |path| {
        CheckRequest::new(normalized(path))
            .with_compilation_mode(mode)
            .with_module_map(modules.clone())
    };
    let primary = library.as_deref().filter(|library| {
        artifact.is_none() && *library != entry && entry != directory.join("build.nia")
    });
    match primary {
        Some(library) => {
            let fallback = request(entry);
            Ok((
                request(library).with_module_map(ModuleMap::new()),
                Some(fallback),
            ))
        }
        None => Ok((request(entry), None)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Symbol {
    Global(GlobalDefId),
    Local(ModuleId, LocalId),
}

struct Occurrence {
    span: Span,
    symbol: Symbol,
    declaration: bool,
}

pub(crate) struct File {
    module: ModuleId,
    uri: Uri,
    pub(crate) text: Arc<str>,
    pub(crate) lines: Lines,
    occurrences: Vec<Occurrence>,
    locals: HashMap<LocalId, (Kind, bool)>,
    type_highlights: Resolved,
}

pub(crate) struct Analysis {
    files: BTreeMap<String, File>,
    definitions: HashMap<Symbol, (String, Span)>,
    diagnostics: Vec<nia_compiler_query::ProgramDiagnostic>,
    db: CompilerDatabase,
    graph: ModuleGraphSnapshot,
    defs: HashMap<ModuleId, Arc<DefCollection>>,
}

impl Analysis {
    fn from_program(
        driver: &Driver,
        db: &CompilerDatabase,
        program: CheckedProgramAnalysis,
    ) -> nia_query::QueryResult<Self> {
        let mut result = Self {
            files: BTreeMap::new(),
            definitions: HashMap::new(),
            diagnostics: program.diagnostics,
            db: db.clone(),
            graph: program.graph.clone(),
            defs: program
                .modules
                .iter()
                .map(|module| (module.id, module.defs.clone()))
                .collect(),
        };
        let modules: HashMap<_, _> = program
            .modules
            .iter()
            .map(|module| (module.id, module))
            .collect();
        for node in program.graph.modules() {
            let path = node.path.as_str();
            let Some(file) = driver.sources().source_for_id(node.source_id) else {
                continue;
            };
            let Some(file_uri) = uri(Path::new(path)) else {
                continue;
            };
            let module = modules.get(&node.id).copied();
            let defs = match module {
                Some(module) => module.defs.clone(),
                None => db.source_definitions(node.id)?,
            };
            result.defs.insert(node.id, defs.clone());
            let tokens = tokenize_lossless(&file.text);
            let names: Vec<_> = tokens
                .iter()
                .filter(|token| {
                    matches!(
                        token.kind,
                        LosslessTokenKind::Token(TokenKind::Ident | TokenKind::SelfValue)
                    )
                })
                .map(|token| token.span)
                .collect();
            let tokens_in = |span: Span| {
                let start = names.partition_point(|name| name.start < span.start);
                let end = names
                    .partition_point(|name| name.end <= span.end)
                    .max(start);
                &names[start..end]
            };
            let name_matches = |span: &Span, name| {
                SymbolId::from_stable_hash(stable_hash(&file.text[span.start..span.end])) == name
            };
            let mut occurrences = Vec::new();
            let mut locals = HashMap::new();
            let mut type_highlights = Resolved::new();
            let mut declare = |symbol, span| {
                result.definitions.insert(symbol, (path.to_owned(), span));
                occurrences.push(Occurrence {
                    span,
                    symbol,
                    declaration: true,
                });
            };
            for (id, def) in defs.defs.iter() {
                if let Some(&span) = tokens_in(def.span)
                    .iter()
                    .find(|span| name_matches(span, def.name))
                {
                    declare(
                        Symbol::Global(GlobalDefId {
                            module_id: node.id,
                            def_id: id,
                        }),
                        span,
                    );
                }
            }
            if let Some(module) = module {
                for (id, local) in module.local_resolution.locals.iter() {
                    use nia_local_resolve::LocalKind;
                    let kind = match local.kind {
                        LocalKind::Param => Kind::Parameter,
                        _ => Kind::Variable,
                    };
                    locals.insert(id, (kind, local.kind == LocalKind::ConstBinding));
                    let names = tokens_in(local.span);
                    let name = if let Some(name) = local.name.symbol() {
                        names.iter().find(|span| name_matches(span, name))
                    } else if local.name.is_self_value() {
                        names
                            .iter()
                            .find(|span| &file.text[span.start..span.end] == "self")
                    } else {
                        None
                    };
                    if let Some(&span) = name {
                        declare(Symbol::Local(module.id, id), span);
                    }
                }
                let call_targets: HashMap<_, _> = module
                    .semantic_facts
                    .iter_node_resolved_calls()
                    .filter_map(|(key, call)| match call {
                        ResolvedCall::Method { def_id, .. } => Some((key, *def_id)),
                        ResolvedCall::TraitMethod { method_id, .. }
                        | ResolvedCall::DynamicTraitMethod { method_id, .. } => {
                            Some((key, *method_id))
                        }
                        _ => None,
                    })
                    .collect();
                for (span, key) in db.source_origins(module.id)?.entries() {
                    let type_class = match module.type_resolution.node_type_names.get(key.site()) {
                        Some(TypeNameResolution::GenericParam) => {
                            Some(Class::new(Kind::TypeParameter))
                        }
                        Some(TypeNameResolution::BuiltinTrait(_)) => {
                            Some(Class::builtin(Kind::Interface))
                        }
                        Some(TypeNameResolution::Def(id)) => {
                            defs.defs.get(*id).map(|def| Class::new(def.kind.into()))
                        }
                        Some(TypeNameResolution::External(id)) => modules
                            .get(&id.module_id)
                            .and_then(|owner| owner.defs.defs.get(id.def_id))
                            .map(|def| Class::new(def.kind.into())),
                        _ => None,
                    };
                    if let Some(class) = type_class
                        && let Some(name) = type_name_span(&tokens, span)
                    {
                        type_highlights.insert(name, class);
                    }
                    let names = tokens_in(span);
                    let target = match module.semantic_uses.node_value_use(&key) {
                        Some(SemanticValueUse::Local(id)) => Some(Symbol::Local(module.id, id)),
                        Some(SemanticValueUse::Global(id)) => Some(Symbol::Global(id)),
                        None => module
                            .semantic_uses
                            .node_type_prefix(&key)
                            .map(Symbol::Global),
                    };
                    if let Some(symbol) = target
                        && let Some(&span) = names.last()
                    {
                        occurrences.push(Occurrence {
                            span,
                            symbol,
                            declaration: false,
                        });
                    }
                    if let Some(id) = call_targets.get(&key)
                        && let Some(def) = modules
                            .get(&id.module_id)
                            .and_then(|module| module.defs.defs.get(id.def_id))
                        && let Some(&span) = names.iter().find(|span| name_matches(span, def.name))
                    {
                        occurrences.push(Occurrence {
                            span,
                            symbol: Symbol::Global(*id),
                            declaration: false,
                        });
                    }
                }
            }
            occurrences.sort_by_key(|occurrence| (occurrence.span, !occurrence.declaration));
            result.files.insert(
                path.to_owned(),
                File {
                    module: node.id,
                    uri: file_uri,
                    lines: Lines::new(&file.text),
                    text: file.text,
                    occurrences,
                    locals,
                    type_highlights,
                },
            );
        }
        Ok(result)
    }

    fn symbol_at(&self, path: &Path, position: Position) -> Option<Symbol> {
        let file = self.file(path)?;
        let offset = file.lines.offset(&file.text, position)?;
        self.symbol_at_offset(file, offset)
    }

    pub(crate) fn file(&self, path: &Path) -> Option<&File> {
        self.files.get(&normalized(path))
    }

    pub(crate) fn semantic_highlights(&self, path: &Path) -> Resolved {
        let Some(file) = self.file(path) else {
            return Resolved::new();
        };
        let mut resolved = file.type_highlights.clone();
        let mut previous = None;
        resolved.reserve(file.occurrences.len());
        for item in &file.occurrences {
            if previous == Some(item.span) {
                continue;
            }
            let kind = match item.symbol {
                Symbol::Local(_, id) => file.locals.get(&id).copied(),
                Symbol::Global(id) => self
                    .defs
                    .get(&id.module_id)
                    .and_then(|defs| defs.defs.get(id.def_id))
                    .map(|def| (Kind::from(def.kind), def.kind == DefKind::Const)),
            };
            if let Some((kind, readonly)) = kind {
                resolved.insert(item.span, Class::symbol(kind, item.declaration, readonly));
                previous = Some(item.span);
            }
        }
        resolved
    }

    pub(crate) fn symbol_at_offset(&self, file: &File, offset: usize) -> Option<Symbol> {
        let end = file
            .occurrences
            .partition_point(|item| item.span.start <= offset);
        file.occurrences[..end]
            .iter()
            .rev()
            .take_while(|item| item.span.end > offset)
            .filter(|item| item.span.start <= offset)
            .min_by_key(|item| (item.span.len(), !item.declaration))
            .map(|item| item.symbol)
    }

    pub(crate) fn declaration_source(&self, symbol: Symbol) -> Option<(&str, Span)> {
        let (source, span) = self.definitions.get(&symbol)?;
        Some((&self.files.get(source)?.text, *span))
    }

    pub(crate) fn names(
        &self,
        file: &File,
        offset: usize,
        qualifier: &[&str],
    ) -> nia_query::QueryResult<Vec<VisibleName>> {
        let module = file.module;
        let imports = self.db.source_imports(module, offset)?;
        if !qualifier.is_empty() {
            let Some(target) = self.resolve_namespace(module, &imports, qualifier)? else {
                return Ok(Vec::new());
            };
            return self.namespace_names(module, target);
        }
        let mut names = self.direct_names(module, module);
        for (name, item) in imports.values.iter().chain(&imports.types) {
            if let Some(name) = self.db.source_symbol_text(*name) {
                names.push(self.item_name(name, item.target_module, item.target_def_id));
            }
        }
        for name in imports.modules.keys().chain(
            self.graph
                .get(module)
                .into_iter()
                .flat_map(|node| node.children.keys()),
        ) {
            if let Some(name) = self.db.source_symbol_text(*name) {
                names.push(VisibleName {
                    name,
                    kind: Some(DefKind::Module),
                    symbol: None,
                });
            }
        }
        Ok(names)
    }

    fn resolve_namespace(
        &self,
        current: ModuleId,
        imports: &ModuleUsingScope,
        qualifier: &[&str],
    ) -> nia_query::QueryResult<Option<ModuleId>> {
        let root = match qualifier[0] {
            "self" => ModuleRootSegment::Current,
            "root" => ModuleRootSegment::Root,
            "super" => ModuleRootSegment::Parent,
            name => ModuleRootSegment::Named(SymbolId::from_stable_hash(stable_hash(name))),
        };
        let initial = match root {
            ModuleRootSegment::Named(name) => imports
                .lookup_module(&name)
                .or_else(|| self.graph.root_module_for_segment(current, root)),
            _ => self.graph.root_module_for_segment(current, root),
        };
        let Some(mut target) = initial else {
            return Ok(None);
        };
        for name in &qualifier[1..] {
            let id = SymbolId::from_stable_hash(stable_hash(name));
            let child = self.visible_child(current, target, id);
            let exported = if child.is_none() {
                self.db.source_exports(target)?.and_then(|surface| {
                    surface.lookup_module(&id).filter(|_| {
                        surface.module_scopes.get(&id).is_none_or(|scope| {
                            scope.allows(&|restriction| {
                                nia_imports::access_restriction_allows(
                                    restriction,
                                    &self.graph,
                                    current,
                                )
                            })
                        })
                    })
                })
            } else {
                None
            };
            let Some(next) = child.or(exported) else {
                return Ok(None);
            };
            target = next;
        }
        Ok(Some(target))
    }

    fn visible_child(
        &self,
        current: ModuleId,
        parent: ModuleId,
        name: SymbolId,
    ) -> Option<ModuleId> {
        let node = self.graph.get(parent)?;
        let declaration = node.declarations.iter().find(|item| item.name == name)?;
        nia_imports::module_declaration_visibility_allows(
            declaration.visibility,
            &self.graph,
            parent,
            current,
        )
        .then(|| node.children.get(&name).copied())
        .flatten()
    }

    fn direct_names(&self, current: ModuleId, target: ModuleId) -> Vec<VisibleName> {
        let Some(defs) = self.defs.get(&target) else {
            return Vec::new();
        };
        defs.module_scope
            .values
            .entries()
            .chain(defs.module_scope.types.entries())
            .filter_map(|(name, id)| {
                let def = defs.defs.get(id)?;
                nia_imports::visibility_allows(def.visibility, &self.graph, target, current)
                    .then(|| self.db.source_symbol_text(*name))?
                    .map(|name| self.item_name(name, target, id))
            })
            .collect()
    }

    fn namespace_names(
        &self,
        current: ModuleId,
        target: ModuleId,
    ) -> nia_query::QueryResult<Vec<VisibleName>> {
        let mut names = self.direct_names(current, target);
        if let Some(node) = self.graph.get(target) {
            names.extend(node.children.keys().filter_map(|&name| {
                self.visible_child(current, target, name)?;
                Some(VisibleName {
                    name: self.db.source_symbol_text(name)?,
                    kind: Some(DefKind::Module),
                    symbol: None,
                })
            }));
        }
        if let Some(surface) = self.db.source_exports(target)? {
            let allowed = |scope: &nia_defs::ExportScope| {
                scope.allows(&|restriction| {
                    nia_imports::access_restriction_allows(restriction, &self.graph, current)
                })
            };
            for (&name, item) in surface.values.iter().chain(&surface.types) {
                if surface
                    .item_scopes
                    .get(&(item.namespace, name))
                    .is_none_or(allowed)
                    && let Some(name) = self.db.source_symbol_text(name)
                {
                    names.push(self.item_name(name, item.target_module, item.target_def_id));
                }
            }
            names.extend(surface.modules.keys().filter_map(|name| {
                if !surface.module_scopes.get(name).is_none_or(allowed) {
                    return None;
                }
                Some(VisibleName {
                    name: self.db.source_symbol_text(*name)?,
                    kind: Some(DefKind::Module),
                    symbol: None,
                })
            }));
        }
        Ok(names)
    }

    fn item_name(
        &self,
        name: Arc<str>,
        module_id: ModuleId,
        def_id: nia_ids::DefId,
    ) -> VisibleName {
        let kind = self
            .defs
            .get(&module_id)
            .and_then(|defs| defs.defs.get(def_id))
            .map(|def| def.kind);
        VisibleName {
            name,
            kind,
            symbol: Some(Symbol::Global(GlobalDefId { module_id, def_id })),
        }
    }

    pub(crate) fn definition(&self, path: &Path, position: Position) -> Option<Location> {
        let (source, span) = self.definitions.get(&self.symbol_at(path, position)?)?;
        let file = self.files.get(source)?;
        Some(Location::new(
            file.uri.clone(),
            file.lines.range(&file.text, *span),
        ))
    }

    pub(crate) fn hover(&self, path: &Path, position: Position) -> Option<lsp_types::Hover> {
        let (source, span) = self.definitions.get(&self.symbol_at(path, position)?)?;
        let file = self.files.get(source)?;
        let start = file.text[..span.start]
            .rfind(['\r', '\n'])
            .map_or(0, |index| index + 1);
        let line = file.text[start..].lines().next()?.trim();
        let header = line.split_once(" {").map_or(line, |(header, _)| header);
        Some(lsp_types::Hover {
            contents: lsp_types::HoverContents::Scalar(lsp_types::MarkedString::LanguageString(
                lsp_types::LanguageString {
                    language: "nia".into(),
                    value: header.into(),
                },
            )),
            range: None,
        })
    }

    pub(crate) fn references(
        &self,
        path: &Path,
        position: Position,
        declaration: bool,
    ) -> Vec<Location> {
        let Some(symbol) = self.symbol_at(path, position) else {
            return Vec::new();
        };
        let mut locations = Vec::new();
        for file in self.files.values() {
            let mut previous = None;
            for item in &file.occurrences {
                if item.symbol == symbol
                    && (declaration || !item.declaration)
                    && previous != Some(item.span)
                {
                    locations.push(Location::new(
                        file.uri.clone(),
                        file.lines.range(&file.text, item.span),
                    ));
                    previous = Some(item.span);
                }
            }
        }
        locations
    }

    pub(crate) fn diagnostics(&self, path: &Path, document: &Document) -> Vec<Diagnostic> {
        let path = normalized(path);
        self.diagnostics
            .iter()
            .filter(|item| item.path.as_str() == path)
            .map(|item| {
                let diagnostic = &item.diagnostic;
                let primary = diagnostic
                    .labels
                    .iter()
                    .find(|label| label.style == LabelStyle::Primary);
                let mut parts = vec![diagnostic.summary.as_str()];
                if let Some(message) = primary.and_then(|label| label.message.as_deref())
                    && message != diagnostic.summary
                {
                    parts.push(message);
                }
                parts.extend(diagnostic.notes.iter().map(String::as_str));
                parts.extend(diagnostic.help.iter().map(String::as_str));
                Diagnostic {
                    range: document.lines.range(
                        &document.text,
                        primary.map_or(Span::default(), |label| label.span),
                    ),
                    severity: Some(if item.is_error() {
                        DiagnosticSeverity::ERROR
                    } else {
                        DiagnosticSeverity::WARNING
                    }),
                    code: Some(NumberOrString::String(diagnostic.code.as_str().to_owned())),
                    source: Some("nia".to_owned()),
                    message: parts.join("\n"),
                    ..Diagnostic::default()
                }
            })
            .collect()
    }
}

fn type_name_span(tokens: &[LosslessToken], span: Span) -> Option<Span> {
    let start = tokens.partition_point(|token| token.span.start < span.start);
    let mut depth = 0_u32;
    let mut name = None;
    for token in tokens[start..]
        .iter()
        .take_while(|token| token.span.end <= span.end)
    {
        match token.kind {
            LosslessTokenKind::Token(TokenKind::LBracket) => depth += 1,
            LosslessTokenKind::Token(TokenKind::RBracket) => depth = depth.saturating_sub(1),
            LosslessTokenKind::Token(TokenKind::Ident) if depth == 0 => name = Some(token.span),
            _ => {}
        }
    }
    name
}

pub(crate) struct VisibleName {
    pub(crate) name: Arc<str>,
    pub(crate) kind: Option<DefKind>,
    pub(crate) symbol: Option<Symbol>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_test_support::{TestWorkload, acquire_test_resources, test_dir, workspace_root};
    use nia_toolchain::ToolchainLayoutRequest;

    fn test_session() -> Session {
        let toolchain = ToolchainLayout::resolve(ToolchainLayoutRequest::explicit(
            std::env::current_exe().unwrap(),
            workspace_root().join("lib"),
        ))
        .unwrap();
        Session::new(Arc::new(toolchain))
    }

    #[test]
    fn compiler_queries_follow_unsaved_cross_module_changes() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-cross-module");
        let main = root.join("main.nia");
        let child = root.join("math.nia");
        let main_text =
            "module math;\nfun f() i32 {\n    let local = math::amount();\n    local\n}\n";
        let child_text = "pub fun amount() i32 { 42 }\n";
        std::fs::write(&main, main_text).unwrap();
        std::fs::write(&child, child_text).unwrap();
        let mut documents: Documents = [(&main, main_text), (&child, child_text)]
            .into_iter()
            .map(|(path, text)| {
                (
                    path.clone(),
                    OpenDocument {
                        uri: uri(path).unwrap(),
                        document: Document::new(1, text.to_owned()),
                    },
                )
            })
            .collect();
        let mut session = test_session();
        let first = session.analyze(&documents, &main).unwrap();
        assert!(
            first
                .diagnostics(&main, &documents[&main].document)
                .is_empty()
        );
        let imported = Position::new(2, 25);
        let target = first.definition(&main, imported).unwrap();
        assert_eq!(target.uri, documents[&child].uri);
        assert_eq!(target.range.start, Position::new(0, 8));
        assert_eq!(first.references(&main, imported, true).len(), 2);
        assert!(first.hover(&main, imported).is_some());
        let local = first.definition(&main, Position::new(3, 4)).unwrap();
        assert_eq!(local.range.start, Position::new(2, 8));

        documents.get_mut(&child).unwrap().document =
            Document::new(2, "\npub fun amount() bool { true }\n".into());
        let edited = session.analyze(&documents, &main).unwrap();
        assert!(
            !edited
                .diagnostics(&main, &documents[&main].document)
                .is_empty()
        );

        documents.remove(&child);
        let restored = session.analyze(&documents, &main).unwrap();
        assert!(
            restored
                .diagnostics(&main, &documents[&main].document)
                .is_empty()
        );
        assert_eq!(restored.definition(&main, imported), Some(target));
    }

    #[test]
    fn highlights_resolve_imported_symbols_and_nested_generic_types() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-highlights");
        let main = root.join("main.nia");
        let child = root.join("shapes.nia");
        let source = concat!(
            "module shapes;\n",
            "using root::shapes::Boxed as Imported;\n",
            "using root::shapes::amount as imported_amount;\n",
            "fun combine[T](value: Imported[T], count: i32) i32 {\n",
            "    let callback = imported_amount;\n",
            "    count + callback()\n",
            "}\n",
        );
        std::fs::write(&main, source).unwrap();
        std::fs::write(
            &child,
            "pub struct Boxed[T] { item: T }\npub fun amount() i32 { 1 }\n",
        )
        .unwrap();
        let mut documents = Documents::new();
        documents.insert(
            main.clone(),
            OpenDocument {
                uri: uri(&main).unwrap(),
                document: Document::new(1, source.into()),
            },
        );
        let analysis = test_session().analyze(&documents, &main).unwrap();
        let diagnostics = analysis.diagnostics(&main, &documents[&main].document);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let resolved = analysis.semantic_highlights(&main);
        for (name, kind) in [
            ("Imported", Kind::Struct),
            ("T", Kind::TypeParameter),
            ("imported_amount", Kind::Function),
            ("count", Kind::Parameter),
            ("callback", Kind::Variable),
        ] {
            let start = source.rfind(name).unwrap();
            assert_eq!(
                resolved.get(&Span::new(start, start + name.len())),
                Some(&Class::new(kind)),
                "{name}"
            );
        }
    }

    #[test]
    fn completion_and_signature_help_follow_the_current_buffer() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-editing");
        let main = root.join("main.nia");
        let math = root.join("math.nia");
        std::fs::write(
            &math,
            "pub fun add(a: i32, b: i32) i32 { a + b }\nfun hidden() i32 { 0 }\n",
        )
        .unwrap();
        let mut session = test_session();
        let use_buffer = |session: &mut Session, text: &str| {
            let documents = [(
                main.clone(),
                OpenDocument {
                    uri: uri(&main).unwrap(),
                    document: Document::new(1, text.to_owned()),
                },
            )]
            .into_iter()
            .collect();
            session.analyze(&documents, &main).unwrap()
        };
        let text = concat!(
            "module math;\n",
            "using root::math as calc;\n",
            "fun run(param: i32) i32 {\n",
            "    let prior = 1;\n",
            "    { let private = 2; _ = private; };\n",
            "    _ = priXX;\n",
            "    let later = 3;\n",
            "    param + prior + later\n",
            "}\n",
        );
        std::fs::write(&main, "module math;\n").unwrap();
        let analysis = use_buffer(&mut session, text);
        let file = analysis.file(&main).expect("main buffer in analysis");
        analysis.names(file, 0, &[]).expect("visible names of main");
        let completion = analysis.complete(&main, Position::new(5, 11)).unwrap();
        assert_eq!(
            completion
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["prior"]
        );
        let Some(lsp_types::CompletionTextEdit::Edit(edit)) = &completion[0].text_edit else {
            panic!("completion must replace the entire identifier");
        };
        assert_eq!(
            edit.range,
            lsp_types::Range::new(Position::new(5, 8), Position::new(5, 13))
        );

        let incomplete = "module math;\nusing root::math as calc;\nfun run() i32 { calc:: }\n";
        let analysis = use_buffer(&mut session, incomplete);
        let completion = analysis.complete(&main, Position::new(2, 22)).unwrap();
        assert_eq!(
            completion
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["add"]
        );

        let incomplete =
            "module math;\nusing root::math as calc;\nfun run() i32 { calc::add((1, 2).0, \n";
        let analysis = use_buffer(&mut session, incomplete);
        let help = analysis
            .signature_help(&main, Position::new(2, 35))
            .unwrap();
        assert_eq!(help.active_parameter, Some(1));
        assert_eq!(help.signatures[0].label, "fun add(a: i32, b: i32) i32");
        let params = help.signatures[0].parameters.as_ref().unwrap();
        assert_eq!(
            params[1].label,
            lsp_types::ParameterLabel::Simple("b: i32".into())
        );
    }
}
