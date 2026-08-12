// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
mod function_body;
mod layouts;
mod static_init;
mod value_refs;

pub(in crate::query) use function_body::provide_executable_function_body;
use layouts::{
    ExecutableLayoutModule, executable_layout_roots, has_reachable_executable_body_items,
    rooted_layouts_for_checked_module,
};
pub(in crate::query) use static_init::provide_executable_static_init;
pub(in crate::query) use value_refs::ExecutableValueRefEdges;
use value_refs::{
    ExecutableValueRefIndex, collect_executable_value_ref_index_for_items,
    executable_value_ref_active_item_tree, walk_executable_value_ref_closure,
};

struct BodyCheckConstInputs {
    module: ConstModuleLowering,
    array_lengths: nia_const_check::ConstArrayLengths,
    enum_values: nia_const_check::ConstEnumValues,
    values: nia_const_check::ConstValues,
    typed_facts: nia_const_check::ConstTypedFacts,
}

#[derive(Clone, Copy)]
pub(super) struct ReachableBodyModules<'a> {
    base: &'a HashSet<ModuleId>,
    extra: Option<ModuleId>,
}

impl<'a> ReachableBodyModules<'a> {
    pub(super) fn new(base: &'a HashSet<ModuleId>) -> Self {
        Self { base, extra: None }
    }

    pub(super) fn with_extra(self, module_id: ModuleId) -> Self {
        Self {
            base: self.base,
            extra: Some(module_id),
        }
    }

    fn contains(self, module_id: ModuleId) -> bool {
        self.extra == Some(module_id) || self.base.contains(&module_id)
    }
}

#[derive(Clone, Copy)]
pub(super) struct ExecutableFactMode<'a> {
    non_function_signatures: Option<&'a ProgramExecutableNonFunctionSignatures>,
    reachable_body_modules: Option<ReachableBodyModules<'a>>,
}

impl<'a> ExecutableFactMode<'a> {
    fn full() -> Self {
        Self {
            non_function_signatures: None,
            reachable_body_modules: None,
        }
    }

    pub(super) fn executable(reachable_body_modules: ReachableBodyModules<'a>) -> Self {
        Self {
            non_function_signatures: None,
            reachable_body_modules: Some(reachable_body_modules),
        }
    }

    fn signature_facts_for(self, module_id: ModuleId) -> bool {
        if let Some(reachable_body_modules) = self.reachable_body_modules {
            return !reachable_body_modules.contains(module_id);
        }
        self.non_function_signatures.is_some()
    }
}

impl BodyCheckConstInputs {
    fn into_check(self) -> ConstCheck {
        ConstCheck {
            values: self.values.values,
            typed_values: self.typed_facts.typed_values,
            enum_values: self.enum_values.values,
            typed_enum_values: self.enum_values.typed_values,
            array_lengths: self.array_lengths.values,
            diagnostics: self.typed_facts.diagnostics,
        }
    }
}

fn filtered_const_global_initializer_for_body_check(
    db: &QueryDb<CompilerContext>,
    global_id: GlobalDefId,
) -> QueryResult<Option<nia_const_ir::ResolvedConstExpr>> {
    let query_failure = RefCell::new(None);
    let defs = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.defs",
        global_id.module_id,
        || full_module_defs_semantic(db, global_id.module_id),
    )?;
    let source_path = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.source_path",
        global_id.module_id,
        || db.get(ModulePathQuery(global_id.module_id)),
    )?;
    let active_item_tree = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.active_item_tree",
        global_id.module_id,
        || db.get(FullActiveModuleItemTreeQuery(global_id.module_id)),
    )?;
    let filtered_active_item_tree = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.filter_item_tree",
        global_id.module_id,
        || {
            active_item_tree_for_body_check_filter(
                global_id.module_id,
                &defs,
                &active_item_tree,
                nia_body_check::BodyCheckFilter::ReachableItems {
                    functions: &HashSet::new(),
                    globals: &HashSet::from([global_id]),
                    already_checked_functions: None,
                    already_checked_globals: None,
                },
            )
        },
    );
    let graph = db.get(ModuleGraphQuery)?;
    let program_defs =
        |module_id| capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id));
    let public_surfaces = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.public_surfaces",
        global_id.module_id,
        || db.get(PublicSurfacesQuery),
    )?;
    let using_scope = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.module_using_scope",
        global_id.module_id,
        || db.get(ModuleUsingScopeQuery(global_id.module_id)),
    )?;
    let source_version = *db.get(ModuleSourceVersionQuery(global_id.module_id))?;
    let origins = db.get(ModuleOriginsQuery(global_id.module_id))?;
    let lowered = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.type_lowering",
        global_id.module_id,
        || type_lowering_semantic(db, global_id.module_id),
    )?;
    let type_resolution = type_resolution_semantic(db, global_id.module_id)?;
    let signatures = item_signatures_semantic(db, global_id.module_id)?;
    let needed_const_exprs = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.needed_const_exprs",
        global_id.module_id,
        || {
            needed_const_exprs_for_active_item_tree(
                &db.context().type_store,
                &filtered_active_item_tree,
                &lowered,
            )
        },
    );
    let symbols = db.context().symbols();
    let const_expr_value_resolution = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.const_expr_value_resolution",
        global_id.module_id,
        || {
            let visible_extensions = || db.get(VisibleExtensionsQuery(global_id.module_id));
            let associated_values =
                LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
            let values = nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols_in_store(
                lowered.const_exprs.iter().filter_map(|(id, expr)| {
                    needed_const_exprs.contains(id).then_some(expr.clone())
                }),
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&graph),
                },
                &public_surfaces.surfaces,
                using_scope.as_ref(),
                nia_value_resolve::ValueResolveOptions::with_store(
                    Some(&associated_values),
                    Some(&symbols),
                    db.context().node_store(),
                ),
            );
            if let Some(error) = associated_values.take_failure() {
                let _ = capture_query_failure(&query_failure, Err::<(), _>(error));
            }
            values
        },
    );
    let filtered_const_exprs = const_expr_subset_for_ids(&lowered.const_exprs, &needed_const_exprs);
    let lower_with_values = |values: ValueResolution| {
        let locals = time_module_provider(
            db,
            "executable_body_check.const_eval.global_initializer.local_resolution",
            global_id.module_id,
            || {
                nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
                    &filtered_active_item_tree,
                    &active_item_tree,
                    &defs,
                    &values,
                    Some(source_version),
                    &origins,
                    &symbols,
                )
            },
        );
        let semantic_uses = time_module_provider(
            db,
            "executable_body_check.const_eval.global_initializer.semantic_uses",
            global_id.module_id,
            || {
                semantic_use_table_from_resolution_inputs_with_const_expr_values(
                    SemanticUseInputs {
                        module_id: global_id.module_id,
                        node_store: db.context().node_store(),
                        type_store: &db.context().type_store,
                        active_item_tree: &filtered_active_item_tree,
                        values: &values,
                        const_expr_values: Some(&const_expr_value_resolution),
                        const_expr_value_ids: Some(&needed_const_exprs),
                        locals: &locals,
                        type_resolution: &type_resolution,
                        type_lowering: &lowered,
                    },
                )
            },
        );
        let lowered = time_module_provider(
            db,
            "executable_body_check.const_eval.global_initializer.lower_module",
            global_id.module_id,
            || {
                let symbols = db.context().symbols();
                nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
                    active_item_tree: &filtered_active_item_tree,
                    defs: &defs,
                    signatures: &signatures,
                    values: &values,
                    locals: &locals,
                    semantic_uses: &semantic_uses,
                    symbols: &symbols,
                    const_exprs: &filtered_const_exprs,
                    source_path: &source_path,
                })
            },
        );
        lowered
            .module
            .global_initializers()
            .get(&global_id)
            .or_else(|| {
                lowered
                    .module
                    .deferred_global_initializers()
                    .get(&global_id)
            })
            .cloned()
    };
    let values = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.value_resolution",
        global_id.module_id,
        || {
            let visible_extensions = || {
                time_module_provider(
                    db,
                    "executable_body_check.const_eval.global_initializer.visible_extensions",
                    global_id.module_id,
                    || db.get(VisibleExtensionsQuery(global_id.module_id)),
                )
            };
            let associated_values =
                LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
            let symbols = db.context().symbols();
            let values = nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols_in_store(
                &filtered_active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&graph),
                },
                &public_surfaces.surfaces,
                using_scope.as_ref(),
                nia_value_resolve::ValueResolveOptions::with_store(
                    Some(&associated_values),
                    Some(&symbols),
                    db.context().node_store(),
                ),
            );
            if let Some(error) = associated_values.take_failure() {
                let _ = capture_query_failure(&query_failure, Err::<(), _>(error));
            }
            values
        },
    );
    let initializer = lower_with_values(values);
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(initializer),
    }
}

fn executable_program_global_initializer(
    db: &QueryDb<CompilerContext>,
    global_id: GlobalDefId,
    fact_mode: ExecutableFactMode<'_>,
) -> QueryResult<Option<nia_const_ir::ResolvedConstExpr>> {
    if fact_mode.signature_facts_for(global_id.module_id) {
        let lowering = signature_const_module_lowering(db, global_id.module_id)?;
        let module = &lowering.module;
        return Ok(module
            .global_initializers()
            .get(&global_id)
            .or_else(|| module.deferred_global_initializers().get(&global_id))
            .cloned());
    }
    filtered_const_global_initializer_for_body_check(db, global_id)
}

struct ConstBodyModuleInput<'a> {
    module_id: ModuleId,
    defs: &'a DefCollection,
    source_path: &'a SourcePath,
    signatures: &'a ItemSignatures,
    normalization: &'a TypeNormalization,
    lowered: &'a TypeLowering,
    resolution: &'a BodyCheckResolutionInputs,
}

fn const_inputs_for_body_check(
    db: &QueryDb<CompilerContext>,
    module: ConstBodyModuleInput<'_>,
    fact_mode: ExecutableFactMode<'_>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_const_ir::ResolvedConstExpr>>>,
    >,
    const_module_cache: Option<&RefCell<HashMap<ModuleId, ConstModuleLowering>>>,
) -> QueryResult<BodyCheckConstInputs> {
    let ConstBodyModuleInput {
        module_id,
        defs,
        source_path,
        signatures,
        normalization,
        lowered,
        resolution: inputs,
    } = module;
    let needed_const_exprs = needed_const_exprs_for_active_item_tree(
        &db.context().type_store,
        &inputs.active_item_tree,
        lowered,
    );
    let filtered_const_exprs = const_expr_subset_for_ids(&lowered.const_exprs, &needed_const_exprs);
    let lower_module = || {
        time_module_provider(
            db,
            "executable_body_check.const_eval.lower_module",
            module_id,
            || {
                let symbols = db.context().symbols();
                nia_const_check::lower_module_const(nia_const_check::ConstModuleInput {
                    active_item_tree: &inputs.active_item_tree,
                    defs,
                    signatures,
                    values: &inputs.values,
                    locals: &inputs.locals,
                    semantic_uses: &inputs.semantic_uses,
                    symbols: &symbols,
                    const_exprs: &filtered_const_exprs,
                    source_path,
                })
            },
        )
    };
    let module = if let Some(cache) = const_module_cache {
        if !cache.borrow().contains_key(&module_id) {
            let module = lower_module();
            cache.borrow_mut().insert(module_id, module);
        }
        cache
            .borrow()
            .get(&module_id)
            .expect("cached const module lowering must exist")
            .clone()
    } else {
        lower_module()
    };
    let query_failure = RefCell::new(None);
    let program_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_const_module_lowering(db, module_id),
            )
            .map(|lowering| lowering.module.clone());
        }
        capture_query_failure(&query_failure, db.get(ConstModuleQuery(module_id)))
            .map(|lowering| lowering.module.clone())
    };
    let program_source_path = |module_id| {
        capture_query_failure(&query_failure, db.get(ModulePathQuery(module_id)))
            .map(|path| path.as_ref().clone())
    };
    let program_defs =
        |module_id| capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id));
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_type_normalization_semantic(
                    db,
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            );
        }
        capture_query_failure(&query_failure, type_normalization_semantic(db, module_id))
    };
    let local_trait_impls = if fact_mode.non_function_signatures.is_none() {
        Some(db.get(VisibleTraitImplsQuery(module_id))?)
    } else {
        None
    };
    let trait_impls_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            return fact_mode
                .non_function_signatures
                .map(|signatures| signatures.trait_impls.clone())
                .or_else(|| {
                    local_trait_impls
                        .as_ref()
                        .map(|signatures| signatures.trait_impls.clone())
                });
        }
        if let Some(signatures) = fact_mode.non_function_signatures {
            return Some(signatures.trait_impls.clone());
        }
        capture_query_failure(
            &query_failure,
            db.get(VisibleTraitImplsQuery(requested_module_id)),
        )
        .map(|impls| impls.trait_impls.clone())
    };
    let program_is_enum = |def_id: GlobalDefId| {
        fact_mode
            .non_function_signatures
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )
            .is_some_and(|signatures| signatures.semantic.enums.contains_key(&def_id.def_id))
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_item_signatures_semantic(
                    db,
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            );
        }
        capture_query_failure(&query_failure, item_signatures_semantic(db, module_id))
    };
    let value_signatures_for_module = |module_id| {
        capture_query_failure(
            &query_failure,
            signature_item_signatures_semantic(
                db,
                module_id,
                nia_item_tree::SignatureItemSet::Values,
            ),
        )
    };
    let function_signatures_for_module = |module_id| {
        capture_query_failure(
            &query_failure,
            signature_item_signatures_semantic(
                db,
                module_id,
                nia_item_tree::SignatureItemSet::Functions,
            ),
        )
    };
    let local_visible_extensions = db.get(VisibleExtensionsQuery(module_id))?;
    let visible_extensions_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            return Some(local_visible_extensions.methods.clone());
        }
        capture_query_failure(
            &query_failure,
            db.get(VisibleExtensionsQuery(requested_module_id)),
        )
        .map(|extensions| extensions.methods.clone())
    };
    let program_global_initializer = |global_id| {
        if let Some(cache) = global_initializer_cache {
            if !cache.borrow().contains_key(&global_id) {
                let initializer = capture_query_failure(
                    &query_failure,
                    executable_program_global_initializer(db, global_id, fact_mode),
                )
                .flatten();
                cache.borrow_mut().insert(global_id, initializer);
            }
            return cache.borrow().get(&global_id).cloned().flatten();
        }
        capture_query_failure(
            &query_failure,
            executable_program_global_initializer(db, global_id, fact_mode),
        )
        .flatten()
    };
    let target = db.get(CompilerTargetQuery)?;
    let symbols = db.context().symbols();
    let const_input = nia_const_check::ConstInput {
        type_store: &db.context().type_store,
        module: &module.module,
        defs,
        values: &inputs.values,
        locals: &inputs.locals,
        semantic_uses: &inputs.semantic_uses,
        symbols: &symbols,
        lowered,
        signatures,
        normalization,
        target: &target,
        source_path,
        program: nia_const_check::ConstProgramContext {
            module: Some(&program_module),
            source_path: Some(&program_source_path),
            defs: Some(&program_defs),
            type_normalizations: Some(&program_type_normalization),
            signatures: Some(&item_signatures_for_module),
            function_signatures: Some(&function_signatures_for_module),
            value_signatures: Some(&value_signatures_for_module),
            const_values: None,
            global_initializer: Some(&program_global_initializer),
            program_is_enum: Some(&program_is_enum),
            trait_impls_for_module: Some(&trait_impls_for_module),
            visible_extensions: Some(&visible_extensions_for_module),
        },
    };
    let mut array_lengths = time_module_provider(
        db,
        "executable_body_check.const_eval.array_lengths",
        module_id,
        || nia_const_check::compute_module_const_array_lengths(const_input),
    );
    array_lengths.diagnostics.extend(module.diagnostics.clone());
    let enum_values = time_module_provider(
        db,
        "executable_body_check.const_eval.enum_values",
        module_id,
        || nia_const_check::compute_module_const_enum_values(const_input, array_lengths.clone()),
    );
    let values = time_module_provider(
        db,
        "executable_body_check.const_eval.values",
        module_id,
        || {
            nia_const_check::compute_module_const_values(
                const_input,
                array_lengths.clone(),
                enum_values.clone(),
            )
        },
    );
    let typed_facts = time_module_provider(
        db,
        "executable_body_check.const_eval.typed_facts",
        module_id,
        || {
            nia_const_check::compute_module_const_typed_facts(
                const_input,
                array_lengths.clone(),
                enum_values.clone(),
                values.clone(),
            )
        },
    );
    let output = BodyCheckConstInputs {
        module,
        array_lengths,
        enum_values,
        values,
        typed_facts,
    };
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(output),
    }
}

pub(super) fn body_check_with_filter_and_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    layouts: Option<Arc<nia_layout::Layouts>>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>>>,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> QueryResult<nia_body_check::BodyCheck> {
    Ok(body_check_with_filter_and_layouts_with_inputs(
        db,
        ExecutableBodyCheckInput {
            module_id,
            filter,
            layouts,
            program_layouts_override,
            fact_mode: match non_function_signatures_override {
                Some(program_signatures) => ExecutableFactMode {
                    non_function_signatures: Some(program_signatures),
                    reachable_body_modules: None,
                },
                None => ExecutableFactMode::full(),
            },
            resolution_inputs: None,
            seed: None,
            global_initializer_cache: None,
            const_module_cache: None,
            const_inputs: None,
            program_function_signature_cache: None,
            product: nia_body_check::BodyCheckProduct::Full,
            prechecked: None,
        },
    )?
    .body_check)
}

pub(super) fn body_check_const_declarations(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<nia_body_check::BodyCheck> {
    Ok(body_check_with_filter_and_layouts_with_inputs(
        db,
        ExecutableBodyCheckInput {
            module_id,
            filter: nia_body_check::BodyCheckFilter::ConstDeclarations,
            layouts: None,
            program_layouts_override: None,
            fact_mode: ExecutableFactMode::full(),
            resolution_inputs: None,
            seed: None,
            global_initializer_cache: None,
            const_module_cache: None,
            const_inputs: None,
            program_function_signature_cache: None,
            product: nia_body_check::BodyCheckProduct::FactsOnly,
            prechecked: None,
        },
    )?
    .body_check)
}

pub(super) struct ExecutableBodyCheckInput<'a> {
    pub module_id: ModuleId,
    pub filter: nia_body_check::BodyCheckFilter<'a>,
    pub layouts: Option<Arc<nia_layout::Layouts>>,
    pub program_layouts_override: Option<&'a dyn Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>>>,
    pub fact_mode: ExecutableFactMode<'a>,
    pub resolution_inputs: Option<BodyCheckResolutionInputs>,
    pub seed: Option<nia_body_check::BodyCheckSeed<'a>>,
    pub global_initializer_cache:
        Option<&'a RefCell<HashMap<GlobalDefId, Option<nia_const_ir::ResolvedConstExpr>>>>,
    pub const_module_cache: Option<&'a RefCell<HashMap<ModuleId, ConstModuleLowering>>>,
    pub const_inputs: Option<(&'a ConstCheck, &'a nia_const_ir::ResolvedConstModule)>,
    pub program_function_signature_cache:
        Option<&'a RefCell<HashMap<GlobalDefId, ProgramFunctionSignature>>>,
    pub product: nia_body_check::BodyCheckProduct,
    pub prechecked: Option<nia_body_check::PrecheckedBodyCheck>,
}

type ExecutableProgramLayoutCache<'a> = (
    &'a RefCell<HashMap<ModuleId, ModuleLayouts>>,
    &'a RefCell<Option<QueryError>>,
);

pub(super) fn body_check_with_filter_and_layouts_with_inputs(
    db: &QueryDb<CompilerContext>,
    input: ExecutableBodyCheckInput<'_>,
) -> QueryResult<BodyCheckWithResolutionInputs> {
    let ExecutableBodyCheckInput {
        module_id,
        filter,
        layouts,
        program_layouts_override,
        fact_mode,
        resolution_inputs,
        seed,
        global_initializer_cache,
        const_module_cache,
        const_inputs,
        program_function_signature_cache,
        product,
        prechecked,
    } = input;
    let query_failure = RefCell::new(None);
    let source_version = *db.get(ModuleSourceVersionQuery(module_id))?;
    let origins = db.get(ModuleOriginsQuery(module_id))?;
    let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
    let defs = full_module_defs_semantic(db, module_id)?;
    let program_defs =
        |module_id| capture_query_failure(&query_failure, full_module_defs_semantic(db, module_id));
    let type_resolution = type_resolution_semantic(db, module_id)?;
    let lowered = type_lowering_semantic(db, module_id)?;
    let executable_reachable_filter = matches!(
        filter,
        nia_body_check::BodyCheckFilter::ReachableItems { .. }
            | nia_body_check::BodyCheckFilter::ReachableFunctions(_)
    );
    let filtered_inputs = match resolution_inputs {
        Some(inputs) => inputs,
        None => {
            let input_filter = if executable_reachable_filter {
                nia_body_check::BodyCheckFilter::All
            } else {
                filter
            };
            body_check_resolution_inputs_for_filter(
                db,
                module_id,
                input_filter,
                BodyCheckResolutionContext {
                    source_version,
                    origins: &origins,
                    active_item_tree: active_item_tree.clone(),
                    defs: &defs,
                    type_resolution: &type_resolution,
                    lowered: &lowered,
                },
            )?
        }
    };
    let inputs = &filtered_inputs;
    let source_path = db.get(ModulePathQuery(module_id))?;
    let signatures = body_local_item_signatures(db, module_id, &lowered)?;
    let normalization = db.get(TypeNormalizationQuery(module_id))?;
    let extension_method_normalization = |module_id| {
        capture_query_failure(
            &query_failure,
            signature_type_normalization_semantic(
                db,
                module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ),
        )
    };
    let mut filtered_const_inputs = None;
    let full_const_values;
    let full_const_array_lengths;
    let full_const_typed_facts;
    let full_const_module;
    let (body_const, const_module) = if let Some((const_eval, const_module)) = const_inputs {
        (
            nia_body_check::BodyConst {
                values: &const_eval.values,
                typed_values: &const_eval.typed_values,
                array_lengths: &const_eval.array_lengths,
            },
            const_module,
        )
    } else {
        match filter {
            nia_body_check::BodyCheckFilter::All
            | nia_body_check::BodyCheckFilter::ConstDeclarations => {
                full_const_values = db.get(ConstValuesQuery(module_id))?;
                full_const_array_lengths = db.get(ConstArrayLengthsQuery(module_id))?;
                full_const_typed_facts = db.get(ConstTypedFactsQuery(module_id))?;
                full_const_module = db.get(ConstModuleQuery(module_id))?;
                (
                    nia_body_check::BodyConst::from_phases(
                        &full_const_values,
                        &full_const_array_lengths,
                        &full_const_typed_facts,
                    ),
                    full_const_module.module.as_ref(),
                )
            }
            _ => {
                filtered_const_inputs = Some(time_module_provider(
                    db,
                    "executable_body_check.const_inputs",
                    module_id,
                    || {
                        const_inputs_for_body_check(
                            db,
                            ConstBodyModuleInput {
                                module_id,
                                defs: &defs,
                                source_path: &source_path,
                                signatures: &signatures,
                                normalization: &normalization.semantic,
                                lowered: &lowered,
                                resolution: inputs,
                            },
                            fact_mode,
                            global_initializer_cache,
                            const_module_cache,
                        )
                    },
                )?);
                let filtered = filtered_const_inputs
                    .as_ref()
                    .expect("filtered const inputs must be initialized");
                (
                    nia_body_check::BodyConst::from_phases(
                        &filtered.values,
                        &filtered.array_lengths,
                        &filtered.typed_facts,
                    ),
                    filtered.module.module.as_ref(),
                )
            }
        }
    };
    let layouts = match layouts {
        Some(layouts) => layouts,
        None => Arc::clone(&db.get(LayoutsQuery(module_id))?.semantic),
    };
    let program_layouts = |module_id| {
        if let Some(program_layouts) = program_layouts_override {
            match capture_query_failure(&query_failure, Ok(program_layouts(module_id))) {
                Some(Some(layouts)) => return Some(layouts),
                Some(None) => {}
                None => return None,
            }
        }
        capture_query_failure(&query_failure, db.get(SignatureLayoutsQuery(module_id)))
            .map(|layouts| Arc::clone(&layouts.semantic))
    };
    let empty_extensions = nia_defs::VisibleExtensionMethods::default();
    let lazy_extensions = || {
        capture_query_failure(&query_failure, db.get(VisibleExtensionsQuery(module_id)))
            .map(|extensions| extensions.methods.clone())
            .unwrap_or_default()
    };
    let empty_program_extension_methods = nia_defs::ExtensionMethods::default();
    let program_extension_methods = &empty_program_extension_methods;
    let program_extension_method_by_id = |def_id: GlobalDefId| {
        capture_query_failure(&query_failure, db.get(ExtensionMethodByIdQuery(def_id)))
            .and_then(|method| method.method.clone())
    };
    let program_extension_methods_named = |name: &SymbolId| {
        capture_query_failure(&query_failure, db.get(ExtensionMethodsNamedQuery(*name)))
            .map(|methods| methods.methods.clone())
            .unwrap_or_default()
    };
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_type_normalization_semantic(
                    db,
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            );
        }
        capture_query_failure(&query_failure, type_normalization_semantic(db, module_id))
    };
    let local_function_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ))?;
    let local_program_function_signature_cache =
        RefCell::new(HashMap::<GlobalDefId, ProgramFunctionSignature>::new());
    let program_function_signature = |def_id: GlobalDefId| {
        if let Some(cache) = program_function_signature_cache
            && let Some(signature) = cache.borrow().get(&def_id)
        {
            return Some(signature.clone());
        }
        if let Some(signature) = local_program_function_signature_cache.borrow().get(&def_id) {
            return Some(signature.clone());
        }
        if def_id.module_id == module_id {
            return local_function_signatures
                .semantic
                .functions
                .get(&def_id.def_id)
                .cloned()
                .map(|signature| {
                    let signature = ProgramFunctionSignature {
                        name: defs
                            .defs
                            .get(def_id.def_id)
                            .map(|def| def.name)
                            .unwrap_or_default(),
                        signature,
                    };
                    local_program_function_signature_cache
                        .borrow_mut()
                        .insert(def_id, signature.clone());
                    signature
                });
        }
        let signatures = capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Functions,
            )),
        )?;
        let signature = signatures.semantic.functions.get(&def_id.def_id).cloned()?;
        let defs =
            capture_query_failure(&query_failure, module_defs_semantic(db, def_id.module_id))?;
        let signature = ProgramFunctionSignature {
            name: defs
                .defs
                .get(def_id.def_id)
                .map(|def| def.name)
                .unwrap_or_default(),
            signature,
        };
        if let Some(cache) = program_function_signature_cache {
            cache.borrow_mut().insert(def_id, signature.clone());
        } else {
            local_program_function_signature_cache
                .borrow_mut()
                .insert(def_id, signature.clone());
        }
        Some(signature)
    };
    let program_global_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Values,
            )),
        )?
        .semantic
        .globals
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramGlobalSignature { signature })
    };
    let program_const_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Values,
            )),
        )?
        .semantic
        .consts
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramConstSignature { signature })
    };
    let program_struct_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .structs
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramStructSignature { signature })
    };
    let program_union_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .unions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramUnionSignature { signature })
    };
    let program_enum_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .enums
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramEnumSignature { signature })
    };
    let program_trait_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Traits,
            )),
        )?
        .semantic
        .traits
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTraitSignature { signature })
    };
    let program_type_alias_signature = |def_id: GlobalDefId| {
        capture_query_failure(
            &query_failure,
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            )),
        )?
        .semantic
        .type_aliases
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTypeAliasSignature { signature })
    };
    let program_traits_by_method_name = |name: &SymbolId| {
        capture_query_failure(&query_failure, db.get(ProgramTraitMethodIndexQuery))
            .map(|index| index.trait_ids_with_method_named(name))
            .unwrap_or_default()
    };
    let program_trait_owning_method = |method_id: GlobalDefId| {
        capture_query_failure(&query_failure, db.get(ProgramTraitMethodIndexQuery))?
            .trait_owning_method_id(method_id)
            .and_then(|trait_id| {
                program_trait_signature(trait_id).map(|signature| (trait_id, signature))
            })
    };
    let resolver_program_signatures = ProgramSignatureResolvers {
        function: &program_function_signature,
        global: &program_global_signature,
        const_eval: &program_const_signature,
        struct_: &program_struct_signature,
        union: &program_union_signature,
        enum_: &program_enum_signature,
        trait_: &program_trait_signature,
        type_alias: &program_type_alias_signature,
        trait_ids_with_method_named: &program_traits_by_method_name,
        trait_owning_method: &program_trait_owning_method,
    };
    let map_program_signatures =
        fact_mode
            .non_function_signatures
            .map(|signatures| ProgramNonFunctionSignatureMaps {
                globals: &signatures.globals,
                consts: &signatures.consts,
                structs: &signatures.structs,
                unions: &signatures.unions,
                enums: &signatures.enums,
                traits: &signatures.traits,
                type_aliases: &signatures.type_aliases,
                trait_method_index: &signatures.trait_method_index,
            });
    let program_signature_lookup = BodyProgramSignatureLookup {
        functions: &program_function_signature,
        fallback: resolver_program_signatures,
        maps: map_program_signatures,
    };
    let visible_trait_impls;
    let program_signatures = if let Some(signatures) = fact_mode.non_function_signatures {
        ProgramSignatureContext::new_indexed(
            &program_signature_lookup,
            &signatures.trait_impls,
            &signatures.trait_impl_index,
        )
    } else {
        visible_trait_impls = db.get(VisibleTraitImplsQuery(module_id))?;
        ProgramSignatureContext::new_indexed(
            &program_signature_lookup,
            &visible_trait_impls.trait_impls,
            &visible_trait_impls.trait_impl_index,
        )
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_item_signatures_semantic(
                    db,
                    module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            );
        }
        capture_query_failure(&query_failure, item_signatures_semantic(db, module_id))
    };
    let executable_program_const_array_lengths =
        RefCell::new(HashMap::<ModuleId, Arc<nia_const_check::ConstArrayLengths>>::new());
    let executable_program_const_values =
        RefCell::new(HashMap::<ModuleId, Arc<nia_const_check::ConstValues>>::new());
    let program_const_array_lengths = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            if !executable_program_const_array_lengths
                .borrow()
                .contains_key(&module_id)
                && let Some(array_lengths) = capture_query_failure(
                    &query_failure,
                    signature_const_array_lengths(db, module_id, fact_mode.non_function_signatures),
                )
            {
                executable_program_const_array_lengths
                    .borrow_mut()
                    .insert(module_id, Arc::new(array_lengths));
            }
            return executable_program_const_array_lengths
                .borrow()
                .get(&module_id)
                .cloned();
        }
        if let Some(signatures) = fact_mode.non_function_signatures {
            if !executable_program_const_array_lengths
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths = with_const_input_and_program_facts(
                    db,
                    module_id,
                    Some(signatures),
                    |module_id| fact_mode.signature_facts_for(module_id),
                    |input, module| {
                        let mut array_lengths =
                            nia_const_check::compute_module_const_array_lengths(input);
                        array_lengths.diagnostics.extend(module.diagnostics.clone());
                        array_lengths
                    },
                );
                if let Some(array_lengths) = capture_query_failure(&query_failure, array_lengths) {
                    executable_program_const_array_lengths
                        .borrow_mut()
                        .insert(module_id, Arc::new(array_lengths));
                }
            }
            return executable_program_const_array_lengths
                .borrow()
                .get(&module_id)
                .cloned();
        }
        capture_query_failure(&query_failure, db.get(ConstArrayLengthsQuery(module_id)))
    };
    let program_const_values = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            if !executable_program_const_values
                .borrow()
                .contains_key(&module_id)
                && let Some(values) = capture_query_failure(
                    &query_failure,
                    signature_const_values(db, module_id, fact_mode.non_function_signatures),
                )
            {
                executable_program_const_values
                    .borrow_mut()
                    .insert(module_id, Arc::new(values));
            }
            return executable_program_const_values
                .borrow()
                .get(&module_id)
                .cloned();
        }
        if let Some(signatures) = fact_mode.non_function_signatures {
            if !executable_program_const_values
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths = program_const_array_lengths(module_id)?;
                let enum_values = with_const_input_and_program_facts(
                    db,
                    module_id,
                    Some(signatures),
                    |module_id| fact_mode.signature_facts_for(module_id),
                    |input, module| {
                        let mut enum_values = nia_const_check::compute_module_const_enum_values(
                            input,
                            Arc::unwrap_or_clone(Arc::clone(&array_lengths)),
                        );
                        enum_values.diagnostics.extend(module.diagnostics.clone());
                        enum_values
                    },
                );
                let enum_values = capture_query_failure(&query_failure, enum_values)?;
                let values = with_const_input_and_program_facts(
                    db,
                    module_id,
                    Some(signatures),
                    |module_id| fact_mode.signature_facts_for(module_id),
                    |input, module| {
                        let mut values = nia_const_check::compute_module_const_values(
                            input,
                            Arc::unwrap_or_clone(array_lengths),
                            enum_values,
                        );
                        values.diagnostics.extend(module.diagnostics.clone());
                        values
                    },
                );
                if let Some(values) = capture_query_failure(&query_failure, values) {
                    executable_program_const_values
                        .borrow_mut()
                        .insert(module_id, Arc::new(values));
                }
            }
            return executable_program_const_values
                .borrow()
                .get(&module_id)
                .cloned();
        }
        capture_query_failure(&query_failure, db.get(ConstValuesQuery(module_id)))
    };
    let program_const_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return capture_query_failure(
                &query_failure,
                signature_const_module_lowering(db, module_id),
            )
            .map(|lowering| lowering.module.clone());
        }
        capture_query_failure(&query_failure, db.get(ConstModuleQuery(module_id)))
            .map(|lowering| lowering.module.clone())
    };
    let program_visible_extensions = |module_id| {
        capture_query_failure(&query_failure, db.get(VisibleExtensionsQuery(module_id)))
            .map(|extensions| extensions.methods.clone())
    };
    let program_module_source_path = |module_id| {
        capture_query_failure(
            &query_failure,
            db.context().loader_facts().module_path(module_id),
        )
        .flatten()
    };
    let target = db.get(CompilerTargetQuery)?;
    let run_body_check =
        |inputs: &BodyCheckResolutionInputs,
         body_const: nia_body_check::BodyConst<'_>,
         const_module: &nia_const_ir::ResolvedConstModule,
         filter: nia_body_check::BodyCheckFilter<'_>,
         prechecked: Option<nia_body_check::PrecheckedBodyCheck>| {
            nia_body_check::check_module_bodies_with_program_signatures_and_layouts_with_timings(
                nia_body_check::BodyCheckInput {
                    type_store: &db.context().type_store,
                    source_version: Some(source_version),
                    source_path: &source_path,
                    symbols: &db.context().symbols(),
                    origins: &origins,
                    active_item_tree: &inputs.active_item_tree,
                    defs: &defs,
                    values: &inputs.values,
                    locals: &inputs.locals,
                    semantic_uses: &inputs.semantic_uses,
                    lowered: &lowered,
                    signatures: nia_body_check::BodyLocalSignatures::from_item_signatures(
                        &signatures,
                    ),
                    const_signatures: &signatures,
                    normalization: &normalization.semantic,
                    seed,
                    target: target.as_ref(),
                    const_eval: body_const,
                    const_module,
                    layouts: &layouts,
                    extensions: &empty_extensions,
                    lazy_extensions: Some(&lazy_extensions),
                    program_extension_methods,
                    program: nia_body_check::BodyProgramContext {
                        defs: Some(&program_defs),
                        module_source_path: Some(&program_module_source_path),
                        type_normalizations: Some(&program_type_normalization),
                        extension_type_normalizations: Some(&extension_method_normalization),
                        signatures: Some(&item_signatures_for_module),
                        layouts: Some(&program_layouts),
                        visible_extensions: Some(&program_visible_extensions),
                        extension_method_by_id: Some(&program_extension_method_by_id),
                        extension_methods_named: Some(&program_extension_methods_named),
                    },
                    program_signatures,
                    function_scope: nia_body_check::FunctionCheckScope::ProgramSignatures,
                    program_const: nia_body_check::ProgramConstMaps {
                        values: &program_const_values,
                        array_lengths: &program_const_array_lengths,
                        module: &program_const_module,
                    },
                    filter,
                    product,
                    prechecked,
                },
                db.context().timings(),
            )
        };
    let body_check = run_body_check(inputs, body_const, const_module, filter, prechecked);
    let stored_inputs = match (product, filter) {
        (
            nia_body_check::BodyCheckProduct::FactsOnly
            | nia_body_check::BodyCheckProduct::BodyOnly
            | nia_body_check::BodyCheckProduct::StaticInitOnly,
            _,
        ) => filtered_inputs,
        (
            nia_body_check::BodyCheckProduct::Full,
            nia_body_check::BodyCheckFilter::ReachableItems {
                globals,
                already_checked_functions,
                already_checked_globals,
                ..
            },
        ) => {
            let checked_functions = body_check.checked_functions.clone();
            let stored_filter = nia_body_check::BodyCheckFilter::ReachableItems {
                functions: &checked_functions,
                globals,
                already_checked_functions,
                already_checked_globals,
            };
            body_check_resolution_inputs_for_filter(
                db,
                module_id,
                stored_filter,
                BodyCheckResolutionContext {
                    source_version,
                    origins: &origins,
                    active_item_tree: active_item_tree.clone(),
                    defs: &defs,
                    type_resolution: &type_resolution,
                    lowered: &lowered,
                },
            )?
        }
        (
            nia_body_check::BodyCheckProduct::Full,
            nia_body_check::BodyCheckFilter::ReachableFunctions(_)
            | nia_body_check::BodyCheckFilter::All
            | nia_body_check::BodyCheckFilter::ConstDeclarations,
        ) => filtered_inputs,
    };
    let output = BodyCheckWithResolutionInputs {
        body_check,
        inputs: stored_inputs,
        const_eval: filtered_const_inputs.map(BodyCheckConstInputs::into_check),
    };
    match query_failure.into_inner() {
        Some(error) => Err(error),
        None => Ok(output),
    }
}

pub(super) fn executable_layouts_for_reachable_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    array_length_cache: Option<&RefCell<HashMap<ModuleId, nia_const_check::ConstArrayLengths>>>,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
    reachable_body_modules_override: Option<ReachableBodyModules<'_>>,
) -> QueryResult<nia_layout::Layouts> {
    time_module_provider(db, "executable_layouts", module_id, || {
        let query_failure = RefCell::new(None);
        let defs = full_module_defs_semantic(db, module_id)?;
        let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
        let type_lowering = type_lowering_semantic(db, module_id)?;
        let type_normalization = db.get(LayoutTypeNormalizationQuery(module_id))?;
        let item_signatures = item_signatures_semantic(db, module_id)?;
        let program_struct = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )?
            .semantic
            .structs
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramStructSignature { signature })
        };
        let program_union = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )?
            .semantic
            .unions
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramUnionSignature { signature })
        };
        let program_enum = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )?
            .semantic
            .enums
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramEnumSignature { signature })
        };
        let program_type_alias = |def_id: GlobalDefId| {
            capture_query_failure(
                &query_failure,
                db.get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                )),
            )?
            .semantic
            .type_aliases
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramTypeAliasSignature { signature })
        };
        let load_filtered_array_lengths = |target_module_id| {
            let has_reachable_body_items = reachable_body_modules_override
                .map(|modules| modules.contains(target_module_id))
                .map(Ok)
                .unwrap_or_else(|| {
                    has_reachable_executable_body_items(
                        db,
                        target_module_id,
                        reachable_functions,
                        reachable_globals,
                    )
                })?;
            if has_reachable_body_items {
                with_const_input_and_program_facts(
                    db,
                    target_module_id,
                    non_function_signatures_override,
                    |module_id| {
                        reachable_body_modules_override
                            .map(|modules| !modules.contains(module_id))
                            .unwrap_or_else(|| {
                                capture_query_failure(
                                    &query_failure,
                                    has_reachable_executable_body_items(
                                        db,
                                        module_id,
                                        reachable_functions,
                                        reachable_globals,
                                    ),
                                )
                                .is_some_and(|has_reachable_items| !has_reachable_items)
                            })
                    },
                    |input, module| {
                        let mut array_lengths =
                            nia_const_check::compute_module_const_array_lengths(input);
                        array_lengths.diagnostics.extend(module.diagnostics.clone());
                        array_lengths
                    },
                )
            } else {
                with_type_signature_const_input(
                    db,
                    target_module_id,
                    non_function_signatures_override,
                    |input, module| {
                        let mut array_lengths =
                            nia_const_check::compute_module_const_array_lengths(input);
                        array_lengths.diagnostics.extend(module.diagnostics.clone());
                        array_lengths
                    },
                )
            }
        };
        let local_array_lengths = if let Some(array_length_cache) = array_length_cache {
            if !array_length_cache.borrow().contains_key(&module_id) {
                let array_lengths = load_filtered_array_lengths(module_id)?;
                array_length_cache
                    .borrow_mut()
                    .insert(module_id, array_lengths);
            }
            array_length_cache
                .borrow()
                .get(&module_id)
                .cloned()
                .expect("local executable array lengths must be cached")
        } else {
            load_filtered_array_lengths(module_id)?
        };
        let signature_array_lengths = RefCell::new(HashMap::new());
        let target = compiler_target_data_layout(db)?;
        let executable_array_lengths = |id: nia_ids::GlobalConstExprId| {
            if id.module_id == module_id {
                return local_array_lengths.values.get(&id).copied();
            }
            if !signature_array_lengths.borrow().contains_key(&id.module_id)
                && let Some(array_lengths) = capture_query_failure(
                    &query_failure,
                    with_type_signature_const_input(
                        db,
                        id.module_id,
                        non_function_signatures_override,
                        |input, module| {
                            let mut array_lengths =
                                nia_const_check::compute_module_const_array_lengths(input);
                            array_lengths.diagnostics.extend(module.diagnostics.clone());
                            array_lengths
                        },
                    ),
                )
            {
                signature_array_lengths
                    .borrow_mut()
                    .insert(id.module_id, array_lengths);
            }
            signature_array_lengths
                .borrow()
                .get(&id.module_id)
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        let layouts = time_module_provider(db, "executable_layouts.compute", module_id, || {
            let symbols = db.context().symbols();
            let roots = time_module_provider(db, "executable_layouts.roots", module_id, || {
                executable_layout_roots(
                    ExecutableLayoutModule {
                        module_id,
                        signatures: &item_signatures,
                        program_struct: &program_struct,
                        program_union: &program_union,
                    },
                    &db.context().type_store,
                    type_lowering
                        .versioned_type_uses_from_active_item_tree(&active_item_tree)
                        .into_iter()
                        .map(|(_, ty)| ty),
                    reachable_functions,
                    reachable_globals,
                )
            });
            nia_layout::compute_layouts_for_roots_with_program_context(
                nia_layout::LayoutComputationInput {
                    type_store: &db.context().type_store,
                    defs: &defs,
                    signatures: &item_signatures,
                    root_types: &[],
                    normalized: &type_normalization.normalized,
                    array_lengths: &executable_array_lengths,
                    target,
                    program: nia_layout::ProgramLayoutContext {
                        symbols: Some(&symbols),
                        array_lengths: Some(&executable_array_lengths),
                        struct_: Some(&program_struct),
                        union: Some(&program_union),
                        enum_: Some(&program_enum),
                        type_alias: Some(&program_type_alias),
                        ..Default::default()
                    },
                },
                nia_layout::LayoutRoots {
                    types: &roots.types,
                    structs: &roots.structs,
                    unions: &roots.unions,
                },
            )
        });
        match query_failure.into_inner() {
            Some(error) => Err(error),
            None => Ok(layouts),
        }
    })
}

pub(super) fn executable_program_layouts<'a>(
    db: &'a QueryDb<CompilerContext>,
    cache_and_failure: ExecutableProgramLayoutCache<'a>,
    reachable_functions: &'a HashSet<GlobalDefId>,
    reachable_globals: &'a HashSet<GlobalDefId>,
    array_length_cache: Option<&'a RefCell<HashMap<ModuleId, nia_const_check::ConstArrayLengths>>>,
    non_function_signatures_override: Option<&'a ProgramExecutableNonFunctionSignatures>,
    reachable_body_modules_override: Option<ReachableBodyModules<'a>>,
) -> impl Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>> + 'a {
    let (cache, failure) = cache_and_failure;
    move |module_id| {
        if let Some(layouts) = cache.borrow().get(&module_id).cloned() {
            return Some(layouts.semantic);
        }
        let has_reachable_body_items = reachable_body_modules_override
            .map(|modules| modules.contains(module_id))
            .unwrap_or_else(|| {
                capture_query_failure(
                    failure,
                    has_reachable_executable_body_items(
                        db,
                        module_id,
                        reachable_functions,
                        reachable_globals,
                    ),
                )
                .unwrap_or(false)
            });
        let layouts = if has_reachable_body_items {
            capture_query_failure(
                failure,
                executable_layouts_for_reachable_items(
                    db,
                    module_id,
                    reachable_functions,
                    reachable_globals,
                    array_length_cache,
                    non_function_signatures_override,
                    reachable_body_modules_override,
                ),
            )?
        } else {
            capture_query_failure(
                failure,
                signature_layouts_for_types(db, module_id, non_function_signatures_override),
            )?
        };
        let layouts = store_module_layouts(db.context(), layouts);
        cache.borrow_mut().insert(module_id, layouts.clone());
        Some(layouts.semantic)
    }
}

pub(super) fn provide_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> QueryResult<CheckedModule> {
    time_module_provider(db, "checked_module", module_id, || {
        checked_module_with_body_and_flow_check(
            db,
            module_id,
            db.get(BodyCheckQuery(module_id))?,
            db.get(FlowCheckQuery(module_id))?,
            None,
        )
    })
}

fn checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: Arc<ModuleBodyCheck>,
    flow_check: Arc<ModuleFlowCheck>,
    layouts: Option<Arc<nia_layout::Layouts>>,
) -> QueryResult<CheckedModule> {
    let path = db.get(ModulePathQuery(module_id))?.as_ref().clone();
    let type_resolution = db.get(TypeResolutionQuery(module_id))?;
    let type_lowering = db.get(TypeLoweringQuery(module_id))?;
    let value_resolution = db.get(ValueResolutionQuery(module_id))?;
    let local_resolution = db.get(LocalResolutionQuery(module_id))?;
    let item_signatures = db.get(ItemSignaturesQuery(module_id))?;
    let type_normalization = db.get(TypeNormalizationQuery(module_id))?;
    let query_layouts = layouts
        .is_none()
        .then(|| db.get(LayoutsQuery(module_id)))
        .transpose()?;
    let const_eval = db.get(ConstQuery(module_id))?;
    let static_check = db.get(StaticCheckQuery(module_id))?;
    let abi_check = db.get(AbiCheckQuery(module_id))?;
    let definitions = db.get(FullModuleDefsQuery(module_id))?;
    Ok(CheckedModule {
        id: module_id,
        path,
        defs: Arc::clone(&definitions.semantic),
        definition_diagnostics: definitions.diagnostics.clone(),
        type_resolution: Arc::clone(&type_resolution.semantic),
        type_lowering: Arc::clone(&type_lowering.semantic),
        value_resolution: Arc::clone(&value_resolution.semantic),
        local_resolution: Arc::clone(&local_resolution.semantic),
        type_normalization: Arc::clone(&type_normalization.semantic),
        const_eval: Arc::clone(&const_eval.semantic),
        static_check: Arc::clone(&static_check.semantic),
        layouts: match layouts {
            Some(layouts) => layouts,
            None => Arc::clone(
                &query_layouts
                    .as_ref()
                    .expect("layouts query must run")
                    .semantic,
            ),
        },
        abi_check: Arc::clone(&abi_check.semantic),
        flow_check: Arc::clone(&flow_check.semantic),
        body_ir: Arc::clone(&body_check.semantic.ir),
        semantic_uses: db.get(SemanticUseTableQuery(module_id))?,
        semantic_facts: Arc::clone(&body_check.semantic.facts),
        provider_demands: Arc::clone(&body_check.semantic.provider_demands),
        executable_reachable_globals: None,
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: false,
        body_diagnostics: body_check.diagnostics.clone(),
        frontend_diagnostics: vec![
            type_resolution.diagnostics.clone(),
            type_normalization.diagnostics.clone(),
            type_lowering.diagnostics.clone(),
        ],
        resolution_diagnostics: vec![
            value_resolution.diagnostics.clone(),
            local_resolution.diagnostics.clone(),
        ],
        item_diagnostics: item_signatures.diagnostics.clone(),
        const_diagnostics: const_eval.diagnostics.clone(),
        static_diagnostics: static_check.diagnostics.clone(),
        layout_diagnostics: query_layouts
            .as_ref()
            .map(|layouts| layouts.diagnostics.clone())
            .unwrap_or_else(|| db.context().diagnostic_store.bundle(Vec::new())),
        abi_diagnostics: abi_check.diagnostics.clone(),
        flow_diagnostics: flow_check.diagnostics.clone(),
    })
}

pub(super) fn executable_checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: BodyCheckWithResolutionInputs,
    mut flow_check: nia_flow_check::FlowCheck,
    layouts: ModuleLayouts,
) -> QueryResult<CheckedModule> {
    let BodyCheckWithResolutionInputs {
        body_check,
        inputs: body_inputs,
        const_eval,
    } = body_check;
    let type_resolution = db.get(TypeResolutionQuery(module_id))?;
    let type_lowering = db.get(TypeLoweringQuery(module_id))?;
    let item_signatures = db.get(ItemSignaturesQuery(module_id))?;
    let type_normalization = db.get(TypeNormalizationQuery(module_id))?;
    let definitions = db.get(FullModuleDefsQuery(module_id))?;
    let flow_diagnostics = db
        .context()
        .diagnostic_store
        .bundle(std::mem::take(&mut flow_check.diagnostics));
    let (const_eval, const_diagnostics) = match const_eval {
        Some(mut const_eval) => {
            let diagnostics = db
                .context()
                .diagnostic_store
                .bundle(std::mem::take(&mut const_eval.diagnostics));
            (Arc::new(const_eval), diagnostics)
        }
        None => {
            let const_eval = db.get(ConstQuery(module_id))?;
            (
                Arc::clone(&const_eval.semantic),
                const_eval.diagnostics.clone(),
            )
        }
    };
    Ok(CheckedModule {
        id: module_id,
        path: db.get(ModulePathQuery(module_id))?.as_ref().clone(),
        defs: Arc::clone(&definitions.semantic),
        definition_diagnostics: definitions.diagnostics.clone(),
        type_resolution: Arc::clone(&type_resolution.semantic),
        type_lowering: Arc::clone(&type_lowering.semantic),
        value_resolution: body_inputs.values,
        local_resolution: body_inputs.locals,
        type_normalization: Arc::clone(&type_normalization.semantic),
        const_eval,
        static_check: Arc::new(nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        }),
        layouts: layouts.semantic,
        abi_check: Arc::new(nia_abi_check::AbiCheck {
            diagnostics: Vec::new(),
        }),
        flow_check: Arc::new(flow_check),
        body_ir: body_check.ir,
        semantic_uses: body_inputs.semantic_uses,
        semantic_facts: body_check.facts,
        provider_demands: body_check.provider_demands,
        executable_reachable_globals: None,
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: false,
        body_diagnostics: db
            .context()
            .diagnostic_store
            .bundle_shared(body_check.diagnostics),
        frontend_diagnostics: vec![
            type_resolution.diagnostics.clone(),
            type_normalization.diagnostics.clone(),
            type_lowering.diagnostics.clone(),
        ],
        resolution_diagnostics: body_inputs.resolution_diagnostics,
        item_diagnostics: item_signatures.diagnostics.clone(),
        const_diagnostics,
        static_diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        layout_diagnostics: layouts.diagnostics,
        abi_diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        flow_diagnostics,
    })
}

pub(super) fn executable_signature_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    layouts: ModuleLayouts,
    program_signatures: &ProgramExecutableNonFunctionSignatures,
) -> QueryResult<CheckedModule> {
    let signature_type_resolution = db.get(SignatureTypeResolutionQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ))?;
    let type_lowering = db.get(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ))?;
    let item_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ))?;
    let type_normalization = db.get(SignatureTypeNormalizationQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ))?;
    let definitions = db.get(ModuleDefsQuery(module_id))?;
    let (array_lengths, enum_values) = with_type_signature_const_input(
        db,
        module_id,
        Some(program_signatures),
        |input, module| {
            let mut array_lengths = nia_const_check::compute_module_const_array_lengths(input);
            array_lengths.diagnostics.extend(module.diagnostics.clone());
            let mut enum_values =
                nia_const_check::compute_module_const_enum_values(input, array_lengths.clone());
            enum_values.diagnostics.extend(module.diagnostics.clone());
            (array_lengths, enum_values)
        },
    )?;
    let mut const_diagnostics = array_lengths.diagnostics.clone();
    const_diagnostics.extend(enum_values.diagnostics.clone());
    let mut const_eval = ConstCheck {
        values: Arc::new(HashMap::new()),
        typed_values: Arc::new(HashMap::new()),
        enum_values: enum_values.values,
        typed_enum_values: enum_values.typed_values,
        array_lengths: array_lengths.values,
        diagnostics: const_diagnostics,
    };
    let const_diagnostics = db
        .context()
        .diagnostic_store
        .bundle(std::mem::take(&mut const_eval.diagnostics));
    Ok(CheckedModule {
        id: module_id,
        path: db.get(ModulePathQuery(module_id))?.as_ref().clone(),
        defs: Arc::clone(&definitions.semantic),
        definition_diagnostics: definitions.diagnostics.clone(),
        type_resolution: Arc::clone(&signature_type_resolution.semantic),
        type_lowering: Arc::clone(&type_lowering.semantic),
        value_resolution: Arc::new(ValueResolution::with_store(db.context().node_store())),
        local_resolution: Arc::new(nia_local_resolve::LocalResolution::with_store(
            db.context().node_store(),
        )),
        type_normalization: Arc::clone(&type_normalization.semantic),
        const_eval: Arc::new(const_eval),
        static_check: Arc::new(nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        }),
        layouts: layouts.semantic,
        abi_check: Arc::new(nia_abi_check::AbiCheck {
            diagnostics: Vec::new(),
        }),
        flow_check: Arc::new(nia_flow_check::FlowCheck {
            diagnostics: Vec::new(),
        }),
        body_ir: Arc::new(nia_body_ir::BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        }),
        semantic_uses: Arc::new(nia_sema_ir::SemanticUseTable::default()),
        semantic_facts: Arc::new(nia_sema_ir::SemanticFacts::default()),
        provider_demands: Arc::new(HashSet::new()),
        executable_reachable_globals: Some(HashSet::new()),
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: true,
        body_diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        frontend_diagnostics: vec![
            signature_type_resolution.diagnostics.clone(),
            type_lowering.diagnostics.clone(),
            item_signatures.diagnostics.clone(),
            type_normalization.diagnostics.clone(),
        ],
        resolution_diagnostics: Vec::new(),
        item_diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        const_diagnostics,
        static_diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        layout_diagnostics: layouts.diagnostics,
        abi_diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
        flow_diagnostics: db.context().diagnostic_store.bundle(Vec::new()),
    })
}

pub(super) fn extend_module_functions_from_filtered_value_refs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    mut module_functions: HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> QueryResult<HashSet<GlobalDefId>> {
    time_module_provider(db, "extend_value_refs.scan_refs", module_id, || {
        walk_executable_value_ref_closure(
            db,
            module_id,
            &mut module_functions,
            module_globals,
            checked_functions,
            |_| false,
            |_| false,
        )
    })?;
    Ok(module_functions)
}

pub(super) fn executable_value_ref_edges_from_reachable_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    module_functions: &HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
) -> QueryResult<ExecutableValueRefEdges> {
    let mut scan_functions = module_functions.clone();
    let mut all_edges = ExecutableValueRefEdges::default();
    time_module_provider(db, "executable_value_refs.scan_refs", module_id, || {
        walk_executable_value_ref_closure(
            db,
            module_id,
            &mut scan_functions,
            module_globals,
            None,
            |def_id| all_edges.functions.insert(def_id),
            |def_id| all_edges.globals.insert(def_id),
        )
    })?;
    Ok(all_edges)
}

pub(in crate::query) fn provide_executable_value_ref_edges(
    db: &QueryDb<CompilerContext>,
    owner: GlobalDefId,
) -> QueryResult<ExecutableValueRefEdges> {
    time_module_provider(db, "executable_value_ref_edges", owner.module_id, || {
        let program_sources = db.get(FrontendProgramSourcesQuery)?;
        let cache_input = program_sources
            .as_ref()
            .as_ref()
            .and_then(|program_sources| {
                let source = program_sources.by_module.get(&owner.module_id)?;
                let namespace = db.context().frontend_cache_namespace();
                let key = crate::FrontendExecutableValueRefEdgesCacheKey::new(
                    namespace,
                    &source.module,
                    owner.def_id,
                    program_sources.fingerprint,
                );
                Some((program_sources, source, namespace, key))
            });
        let cached = if let Some(cache) = db.context().signature_cache.as_ref()
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            match cache.load_executable_value_ref_edges(
                crate::signature_cache::ExecutableValueRefEdgesIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    owner: owner.def_id,
                    program_sources: program_sources.fingerprint,
                },
                &program_sources.module_by_path,
            ) {
                Ok(lookup) => {
                    match lookup {
                        crate::signature_cache::ExecutableValueRefEdgesLookup::Hit(_) => {
                            nia_timing::emit_counter(
                                "frontend.executable_value_ref_edges_reuse_hits",
                                1,
                            );
                        }
                        crate::signature_cache::ExecutableValueRefEdgesLookup::NotFound => {
                            nia_timing::emit_counter(
                                "frontend.executable_value_ref_edges_reuse_miss_not_found",
                                1,
                            );
                        }
                        crate::signature_cache::ExecutableValueRefEdgesLookup::Corrupt => {
                            nia_timing::emit_counter(
                                "frontend.executable_value_ref_edges_reuse_miss_corrupt",
                                1,
                            );
                        }
                    }
                    Some(lookup)
                }
                Err(_) => {
                    nia_timing::emit_counter(
                        "frontend.executable_value_ref_edges_reuse_miss_read_error",
                        1,
                    );
                    None
                }
            }
        } else {
            None
        };
        let cached = if db.context().verify_frontend_cache {
            cached
        } else {
            match cached {
                Some(crate::signature_cache::ExecutableValueRefEdgesLookup::Hit(cached)) => {
                    return Ok(ExecutableValueRefEdges {
                        functions: cached.functions,
                        globals: cached.globals,
                    });
                }
                cached => cached,
            }
        };

        let edges = if let Some(item_input) = db.get(ExecutableValueRefItemQuery(owner))?.as_ref() {
            let full_active_item_tree = db.get(FullActiveModuleItemTreeQuery(owner.module_id))?;
            let active_item_tree =
                executable_value_ref_active_item_tree(item_input, &full_active_item_tree);
            let defs = module_defs_semantic(db, owner.module_id)?;
            let query_failure = RefCell::new(None);
            let program_defs = |module_id| {
                capture_query_failure(&query_failure, module_defs_semantic(db, module_id))
            };
            let graph = QueryModuleGraphLookup::new(db)?;
            let public_surfaces = QueryPublicSurfaceLookup::new(db);
            let using_scope = QueryUsingScopeLookup::new(db, owner.module_id);
            let visible_extensions = || db.get(VisibleExtensionsQuery(owner.module_id));
            let associated_values =
                LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
            let symbols = db.context().symbols();
            let values = nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols_in_store(
                &active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&graph),
                },
                &public_surfaces,
                &using_scope,
                nia_value_resolve::ValueResolveOptions::with_store(
                    Some(&associated_values),
                    Some(&symbols),
                    db.context().node_store(),
                ),
            );
            if let Some(error) = query_failure
                .into_inner()
                .or_else(|| graph.take_failure())
                .or_else(|| public_surfaces.take_failure())
                .or_else(|| using_scope.take_failure())
                .or_else(|| associated_values.take_failure())
            {
                return Err(error);
            }
            let origins = nia_node_id::NodeOriginTable::with_store(db.context().node_store());
            let locals =
                nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
                    &active_item_tree,
                    &full_active_item_tree,
                    &defs,
                    &values,
                    None,
                    &origins,
                    &symbols,
                );
            let mut index = ExecutableValueRefIndex::default();
            collect_executable_value_ref_index_for_items(
                db,
                owner.module_id,
                &active_item_tree.items,
                &defs,
                &values,
                &locals,
                &mut index,
            )?;
            index
                .functions
                .remove(&owner)
                .or_else(|| index.globals.remove(&owner))
                .unwrap_or_default()
        } else {
            ExecutableValueRefEdges::default()
        };

        if let Some(cache) = &db.context().signature_cache
            && let Some((program_sources, source, namespace, key)) = cache_input
        {
            let stable_edges = crate::signature_cache::CachedExecutableValueRefEdges {
                functions: edges.functions.clone(),
                globals: edges.globals.clone(),
            };
            let replace = matches!(
                &cached,
                Some(crate::signature_cache::ExecutableValueRefEdgesLookup::Hit(cached))
                    if cached != &stable_edges
            );
            if replace {
                cache.remove_executable_value_ref_edges(key);
            }
            let published = cache.publish_executable_value_ref_edges(
                crate::signature_cache::ExecutableValueRefEdgesIdentity {
                    key,
                    namespace,
                    module: &source.module,
                    owner: owner.def_id,
                    program_sources: program_sources.fingerprint,
                },
                &stable_edges,
                &program_sources.path_by_module,
                replace,
            );
            nia_timing::emit_counter(
                if published.is_ok() {
                    "frontend.executable_value_ref_edges_cacheable"
                } else {
                    "frontend.executable_value_ref_edges_uncacheable"
                },
                1,
            );
        }
        Ok(edges)
    })
}

pub(super) fn provide_checked_module_ids(
    db: &QueryDb<CompilerContext>,
) -> QueryResult<Vec<ModuleId>> {
    time_provider(db.context().timings(), "checked_module_ids", || {
        let module_ids = db.get(SemanticModuleIdsQuery)?;
        let _graph = db.get(ModuleGraphQuery)?;
        resolve_stable_module_sequence_from_current_inputs(db, &module_ids)
    })
}

pub(super) fn extend_module_functions_from_local_static_globals(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    mut module_functions: HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> QueryResult<HashSet<GlobalDefId>> {
    let defs = full_module_defs_semantic(db, module_id)?;
    for global in module_globals {
        let Some(def) = defs.defs.get(global.def_id) else {
            continue;
        };
        if def.kind != DefKind::Global {
            continue;
        }
        let Some(owner) = def.parent else {
            continue;
        };
        let owner = GlobalDefId {
            module_id,
            def_id: owner,
        };
        if checked_functions.is_some_and(|checked| checked.contains(&owner)) {
            continue;
        }
        module_functions.insert(owner);
    }
    Ok(module_functions)
}

pub(super) fn filter_checked_module_for_codegen(
    mut module: CheckedModule,
    db: &QueryDb<CompilerContext>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> QueryResult<CheckedModule> {
    Arc::make_mut(&mut module.body_ir)
        .function_bodies
        .retain(|def_id, _| reachable_functions.contains(def_id));
    Arc::make_mut(&mut module.body_ir)
        .global_inits
        .retain(|def_id, _| reachable_globals.contains(def_id));
    module.semantic_facts = Arc::new(filter_semantic_facts_for_reachable_items(
        Arc::unwrap_or_clone(module.semantic_facts),
        reachable_functions,
        reachable_globals,
    ));
    let layouts = rooted_layouts_for_checked_module(
        db,
        &module,
        program_layouts_override,
        program_array_lengths_override,
    )?;
    module.layouts = layouts.semantic;
    module.layout_diagnostics = layouts.diagnostics;
    module.executable_reachable_globals = Some(reachable_globals.clone());
    Ok(module)
}

pub(super) fn executable_reachable_aggregate_roots(
    type_store: &nia_ty::TypeStore,
    function_signature: &dyn Fn(GlobalDefId) -> Option<Arc<ProgramFunctionSignature>>,
    struct_signature: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    union_signature: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    modules: &[CheckedModule],
) -> ExecutableReachableAggregateRoots {
    let mut structs = HashSet::new();
    let mut unions = HashSet::new();
    for module in modules {
        let mut roots = LayoutRootCollector::with_program_including_local_aggregates(
            type_store,
            module.id,
            struct_signature,
            union_signature,
        );
        collect_semantic_layout_roots(&module.semantic_facts, &mut roots);
        for (def_id, def) in module.defs.defs.iter() {
            if !matches!(def.kind, DefKind::Function | DefKind::Method) {
                continue;
            }
            let def_id = GlobalDefId {
                module_id: module.id,
                def_id,
            };
            let Some(signature) = function_signature(def_id) else {
                continue;
            };
            if !signature.signature.is_extern {
                continue;
            }
            for param in &signature.signature.params {
                roots.add(param.ty);
            }
            roots.add(signature.signature.return_type);
        }
        let roots = roots.finish_global();
        structs.extend(roots.structs);
        unions.extend(roots.unions);
    }
    ExecutableReachableAggregateRoots { structs, unions }
}

pub(super) struct ExecutableReachableAggregateRoots {
    pub(super) structs: HashSet<GlobalDefId>,
    pub(super) unions: HashSet<GlobalDefId>,
}

pub(super) fn executable_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
) -> QueryResult<nia_flow_check::FlowCheck> {
    time_module_provider(db, "executable_flow_check", module_id, || {
        let active_item_tree = db.get(FullActiveModuleItemTreeQuery(module_id))?;
        let signatures = db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))?;
        Ok(
            nia_flow_check::check_active_module_flow_with_signatures_and_filter(
                &active_item_tree,
                db.context().type_store(),
                nia_flow_check::FlowCheckSignatures {
                    functions: &signatures.semantic.functions,
                },
                nia_flow_check::FlowCheckFilter::ReachableFunctions {
                    module_id,
                    functions: reachable_functions,
                },
            ),
        )
    })
}
