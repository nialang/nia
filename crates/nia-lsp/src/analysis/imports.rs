// SPDX-License-Identifier: GPL-3.0-or-later
//! Source occurrences backed by the compiler's import scopes.

use super::*;
use nia_compiler_query::LexicalUsingScopes;
use nia_defs::{UsingGroupItem, UsingName, UsingScopeLookup, UsingSelector};
use std::collections::HashSet;

fn visit_using_names(selector: &UsingSelector, visit: &mut impl FnMut(&UsingName)) {
    match selector {
        UsingSelector::Single(name) => visit(name),
        UsingSelector::Group(items) => {
            for item in items {
                match item {
                    UsingGroupItem::Name(name) => visit(name),
                    UsingGroupItem::Nested { selector, .. } => visit_using_names(selector, visit),
                }
            }
        }
        UsingSelector::Wildcard { .. } | UsingSelector::SelfName => {}
    }
}

fn index_using_name(
    name: &UsingName,
    directive: Span,
    imports: &ModuleUsingScope,
    occurrences: &mut Vec<Occurrence>,
) {
    let alias = name.alias.unwrap_or(name.name);
    let binding_span = name.alias_span.unwrap_or(name.name_span);
    let Some(target) = using_target(alias, directive, binding_span, imports) else {
        return;
    };
    for span in std::iter::once(name.name_span).chain(name.alias_span) {
        occurrences.push(Occurrence {
            span,
            symbol: Symbol::Global(target),
            declaration: false,
        });
    }
}

fn using_target(
    alias: SymbolId,
    directive: Span,
    binding_span: Span,
    imports: &ModuleUsingScope,
) -> Option<GlobalDefId> {
    let mut targets = [&imports.types, &imports.values]
        .into_iter()
        .filter_map(|items| {
            let item = items.get(&alias)?;
            (item.directive_span == directive && item.name_span == binding_span).then_some(
                GlobalDefId {
                    module_id: item.target_module,
                    def_id: item.target_def_id,
                },
            )
        });
    let target = targets.next()?;
    // One selector can introduce names in multiple namespaces.
    if targets.any(|other| other != target) {
        return None;
    }
    Some(target)
}

pub(super) struct ImportOccurrences<'a> {
    pub(super) graph: &'a ModuleGraphSnapshot,
    pub(super) module: ModuleId,
    pub(super) text: &'a str,
    pub(super) tokens: &'a [LosslessToken],
    pub(super) occurrences: &'a mut Vec<Occurrence>,
    pub(super) aliases: &'a mut HashMap<Span, Symbol>,
    pub(super) bindings: HashMap<Span, Symbol>,
}

#[derive(Clone, Copy)]
enum PathContext {
    Reference,
    ModuleUsing,
    BlockUsing,
}

impl ImportOccurrences<'_> {
    pub(super) fn index(
        &mut self,
        root: &GreenNode,
        defs: &DefCollection,
        scopes: &LexicalUsingScopes,
    ) {
        let mut paths = Vec::new();
        let mut selector_names = HashSet::new();
        self.collect(root, None, scopes, &mut paths, &mut selector_names);
        if !defs.module_usings.is_empty() {
            let scope = scopes.effective_scope(Span::default());
            for directive in &defs.module_usings {
                visit_using_names(&directive.selector, &mut |name| {
                    selector_names.insert(name.name_span);
                    self.selector(name, directive.span, scope);
                });
            }
        }
        let alias_names: HashSet<_> = self.bindings.keys().map(|span| self.name(*span)).collect();
        for (span, context) in paths {
            let name = self.name(span);
            if !alias_names.contains(&name) {
                continue;
            }
            let scope = scopes.effective_scope(span);
            let direct = self
                .graph
                .root_module_for_segment(self.module, ModuleRootSegment::Named(name));
            let binding = match context {
                PathContext::BlockUsing => scopes.module_binding_in_directive(span),
                PathContext::Reference
                    if direct.is_none() || scopes.shadows_module_namespace(&name, span) =>
                {
                    scope.module_origins.get(&name).map(|&(_, binding)| binding)
                }
                PathContext::ModuleUsing if direct.is_none() => {
                    scope.module_origins.get(&name).map(|&(_, binding)| binding)
                }
                _ => None,
            };
            if let Some(binding) = binding
                && let Some(&symbol @ Symbol::ModuleAlias(..)) = self.bindings.get(&binding)
            {
                self.occurrences.push(Occurrence {
                    span,
                    symbol,
                    declaration: false,
                });
            }
        }
        let mut aliases = Vec::new();
        for occurrence in &*self.occurrences {
            let Symbol::Global(target) = occurrence.symbol else {
                continue;
            };
            let span = occurrence.span;
            let name = self.name(span);
            if occurrence.declaration
                || !alias_names.contains(&name)
                || selector_names.contains(&span)
                || self.aliases.contains_key(&span)
                || self.is_qualified(span)
            {
                continue;
            }
            let scope = scopes.effective_scope(span);
            for table in [&scope.values, &scope.types] {
                if let Some(entry) = table.get(&name)
                    && entry.target_module == target.module_id
                    && entry.target_def_id == target.def_id
                    && let Some(&symbol @ Symbol::ItemAlias(..)) =
                        self.bindings.get(&entry.name_span)
                {
                    aliases.push((span, symbol));
                }
            }
        }
        self.aliases.extend(aliases);
    }

    fn name(&self, span: Span) -> SymbolId {
        SymbolId::from_stable_hash(stable_hash(&self.text[span.start..span.end]))
    }

    fn is_qualified(&self, span: Span) -> bool {
        let index = self
            .tokens
            .partition_point(|token| token.span.start < span.start);
        self.tokens[..index]
            .iter()
            .rev()
            .find_map(|token| match &token.kind {
                LosslessTokenKind::Token(kind) => Some(kind),
                _ => None,
            })
            == Some(&TokenKind::ColonColon)
    }

    fn selector(&mut self, name: &UsingName, directive: Span, scope: &ModuleUsingScope) {
        index_using_name(name, directive, scope, self.occurrences);
        let Some(binding) = name.alias_span else {
            return;
        };
        let alias = name.alias.unwrap_or_else(|| self.name(binding));
        if scope.module_origins.get(&alias) == Some(&(directive, binding)) {
            let symbol = Symbol::ModuleAlias(self.module, binding);
            self.bindings.insert(binding, symbol);
            self.occurrences.push(Occurrence {
                span: binding,
                symbol,
                declaration: true,
            });
        } else if using_target(alias, directive, binding, scope).is_some() {
            let symbol = Symbol::ItemAlias(self.module, binding);
            self.bindings.insert(binding, symbol);
            self.aliases.insert(binding, symbol);
        }
    }

    fn collect(
        &mut self,
        node: &GreenNode,
        parent: Option<&SyntaxKind>,
        scopes: &LexicalUsingScopes,
        paths: &mut Vec<(Span, PathContext)>,
        selector_names: &mut HashSet<Span>,
    ) {
        let kind = node.kind();
        if kind == &SyntaxKind::UsingStmt
            && let Some(directive) = node.token_span()
        {
            self.block_selectors(
                node,
                directive,
                scopes.effective_scope(directive),
                selector_names,
            );
        }
        let path_root = match kind {
            SyntaxKind::PathType
                if node
                    .child_nodes()
                    .filter(|child| child.kind() == &SyntaxKind::PathSegment)
                    .count()
                    > 1 =>
            {
                node.child(&SyntaxKind::PathSegment)
                    .and_then(|segment| segment.token(&TokenKind::Ident))
            }
            SyntaxKind::QualifiedExpr => node
                .child(&SyntaxKind::NameExpr)
                .and_then(|name| name.token(&TokenKind::Ident)),
            SyntaxKind::UsingTree if parent != Some(&SyntaxKind::UsingGroup) => {
                node.token(&TokenKind::Ident)
            }
            _ => None,
        };
        if let Some(token) = path_root {
            let context = match parent {
                Some(SyntaxKind::UsingStmt) => PathContext::BlockUsing,
                Some(SyntaxKind::UsingDecl) => PathContext::ModuleUsing,
                _ => PathContext::Reference,
            };
            paths.push((token.span(), context));
        }
        for child in node.child_nodes() {
            self.collect(child, Some(kind), scopes, paths, selector_names);
        }
    }

    fn block_selectors(
        &mut self,
        node: &GreenNode,
        directive: Span,
        scope: &ModuleUsingScope,
        selector_names: &mut HashSet<Span>,
    ) {
        if node.kind() == &SyntaxKind::UsingName {
            let mut identifiers = node
                .child_tokens()
                .filter(|token| token.is(&TokenKind::Ident));
            if let Some(name) = identifiers.next() {
                let name_span = name.span();
                selector_names.insert(name_span);
                let alias_span = identifiers.next().map(|token| token.span());
                self.selector(
                    &UsingName {
                        name: self.name(name_span),
                        name_span,
                        alias: alias_span.map(|span| self.name(span)),
                        alias_span,
                    },
                    directive,
                    scope,
                );
            }
        }
        for child in node.child_nodes() {
            self.block_selectors(child, directive, scope, selector_names);
        }
    }
}
