// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use lsp_types::{
    CompletionItemKind, Diagnostic, DiagnosticSeverity, DocumentHighlight, Location,
    NumberOrString, Position, Uri,
};
use nia_compiler_query::{CheckedProgramAnalysis, CompilerDatabase};
use nia_defs::{DefCollection, DefKind, ModuleUsingScope};
use nia_diagnostic::LabelStyle;
use nia_driver::{CheckRequest, Driver, DriverConfig};
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_imports::{ModuleGraphSnapshot, ModuleRootSegment};
use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_sema_ir::{ResolvedCall, SemanticValueUse};
use nia_source::SourcePath;
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_syntax::{GreenNode, SyntaxKind};
use nia_toolchain::ToolchainLayout;
use nia_ty::{TyKind, TypeStore};
use nia_type_resolve::TypeNameResolution;
use url::Url;

use crate::document::{Document, Lines};
use crate::editing::{MemberCompletion, field_accesses};
use crate::highlight::{Class, Kind, Resolved};
use crate::project::{EditorRequest, Projects};
use crate::workspace::Cancellation;

mod imports;
mod namespaces;
#[cfg(test)]
mod rename_tests;
use imports::ImportOccurrences;

pub(crate) type Documents = BTreeMap<PathBuf, OpenDocument>;

#[derive(Clone)]
pub(crate) struct OpenDocument {
    pub(crate) uri: Uri,
    pub(crate) document: Document,
}

pub(crate) fn path(uri: &Uri) -> Option<PathBuf> {
    Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

pub(crate) fn uri(path: &Path) -> Option<Uri> {
    Url::from_file_path(path).ok()?.as_str().parse().ok()
}

pub(crate) fn normalized(path: &Path) -> String {
    nia_source::normalize_path(&path.to_string_lossy())
}

pub(crate) struct Session {
    pub(crate) toolchain: Arc<ToolchainLayout>,
    driver: Driver,
    target_drivers: RefCell<HashMap<(nia_target::TargetConfig, nia_target::CpuLevel), Driver>>,
    overlays: BTreeMap<PathBuf, Arc<str>>,
    pub(crate) projects: Projects,
    pub(crate) workspace_index: Option<Arc<crate::workspace::WorkspaceIndex>>,
}

impl Session {
    pub(crate) fn new(toolchain: Arc<ToolchainLayout>) -> Self {
        Self {
            driver: Driver::new(toolchain.clone()),
            target_drivers: RefCell::default(),
            toolchain,
            overlays: BTreeMap::new(),
            projects: Projects::default(),
            workspace_index: None,
        }
    }

    pub(crate) fn analyze(
        &mut self,
        documents: &Documents,
        entry: &Path,
    ) -> Result<Analysis, String> {
        self.analyze_with_cancel(documents, entry, None)
    }

    pub(crate) fn analyze_with_cancel(
        &mut self,
        documents: &Documents,
        entry: &Path,
        cancel: Option<&Cancellation>,
    ) -> Result<Analysis, String> {
        nia_timing::time_stage(
            crate::timing_mode(),
            nia_timing::TimingLevel::Summary,
            "lsp.install_sources",
            || self.install_documents(documents),
        )?;
        let analyze = |request| {
            if let Some(cancel) = cancel {
                cancel.check()?;
            }
            analyze_editor_request(&self.driver, &self.target_drivers, &self.overlays, request)
        };
        let Some(package) = self.projects.package(entry)? else {
            return analyze(CheckRequest::new(normalized(entry)).into());
        };
        let entry_path = normalized(entry);
        let (mut configured, configuration_warning) = match package
            .configured_requests(&self.toolchain)
        {
            Ok(configured) => (configured, None),
            Err(error) => (
                Vec::new(),
                Some(package.configuration_message().unwrap_or_else(|| {
                    format!("Configured build plan could not be applied. Using manifest entries. {error}")
                })),
            ),
        };
        configured.sort_by_key(|request| request.check.entry_path.as_str() != entry_path);
        for request in configured {
            if let Ok(analysis) = analyze(request)
                && analysis.files.contains_key(&entry_path)
            {
                return Ok(analysis);
            }
        }
        let entries = package.entries(entry);
        let mut entries = entries.into_iter().peekable();
        while let Some((path, context)) = entries.next() {
            let analysis = package
                .editor_request(&path, context, &self.toolchain)
                .and_then(analyze);
            if entries.peek().is_none()
                || analysis
                    .as_ref()
                    .is_ok_and(|analysis| analysis.files.contains_key(&entry_path))
            {
                return analysis.map(|mut analysis| {
                    analysis.configuration_warning = configuration_warning;
                    analysis
                });
            }
        }
        unreachable!("editor requests always include the current source")
    }

    pub(crate) fn install_documents(&mut self, documents: &Documents) -> Result<(), String> {
        // A fresh source database lets closed files return to disk ownership.
        if self
            .overlays
            .keys()
            .any(|path| !documents.contains_key(path))
        {
            self.workspace_index = None;
            self.driver = Driver::new(self.toolchain.clone());
            self.target_drivers.get_mut().clear();
            self.overlays.clear();
        }
        for (path, open) in documents {
            let text = &open.document.text;
            if self.overlays.get(path).is_some_and(|old| old == text) {
                continue;
            }
            self.workspace_index = None;
            for driver in
                std::iter::once(&self.driver).chain(self.target_drivers.get_mut().values())
            {
                set_source(driver, path, text.clone())?;
            }
            self.overlays.insert(path.clone(), text.clone());
        }
        Ok(())
    }

    pub(crate) fn analyze_request(&self, request: EditorRequest) -> Result<Analysis, String> {
        analyze_editor_request(&self.driver, &self.target_drivers, &self.overlays, request)
    }

    pub(crate) fn reload(&mut self) {
        *self = Self::new(self.toolchain.clone());
    }

    pub(crate) fn complete(
        &mut self,
        analysis: &Analysis,
        documents: &Documents,
        path: &Path,
        position: Position,
    ) -> Option<Vec<lsp_types::CompletionItem>> {
        let file = analysis.file(path)?;
        let offset = file.lines.offset(&file.text, position)?;
        let Some(member) = MemberCompletion::at(&file.text, offset) else {
            return analysis.complete(path, position);
        };
        let names = analysis.method_names(member.prefix(&file.text));
        let fields = member.candidates(analysis, file, &[]);
        let candidates = if names.is_empty() && fields.is_some() {
            fields
        } else {
            let mut overlay = documents.clone();
            if fields.is_none()
                && let Some(text) = member.probe(&file.text)
            {
                let document = &mut overlay.get_mut(path)?.document;
                *document = Document::new(document.version, text);
            }
            nia_timing::time_stage(
                crate::timing_mode(),
                nia_timing::TimingLevel::Summary,
                "lsp.member_completion_analysis",
                || {
                    self.analyze(&overlay, path)
                        .ok()
                        .and_then(|checked| {
                            member.candidates(&checked, checked.file(path)?, &names)
                        })
                        .or(fields)
                },
            )
        };
        Some(member.items(&file.text, &file.lines, candidates.unwrap_or_default()))
    }
}

fn analyze_editor_request(
    driver: &Driver,
    drivers: &RefCell<HashMap<(nia_target::TargetConfig, nia_target::CpuLevel), Driver>>,
    overlays: &BTreeMap<PathBuf, Arc<str>>,
    request: EditorRequest,
) -> Result<Analysis, String> {
    let Some(context) = request.target else {
        return analyze_request(driver, request.check);
    };
    let mut drivers = drivers.borrow_mut();
    let configured = match drivers.entry((context.target, context.cpu)) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            let config = DriverConfig::new(driver.config().toolchain.clone())
                .with_artifact_target(context.target)
                .with_cpu(context.cpu);
            let configured = Driver::with_config(config);
            for (path, text) in overlays {
                set_source(&configured, path, text.clone())?;
            }
            entry.insert(configured)
        }
    };
    let runtime = match context.runtime {
        nia_build_plan::Runtime::Bare => nia_toolchain::RuntimeSpec::Bare,
        nia_build_plan::Runtime::Freestanding => {
            nia_toolchain::RuntimeSpec::freestanding(&driver.config().toolchain, &context.target)
                .map_err(|error| error.to_string())?
        }
    };
    analyze_request(
        configured,
        request
            .check
            .with_runtime(runtime)
            .with_profile(context.profile)
            .with_compilation_mode(match context.mode {
                nia_build_plan::CompilationMode::Normal => {
                    nia_target_config::CompilationMode::Normal
                }
                nia_build_plan::CompilationMode::Test => nia_target_config::CompilationMode::Test,
            }),
    )
}

fn set_source(driver: &Driver, path: &Path, text: Arc<str>) -> Result<(), String> {
    let physical = normalized(path);
    let toolchain = &driver.config().toolchain;
    let mut paths = vec![SourcePath::new(physical.clone())];
    for package in [
        toolchain.std_package(),
        toolchain.freestanding_runtime_package(),
    ] {
        if let Ok(relative) = Path::new(&physical).strip_prefix(normalized(package.directory())) {
            paths.push(SourcePath::with_identity(
                physical.clone(),
                package.identity(&normalized(relative)),
            ));
        }
    }
    for path in paths {
        driver
            .set_source_path(path, text.clone())
            .map_err(|error| match error {
                nia_driver::DriverError::InternalDiagnostic(diagnostic) => diagnostic.summary,
                other => format!("{other:?}"),
            })?;
    }
    Ok(())
}

fn analyze_request(driver: &Driver, request: CheckRequest) -> Result<Analysis, String> {
    driver
        .analyze_for_editor(request.with_timings(crate::timing_mode()), |db, program| {
            Analysis::from_program(driver, db, program)
        })
        .map_err(|error| error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Symbol {
    Global(GlobalDefId),
    Module(ModuleId),
    Local(ModuleId, LocalId),
    ModuleAlias(ModuleId, Span),
    ItemAlias(ModuleId, Span),
}

struct Occurrence {
    span: Span,
    symbol: Symbol,
    declaration: bool,
}

pub(crate) struct File {
    module: ModuleId,
    pub(crate) uri: Uri,
    pub(crate) text: Arc<str>,
    pub(crate) lines: Lines,
    occurrences: Vec<Occurrence>,
    locals: HashMap<LocalId, (Kind, bool)>,
    type_highlights: Resolved,
    expr_types: HashMap<Span, InternedTyId>,
    syntax: OnceLock<Option<nia_syntax::Parse>>,
    field_uses: OnceLock<Vec<(Span, GlobalDefId)>>,
    rename_aliases: HashMap<Span, Symbol>,
}

impl File {
    pub(crate) fn syntax(&self) -> Option<&nia_syntax::Parse> {
        self.syntax
            .get_or_init(|| nia_syntax::parse(&self.text, None).ok())
            .as_ref()
    }
}

pub(crate) struct Analysis {
    pub(crate) files: BTreeMap<String, File>,
    pub(crate) definitions: HashMap<Symbol, (String, Span)>,
    diagnostics: Vec<nia_compiler_query::ProgramDiagnostic>,
    configuration_warning: Option<String>,
    trait_implementations: Vec<(GlobalDefId, String, Span)>,
    trait_members: HashMap<GlobalDefId, GlobalDefId>,
    db: CompilerDatabase,
    graph: ModuleGraphSnapshot,
    defs: HashMap<ModuleId, Arc<DefCollection>>,
    types: Arc<TypeStore>,
}

impl Analysis {
    pub(crate) fn is_field(&self, symbol: Symbol) -> bool {
        let Symbol::Global(id) = symbol else {
            return false;
        };
        self.defs
            .get(&id.module_id)
            .and_then(|defs| defs.defs.get(id.def_id))
            .is_some_and(|def| {
                matches!(
                    def.kind,
                    DefKind::StructField | DefKind::UnionField | DefKind::EnumVariantField
                )
            })
    }

    pub(crate) fn foreign_link_attribute(&self, symbol: Symbol) -> Option<(Span, &'static str)> {
        let Symbol::Global(id) = symbol else {
            return None;
        };
        let (source, name) = self.definitions.get(&symbol)?;
        let file = self.files.get(source)?;
        let def = self.defs.get(&id.module_id)?.defs.get(id.def_id)?;
        if !matches!(def.kind, DefKind::Function | DefKind::Global) {
            return None;
        }
        let item = file
            .syntax()?
            .tree
            .green_root()
            .child_nodes()
            .find(|node| {
                node.kind() == &SyntaxKind::Item
                    && node.span().start <= name.start
                    && name.end <= node.span().end
            })?;
        let decl = item.child_nodes().find(|node| {
            matches!(
                node.kind(),
                SyntaxKind::FunctionDecl | SyntaxKind::BindingDecl
            )
        })?;
        item.token(&TokenKind::Extern)?;
        let attribute = if decl.child(&SyntaxKind::Block).is_some() {
            "exportName"
        } else {
            "linkName"
        };
        if item
            .child_nodes()
            .filter(|node| node.kind() == &SyntaxKind::Attribute)
            .any(|node| {
                node.token(&TokenKind::Ident)
                    .is_some_and(|token| token.text() == attribute)
            })
        {
            return None;
        }
        let span = item.token_span()?;
        Some((Span::new(span.start, span.start), attribute))
    }

    fn from_program(
        driver: &Driver,
        db: &CompilerDatabase,
        program: CheckedProgramAnalysis,
    ) -> nia_query::QueryResult<Self> {
        let mut result = Self {
            files: BTreeMap::new(),
            definitions: HashMap::new(),
            diagnostics: program.diagnostics,
            configuration_warning: None,
            trait_implementations: Vec::new(),
            trait_members: HashMap::new(),
            db: db.clone(),
            graph: program.graph.clone(),
            defs: program
                .modules
                .iter()
                .map(|module| (module.id, module.defs.clone()))
                .collect(),
            types: db.type_store(),
        };
        let modules: HashMap<_, _> = program
            .modules
            .iter()
            .map(|module| (module.id, module))
            .collect();
        let mut import_scopes = HashMap::new();
        let mut trait_members = HashMap::new();
        let mut projections = Vec::new();
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
            let mut expr_types = HashMap::new();
            let syntax = OnceLock::new();
            let mut rename_aliases = HashMap::new();
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
                    let symbol = if def.kind == DefKind::Module {
                        node.children.get(&def.name).copied().map(Symbol::Module)
                    } else {
                        None
                    }
                    .unwrap_or(Symbol::Global(GlobalDefId {
                        module_id: node.id,
                        def_id: id,
                    }));
                    declare(symbol, span);
                }
            }
            if let Some(module) = module {
                let local_types: HashMap<_, _> = module
                    .semantic_facts
                    .function_facts
                    .values()
                    .flat_map(|facts| facts.local_types.iter())
                    .map(|(&id, &ty)| (id, ty))
                    .collect();
                if file.text.contains("extend") {
                    let implementations = db.source_trait_implementations(module)?;
                    result.trait_implementations.extend(
                        implementations
                            .traits
                            .into_iter()
                            .map(|(id, span)| (id, path.to_owned(), span)),
                    );
                    trait_members.extend(implementations.members.into_iter().map(
                        |(definition, member)| {
                            (
                                GlobalDefId {
                                    module_id: module.id,
                                    def_id: definition,
                                },
                                member,
                            )
                        },
                    ));
                }
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
                        if let Some(&ty) = local_types.get(&id) {
                            expr_types.insert(span, module.type_normalization.normalize(ty));
                        }
                    }
                }
                let call_targets: HashMap<_, _> = module
                    .semantic_facts
                    .iter_node_resolved_calls()
                    .filter_map(|(key, call)| match call {
                        ResolvedCall::Method { def_id, .. } => Some((key, *def_id)),
                        ResolvedCall::TraitMethod { method_id, .. }
                        | ResolvedCall::DynamicTraitMethod { method_id, .. }
                        | ResolvedCall::TraitAssociatedFunction { method_id, .. } => {
                            Some((key, *method_id))
                        }
                        _ => None,
                    })
                    .collect();
                let mut method_calls = HashMap::new();
                for (span, key) in db.source_origins(module.id)?.entries() {
                    if let Some(ty) = module.semantic_facts.node_expr_type(&key)
                        && ty != result.types.error()
                    {
                        expr_types.insert(span, module.type_normalization.normalize(ty));
                    }
                    if let Some(ty) = module.type_lowering.ty_for_key(&key)
                        && let Some(TyKind::Projection {
                            trait_id: nia_ty::TraitId::Source(trait_id),
                            name,
                            ..
                        }) = result.types.get(ty)
                        && let Some(token) = type_name_span(&tokens, span)
                        && name_matches(&token, *name)
                    {
                        projections.push((
                            path.to_owned(),
                            token,
                            *trait_id,
                            *name,
                            DefKind::TraitAssociatedType,
                        ));
                    }
                    if let Some(projection) =
                        module.semantic_uses.node_associated_const_projection(&key)
                        && let nia_ty::TraitId::Source(trait_id) = projection.trait_id
                        && let Some(token) = type_name_span(&tokens, span)
                        && name_matches(&token, projection.name)
                    {
                        projections.push((
                            path.to_owned(),
                            token,
                            trait_id,
                            projection.name,
                            DefKind::Const,
                        ));
                    }
                    if let Some(resolution) = module.type_resolution.node_type_names.get(key.site())
                        && let Some(name) = type_name_span(&tokens, span)
                    {
                        let symbol = match resolution {
                            TypeNameResolution::Def(def_id) => Some(GlobalDefId {
                                module_id: module.id,
                                def_id: *def_id,
                            }),
                            TypeNameResolution::External(id) => Some(*id),
                            TypeNameResolution::GenericParam => {
                                type_highlights.insert(name, Class::new(Kind::TypeParameter));
                                None
                            }
                            TypeNameResolution::BuiltinTrait(_) => {
                                type_highlights.insert(name, Class::builtin(Kind::Interface));
                                None
                            }
                            _ => None,
                        };
                        if let Some(id) = symbol {
                            occurrences.push(Occurrence {
                                span: name,
                                symbol: Symbol::Global(id),
                                declaration: false,
                            });
                        }
                    }
                    let names = tokens_in(span);
                    let target = match module.semantic_uses.node_value_use(&key) {
                        Some(SemanticValueUse::Local(id)) => names
                            .last()
                            .map(|&name| (name, Symbol::Local(module.id, id))),
                        Some(SemanticValueUse::Global(id)) => {
                            names.last().map(|&name| (name, Symbol::Global(id)))
                        }
                        None => module.semantic_uses.node_type_prefix(&key).and_then(|id| {
                            Some((type_name_span(&tokens, span)?, Symbol::Global(id)))
                        }),
                    };
                    if let Some((span, symbol)) = target {
                        occurrences.push(Occurrence {
                            span,
                            symbol,
                            declaration: false,
                        });
                    }
                    if let Some(&id) = call_targets.get(&key) {
                        method_calls.insert(span, id);
                    }
                }
                if !method_calls.is_empty()
                    && let Some(parsed) =
                        syntax.get_or_init(|| nia_syntax::parse(&file.text, None).ok())
                {
                    index_method_calls(
                        parsed.tree.green_root(),
                        &mut method_calls,
                        &mut occurrences,
                    );
                }
            }
            if tokens
                .iter()
                .any(|token| token.kind == LosslessTokenKind::Token(TokenKind::Using))
                && let Some(parsed) =
                    syntax.get_or_init(|| nia_syntax::parse(&file.text, None).ok())
            {
                let mut imports = ImportOccurrences {
                    graph: &program.graph,
                    module: node.id,
                    text: &file.text,
                    tokens: &tokens,
                    occurrences: &mut occurrences,
                    aliases: &mut rename_aliases,
                    bindings: HashMap::new(),
                };
                let scopes = db.source_import_scopes(node.id)?;
                imports.index(parsed.tree.green_root(), &defs, &scopes);
                for (span, symbol) in imports.bindings {
                    result.definitions.insert(symbol, (path.to_owned(), span));
                }
                import_scopes.insert(node.id, scopes);
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
                    expr_types,
                    syntax,
                    field_uses: OnceLock::new(),
                    rename_aliases,
                },
            );
        }
        for (path, span, trait_id, name, kind) in projections {
            if let Some(id) = result.associated_member(trait_id, name, kind)
                && let Some(file) = result.files.get_mut(&path)
            {
                file.occurrences.push(Occurrence {
                    span,
                    symbol: Symbol::Global(id),
                    declaration: false,
                });
            }
        }
        result.index_named_associated_types();
        result.index_namespaces(&import_scopes)?;
        for file in result.files.values_mut() {
            file.occurrences
                .sort_by_key(|item| (item.span, !item.declaration));
            for occurrence in &file.occurrences {
                if let Symbol::Global(id) = occurrence.symbol
                    && let Some(&member) = trait_members.get(&id)
                {
                    file.rename_aliases
                        .insert(occurrence.span, Symbol::Global(member));
                }
            }
        }
        result.trait_members = trait_members;
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

    pub(crate) fn rename_symbol_at(&self, file: &File, offset: usize) -> Option<(Symbol, Span)> {
        let symbol = self.symbol_at_offset(file, offset)?;
        let span = self
            .reference_spans(file, symbol, true)
            .into_iter()
            .find(|span| span.start <= offset && offset < span.end)?;
        Some((
            file.rename_aliases.get(&span).copied().unwrap_or(symbol),
            span,
        ))
    }

    pub(crate) fn rename_supported(&self, symbol: Symbol) -> bool {
        match symbol {
            Symbol::Local(..) => true,
            Symbol::Global(id) => self
                .defs
                .get(&id.module_id)
                .and_then(|defs| defs.defs.get(id.def_id))
                .is_some_and(|def| {
                    let Some((source, name)) = self.declaration_source(symbol) else {
                        return false;
                    };
                    !(def.kind == DefKind::Function
                        && matches!(&source[name.start..name.end], "main" | "build"))
                        && matches!(
                            def.kind,
                            DefKind::Function
                                | DefKind::Global
                                | DefKind::Const
                                | DefKind::Struct
                                | DefKind::Union
                                | DefKind::Enum
                                | DefKind::Trait
                                | DefKind::TypeAlias
                                | DefKind::StructField
                                | DefKind::UnionField
                                | DefKind::EnumVariant
                                | DefKind::EnumVariantField
                                | DefKind::Method
                                | DefKind::TraitMethod
                                | DefKind::TraitAssociatedType
                        )
                }),
            Symbol::Module(module) => self.graph.get(module).is_some_and(|node| {
                node.parent.is_some() && self.files.contains_key(node.path.as_str())
            }),
            Symbol::ModuleAlias(..) | Symbol::ItemAlias(..) => true,
        }
    }

    fn associated_member(
        &self,
        trait_id: GlobalDefId,
        name: SymbolId,
        kind: DefKind,
    ) -> Option<GlobalDefId> {
        let defs = self.defs.get(&trait_id.module_id)?;
        if defs.defs.get(trait_id.def_id)?.kind != DefKind::Trait {
            return None;
        }
        let members = defs.scopes.struct_members.get(&trait_id.def_id)?;
        let table = if kind == DefKind::Const {
            &members.values
        } else {
            &members.fields
        };
        Some(GlobalDefId {
            module_id: trait_id.module_id,
            def_id: table.get(&name)?,
        })
    }

    fn index_named_associated_types(&mut self) {
        let mut additions = Vec::new();
        for (source, file) in &self.files {
            if !file.text.contains('=') || !file.text.contains('[') {
                continue;
            }
            if let Some(syntax) = file.syntax() {
                let mut indexed = Vec::new();
                self.named_associated_types(file, syntax.tree.green_root(), &mut indexed);
                additions.push((source.clone(), indexed));
            }
        }
        for (source, indexed) in additions {
            let file = self.files.get_mut(&source).expect("indexed source");
            file.occurrences.extend(indexed);
        }
    }

    fn named_associated_types(
        &self,
        file: &File,
        node: &GreenNode,
        occurrences: &mut Vec<Occurrence>,
    ) {
        if node.kind() == &SyntaxKind::PathSegment
            && let Some(arguments) = node.child(&SyntaxKind::TypeArgList)
            && let Some(owner) = node.token(&TokenKind::Ident)
        {
            for binding in arguments
                .child_nodes()
                .filter(|node| node.kind() == &SyntaxKind::AssocBindingArg)
            {
                let Some(key) = binding.child_nodes().find(|node| node.kind().is_type()) else {
                    continue;
                };
                if key.kind() != &SyntaxKind::PathType {
                    continue;
                }
                let Some(key) = key
                    .child(&SyntaxKind::PathSegment)
                    .and_then(|node| node.token(&TokenKind::Ident))
                else {
                    continue;
                };
                let name = SymbolId::from_stable_hash(stable_hash(key.text()));
                if let Some(target) = file.occurrences.iter().find_map(|occurrence| {
                    let Symbol::Global(trait_id) = occurrence.symbol else {
                        return None;
                    };
                    (occurrence.span == owner.span() && !occurrence.declaration)
                        .then(|| {
                            self.associated_member(trait_id, name, DefKind::TraitAssociatedType)
                        })
                        .flatten()
                }) {
                    occurrences.push(Occurrence {
                        span: key.span(),
                        symbol: Symbol::Global(target),
                        declaration: false,
                    });
                }
            }
        }
        for child in node.child_nodes() {
            self.named_associated_types(file, child, occurrences);
        }
    }

    pub(crate) fn rename_scope(&self, symbol: Symbol) -> Option<Span> {
        if self.rename_workspace_symbol(symbol) {
            return None;
        }
        let (source, span) = self.definitions.get(&symbol)?;
        let root = self.files.get(source)?.syntax()?.tree.green_root();
        let item = root.child_nodes().find(|node| {
            let item = node.span();
            item.start <= span.start && span.end <= item.end
        })?;
        if matches!(symbol, Symbol::Local(..)) {
            return Some(item.span());
        }
        let mut enclosing = item;
        let mut block = None;
        while let Some(child) = enclosing.child_nodes().find(|node| {
            let region = node.span();
            region.start <= span.start && span.end <= region.end
        }) {
            if child.kind() == &SyntaxKind::Block {
                block = Some(child.span());
            }
            enclosing = child;
        }
        block
    }

    pub(crate) fn rename_workspace_symbol(&self, symbol: Symbol) -> bool {
        match symbol {
            Symbol::Global(_) | Symbol::Module(_) => true,
            Symbol::Local(..) => false,
            Symbol::ModuleAlias(module, span) | Symbol::ItemAlias(module, span) => {
                self.defs.get(&module).is_some_and(|defs| {
                    defs.module_usings.iter().any(|directive| {
                        directive.visibility != nia_ids::Visibility::Private
                            && directive.span.start <= span.start
                            && span.end <= directive.span.end
                    })
                })
            }
        }
    }

    pub(crate) fn module_file(&self, symbol: Symbol) -> Option<&str> {
        let Symbol::Module(module) = symbol else {
            return None;
        };
        Some(self.graph.get(module)?.path.as_str())
    }

    pub(crate) fn first_error(&self) -> Option<&str> {
        self.diagnostics
            .iter()
            .find(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.diagnostic.summary.as_str())
    }

    pub(crate) fn rename_occurrences<'a>(
        &self,
        file: &'a File,
    ) -> impl Iterator<Item = (Span, Symbol, Option<Symbol>)> + 'a {
        file.occurrences
            .iter()
            .map(|item| {
                (
                    item.span,
                    item.symbol,
                    file.rename_aliases.get(&item.span).copied(),
                )
            })
            .chain(
                self.field_uses(file)
                    .iter()
                    .map(|&(span, field)| (span, Symbol::Global(field), None)),
            )
    }

    fn receiver_type(&self, file: &File, receiver: Span) -> Option<&TyKind> {
        let mut ty = *file.expr_types.get(&receiver)?;
        loop {
            match self.types.get(ty)? {
                TyKind::Pointer { elem, .. } | TyKind::VolatilePointer { elem, .. } => ty = *elem,
                kind => return Some(kind),
            }
        }
    }

    fn nominal_fields(&self, id: GlobalDefId) -> Option<&nia_defs::NameTable> {
        let defs = self.defs.get(&id.module_id)?;
        let scope = defs
            .scopes
            .struct_members
            .get(&id.def_id)
            .or_else(|| defs.scopes.union_members.get(&id.def_id))?;
        Some(&scope.fields)
    }

    pub(crate) fn receiver_members(
        &self,
        file: &File,
        receiver: Span,
        names: &[SymbolId],
    ) -> Option<Vec<(String, CompletionItemKind)>> {
        let mut members: Vec<_> = self
            .receiver_fields(file, receiver)?
            .into_iter()
            .map(|name| (name, CompletionItemKind::FIELD))
            .collect();
        if let Ok(methods) = self
            .db
            .source_receiver_methods(file.module, receiver, names)
        {
            members.extend(methods.into_iter().filter_map(|name| {
                Some((
                    self.db.source_symbol_text(name)?.to_string(),
                    CompletionItemKind::METHOD,
                ))
            }));
        }
        Some(members)
    }

    fn method_names(&self, prefix: &str) -> Vec<SymbolId> {
        let mut names: Vec<_> = self
            .defs
            .values()
            .flat_map(|defs| defs.defs.iter())
            .filter_map(|(_, def)| {
                (matches!(def.kind, DefKind::Method | DefKind::TraitMethod)
                    && self.db.source_symbol_text(def.name)?.starts_with(prefix))
                .then_some(def.name)
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    fn receiver_fields(&self, file: &File, receiver: Span) -> Option<Vec<String>> {
        Some(match self.receiver_type(file, receiver)? {
            TyKind::Nominal { def_id, .. } => self
                .nominal_fields(*def_id)
                .into_iter()
                .flat_map(nia_defs::NameTable::entries)
                .filter_map(|(name, _)| {
                    self.db
                        .source_symbol_text(*name)
                        .map(|name| name.to_string())
                })
                .collect(),
            TyKind::Tuple(elems) => (0..elems.len()).map(|index| index.to_string()).collect(),
            _ => Vec::new(),
        })
    }

    fn field_uses<'a>(&self, file: &'a File) -> &'a [(Span, GlobalDefId)] {
        file.field_uses.get_or_init(|| {
            let mut uses = Vec::new();
            if file.expr_types.is_empty() {
                return uses;
            }
            if let Some(syntax) = file.syntax() {
                field_accesses(syntax.tree.green_root(), &mut |name, receiver| {
                    // A dot may introduce a resolved method call instead of a field access.
                    if file
                        .occurrences
                        .binary_search_by_key(&name, |item| item.span)
                        .is_ok()
                    {
                        return;
                    }
                    let Some(TyKind::Nominal { def_id, .. }) = self.receiver_type(file, receiver)
                    else {
                        return;
                    };
                    let symbol =
                        SymbolId::from_stable_hash(stable_hash(&file.text[name.start..name.end]));
                    if let Some(field) = self
                        .nominal_fields(*def_id)
                        .and_then(|fields| fields.get(&symbol))
                    {
                        uses.push((
                            name,
                            GlobalDefId {
                                module_id: def_id.module_id,
                                def_id: field,
                            },
                        ));
                    }
                });
                self.named_fields(file, syntax.tree.green_root(), &mut uses);
            }
            uses.sort_unstable();
            uses.dedup();
            uses
        })
    }

    fn named_fields(&self, file: &File, node: &GreenNode, uses: &mut Vec<(Span, GlobalDefId)>) {
        let list = node
            .child(&SyntaxKind::FieldInitList)
            .or_else(|| node.child(&SyntaxKind::NominalNamedFields));
        if let Some(list) = list {
            let constructor = node
                .child_nodes()
                .find(|child| child.kind().is_expr() || child.kind().is_type());
            let variant = constructor.and_then(|constructor| {
                file.occurrences.iter().find_map(|occurrence| {
                    let Symbol::Global(id) = occurrence.symbol else {
                        return None;
                    };
                    let span = constructor.span();
                    (span.start <= occurrence.span.start
                        && occurrence.span.end <= span.end
                        && self.defs.get(&id.module_id)?.defs.get(id.def_id)?.kind
                            == DefKind::EnumVariant)
                        .then_some(id)
                })
            });
            let parent = variant
                .or_else(|| {
                    let span = node.token_span()?;
                    let TyKind::Nominal { def_id, .. } = self.receiver_type(file, span)? else {
                        return None;
                    };
                    Some(*def_id)
                })
                .or_else(|| {
                    let span = constructor?.span();
                    file.occurrences.iter().rev().find_map(|occurrence| {
                        let Symbol::Global(id) = occurrence.symbol else {
                            return None;
                        };
                        (span.start <= occurrence.span.start
                            && occurrence.span.end <= span.end
                            && self.nominal_fields(id).is_some())
                        .then_some(id)
                    })
                });
            if let Some(parent) = parent {
                for field in list
                    .child_nodes()
                    .filter_map(|field| field.token(&TokenKind::Ident))
                {
                    let name = SymbolId::from_stable_hash(stable_hash(field.text()));
                    let def =
                        self.nominal_fields(parent)
                            .and_then(|fields| fields.get(&name))
                            .or_else(|| {
                                self.defs.get(&parent.module_id)?.defs.iter().find_map(
                                    |(id, def)| {
                                        (def.parent == Some(parent.def_id)
                                            && def.kind == DefKind::EnumVariantField
                                            && def.name == name)
                                            .then_some(id)
                                    },
                                )
                            });
                    if let Some(def_id) = def {
                        uses.push((
                            field.span(),
                            GlobalDefId {
                                module_id: parent.module_id,
                                def_id,
                            },
                        ));
                    }
                }
            }
        }
        for child in node.child_nodes() {
            self.named_fields(file, child, uses);
        }
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
                Symbol::Module(_) | Symbol::ModuleAlias(..) => Some((Kind::Namespace, false)),
                Symbol::ItemAlias(..) => None,
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
            .or_else(|| {
                let fields = self.field_uses(file);
                let end = fields.partition_point(|(name, _)| name.start <= offset);
                let (span, field) = fields.get(end.checked_sub(1)?)?;
                (offset < span.end).then_some(Symbol::Global(*field))
            })
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
        self.symbol_location(self.symbol_at(path, position)?)
    }

    pub(crate) fn type_definition(&self, path: &Path, position: Position) -> Option<Location> {
        let file = self.file(path)?;
        let offset = file.lines.offset(&file.text, position)?;
        if let Some(Symbol::Global(id)) = self.symbol_at_offset(file, offset)
            && self
                .defs
                .get(&id.module_id)
                .and_then(|defs| defs.defs.get(id.def_id))
                .is_some_and(|def| {
                    matches!(
                        def.kind,
                        DefKind::Struct
                            | DefKind::Enum
                            | DefKind::Union
                            | DefKind::Trait
                            | DefKind::TypeAlias
                            | DefKind::TraitAssociatedType
                    )
                })
        {
            return self.symbol_location(Symbol::Global(id));
        }

        let (_, &inferred) = file
            .expr_types
            .iter()
            .filter(|(span, _)| span.start <= offset && offset < span.end)
            .min_by_key(|(span, _)| (span.len(), span.start))?;
        let mut ty = inferred;
        let id = loop {
            match self.types.get(ty)? {
                TyKind::Nominal { def_id, .. } => break *def_id,
                TyKind::TraitObject {
                    trait_id: nia_ty::TraitId::Source(id),
                    ..
                }
                | TyKind::TraitObjectPointee {
                    trait_id: nia_ty::TraitId::Source(id),
                    ..
                } => break *id,
                TyKind::Projection {
                    trait_id: nia_ty::TraitId::Source(id),
                    name,
                    ..
                } => {
                    break self.associated_member(*id, *name, DefKind::TraitAssociatedType)?;
                }
                TyKind::Pointer { elem, .. }
                | TyKind::VolatilePointer { elem, .. }
                | TyKind::Optional { elem } => ty = *elem,
                TyKind::FunctionItem { return_type, .. } => ty = *return_type,
                _ => return None,
            }
        };
        self.symbol_location(Symbol::Global(id))
    }

    fn symbol_location(&self, symbol: Symbol) -> Option<Location> {
        let (source, span) = self.definitions.get(&symbol)?;
        let file = self.files.get(source)?;
        Some(Location::new(
            file.uri.clone(),
            file.lines.range(&file.text, *span),
        ))
    }

    pub(crate) fn workspace_declarations(
        &self,
    ) -> impl Iterator<Item = (&str, Span, String, lsp_types::SymbolKind, String)> {
        self.definitions
            .iter()
            .filter_map(|(symbol, (source, span))| {
                use lsp_types::SymbolKind as Kind;
                let (module, kind) = match *symbol {
                    Symbol::Global(id) => {
                        let def = self.defs.get(&id.module_id)?.defs.get(id.def_id)?;
                        let kind = match def.kind {
                            DefKind::Module => Kind::MODULE,
                            DefKind::Function => Kind::FUNCTION,
                            DefKind::Global => Kind::VARIABLE,
                            DefKind::Const => Kind::CONSTANT,
                            DefKind::Struct | DefKind::Union => Kind::STRUCT,
                            DefKind::Enum => Kind::ENUM,
                            DefKind::Trait => Kind::INTERFACE,
                            DefKind::TypeAlias | DefKind::TraitAssociatedType => {
                                Kind::TYPE_PARAMETER
                            }
                            DefKind::StructField
                            | DefKind::UnionField
                            | DefKind::EnumVariantField => Kind::FIELD,
                            DefKind::EnumVariant => Kind::ENUM_MEMBER,
                            DefKind::Method | DefKind::TraitMethod => Kind::METHOD,
                        };
                        (id.module_id, kind)
                    }
                    Symbol::Module(id) => (self.graph.get(id)?.parent?, Kind::MODULE),
                    _ => return None,
                };
                let file = self.files.get(source)?;
                let name = file.text.get(span.start..span.end)?.to_owned();
                let container = self
                    .graph
                    .get(module)?
                    .module_path
                    .segments
                    .iter()
                    .filter_map(|&name| self.db.source_symbol_text(name))
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                Some((source.as_str(), *span, name, kind, container))
            })
    }

    pub(crate) fn workspace_implementations(&self) -> impl Iterator<Item = (Symbol, &str, Span)> {
        self.trait_implementations
            .iter()
            .map(|(id, source, span)| (Symbol::Global(*id), source.as_str(), *span))
            .chain(
                self.trait_members
                    .iter()
                    .filter_map(|(implementation, member)| {
                        let (source, span) =
                            self.definitions.get(&Symbol::Global(*implementation))?;
                        Some((Symbol::Global(*member), source.as_str(), *span))
                    }),
            )
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
        self.files
            .values()
            .flat_map(|file| {
                self.reference_spans(file, symbol, declaration)
                    .into_iter()
                    .map(|span| Location::new(file.uri.clone(), file.lines.range(&file.text, span)))
            })
            .collect()
    }

    pub(crate) fn reference_symbol(&self, path: &Path, position: Position) -> Option<Symbol> {
        let file = self.file(path)?;
        let offset = file.lines.offset(&file.text, position)?;
        self.rename_symbol_at(file, offset)
            .map(|(symbol, _)| symbol)
    }

    pub(crate) fn document_highlights(
        &self,
        path: &Path,
        position: Position,
    ) -> Option<Vec<DocumentHighlight>> {
        let file = self.file(path)?;
        let symbol = self.symbol_at(path, position)?;
        Some(
            self.reference_spans(file, symbol, true)
                .into_iter()
                .map(|span| DocumentHighlight {
                    range: file.lines.range(&file.text, span),
                    kind: None,
                })
                .collect(),
        )
    }

    fn reference_spans(&self, file: &File, symbol: Symbol, declaration: bool) -> Vec<Span> {
        let field = match symbol {
            Symbol::Global(id) => self
                .defs
                .get(&id.module_id)
                .and_then(|defs| defs.defs.get(id.def_id))
                .filter(|def| {
                    matches!(
                        def.kind,
                        DefKind::StructField | DefKind::UnionField | DefKind::EnumVariantField
                    )
                })
                .map(|_| id),
            _ => None,
        };
        let mut spans: Vec<_> = file
            .occurrences
            .iter()
            .filter(|item| item.symbol == symbol && (declaration || !item.declaration))
            .map(|item| item.span)
            .collect();
        if let Some(field) = field {
            spans.extend(
                self.field_uses(file)
                    .iter()
                    .filter_map(|&(span, target)| (target == field).then_some(span)),
            );
        }
        spans.sort_unstable();
        spans.dedup();
        spans
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
            .chain(self.configuration_warning.iter().map(|message| Diagnostic {
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("nia-lsp".to_owned()),
                message: message.clone(),
                ..Diagnostic::default()
            }))
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

fn index_method_calls(
    node: &GreenNode,
    targets: &mut HashMap<Span, GlobalDefId>,
    occurrences: &mut Vec<Occurrence>,
) {
    if targets.is_empty() {
        return;
    }
    if node.kind() == &SyntaxKind::CallExpr
        && let Some(span) = node.token_span()
        && let Some(id) = targets.remove(&span)
        && let Some(name) = method_callee_name(node)
    {
        occurrences.push(Occurrence {
            span: name,
            symbol: Symbol::Global(id),
            declaration: false,
        });
    }
    for child in node.child_nodes() {
        index_method_calls(child, targets, occurrences);
    }
}

fn method_callee_name(call: &GreenNode) -> Option<Span> {
    let mut callee = call.child_nodes().find(|child| child.kind().is_expr())?;
    while callee.kind() == &SyntaxKind::BracketExpr {
        callee = callee.child_nodes().find(|child| child.kind().is_expr())?;
    }
    if matches!(
        callee.kind(),
        SyntaxKind::FieldExpr | SyntaxKind::QualifiedExpr | SyntaxKind::OmittedMemberExpr
    ) {
        return callee.token(&TokenKind::Ident).map(|token| token.span());
    }
    None
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

    pub(super) fn test_session() -> Session {
        let toolchain = ToolchainLayout::resolve(ToolchainLayoutRequest::explicit(
            std::env::current_exe().unwrap(),
            workspace_root().join("lib"),
        ))
        .unwrap();
        Session::new(Arc::new(toolchain))
    }

    #[test]
    fn editor_overlays_keep_toolchain_and_physical_source_in_sync() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let mut session = test_session();
        let package = session.toolchain.std_package();
        let file = package.root_file();
        let physical = normalized(&file);
        let logical = SourcePath::with_identity(physical.clone(), package.root_identity());
        for version in 1..=2 {
            let text = format!("pub const editorVersion: i32 = {version};");
            let documents = Documents::from([(
                file.clone(),
                OpenDocument {
                    uri: uri(&file).unwrap(),
                    document: Document::new(version, text.clone()),
                },
            )]);
            session.install_documents(&documents).unwrap();
            for path in [SourcePath::new(&physical), logical.clone()] {
                assert_eq!(
                    session
                        .driver
                        .sources()
                        .source_for_path(&path)
                        .unwrap()
                        .text
                        .as_ref(),
                    text
                );
            }
        }
    }

    #[test]
    fn rename_tracks_nested_import_bindings_and_unsaved_utf16_positions() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-rename-imports");
        let entry = root.join("root.nia");
        let math = root.join("math.nia");
        let other = root.join("other.nia");
        std::fs::write(&math, "pub fun amount() i32 { 1 }\n").unwrap();
        std::fs::write(&other, "pub fun amount() i32 { 2 }\n").unwrap();
        std::fs::write(&entry, "").unwrap();
        let source = concat!(
            "module math;\n",
            "module other;\n",
            "using root::math as calc;\n",
            "fun run() i32 {\n",
            "    using calc::{amount as picked};\n",
            "    { using root::other::{amount as picked}; _ = picked(); };\n",
            "    _ = \"🦀\"; picked() + calc::amount()\n",
            "}\n",
        );
        let documents = Documents::from([(
            entry.clone(),
            OpenDocument {
                uri: uri(&entry).unwrap(),
                document: Document::new(7, source.into()),
            },
        )]);
        let analysis = test_session().analyze(&documents, &entry).unwrap();
        assert_eq!(analysis.first_error(), None);
        let position = Position::new(6, 15);
        assert_eq!(
            analysis.definition(&entry, position).unwrap().uri,
            uri(&math).unwrap()
        );
        assert_eq!(
            analysis
                .definition(&entry, Position::new(5, 51))
                .unwrap()
                .uri,
            uri(&other).unwrap()
        );
        assert_eq!(
            analysis
                .definition(&entry, Position::new(6, 26))
                .unwrap()
                .range
                .start,
            Position::new(2, 20)
        );
        let rename = |position, new_name| {
            test_session().rename(
                &analysis,
                &documents,
                crate::rename::Rename {
                    path: &entry,
                    position,
                    new_name,
                    folders: &[],
                    versioned: true,
                    resource_rename: false,
                },
                &Cancellation::default(),
            )
        };
        for (position, name, positions) in [
            (
                position,
                "chosen",
                vec![Position::new(4, 27), Position::new(6, 14)],
            ),
            (
                Position::new(6, 26),
                "numbers",
                vec![
                    Position::new(2, 20),
                    Position::new(4, 10),
                    Position::new(6, 25),
                ],
            ),
        ] {
            assert!(analysis.prepare_rename(&entry, position, true).is_some());
            let edit = rename(position, name).unwrap();
            let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
                panic!("expected versioned edits");
            };
            assert_eq!(edits.len(), 1);
            assert_eq!(edits[0].text_document.version, Some(7));
            assert_eq!(
                edits[0]
                    .edits
                    .iter()
                    .map(|edit| match edit {
                        lsp_types::OneOf::Left(edit) => edit.range.start,
                        _ => panic!("expected text edit"),
                    })
                    .collect::<Vec<_>>(),
                positions
            );
        }
        assert!(
            rename(position, "using")
                .unwrap_err()
                .contains("identifier")
        );
        assert_eq!(std::fs::read_to_string(&entry).unwrap(), "");
    }

    #[test]
    fn rename_checks_unopened_workspace_members_and_preserves_explicit_aliases() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-rename-workspace");
        let write = |relative: &str, text: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        };
        write("nia.toml", "[workspace]\nmembers = [\"lib\", \"app\"]\n");
        write(
            "lib/nia.toml",
            "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
        );
        write(
            "app/nia.toml",
            concat!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
                "[[test]]\nname = \"suite\"\npath = \"checks.nia\"\n",
                "[dependencies]\nlib = { path = \"../lib\" }\n",
            ),
        );
        let source = "pub fun amount() i32 { 42 }\npub fun other() i32 { 1 }\n";
        let declaration = write("lib/root.nia", source);
        let test_source =
            "using lib::amount as selected;\nfun check() i32 { selected() + lib::amount() }\n";
        let test = write("app/checks.nia", test_source);
        let documents = Documents::new();
        let analysis = test_session().analyze(&documents, &declaration).unwrap();
        let folders = [root.to_path_buf()];
        let position = Position::new(0, 9);
        let cancel = Cancellation::default();
        let mut session = test_session();
        let canceled = Cancellation::default();
        canceled.cancel();
        assert!(
            session
                .workspace_index(&declaration, &documents, &folders, &canceled)
                .is_err()
        );
        let index = session
            .workspace_index(&declaration, &documents, &folders, &cancel)
            .unwrap();
        let references = index
            .references(&analysis, &declaration, position, false, &cancel)
            .unwrap();
        assert_eq!(
            references
                .iter()
                .map(|location| (location.uri.clone(), location.range.start))
                .collect::<Vec<_>>(),
            [
                (uri(&test).unwrap(), Position::new(0, 11)),
                (uri(&test).unwrap(), Position::new(0, 21)),
                (uri(&test).unwrap(), Position::new(1, 18)),
                (uri(&test).unwrap(), Position::new(1, 36)),
            ]
        );
        let mut rename = |new_name| {
            session.rename(
                &analysis,
                &documents,
                crate::rename::Rename {
                    path: &declaration,
                    position,
                    new_name,
                    folders: &folders,
                    versioned: false,
                    resource_rename: false,
                },
                &cancel,
            )
        };
        let edit = rename("quantity").unwrap();
        let Some(ref changes) = edit.changes else {
            panic!("expected unversioned edits");
        };
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[&uri(&declaration).unwrap()].len(), 1);
        assert_eq!(
            changes[&uri(&test).unwrap()]
                .iter()
                .map(|edit| edit.range.start)
                .collect::<Vec<_>>(),
            [Position::new(0, 11), Position::new(1, 36)]
        );
        assert!(rename("other").is_err());
        let reused = session
            .workspace_index(&declaration, &documents, &folders, &cancel)
            .unwrap();
        assert!(Arc::ptr_eq(&reused, &index));
        let changed = Documents::from([(
            test.clone(),
            OpenDocument {
                uri: uri(&test).unwrap(),
                document: Document::new(2, "fun check() i32 { lib::amount() }\n".into()),
            },
        )]);
        let updated = session
            .workspace_index(&declaration, &changed, &folders, &cancel)
            .unwrap();
        assert!(!Arc::ptr_eq(&updated, &index));
        assert_eq!(
            updated
                .references(&analysis, &declaration, position, false, &cancel)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(std::fs::read_to_string(&test).unwrap(), test_source);
        assert_eq!(std::fs::read_to_string(&declaration).unwrap(), source);
    }

    #[test]
    fn rename_rejects_capture_even_when_the_rewritten_program_compiles() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-rename-capture");
        let entry = root.join("root.nia");
        let source = "fun compute() i32 { let first = 1; { let second = 2; first + second } }\n";
        std::fs::write(&entry, source).unwrap();
        let documents = Documents::new();
        let analysis = test_session().analyze(&documents, &entry).unwrap();
        assert_eq!(analysis.first_error(), None);
        let request = |name| crate::rename::Rename {
            path: &entry,
            position: Position::new(0, 25),
            new_name: name,
            folders: &[],
            versioned: false,
            resource_rename: false,
        };
        let cancel = Cancellation::default();
        let result = test_session().rename(&analysis, &documents, request("second"), &cancel);
        assert!(result.unwrap_err().contains("change name resolution"));
        let edit = test_session()
            .rename(&analysis, &documents, request("value"), &cancel)
            .unwrap();
        assert_eq!(edit.changes.unwrap()[&uri(&entry).unwrap()].len(), 2);
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
    fn workspace_members_resolve_inherited_and_package_local_aliases() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-workspace");
        let write = |relative: &str, text: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        };
        write(
            "nia.toml",
            "[workspace]\nmembers = [\"app\", \"middle\", \"leaf\"]\n\
             [workspace.package]\nversion = \"0.1.0\"\n\
             [workspace.dependencies]\ndep = { path = \"middle\" }\n",
        );
        write(
            "app/nia.toml",
            "[package]\nname = \"app\"\nversion.workspace = true\n\
             [library]\npath = \"root.nia\"\n\
             [[test]]\nname = \"suite\"\npath = \"tests/root.nia\"\n\
             [dependencies]\ndep.workspace = true\n\
             [dev-dependencies]\nfixture = { path = \"../leaf\" }\n",
        );
        write(
            "middle/nia.toml",
            "[package]\nname = \"middle\"\nversion.workspace = true\n\
             [library]\npath = \"root.nia\"\n\
             [dependencies]\ndep = { path = \"../leaf\" }\n",
        );
        write(
            "leaf/nia.toml",
            "[package]\nname = \"leaf\"\nversion.workspace = true\n\
             [library]\npath = \"root.nia\"\n",
        );
        let app_source = "fun run() i32 { dep::amount() }\n";
        let middle_source = "pub fun amount() i32 { dep::leaf_amount() }\n";
        let app = write("app/root.nia", app_source);
        let middle = write("middle/root.nia", middle_source);
        let leaf = write("leaf/root.nia", "pub fun leaf_amount() i32 { 42 }\n");
        write("app/tests/root.nia", "module checks;\n");
        let test_source = "fun check() i32 { fixture::leaf_amount() }\n";
        let test = write("app/tests/checks.nia", test_source);
        let mut documents: Documents = [
            (&app, app_source),
            (&middle, middle_source),
            (&test, test_source),
        ]
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
        let analysis = session.analyze(&documents, &app).unwrap();
        for (source, position, target) in [
            (&app, Position::new(0, 23), &middle),
            (&middle, Position::new(0, 29), &leaf),
        ] {
            let diagnostics = analysis.diagnostics(source, &documents[source].document);
            assert!(diagnostics.is_empty(), "{source:?}: {diagnostics:?}");
            let definition = analysis.definition(source, position).unwrap();
            assert_eq!(definition.uri, uri(target).unwrap());
        }
        let tests = session.analyze(&documents, &test).unwrap();
        assert!(
            tests
                .diagnostics(&test, &documents[&test].document)
                .is_empty()
        );
        assert_eq!(
            tests.definition(&test, Position::new(0, 30)).unwrap().uri,
            uri(&leaf).unwrap()
        );

        documents.get_mut(&middle).unwrap().document =
            Document::new(2, "pub fun amount() bool { true }\n".into());
        let changed = session.analyze(&documents, &app).unwrap();
        assert!(
            !changed
                .diagnostics(&app, &documents[&app].document)
                .is_empty()
        );
        documents.remove(&middle);
        let restored = session.analyze(&documents, &app).unwrap();
        assert!(
            restored
                .diagnostics(&app, &documents[&app].document)
                .is_empty()
        );
        assert!(!root.join("nia.lock").exists());
        assert!(!root.join("build").exists());
    }

    #[test]
    fn workspace_symbols_search_unopened_packages_and_use_active_buffer_declarations() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-symbols");
        std::fs::write(root.join("nia.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
        let app = root.join("app");
        std::fs::create_dir(&app).unwrap();
        std::fs::write(
            app.join("nia.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
        )
        .unwrap();
        std::fs::write(app.join("root.nia"), "module types;\n").unwrap();
        let path = app.join("types.nia");
        std::fs::write(
            &path,
            "pub struct DiskValue {}\n@[release] pub struct Hidden {}\n",
        )
        .unwrap();
        let mut session = test_session();
        let cancel = Cancellation::default();
        let folders = [root.to_path_buf()];
        let index = session
            .index_workspace(None, &Documents::new(), &folders, &cancel)
            .unwrap();
        let symbols = index.symbols("APP::types::DiskValue", &cancel).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].location.uri, uri(&path).unwrap());
        assert_eq!(symbols[0].kind, lsp_types::SymbolKind::STRUCT);
        assert!(index.symbols("Hidden", &cancel).unwrap().is_empty());

        let documents = Documents::from([(
            path.clone(),
            OpenDocument {
                uri: uri(&path).unwrap(),
                document: Document::new(1, "pub struct BufferValue {}\n".into()),
            },
        )]);
        let index = session
            .index_workspace(None, &documents, &folders, &cancel)
            .unwrap();
        assert!(index.symbols("DiskValue", &cancel).unwrap().is_empty());
        assert_eq!(
            index.symbols("BufferValue", &cancel).unwrap()[0]
                .location
                .uri,
            uri(&path).unwrap()
        );
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
        let target = analysis.definition(&main, Position::new(3, 25)).unwrap();
        assert_eq!(target.uri, uri(&child).unwrap());
        assert_eq!(target.range.start, Position::new(0, 11));
        let references = analysis.references(&child, target.range.start, true);
        assert_eq!(references.len(), 4);
        for column in [20, 29] {
            assert_eq!(
                analysis.definition(&main, Position::new(1, column)),
                Some(target.clone())
            );
        }
        assert!(
            references
                .iter()
                .any(|reference| reference.uri == uri(&main).unwrap()
                    && reference.range.start == Position::new(3, 22))
        );
    }

    #[test]
    fn method_references_use_the_callee_token_even_when_the_receiver_has_the_same_name() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let entry = test_dir("lsp-method-references").join("main.nia");
        let source = concat!(
            "struct Counter { value: i32 }\n",
            "extend Counter { fun read[T](&self, value: T) T { value } }\n",
            "fun main(read: Counter) i32 { read.read[i32](read.value) }\n",
        );
        let documents: Documents = [(
            entry.clone(),
            OpenDocument {
                uri: uri(&entry).unwrap(),
                document: Document::new(1, source.into()),
            },
        )]
        .into_iter()
        .collect();
        let analysis = test_session().analyze(&documents, &entry).unwrap();
        assert!(
            analysis
                .diagnostics(&entry, &documents[&entry].document)
                .is_empty()
        );
        let method = Position::new(2, 35);
        let target = analysis.definition(&entry, method).unwrap();
        assert_eq!(target.range.start, Position::new(1, 21));
        assert_eq!(
            analysis
                .definition(&entry, Position::new(2, 30))
                .unwrap()
                .range
                .start,
            Position::new(2, 9)
        );
        let highlights = analysis.document_highlights(&entry, method).unwrap();
        assert_eq!(
            highlights
                .iter()
                .map(|highlight| highlight.range.start)
                .collect::<Vec<_>>(),
            [Position::new(1, 21), Position::new(2, 35)]
        );
    }

    #[test]
    fn receiver_fields_complete_in_unsaved_buffers_and_link_to_the_right_declaration() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-fields");
        let main = root.join("main.nia");
        let types = root.join("shapes.nia");
        std::fs::write(
            &types,
            "pub struct Point { width: i32, weight: i32 }\npub struct Other { width: i32 }\n",
        )
        .unwrap();
        let source = concat!(
            "module shapes;\n",
            "fun measure(p: &shapes::Point, other: shapes::Other) i32 {\n",
            "    _ = other.width;\n",
            "    p.width\n",
            "}\n",
        );
        std::fs::write(&main, source).unwrap();
        let mut documents = [(
            main.clone(),
            OpenDocument {
                uri: uri(&main).unwrap(),
                document: Document::new(1, source.into()),
            },
        )]
        .into_iter()
        .collect::<Documents>();
        let mut session = test_session();
        let mut probe = test_session();
        let checked = session.analyze(&documents, &main).unwrap();
        assert!(
            checked
                .diagnostics(&main, &documents[&main].document)
                .is_empty()
        );
        let target = checked.definition(&main, Position::new(3, 8)).unwrap();
        assert_eq!(target.uri, uri(&types).unwrap());
        assert_eq!(target.range.start, Position::new(0, 19));
        for position in [Position::new(1, 12), Position::new(3, 4)] {
            let target = checked.type_definition(&main, position).unwrap();
            assert_eq!(target.uri, uri(&types).unwrap());
            assert_eq!(target.range.start, Position::new(0, 11));
        }
        assert!(
            checked
                .type_definition(&main, Position::new(3, 8))
                .is_none()
        );
        let references = checked.references(&main, Position::new(3, 8), true);
        assert_eq!(references.len(), 2);
        assert!(references.contains(&target));
        assert!(
            references
                .iter()
                .any(|reference| reference.range.start == Position::new(3, 6))
        );

        for (version, typed, cursor) in [(2, "", 6), (3, "weXX", 8)] {
            let text = source.replace("p.width", &format!("p.{typed}"));
            documents.get_mut(&main).unwrap().document = Document::new(version, text);
            let checked = session.analyze(&documents, &main).unwrap();
            let completion = probe
                .complete(&checked, &documents, &main, Position::new(3, cursor))
                .unwrap();
            let labels: Vec<_> = completion.iter().map(|item| item.label.as_str()).collect();
            let expected: &[&str] = if typed.is_empty() {
                &["weight", "width"]
            } else {
                &["weight"]
            };
            assert_eq!(labels, expected, "{typed:?}");
            let Some(lsp_types::CompletionTextEdit::Edit(edit)) = &completion[0].text_edit else {
                panic!("expected replacement edit");
            };
            assert_eq!(edit.range.start, Position::new(3, 6));
            assert_eq!(edit.range.end, Position::new(3, 6 + typed.len() as u32));
        }
        assert_eq!(std::fs::read_to_string(&main).unwrap(), source);
    }

    #[test]
    fn receiver_fields_complete_after_an_inferred_projection() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-field-projection");
        let main = root.join("main.nia");
        let mut session = test_session();
        let mut probe = test_session();
        for (version, receiver, expected) in [
            (1, "shape.point", &["width"][..]),
            (2, "shape.pair", &["0", "1"][..]),
        ] {
            let source = format!(
                "struct Point {{ width: i32 }}\nstruct Shape {{ point: Point, pair: (i32, i32) }}\n\
                 fun inspect(shape: Shape) {{ _ = {receiver}.; }}\n"
            );
            let column = source.lines().nth(2).unwrap().find(".;").unwrap() as u32 + 1;
            let documents = [(
                main.clone(),
                OpenDocument {
                    uri: uri(&main).unwrap(),
                    document: Document::new(version, source),
                },
            )]
            .into_iter()
            .collect::<Documents>();
            let analysis = session.analyze(&documents, &main).unwrap();
            let items = probe
                .complete(&analysis, &documents, &main, Position::new(2, column))
                .unwrap();
            assert_eq!(
                items
                    .iter()
                    .map(|item| item.label.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn receiver_methods_follow_visibility_mutability_and_trait_bounds() {
        let _resources = acquire_test_resources(TestWorkload::Compiler);
        let root = test_dir("lsp-receiver-methods");
        let main = root.join("main.nia");
        std::fs::write(
            root.join("counters.nia"),
            concat!(
                "pub struct Counter { value: i32 }\n",
                "pub trait Scored { fun score(&self) i32; }\n",
                "extend Counter : Scored { fun score(&self) i32 { self.value } }\n",
                "extend Counter {\n",
                "    pub fun read(&self, extra: i32) i32 { self.value + extra }\n",
                "    pub fun reset(&mut self) { self.value = 0; }\n",
                "    pub fun create() i32 { 0 }\n",
                "    fun hidden(&self) i32 { self.value }\n",
                "}\n",
                "pub struct Other {}\n",
                "extend Other { pub fun unrelated(&self) {} }\n",
            ),
        )
        .unwrap();
        let mut session = test_session();
        let mut probe = test_session();
        for (line, suffix, prefix, expected) in [
            (
                "fun inspect(value: &counters::Counter) { _ = value.",
                "",
                0,
                &[
                    ("read", CompletionItemKind::METHOD),
                    ("score", CompletionItemKind::METHOD),
                    ("value", CompletionItemKind::FIELD),
                ][..],
            ),
            (
                "fun inspect(value: &mut counters::Counter) { _ = value.",
                "reXX",
                2,
                &[
                    ("read", CompletionItemKind::METHOD),
                    ("reset", CompletionItemKind::METHOD),
                ][..],
            ),
            (
                "fun inspect[T](value: T) where T: counters::Scored { _ = value.",
                "",
                0,
                &[("score", CompletionItemKind::METHOD)][..],
            ),
            ("fun inspect[T](value: T) { _ = value.", "", 0, &[][..]),
        ] {
            let source = format!("module counters;\n{line}{suffix}; }}\n");
            let documents = [(
                main.clone(),
                OpenDocument {
                    uri: uri(&main).unwrap(),
                    document: Document::new(1, source),
                },
            )]
            .into_iter()
            .collect();
            let analysis = session.analyze(&documents, &main).unwrap();
            let items = probe
                .complete(
                    &analysis,
                    &documents,
                    &main,
                    Position::new(1, line.len() as u32 + prefix),
                )
                .unwrap();
            assert_eq!(
                items
                    .iter()
                    .map(|item| (item.label.as_str(), item.kind.unwrap()))
                    .collect::<Vec<_>>(),
                expected,
                "{line}{suffix}"
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
