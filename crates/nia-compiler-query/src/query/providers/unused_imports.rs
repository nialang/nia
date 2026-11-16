// SPDX-License-Identifier: GPL-3.0-or-later
//! Unused-import warnings, decided after body checking.
//!
//! The loader knows which imports a path names. An import nothing names can
//! still be needed: a `using` of a module makes the extension methods it
//! declares or publishes callable, which only the checked calls reveal. An
//! import is therefore unused when no path names it and no extension method
//! a checked body calls needs it. This holds only where every body of the
//! module was checked, so the warning belongs to `nia check`, whose root
//! scope is the whole tree, and never to an entry-scoped build.
use super::*;
use nia_loader_contract::UsingImportFact;
use nia_symbol_table::SymbolTable;

/// Warns about the explicit imports of `modules` that nothing needs.
///
/// A module whose bodies failed to check is skipped: a call that did not
/// resolve may be the one that needed an import.
pub(super) fn unused_import_diagnostics(
    db: &QueryDb<CompilerContext>,
    modules: &[Arc<CheckedModule>],
) -> QueryResult<Vec<ProgramDiagnostic>> {
    if *db.get(CompilerRootScopeQuery)? != crate::RootScope::Tree {
        return Ok(Vec::new());
    }
    let symbols = db.context().symbols();
    let mut warnings = Vec::new();
    for module in modules {
        let imports = db.get(ModuleUsingImportsQuery(module.id))?;
        if imports.iter().all(|import| import.referenced)
            || resolve_diagnostic_bundle(&module.body_diagnostics)
                .iter()
                .any(|diagnostic| diagnostic.severity() == nia_diagnostic::Severity::Error)
        {
            continue;
        }
        let scope = db.get(ModuleUsingScopeQuery(module.id))?;
        let needed = imported_extension_uses(db, module)?;
        let unused = unused_imports(db, module.id, &imports, &needed)?;
        warnings.extend(
            unused
                .into_iter()
                // A failed `using` already has a root error; calling the same
                // name unused would describe the recovery, not the source.
                .filter(|import| !scope.unresolved_usings.contains_key(&import.name))
                .map(|import| ProgramDiagnostic {
                    path: module.path.clone(),
                    diagnostic: unused_import_warning(import, &symbols),
                }),
        );
    }
    Ok(warnings)
}

/// The imports among `imports` that no path names and no call in `needed`
/// requires.
///
/// Imports are decided in source order, and one found unused counts as
/// removed: of two imports that make the same methods visible, the first is
/// reported and the second is kept, so removing every reported import keeps
/// the module compiling.
fn unused_imports<'a>(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    imports: &'a [UsingImportFact],
    needed: &HashSet<ModuleId>,
) -> QueryResult<Vec<&'a UsingImportFact>> {
    let graph = QueryModuleGraphLookup::new(db)?;
    let public_surfaces = QueryPublicSurfaceLookup::new(db);
    let using_scope = db.get(ModuleUsingScopeQuery(module_id))?;
    let routes = imports
        .iter()
        .map(|import| {
            let published = import.module.map(|module| {
                nia_program_signatures::imported_module_extension_modules(
                    nia_program_signatures::VisibleExtensionProviderModulesInput {
                        module_id,
                        graph: &graph,
                        using_scope: &using_scope,
                        public_surfaces: &public_surfaces,
                    },
                    module,
                )
            });
            published
                .into_iter()
                .flatten()
                .chain(import.demand_providers.iter().copied())
                .filter(|module| needed.contains(module))
                .collect::<HashSet<_>>()
        })
        .collect::<Vec<_>>();
    if let Some(error) = graph
        .take_failure()
        .or_else(|| public_surfaces.take_failure())
    {
        return Err(error);
    }
    let mut removed = vec![false; imports.len()];
    for (index, import) in imports.iter().enumerate() {
        if import.referenced {
            continue;
        }
        // Every needed module this import makes visible must stay visible
        // through an import that is kept.
        let replaceable = routes[index].iter().all(|module| {
            routes
                .iter()
                .enumerate()
                .any(|(other, route)| other != index && !removed[other] && route.contains(module))
        });
        removed[index] = replaceable;
    }
    Ok(imports
        .iter()
        .zip(removed)
        .filter_map(|(import, removed)| removed.then_some(import))
        .collect())
}

/// The modules of the extension methods `module` calls that only an import
/// makes callable.
///
/// A trait method, a function, and a member declared in the module that owns
/// its receiver type are callable without importing their module.
fn imported_extension_uses(
    db: &QueryDb<CompilerContext>,
    module: &CheckedModule,
) -> QueryResult<HashSet<ModuleId>> {
    let facts = &module.semantic_facts;
    let called = facts
        .iter_node_resolved_calls()
        .filter_map(|(_, call)| match call {
            nia_sema_ir::ResolvedCall::Method { def_id, .. }
            | nia_sema_ir::ResolvedCall::Function(def_id)
            | nia_sema_ir::ResolvedCall::FunctionInstance { def_id, .. } => Some(*def_id),
            _ => None,
        })
        .chain(
            facts
                .iter_node_function_references()
                .map(|(_, reference)| reference.def_id),
        )
        .filter(|def_id| def_id.module_id != module.id)
        .collect::<HashSet<_>>();
    let type_store = &db.context().type_store;
    let mut needed = HashSet::new();
    for def_id in called {
        if needed.contains(&def_id.module_id) {
            continue;
        }
        let method = db.get(ExtensionMethodByIdQuery(def_id))?;
        let Some(method) = method.method.as_ref() else {
            continue;
        };
        if method.trait_id.is_some() {
            continue;
        }
        let owned_by_receiver = matches!(
            type_store.get(method.target_ty),
            Some(nia_ty::TyKind::Nominal { def_id: owner, .. }) if owner.module_id == def_id.module_id
        );
        if !owned_by_receiver {
            needed.insert(def_id.module_id);
        }
    }
    Ok(needed)
}

fn unused_import_warning(import: &UsingImportFact, symbols: &SymbolTable) -> Diagnostic {
    let name = nia_symbol::symbol_text_or_unresolved(symbols, import.name);
    let summary = format!("unused import `{name}`");
    Diagnostic::user_warning(codes::UNUSED_IMPORT, summary)
        .primary(import.name_span, "this imported name is never used")
        .help(format!("remove `{name}` from this `using` directive"))
        .finish()
}
