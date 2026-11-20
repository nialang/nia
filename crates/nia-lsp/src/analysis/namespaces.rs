// SPDX-License-Identifier: GPL-3.0-or-later
//! Source locations for module path segments and exported aliases.

use nia_defs::PublicSource;
use nia_syntax::GreenToken;

use super::*;

type ExportAliases = HashMap<(ModuleId, SymbolId, GlobalDefId), Symbol>;

impl Analysis {
    pub(super) fn index_namespaces(
        &mut self,
        imports: &HashMap<ModuleId, LexicalUsingScopes>,
        checkpoint: &impl Fn() -> nia_query::QueryResult<()>,
    ) -> nia_query::QueryResult<()> {
        let mut surfaces = HashMap::new();
        let mut scopes = HashMap::new();
        for (&module, defs) in &self.defs {
            checkpoint()?;
            if let Some(surface) = self.db.source_exports(module)? {
                surfaces.insert(module, surface);
                if !defs.module_usings.is_empty() {
                    scopes.insert(module, self.db.source_imports(module, 0)?);
                }
            }
        }
        let mut exported = ExportAliases::new();
        let mut first = true;
        loop {
            let mut updates = Vec::new();
            for (source, file) in &self.files {
                checkpoint()?;
                if !file.text.contains("::") && !file.text.contains("using") {
                    continue;
                }
                let mut index = Namespaces {
                    analysis: self,
                    surfaces: &surfaces,
                    module_scopes: &scopes,
                    imports: imports.get(&file.module),
                    exported: &exported,
                    file,
                    modules: Vec::new(),
                    aliases: HashMap::new(),
                    exports: HashMap::new(),
                };
                index.visit(file.syntax().tree.green_root(), false);
                updates.push((source.clone(), index.modules, index.aliases, index.exports));
            }
            let mut changed = false;
            for (source, modules, aliases, exports) in updates {
                checkpoint()?;
                let file = self.files.get_mut(&source).expect("indexed source");
                if first {
                    file.occurrences.extend(modules);
                    file.occurrences
                        .sort_by_key(|occurrence| (occurrence.span, !occurrence.declaration));
                } else {
                    for module in modules {
                        if let Err(index) = file
                            .occurrences
                            .binary_search_by_key(&module.span, |occurrence| occurrence.span)
                        {
                            file.occurrences.insert(index, module);
                        }
                    }
                }
                for (span, alias) in aliases {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        file.rename_aliases.entry(span)
                    {
                        entry.insert(alias);
                        changed = true;
                    }
                }
                for (key, alias) in exports {
                    if let std::collections::hash_map::Entry::Vacant(entry) = exported.entry(key) {
                        entry.insert(alias);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
            first = false;
        }
        Ok(())
    }
}

struct Namespaces<'a> {
    analysis: &'a Analysis,
    surfaces: &'a HashMap<ModuleId, Arc<ModulePublicSurface>>,
    module_scopes: &'a HashMap<ModuleId, ModuleUsingScope>,
    imports: Option<&'a LexicalUsingScopes>,
    exported: &'a ExportAliases,
    file: &'a File,
    modules: Vec<Occurrence>,
    aliases: HashMap<Span, Symbol>,
    exports: ExportAliases,
}

impl Namespaces<'_> {
    fn visit(&mut self, node: &GreenNode, path_prefix: bool) {
        match node.kind() {
            SyntaxKind::UsingTree => {
                self.using_tree(node, None);
                return;
            }
            SyntaxKind::QualifiedExpr if !path_prefix => {
                let mut path = Vec::new();
                expression_path(node, &mut path);
                self.path(path, None, false, false);
            }
            SyntaxKind::PathType => {
                let path = node
                    .child_nodes()
                    .filter(|segment| segment.kind() == &SyntaxKind::PathSegment)
                    .filter_map(|segment| segment.child_tokens().find(|token| path_token(token)))
                    .collect::<Vec<_>>();
                if path.len() > 1 {
                    self.path(path, None, false, false);
                }
            }
            _ => {}
        }
        for child in node.child_nodes() {
            let prefix = matches!(
                node.kind(),
                SyntaxKind::QualifiedExpr | SyntaxKind::BracketExpr
            ) && matches!(
                child.kind(),
                SyntaxKind::QualifiedExpr | SyntaxKind::BracketExpr
            );
            self.visit(child, prefix);
        }
    }

    fn using_tree(&mut self, node: &GreenNode, owner: Option<ModuleId>) {
        let path = node
            .child_tokens()
            .filter(|token| path_token(token))
            .collect::<Vec<_>>();
        let has_path = !path.is_empty();
        let host = self.path(path, owner, true, true);
        if has_path && host.is_none() {
            return;
        }
        if let (Some(host), Some(star)) = (host, node.token(&TokenKind::Star)) {
            self.inherit_imports(host, star.span(), None);
        }
        if let Some(name) = node.child(&SyntaxKind::UsingName) {
            self.selector(name, host);
        }
        if let Some(group) = node.child(&SyntaxKind::UsingGroup) {
            for child in group.child_nodes() {
                match child.kind() {
                    SyntaxKind::UsingTree => self.using_tree(child, host),
                    SyntaxKind::UsingName => self.selector(child, host),
                    _ => {}
                }
            }
        }
    }

    fn selector(&mut self, node: &GreenNode, host: Option<ModuleId>) {
        if let (Some(name), Some(host)) = (node.token(&TokenKind::Ident), host) {
            self.path(vec![name], Some(host), true, false);
            if node.token(&TokenKind::As).is_none()
                && let Some(alias) = self.aliases.get(&name.span()).copied()
                && matches!(alias, Symbol::ItemAlias(..))
            {
                self.inherit_imports(host, name.span(), Some((symbol(name), alias)));
            }
        }
    }

    fn inherit_imports(
        &mut self,
        host: ModuleId,
        selector: Span,
        explicit: Option<(SymbolId, Symbol)>,
    ) {
        let Some(imports) = self.imports else { return };
        let alias_for = |entry: &nia_defs::UsingEntry, name| {
            if let Some((wanted, alias)) = explicit {
                return (wanted == name && entry.name_span == selector).then_some(alias);
            }
            if !(entry.directive_span.start <= selector.start
                && selector.end <= entry.directive_span.end)
            {
                return None;
            }
            let target = GlobalDefId {
                module_id: entry.target_module,
                def_id: entry.target_def_id,
            };
            self.export_alias(host, name, target)
        };
        let scope = imports.effective_scope(selector);
        let public = self
            .analysis
            .defs
            .get(&self.file.module)
            .is_some_and(|defs| {
                defs.module_usings.iter().any(|using| {
                    using.visibility != nia_ids::Visibility::Private
                        && using.span.start <= selector.start
                        && selector.end <= using.span.end
                })
            });
        let exports = public.then(|| {
            scope
                .values
                .iter()
                .chain(scope.types.iter())
                .filter_map(|(name, entry)| {
                    let target = GlobalDefId {
                        module_id: entry.target_module,
                        def_id: entry.target_def_id,
                    };
                    Some(((self.file.module, *name, target), alias_for(entry, *name)?))
                })
                .collect::<Vec<_>>()
        });
        let mut aliases = Vec::new();
        for occurrence in &self.file.occurrences {
            if occurrence.declaration
                || occurrence.span == selector
                || self.file.rename_aliases.contains_key(&occurrence.span)
                || self.file.text[..occurrence.span.start]
                    .trim_end()
                    .ends_with("::")
            {
                continue;
            }
            let Symbol::Global(id) = occurrence.symbol else {
                continue;
            };
            let name = SymbolId::from_stable_hash(stable_hash(
                &self.file.text[occurrence.span.start..occurrence.span.end],
            ));
            let scope = imports.effective_scope(occurrence.span);
            if let Some(alias) = [&scope.values, &scope.types].into_iter().find_map(|table| {
                let entry = table.get(&name)?;
                (entry.target_module == id.module_id && entry.target_def_id == id.def_id)
                    .then(|| alias_for(entry, name))
                    .flatten()
            }) {
                aliases.push((occurrence.span, alias));
            }
        }
        self.aliases.extend(aliases);
        self.exports.extend(exports.into_iter().flatten());
    }

    fn path(
        &mut self,
        path: Vec<&GreenToken>,
        owner: Option<ModuleId>,
        using: bool,
        host: bool,
    ) -> Option<ModuleId> {
        let Some((first, rest)) = path.split_first() else {
            return owner;
        };
        if !host
            && rest.is_empty()
            && let Some(owner) = owner
        {
            self.item_alias(owner, first);
        }
        let mut current = match owner {
            Some(owner) => self.child(owner, first, using)?,
            None => self.root(first, using)?,
        };
        for (index, token) in rest.iter().enumerate() {
            if !host && index + 1 == rest.len() {
                self.item_alias(current, token);
            }
            current = self.child(current, token, using)?;
        }
        Some(current)
    }

    fn root(&mut self, token: &GreenToken, using: bool) -> Option<ModuleId> {
        let name = symbol(token);
        let span = token.span();
        let segment = if token.is(&TokenKind::Root) {
            ModuleRootSegment::Root
        } else if token.is(&TokenKind::Super) {
            ModuleRootSegment::Parent
        } else if token.is(&TokenKind::SelfValue) {
            ModuleRootSegment::Current
        } else {
            ModuleRootSegment::Named(name)
        };
        let direct =
            self.analysis
                .direct_namespace_root(self.file.module, segment, self.imports, span);
        let from_import = self.imports.and_then(|imports| {
            let scope = imports.effective_scope(span);
            let target = scope.modules.get(&name).copied()?;
            let origin = scope.module_origins.get(&name).map(|&(_, span)| span);
            Some((target, origin))
        });
        if matches!(segment, ModuleRootSegment::Named(_))
            && direct.is_none()
            && let Some((target, origin)) = from_import
        {
            let explicit = origin.map(|origin| Symbol::ModuleAlias(self.file.module, origin));
            if explicit.is_none_or(|alias| !self.analysis.definitions.contains_key(&alias)) {
                let inherited = origin.and_then(|origin| {
                    self.aliases
                        .get(&origin)
                        .or_else(|| self.file.rename_aliases.get(&origin))
                        .copied()
                });
                self.record_module(span, target, inherited);
            }
            return Some(target);
        }
        let target = direct?;
        if matches!(segment, ModuleRootSegment::Named(_))
            && self
                .analysis
                .graph
                .get(self.file.module)?
                .children
                .get(&name)
                == Some(&target)
        {
            self.record_module(span, target, None);
        }
        // A block using can refer to an earlier binding than the final block scope.
        if using
            && let Some(binding) = self
                .imports
                .and_then(|scopes| scopes.module_binding_in_directive(span))
        {
            let alias = Symbol::ModuleAlias(self.file.module, binding);
            if self.analysis.definitions.contains_key(&alias) {
                return from_import.map(|(module, _)| module);
            }
        }
        Some(target)
    }

    fn child(&mut self, owner: ModuleId, token: &GreenToken, using: bool) -> Option<ModuleId> {
        let name = symbol(token);
        let surface = self.surfaces.get(&owner);
        let target = self
            .analysis
            .visible_child(self.file.module, owner, name, surface, using)?;
        if !surface.is_some_and(|surface| surface.reexported_modules.get(&name) == Some(&target)) {
            self.record_module(token.span(), target, None);
            return Some(target);
        }
        let alias = self
            .module_scopes
            .get(&owner)
            .and_then(|scope| scope.module_origins.get(&name))
            .and_then(|&(_, origin)| {
                let alias = Symbol::ModuleAlias(owner, origin);
                if self.analysis.definitions.contains_key(&alias) {
                    Some(alias)
                } else {
                    let path = self.analysis.graph.get(owner)?.path.as_str();
                    self.analysis
                        .files
                        .get(path)?
                        .rename_aliases
                        .get(&origin)
                        .copied()
                }
            });
        self.record_module(token.span(), target, alias);
        Some(target)
    }

    fn record_module(&mut self, span: Span, module: ModuleId, alias: Option<Symbol>) {
        let symbol = Symbol::Module(module);
        if !self.analysis.definitions.contains_key(&symbol)
            || self.file.occurrences.iter().any(|occurrence| {
                occurrence.span == span && matches!(occurrence.symbol, Symbol::ModuleAlias(..))
            })
        {
            return;
        }
        self.modules.push(Occurrence {
            span,
            symbol,
            declaration: false,
        });
        if let Some(alias) = alias {
            self.aliases.insert(span, alias);
        }
    }

    fn item_alias(&mut self, module: ModuleId, token: &GreenToken) {
        let span = token.span();
        let name = symbol(token);
        let Some(surface) = self.surfaces.get(&module) else {
            return;
        };
        for item in [surface.lookup_value(&name), surface.lookup_type(&name)]
            .into_iter()
            .flatten()
        {
            let target = GlobalDefId {
                module_id: item.target_module,
                def_id: item.target_def_id,
            };
            if let Some(alias) = self.export_alias(module, name, target)
                && self.file.occurrences.iter().any(|item| {
                    item.span == span && item.symbol == Symbol::Global(target) && !item.declaration
                })
            {
                self.aliases.insert(span, alias);
            }
        }
    }

    fn export_alias(
        &self,
        module: ModuleId,
        name: SymbolId,
        target: GlobalDefId,
    ) -> Option<Symbol> {
        let surface = self.surfaces.get(&module)?;
        let alias = [surface.lookup_value(&name), surface.lookup_type(&name)]
            .into_iter()
            .flatten()
            .filter(|item| {
                item.target_module == target.module_id && item.target_def_id == target.def_id
            })
            .filter_map(|item| {
                let PublicSource::PubUsing { directive_span } = item.source else {
                    return None;
                };
                let alias = Symbol::ItemAlias(module, item.name_span);
                (directive_span.start <= item.name_span.start
                    && item.name_span.end <= directive_span.end
                    && self.analysis.definitions.contains_key(&alias))
                .then_some(alias)
            })
            .next();
        alias.or_else(|| self.exported.get(&(module, name, target)).copied())
    }
}

fn path_token(token: &GreenToken) -> bool {
    [
        TokenKind::Ident,
        TokenKind::Root,
        TokenKind::Super,
        TokenKind::SelfValue,
    ]
    .iter()
    .any(|kind| token.is(kind))
}

fn symbol(token: &GreenToken) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(token.text()))
}

fn expression_path<'a>(node: &'a GreenNode, path: &mut Vec<&'a GreenToken>) {
    match node.kind() {
        SyntaxKind::NameExpr | SyntaxKind::QualifiedExpr | SyntaxKind::BracketExpr => {
            if let Some(prefix) = node.child_nodes().find(|child| child.kind().is_expr()) {
                expression_path(prefix, path);
            }
            if node.kind() != &SyntaxKind::BracketExpr
                && let Some(token) = node.child_tokens().find(|token| path_token(token))
            {
                path.push(token);
            }
        }
        _ => {}
    }
}
