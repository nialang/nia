// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LexicalUsingScopesValue {
    pub(super) scopes: nia_public_surface::LexicalUsingScopes,
    pub(super) diagnostics: nia_diagnostic::DiagnosticBundle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ModuleLexicalUsingScopesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for ModuleLexicalUsingScopesQuery {
    type Value = LexicalUsingScopesValue;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "module_lexical_using_scopes"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        let items = db.get(FullActiveModuleItemTreeQuery(self.0))?;
        lexical_using_scopes(db, self.0, &items)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SignatureConstUsingScopesQuery(pub(super) ModuleId);

impl QueryKey<CompilerContext> for SignatureConstUsingScopesQuery {
    type Value = LexicalUsingScopesValue;

    const FINGERPRINT: QueryFingerprintPolicy = QueryFingerprintPolicy::SemanticValue;

    fn name() -> &'static str {
        "signature_const_using_scopes"
    }

    fn execute_result(&self, db: &QueryDb<CompilerContext>) -> QueryResult<Self::Value> {
        let items = db.get(SignatureConstItemTreeQuery(self.0))?;
        lexical_using_scopes(db, self.0, &items)
    }

    fn values_equal(&self, old: &Self::Value, new: &Self::Value) -> bool {
        old == new
    }
}

fn lexical_using_scopes(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    items: &ActiveModuleItemTree,
) -> QueryResult<LexicalUsingScopesValue> {
    use nia_ast_walk::Visitor;
    struct ImportPresence(bool);
    impl<'ast> Visitor<'ast> for ImportPresence {
        fn visit_stmt(&mut self, stmt: &'ast nia_ast::Stmt) {
            if matches!(stmt.kind, nia_ast::StmtKind::Using(_)) {
                self.0 = true;
            } else if !self.0 {
                nia_ast_walk::walk_stmt(self, stmt);
            }
        }
    }
    let scope = db.get(ModuleUsingScopeQuery(module_id))?;
    let mut presence = ImportPresence(false);
    for item in items.items.iter() {
        presence.visit_item(item);
        if presence.0 {
            break;
        }
    }
    if !presence.0 {
        return Ok(LexicalUsingScopesValue {
            scopes: nia_public_surface::LexicalUsingScopes::module_only(scope),
            diagnostics: db.context().diagnostic_store.bundle(Vec::new())?,
        });
    }
    let defs = shared_public_surface_defs_by_module(db)?;
    let current = module_defs_semantic(db, module_id)?;
    let graph = db.get(ModuleGraphQuery)?;
    let surfaces = db.get(PublicSurfacesQuery)?;
    let symbols = db.context().symbols();
    let (mut scopes, diagnostics) = nia_public_surface::compute_lexical_using_scopes_with_symbols(
        items,
        &current,
        &defs,
        &graph,
        &surfaces.surfaces,
        &scope,
        &symbols,
    );
    attach_loader_causes(db, &graph, scopes.unresolved_usings_mut())?;
    Ok(LexicalUsingScopesValue {
        scopes,
        diagnostics: db.context().diagnostic_store.bundle(diagnostics)?,
    })
}

pub(super) fn attach_loader_causes<'a>(
    db: &QueryDb<CompilerContext>,
    graph: &ModuleGraphSnapshot,
    failures: impl Iterator<Item = &'a mut nia_defs::UnresolvedUsing>,
) -> QueryResult<()> {
    let mut load_diagnostics = None;
    for failure in failures {
        let Some(module) = failure.unavailable_module else {
            continue;
        };
        if failure.cause.is_some() {
            continue;
        }
        let Some(node) = graph.get(module) else {
            continue;
        };
        let Some(parent) = node.parent.and_then(|parent| graph.get(parent)) else {
            continue;
        };
        let Some(declaration) = parent
            .declarations
            .iter()
            .find(|item| item.target == module)
        else {
            continue;
        };
        if load_diagnostics.is_none() {
            load_diagnostics = Some(db.get(ProgramLoadDiagnosticsQuery)?.to_diagnostics());
        }
        let mut roots = load_diagnostics.as_ref().unwrap().iter().filter(|item| {
            item.path == parent.path
                && item.diagnostic.code.as_str() == codes::LOAD.as_str()
                && item.diagnostic.primary_span() == Some(declaration.span)
                && item.diagnostic.cause.is_none()
        });
        if let Some(root) = roots.next()
            && roots.next().is_none()
        {
            failure.cause = Some(nia_diagnostic::DiagnosticCause {
                source_path: root.path.as_str().to_owned(),
                code: root.diagnostic.code.as_str().to_owned(),
                span: declaration.span,
            });
        }
    }
    Ok(())
}
