// SPDX-License-Identifier: GPL-3.0-or-later
//! Block import environments shared by type and value resolution.

use super::*;
use nia_ast::{Block, StmtKind};
use nia_ast_walk::Visitor;
use nia_defs::{UnresolvedUsing, UsingScopeLookup};
use nia_item_tree::ActiveModuleItemTree;
use std::{collections::HashSet, sync::Arc};

#[derive(Debug, Clone, PartialEq)]
struct BlockUsingScope {
    span: Span,
    scope: Arc<ModuleUsingScope>,
    namespace_names: HashSet<SymbolId>,
}

/// Resolved module imports and the effective imports in each lexical block.
#[derive(Debug, Clone, PartialEq)]
pub struct LexicalUsingScopes {
    module: Arc<ModuleUsingScope>,
    blocks: Vec<BlockUsingScope>,
}

impl LexicalUsingScopes {
    /// Creates a lookup for syntax with no block-local import directives.
    pub fn module_only(scope: Arc<ModuleUsingScope>) -> Self {
        Self {
            module: scope,
            blocks: Vec::new(),
        }
    }

    /// Visits failure evidence so the query owner can attach actual loader roots.
    pub fn unresolved_usings_mut(&mut self) -> impl Iterator<Item = &mut UnresolvedUsing> {
        std::iter::once(Arc::make_mut(&mut self.module))
            .chain(
                self.blocks
                    .iter_mut()
                    .map(|block| Arc::make_mut(&mut block.scope)),
            )
            .flat_map(|scope| scope.unresolved_usings.values_mut())
    }
}

impl UsingScopeLookup for LexicalUsingScopes {
    fn scope_at(&self, span: Span) -> Option<&ModuleUsingScope> {
        self.blocks
            .iter()
            .filter(|block| block.span.start <= span.start && span.end <= block.span.end)
            .min_by_key(|block| block.span.end - block.span.start)
            .map(|block| block.scope.as_ref())
    }

    fn shadows_module_namespace(&self, name: &SymbolId, span: Span) -> bool {
        self.blocks.iter().any(|block| {
            block.span.start <= span.start
                && span.end <= block.span.end
                && block.namespace_names.contains(name)
        })
    }

    fn using_module(&self, name: &SymbolId) -> Option<ModuleId> {
        self.module.using_module(name)
    }

    fn using_value(&self, name: &SymbolId) -> Option<UsingEntry> {
        self.module.using_value(name)
    }

    fn using_type(&self, name: &SymbolId) -> Option<UsingEntry> {
        self.module.using_type(name)
    }

    fn has_unresolved_using_name(&self, name: &SymbolId) -> bool {
        self.module.has_unresolved_using_name(name)
    }

    fn unresolved_using(&self, name: &SymbolId) -> Option<UnresolvedUsing> {
        self.module.unresolved_using(name)
    }
}

/// Resolves block imports using the same visibility and cause rules as module imports.
pub fn compute_lexical_using_scopes_with_symbols<D: Borrow<DefCollection>>(
    items: &ActiveModuleItemTree,
    current: &DefCollection,
    defs: &[D],
    graph: &ModuleGraph,
    surfaces: &PublicSurfaces,
    module_scope: &ModuleUsingScope,
    symbols: &dyn SymbolText,
) -> (LexicalUsingScopes, Vec<Diagnostic>) {
    let module = Arc::new(module_scope.clone());
    let defs_by_module = defs
        .iter()
        .map(|defs| {
            let defs = defs.borrow();
            (defs.module_id, defs)
        })
        .collect();
    let mut resolver = LexicalResolver {
        context: UsingExpansionContext {
            defs_by_module: &defs_by_module,
            graph,
            accessing_module: current.module_id,
            surfaces,
            symbols,
            mode: UsingLookupMode::Visible,
            imported_types: None,
            lexical_namespaces: None,
        },
        current,
        stack: vec![Arc::clone(&module)],
        blocks: Vec::new(),
        diagnostics: Vec::new(),
    };
    for item in items.items.iter() {
        resolver.visit_item(item);
    }
    (
        LexicalUsingScopes {
            module,
            blocks: resolver.blocks,
        },
        resolver.diagnostics,
    )
}

struct LexicalResolver<'a> {
    context: UsingExpansionContext<'a>,
    current: &'a DefCollection,
    stack: Vec<Arc<ModuleUsingScope>>,
    blocks: Vec<BlockUsingScope>,
    diagnostics: Vec<Diagnostic>,
}

impl<'ast> Visitor<'ast> for LexicalResolver<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        if !block
            .stmts
            .iter()
            .any(|stmt| matches!(stmt.kind, StmtKind::Using(_)))
        {
            nia_ast_walk::walk_block(self, block);
            return;
        }
        let mut scope = self
            .stack
            .last()
            .expect("module import scope")
            .as_ref()
            .clone();
        let mut local = ModuleUsingScope::default();
        let mut module_origins = HashMap::new();
        let mut namespace_names = self
            .blocks
            .iter()
            .filter(|outer| {
                outer.span.start <= block.span.start && block.span.end <= outer.span.end
            })
            .flat_map(|outer| outer.namespace_names.iter().copied())
            .collect::<HashSet<_>>();
        for stmt in &block.stmts {
            let StmtKind::Using(using) = &stmt.kind else {
                continue;
            };
            let using = ModuleUsing::from_ast(Visibility::Private, stmt.span, using);
            let context = UsingExpansionContext {
                imported_types: Some(&scope.types),
                lexical_namespaces: Some(&namespace_names),
                ..self.context
            };
            let expansion = expand_using(&context, self.current, &using, &scope.modules);
            for failure in &expansion.failures {
                let root = using_failure_diagnostic(&using, failure, self.context.symbols);
                let cause = root.as_ref().and_then(|diagnostic| {
                    Some(nia_diagnostic::DiagnosticCause {
                        source_path: self
                            .context
                            .graph
                            .get(self.current.module_id)?
                            .path
                            .as_str()
                            .to_owned(),
                        code: diagnostic.code.as_str().to_owned(),
                        span: diagnostic.primary_span()?,
                    })
                });
                record_unresolved_using_names(&mut local, &using, failure, cause);
                if let Some(root) = root {
                    self.diagnostics.push(root);
                }
            }
            for entry in expansion.entries {
                let previous = match &entry.kind {
                    ResolvedEntryKind::Module(_) => module_origins.get(&entry.name).copied(),
                    ResolvedEntryKind::Item(item) => match item.namespace {
                        PublicNamespace::Value => local.values.get(&entry.name),
                        PublicNamespace::Type => local.types.get(&entry.name),
                    }
                    .map(|entry| (entry.name_span, entry.directive_span)),
                };
                if let Some((name_span, directive_span)) = previous {
                    self.diagnostics.push(
                        Diagnostic::user_error(
                            codes::NAME_RESOLUTION,
                            format!(
                                "duplicate using name `{}` in this block",
                                symbol_text(self.context.symbols, entry.name)
                            ),
                        )
                        .primary(entry.name_span, "this name is imported again here")
                        .related(name_span, "the name was imported here first")
                        .related(directive_span, "the earlier `using` directive is here")
                        .finish(),
                    );
                    continue;
                }
                match entry.kind {
                    ResolvedEntryKind::Module(module) => {
                        local.modules.insert(entry.name, module);
                        module_origins.insert(entry.name, (entry.name_span, using.span));
                    }
                    ResolvedEntryKind::Item(item) => {
                        let table = match item.namespace {
                            PublicNamespace::Value => &mut local.values,
                            PublicNamespace::Type => &mut local.types,
                        };
                        table.insert(
                            entry.name,
                            UsingEntry {
                                target_module: item.target_module,
                                target_def_id: item.target_def_id,
                                namespace: item.namespace,
                                directive_span: using.span,
                                name_span: entry.name_span,
                                parent_enum: item.parent_enum,
                            },
                        );
                    }
                }
            }
            for name in local.unresolved_usings.keys() {
                scope.modules.remove(name);
                scope.values.remove(name);
                scope.types.remove(name);
            }
            scope.modules.extend(local.modules.clone());
            namespace_names.extend(local.modules.keys().copied());
            namespace_names.extend(local.unresolved_usings.keys().copied());
            scope.values.extend(local.values.clone());
            scope.types.extend(local.types.clone());
            scope
                .unresolved_usings
                .extend(local.unresolved_usings.clone());
        }
        let scope = Arc::new(scope);
        self.blocks.push(BlockUsingScope {
            span: block.span,
            scope: Arc::clone(&scope),
            namespace_names,
        });
        self.stack.push(scope);
        nia_ast_walk::walk_block(self, block);
        self.stack.pop();
    }
}
