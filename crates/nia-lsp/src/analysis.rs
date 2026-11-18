// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use lsp_types::{
    CompletionItemKind, Diagnostic, DiagnosticSeverity, Location, NumberOrString, Position, Uri,
};
use nia_compiler_query::{CheckedProgramAnalysis, CompilerDatabase};
use nia_defs::{DefCollection, DefKind, ModuleUsingScope};
use nia_diagnostic::LabelStyle;
use nia_driver::{CheckRequest, Driver};
use nia_ids::{GlobalDefId, InternedTyId, LocalId, ModuleId};
use nia_imports::{ModuleGraphSnapshot, ModuleRootSegment};
use nia_lexer::{LosslessToken, LosslessTokenKind, TokenKind, tokenize_lossless};
use nia_sema_ir::{ResolvedCall, SemanticValueUse};
use nia_span::Span;
use nia_symbol::{SymbolId, stable_hash};
use nia_toolchain::ToolchainLayout;
use nia_ty::{TyKind, TypeStore};
use nia_type_resolve::TypeNameResolution;
use url::Url;

use crate::document::{Document, Lines};
use crate::editing::{MemberCompletion, field_accesses};
use crate::highlight::{Class, Kind, Resolved};
use crate::project::Projects;

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

pub(crate) fn normalized(path: &Path) -> String {
    nia_source::normalize_path(&path.to_string_lossy())
}

pub(crate) struct Session {
    toolchain: Arc<ToolchainLayout>,
    driver: Driver,
    overlays: BTreeMap<PathBuf, Arc<str>>,
    projects: Projects,
}

impl Session {
    pub(crate) fn new(toolchain: Arc<ToolchainLayout>) -> Self {
        Self {
            driver: Driver::new(toolchain.clone()),
            toolchain,
            overlays: BTreeMap::new(),
            projects: Projects::default(),
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
        let analyze = |request| {
            self.driver
                .analyze_for_editor(request, |db, program| {
                    Analysis::from_program(&self.driver, db, program)
                })
                .map_err(|error| error.to_string())
        };
        let Some(package) = self.projects.package(entry)? else {
            return analyze(CheckRequest::new(normalized(entry)));
        };
        let entries = package.entries(entry);
        let entry_path = normalized(entry);
        let mut entries = entries.into_iter().peekable();
        while let Some((path, context)) = entries.next() {
            let analysis = package.request(&path, context).and_then(analyze);
            if entries.peek().is_none()
                || analysis
                    .as_ref()
                    .is_ok_and(|analysis| analysis.files.contains_key(&entry_path))
            {
                return analysis;
            }
        }
        unreachable!("editor requests always include the current source")
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
            self.analyze(&overlay, path)
                .ok()
                .and_then(|checked| member.candidates(&checked, checked.file(path)?, &names))
                .or(fields)
        };
        Some(member.items(&file.text, &file.lines, candidates.unwrap_or_default()))
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
    expr_types: HashMap<Span, InternedTyId>,
    syntax: OnceLock<Option<nia_syntax::Parse>>,
    field_uses: OnceLock<Vec<(Span, GlobalDefId)>>,
}

impl File {
    pub(crate) fn syntax(&self) -> Option<&nia_syntax::Parse> {
        self.syntax
            .get_or_init(|| nia_syntax::parse(&self.text, None).ok())
            .as_ref()
    }
}

pub(crate) struct Analysis {
    files: BTreeMap<String, File>,
    definitions: HashMap<Symbol, (String, Span)>,
    diagnostics: Vec<nia_compiler_query::ProgramDiagnostic>,
    db: CompilerDatabase,
    graph: ModuleGraphSnapshot,
    defs: HashMap<ModuleId, Arc<DefCollection>>,
    types: Arc<TypeStore>,
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
            types: db.type_store(),
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
            let mut expr_types = HashMap::new();
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
                    if let Some(ty) = module.semantic_facts.node_expr_type(&key)
                        && ty != result.types.error()
                    {
                        expr_types.insert(span, module.type_normalization.normalize(ty));
                    }
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
                    expr_types,
                    syntax: OnceLock::new(),
                    field_uses: OnceLock::new(),
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
            if file.expr_types.is_empty() || !file.text.contains('.') {
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
            }
            uses
        })
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
        let field = match symbol {
            Symbol::Global(id) => self
                .defs
                .get(&id.module_id)
                .and_then(|defs| defs.defs.get(id.def_id))
                .filter(|def| matches!(def.kind, DefKind::StructField | DefKind::UnionField))
                .map(|_| id),
            _ => None,
        };
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
            if let Some(field) = field {
                for &(span, target) in self.field_uses(file) {
                    if target == field {
                        locations.push(Location::new(
                            file.uri.clone(),
                            file.lines.range(&file.text, span),
                        ));
                    }
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
