// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn has_reachable_executable_body_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> QueryResult<bool> {
    if reachable_functions
        .iter()
        .any(|def_id| def_id.module_id == module_id)
    {
        return Ok(true);
    }
    for def_id in reachable_globals
        .iter()
        .filter(|def_id| def_id.module_id == module_id)
    {
        // Const definitions can be reachable for type metadata without making
        // this a body-bearing module. Keep the classification centralized in
        // the executable query layer.
        if crate::query::is_runtime_global_def(db, *def_id)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn rooted_layouts_for_checked_module(
    db: &QueryDb<CompilerContext>,
    module: &CheckedModule,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> QueryResult<ModuleLayouts> {
    // Type-only modules were laid out from signature facts and have no body
    // semantic facts that could refine their roots.
    if module.executable_type_only {
        return Ok(ModuleLayouts {
            semantic: Arc::clone(&module.layouts),
            diagnostics: module.layout_diagnostics.clone(),
        });
    }
    let item_signatures = item_signatures_semantic(db, module.id)?;
    let target = compiler_target_data_layout(db)?;
    let roots = checked_module_layout_roots(&db.context().type_store, module);
    let array_lengths = &module.const_eval.array_lengths;
    let symbols = db.context().symbols();
    let query_failure = RefCell::new(None);
    let local_array_lengths = |id| array_lengths.get(&id).copied();
    let layout_query = |module_id| {
        program_layouts_override
            .and_then(|program_layouts| program_layouts(module_id))
            .or_else(|| {
                capture_query_failure(&query_failure, db.get(LayoutsQuery(module_id)))
                    .map(|layouts| Arc::clone(&layouts.semantic))
            })
    };
    let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
        program_array_lengths_override
            .and_then(|array_lengths| array_lengths(id))
            .or_else(|| {
                capture_query_failure(&query_failure, db.get(ConstArrayLengthsQuery(id.module_id)))
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied())
            })
    };
    let layouts = nia_layout::compute_layouts_for_roots_with_program_context(
        nia_layout::LayoutComputationInput {
            type_store: &db.context().type_store,
            defs: &module.defs,
            signatures: &item_signatures,
            root_types: &[],
            normalized: &module.type_normalization.normalized,
            array_lengths: &local_array_lengths,
            target,
            program: nia_layout::ProgramLayoutContext {
                symbols: Some(&symbols),
                layouts: Some(&layout_query),
                array_lengths: Some(&program_array_lengths),
                ..Default::default()
            },
        },
        nia_layout::LayoutRoots {
            types: &roots.types,
            structs: &roots.structs,
            unions: &roots.unions,
        },
    );
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(store_module_layouts(db.context(), layouts)),
    }
}

pub(super) struct ExecutableLayoutModule<'a> {
    pub(super) module_id: ModuleId,
    pub(super) signatures: &'a ItemSignatures,
    pub(super) program_struct: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    pub(super) program_union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
}

pub(super) fn executable_layout_roots(
    module: ExecutableLayoutModule<'_>,
    type_store: &nia_ty::TypeStore,
    type_uses: impl IntoIterator<Item = InternedTyId>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> CollectedLayoutRoots {
    let ExecutableLayoutModule {
        module_id,
        signatures,
        program_struct,
        program_union,
    } = module;
    let mut roots =
        LayoutRootCollector::with_program(type_store, module_id, program_struct, program_union);

    // Explicit type uses carry local body details. Signature roots keep ABI
    // types and trait receivers alive even when no expression mentions them.
    for ty in type_uses {
        roots.add(ty);
    }
    for function_id in reachable_functions
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module_id)
    {
        if let Some(signature) = signatures.functions.get(&function_id.def_id) {
            for param in &signature.params {
                roots.add(param.ty);
            }
            roots.add(signature.return_type);
        }
    }
    for impl_signature in &signatures.trait_impls {
        if impl_signature.methods.iter().any(|method| {
            reachable_functions.contains(&GlobalDefId {
                module_id,
                def_id: method.def_id,
            })
        }) {
            roots.add(impl_signature.target_ty);
        }
    }
    for global_id in reachable_globals
        .iter()
        .copied()
        .filter(|def_id| def_id.module_id == module_id)
    {
        if let Some(signature) = signatures.globals.get(&global_id.def_id)
            && let Some(ty) = signature.explicit_type
        {
            roots.add(ty);
        }
    }
    roots.finish()
}

fn checked_module_layout_roots(
    type_store: &nia_ty::TypeStore,
    module: &CheckedModule,
) -> CollectedLayoutRoots {
    let mut roots = LayoutRootCollector::new(type_store, module.id);
    collect_semantic_layout_roots(&module.semantic_facts, &mut roots);
    roots.finish()
}
