// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

struct LocalExecutableValueRefs<'a> {
    module_id: ModuleId,
    defs: &'a DefCollection,
    values: &'a ValueResolution,
    signatures: &'a HashMap<DefId, nia_item_signatures::FunctionSignature>,
}

#[derive(Default)]
pub(super) struct ExecutableValueRefEdges {
    pub(super) functions: HashSet<GlobalDefId>,
    pub(super) globals: HashSet<GlobalDefId>,
}

struct BodyCheckComptimeInputs {
    module: ComptimeModuleLowering,
    array_lengths: nia_comptime_check::ComptimeArrayLengths,
    enum_values: nia_comptime_check::ComptimeEnumValues,
    values: nia_comptime_check::ComptimeValues,
    typed_facts: nia_comptime_check::ComptimeTypedFacts,
}

#[derive(Clone, Copy)]
pub(super) struct ExecutableFactMode<'a> {
    program_signatures: Option<&'a ProgramExecutableSignatures>,
    reachable_body_modules: Option<&'a HashSet<ModuleId>>,
}

impl<'a> ExecutableFactMode<'a> {
    fn full() -> Self {
        Self {
            program_signatures: None,
            reachable_body_modules: None,
        }
    }

    pub(super) fn executable(reachable_body_modules: &'a HashSet<ModuleId>) -> Self {
        Self {
            program_signatures: None,
            reachable_body_modules: Some(reachable_body_modules),
        }
    }

    fn signature_facts_for(self, module_id: ModuleId) -> bool {
        if let Some(reachable_body_modules) = self.reachable_body_modules {
            return !reachable_body_modules.contains(&module_id);
        }
        self.program_signatures.is_some()
    }
}

impl BodyCheckComptimeInputs {
    fn into_check(self) -> ComptimeCheck {
        ComptimeCheck {
            interner: self.typed_facts.interner,
            values: self.values.values,
            typed_values: self.typed_facts.typed_values,
            enum_values: self.enum_values.values,
            typed_enum_values: self.enum_values.typed_values,
            array_lengths: self.array_lengths.values,
            diagnostics: self.typed_facts.diagnostics,
        }
    }
}

fn filtered_comptime_global_initializer_for_body_check(
    db: &QueryDb<CompilerContext>,
    global_id: GlobalDefId,
) -> Option<nia_comptime_ir::ResolvedComptimeExpr> {
    let defs = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.defs",
        global_id.module_id,
        || db.query(FullModuleDefsQuery(global_id.module_id)),
    );
    let source_path = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.source_path",
        global_id.module_id,
        || db.query(ModulePathQuery(global_id.module_id)),
    );
    let active_item_tree = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.active_item_tree",
        global_id.module_id,
        || db.query_shared(FullActiveModuleItemTreeQuery(global_id.module_id)),
    );
    let filtered_active_item_tree = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.filter_item_tree",
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
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let public_surfaces = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.public_surfaces",
        global_id.module_id,
        || db.query(PublicSurfacesQuery),
    );
    let using_scope = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.module_using_scope",
        global_id.module_id,
        || db.query(ModuleUsingScopeQuery(global_id.module_id)),
    );
    let source_version = db.query(ModuleSourceVersionQuery(global_id.module_id));
    let origins = db.query(ModuleOriginsQuery(global_id.module_id));
    let lowered = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.type_lowering",
        global_id.module_id,
        || db.query(TypeLoweringQuery(global_id.module_id)),
    );
    let type_resolution = db.query(TypeResolutionQuery(global_id.module_id));
    let signatures = db.query(ItemSignaturesQuery(global_id.module_id));
    let needed_const_exprs = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.needed_const_exprs",
        global_id.module_id,
        || needed_const_exprs_for_active_item_tree(&filtered_active_item_tree, &lowered),
    );
    let symbols = db.context().symbols();
    let const_expr_value_resolution = time_module_provider(
        db,
        "executable_body_check.comptime.global_initializer.const_expr_value_resolution",
        global_id.module_id,
        || {
            let visible_extensions = || db.query(VisibleExtensionsQuery(global_id.module_id));
            let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
            nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols(
                lowered.const_exprs.iter().filter_map(|(id, expr)| {
                    needed_const_exprs.contains(id).then_some(expr.clone())
                }),
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query_shared(ModuleGraphQuery)),
                },
                &public_surfaces.surfaces,
                &using_scope,
                Some(&associated_values),
                Some(&symbols),
            )
        },
    );
    let filtered_const_exprs = const_expr_subset_for_ids(&lowered.const_exprs, &needed_const_exprs);
    let lower_with_values = |values: ValueResolution| {
        let locals = time_module_provider(
            db,
            "executable_body_check.comptime.global_initializer.local_resolution",
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
            "executable_body_check.comptime.global_initializer.semantic_uses",
            global_id.module_id,
            || {
                semantic_use_table_from_resolution_inputs_with_const_expr_values(
                    global_id.module_id,
                    &filtered_active_item_tree,
                    &values,
                    Some(&const_expr_value_resolution),
                    Some(&needed_const_exprs),
                    &locals,
                    &type_resolution,
                    &lowered,
                )
            },
        );
        let lowered = time_module_provider(
            db,
            "executable_body_check.comptime.global_initializer.lower_module",
            global_id.module_id,
            || {
                let symbols = db.context().symbols();
                nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
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
        "executable_body_check.comptime.global_initializer.value_resolution",
        global_id.module_id,
        || {
            let visible_extensions = || {
                time_module_provider(
                    db,
                    "executable_body_check.comptime.global_initializer.visible_extensions",
                    global_id.module_id,
                    || db.query(VisibleExtensionsQuery(global_id.module_id)),
                )
            };
            let associated_values = LazyAssociatedValueResolver::new(&visible_extensions);
            let symbols = db.context().symbols();
            nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
                &filtered_active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.query_shared(ModuleGraphQuery)),
                },
                &public_surfaces.surfaces,
                &using_scope,
                Some(&associated_values),
                Some(&symbols),
            )
        },
    );
    lower_with_values(values)
}

fn executable_program_global_initializer(
    db: &QueryDb<CompilerContext>,
    global_id: GlobalDefId,
    fact_mode: ExecutableFactMode<'_>,
) -> Option<nia_comptime_ir::ResolvedComptimeExpr> {
    if fact_mode.signature_facts_for(global_id.module_id) {
        let module = signature_comptime_module_lowering(db, global_id.module_id).module;
        return module
            .global_initializers()
            .get(&global_id)
            .or_else(|| module.deferred_global_initializers().get(&global_id))
            .cloned();
    }
    filtered_comptime_global_initializer_for_body_check(db, global_id)
}

fn comptime_inputs_for_body_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    defs: &DefCollection,
    source_path: &SourcePath,
    signatures: &ItemSignatures,
    normalization: &TypeNormalization,
    lowered: &TypeLowering,
    inputs: &BodyCheckResolutionInputs,
    fact_mode: ExecutableFactMode<'_>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    >,
    comptime_module_cache: Option<&RefCell<HashMap<ModuleId, ComptimeModuleLowering>>>,
) -> BodyCheckComptimeInputs {
    let needed_const_exprs =
        needed_const_exprs_for_active_item_tree(&inputs.active_item_tree, lowered);
    let filtered_const_exprs = const_expr_subset_for_ids(&lowered.const_exprs, &needed_const_exprs);
    let lower_module = || {
        time_module_provider(
            db,
            "executable_body_check.comptime.lower_module",
            module_id,
            || {
                let symbols = db.context().symbols();
                nia_comptime_check::lower_module_comptime(nia_comptime_check::ComptimeModuleInput {
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
    let module = if let Some(cache) = comptime_module_cache {
        if !cache.borrow().contains_key(&module_id) {
            let module = lower_module();
            cache.borrow_mut().insert(module_id, module);
        }
        cache
            .borrow()
            .get(&module_id)
            .expect("cached comptime module lowering must exist")
            .clone()
    } else {
        lower_module()
    };
    let program_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(signature_comptime_module_lowering(db, module_id).module);
        }
        Some(db.query(ComptimeModuleQuery(module_id)).module)
    };
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query(TypeNormalizationQuery(module_id)))
    };
    let value_type_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let trait_impls_for_module = |module_id| {
        if let Some(signatures) = fact_mode.program_signatures {
            return Some(signatures.trait_impls.clone());
        }
        Some(
            db.query(VisibleTraitImplsQuery(module_id))
                .trait_impls
                .clone(),
        )
    };
    let program_is_enum = |def_id: GlobalDefId| {
        fact_mode
            .program_signatures
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || db
                .query_shared(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .enums
                .contains_key(&def_id.def_id)
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query_shared(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query_shared(ItemSignaturesQuery(module_id)))
    };
    let value_signatures_for_module = |module_id| {
        Some(db.query_shared(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let visible_extensions_for_module =
        |module_id| Some(db.query(VisibleExtensionsQuery(module_id)).methods.clone());
    let program_global_initializer = |global_id| {
        if let Some(cache) = global_initializer_cache {
            if !cache.borrow().contains_key(&global_id) {
                let initializer = executable_program_global_initializer(db, global_id, fact_mode);
                cache.borrow_mut().insert(global_id, initializer);
            }
            return cache.borrow().get(&global_id).cloned().flatten();
        }
        executable_program_global_initializer(db, global_id, fact_mode)
    };
    let target = db.query(CompilerTargetQuery);
    let symbols = db.context().symbols();
    let comptime_input = nia_comptime_check::ComptimeInput {
        module: &module.module,
        defs,
        values: &inputs.values,
        locals: &inputs.locals,
        semantic_uses: &inputs.semantic_uses,
        symbols: &symbols,
        lowered,
        signatures,
        interner: &normalization.interner,
        normalized: &normalization.normalized,
        target: &target,
        source_path,
        program: nia_comptime_check::ComptimeProgramContext {
            module: Some(&program_module),
            source_path: Some(&program_source_path),
            defs: Some(&program_defs),
            type_normalizations: Some(&program_type_normalization),
            value_type_normalizations: Some(&value_type_normalization),
            signatures: Some(&item_signatures_for_module),
            value_signatures: Some(&value_signatures_for_module),
            comptime_values: None,
            global_initializer: Some(&program_global_initializer),
            program_is_enum: Some(&program_is_enum),
            trait_impls_for_module: Some(&trait_impls_for_module),
            visible_extensions: Some(&visible_extensions_for_module),
        },
    };
    let mut array_lengths = time_module_provider(
        db,
        "executable_body_check.comptime.array_lengths",
        module_id,
        || nia_comptime_check::compute_module_comptime_array_lengths(comptime_input),
    );
    array_lengths.diagnostics.extend(module.diagnostics.clone());
    let enum_values = time_module_provider(
        db,
        "executable_body_check.comptime.enum_values",
        module_id,
        || {
            nia_comptime_check::compute_module_comptime_enum_values(
                comptime_input,
                array_lengths.clone(),
            )
        },
    );
    let values = time_module_provider(
        db,
        "executable_body_check.comptime.values",
        module_id,
        || {
            nia_comptime_check::compute_module_comptime_values(
                comptime_input,
                array_lengths.clone(),
                enum_values.clone(),
            )
        },
    );
    let typed_facts = time_module_provider(
        db,
        "executable_body_check.comptime.typed_facts",
        module_id,
        || {
            nia_comptime_check::compute_module_comptime_typed_facts(
                comptime_input,
                array_lengths.clone(),
                enum_values.clone(),
                values.clone(),
            )
        },
    );
    BodyCheckComptimeInputs {
        module,
        array_lengths,
        enum_values,
        values,
        typed_facts,
    }
}

pub(super) fn body_check_with_filter_and_layouts(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    layouts: Option<nia_layout::Layouts>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_signatures_override: Option<&ProgramExecutableSignatures>,
) -> nia_body_check::BodyCheck {
    body_check_with_filter_and_layouts_with_inputs(
        db,
        module_id,
        filter,
        layouts,
        program_layouts_override,
        match program_signatures_override {
            Some(program_signatures) => ExecutableFactMode {
                program_signatures: Some(program_signatures),
                reachable_body_modules: None,
            },
            None => ExecutableFactMode::full(),
        },
        None,
        None,
        None,
        None,
        None,
    )
    .body_check
}

pub(super) fn body_check_with_filter_and_layouts_with_inputs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    layouts: Option<nia_layout::Layouts>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    fact_mode: ExecutableFactMode<'_>,
    resolution_inputs: Option<BodyCheckResolutionInputs>,
    seed_interner: Option<nia_ty::TyInterner>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    >,
    comptime_module_cache: Option<&RefCell<HashMap<ModuleId, ComptimeModuleLowering>>>,
    program_function_signature_cache: Option<
        &RefCell<HashMap<GlobalDefId, ProgramFunctionSignature>>,
    >,
) -> BodyCheckWithResolutionInputs {
    body_check_with_filter_and_layouts_with_inputs_and_product(
        db,
        module_id,
        filter,
        layouts,
        program_layouts_override,
        fact_mode,
        resolution_inputs,
        seed_interner,
        global_initializer_cache,
        comptime_module_cache,
        program_function_signature_cache,
        nia_body_check::BodyCheckProduct::Full,
    )
}

#[expect(clippy::too_many_arguments)]
pub(super) fn body_check_with_filter_and_layouts_with_inputs_and_product(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    filter: nia_body_check::BodyCheckFilter<'_>,
    layouts: Option<nia_layout::Layouts>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    fact_mode: ExecutableFactMode<'_>,
    resolution_inputs: Option<BodyCheckResolutionInputs>,
    seed_interner: Option<nia_ty::TyInterner>,
    global_initializer_cache: Option<
        &RefCell<HashMap<GlobalDefId, Option<nia_comptime_ir::ResolvedComptimeExpr>>>,
    >,
    comptime_module_cache: Option<&RefCell<HashMap<ModuleId, ComptimeModuleLowering>>>,
    program_function_signature_cache: Option<
        &RefCell<HashMap<GlobalDefId, ProgramFunctionSignature>>,
    >,
    product: nia_body_check::BodyCheckProduct,
) -> BodyCheckWithResolutionInputs {
    let source_version = db.query(ModuleSourceVersionQuery(module_id));
    let origins = db.query(ModuleOriginsQuery(module_id));
    let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let type_resolution = db.query(TypeResolutionQuery(module_id));
    let lowered = db.query(TypeLoweringQuery(module_id));
    let executable_reachable_filter = matches!(
        filter,
        nia_body_check::BodyCheckFilter::ReachableItems { .. }
            | nia_body_check::BodyCheckFilter::ReachableFunctions(_)
    );
    let filtered_inputs = resolution_inputs.unwrap_or_else(|| {
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
                active_item_tree,
                defs: &defs,
                type_resolution: &type_resolution,
                lowered: &lowered,
            },
        )
    });
    let inputs = &filtered_inputs;
    let source_path = db.query(ModulePathQuery(module_id));
    let signatures = body_local_item_signatures(db, module_id, &lowered);
    let normalization = db.query(TypeNormalizationQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.query(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let mut filtered_comptime_inputs = None;
    let full_comptime_values;
    let full_comptime_array_lengths;
    let full_comptime_typed_facts;
    let full_comptime_module;
    let (body_comptime, comptime_module) = match filter {
        nia_body_check::BodyCheckFilter::All => {
            full_comptime_values = db.query(ComptimeValuesQuery(module_id));
            full_comptime_array_lengths = db.query(ComptimeArrayLengthsQuery(module_id));
            full_comptime_typed_facts = db.query(ComptimeTypedFactsQuery(module_id));
            full_comptime_module = db.query(ComptimeModuleQuery(module_id));
            (
                nia_body_check::BodyComptime::from_phases(
                    &full_comptime_values,
                    &full_comptime_array_lengths,
                    &full_comptime_typed_facts,
                ),
                &full_comptime_module.module,
            )
        }
        _ => {
            filtered_comptime_inputs = Some(time_module_provider(
                db,
                "executable_body_check.comptime_inputs",
                module_id,
                || {
                    comptime_inputs_for_body_check(
                        db,
                        module_id,
                        &defs,
                        &source_path,
                        &signatures,
                        &normalization,
                        &lowered,
                        inputs,
                        fact_mode,
                        global_initializer_cache,
                        comptime_module_cache,
                    )
                },
            ));
            let filtered = filtered_comptime_inputs
                .as_ref()
                .expect("filtered comptime inputs must be initialized");
            (
                nia_body_check::BodyComptime::from_phases(
                    &filtered.values,
                    &filtered.array_lengths,
                    &filtered.typed_facts,
                ),
                &filtered.module.module,
            )
        }
    };
    let layouts = layouts.unwrap_or_else(|| db.query(LayoutsQuery(module_id)));
    let program_layouts = |module_id| {
        program_layouts_override
            .and_then(|program_layouts| program_layouts(module_id))
            .or_else(|| Some(db.query(SignatureLayoutsQuery(module_id))))
    };
    let empty_extensions = nia_defs::VisibleExtensionMethods::default();
    let lazy_extensions = || {
        let extensions = db.query(VisibleExtensionsQuery(module_id));
        (extensions.methods.clone(), extensions.interner.clone())
    };
    let empty_program_extension_methods = nia_defs::ExtensionMethods::default();
    let program_extension_methods = &empty_program_extension_methods;
    let program_extension_method_by_id =
        |def_id: GlobalDefId| db.query(ExtensionMethodByIdQuery(def_id)).method.clone();
    let program_extension_methods_named =
        |name: &SymbolId| db.query(ExtensionMethodsNamedQuery(*name)).methods.clone();
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query(TypeNormalizationQuery(module_id)))
    };
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
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))
        .functions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| {
            let signature = ProgramFunctionSignature {
                name: db
                    .query(ModuleDefsQuery(def_id.module_id))
                    .defs
                    .get(def_id.def_id)
                    .map(|def| def.name.clone())
                    .unwrap_or_default(),
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Functions,
                ),
            };
            if let Some(cache) = program_function_signature_cache {
                cache.borrow_mut().insert(def_id, signature.clone());
            } else {
                local_program_function_signature_cache
                    .borrow_mut()
                    .insert(def_id, signature.clone());
            }
            signature
        })
    };
    let program_global_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Values,
        ))
        .globals
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramGlobalSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Values,
            ),
        })
    };
    let program_comptime_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Values,
        ))
        .comptimes
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramComptimeSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Values,
            ),
        })
    };
    let program_struct_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .structs
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramStructSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_union_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .unions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramUnionSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_enum_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .enums
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramEnumSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_trait_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))
        .traits
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTraitSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Traits,
            ),
        })
    };
    let program_type_alias_signature = |def_id: GlobalDefId| {
        db.query_shared(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .type_aliases
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTypeAliasSignature {
            signature,
            interner: signature_type_interner(
                db,
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ),
        })
    };
    let program_traits_by_method_name = |name: &SymbolId| {
        db.query(ProgramTraitMethodIndexQuery)
            .trait_ids_with_method_named(name)
    };
    let program_trait_owning_method = |method_id: GlobalDefId| {
        db.query(ProgramTraitMethodIndexQuery)
            .trait_owning_method_id(method_id)
            .and_then(|trait_id| {
                program_trait_signature(trait_id).map(|signature| (trait_id, signature))
            })
    };
    let resolver_program_signatures = ProgramSignatureResolvers {
        function: &program_function_signature,
        global: &program_global_signature,
        comptime: &program_comptime_signature,
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
            .program_signatures
            .map(|signatures| ProgramSignatureMaps {
                functions: &signatures.functions,
                globals: &signatures.globals,
                comptimes: &signatures.comptimes,
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
    let program_signatures = if let Some(signatures) = fact_mode.program_signatures {
        ProgramSignatureContext::new_indexed(
            &program_signature_lookup,
            &signatures.trait_impls,
            &signatures.trait_impl_index,
        )
    } else {
        visible_trait_impls = db.query(VisibleTraitImplsQuery(module_id));
        ProgramSignatureContext::new_indexed(
            &program_signature_lookup,
            &visible_trait_impls.trait_impls,
            &visible_trait_impls.trait_impl_index,
        )
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.query_shared(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.query_shared(ItemSignaturesQuery(module_id)))
    };
    let executable_program_comptime_array_lengths =
        RefCell::new(HashMap::<ModuleId, nia_comptime_check::ComptimeArrayLengths>::new());
    let executable_program_comptime_values =
        RefCell::new(HashMap::<ModuleId, nia_comptime_check::ComptimeValues>::new());
    let program_comptime_array_lengths = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            if !executable_program_comptime_array_lengths
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths =
                    signature_comptime_array_lengths(db, module_id, fact_mode.program_signatures);
                executable_program_comptime_array_lengths
                    .borrow_mut()
                    .insert(module_id, array_lengths);
            }
            return executable_program_comptime_array_lengths
                .borrow()
                .get(&module_id)
                .cloned();
        }
        if let Some(signatures) = fact_mode.program_signatures {
            if !executable_program_comptime_array_lengths
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths = with_comptime_input_and_program_signatures(
                    db,
                    module_id,
                    Some(signatures),
                    |input, module| {
                        let mut array_lengths =
                            nia_comptime_check::compute_module_comptime_array_lengths(input);
                        array_lengths.diagnostics.extend(module.diagnostics.clone());
                        array_lengths
                    },
                );
                executable_program_comptime_array_lengths
                    .borrow_mut()
                    .insert(module_id, array_lengths);
            }
            return executable_program_comptime_array_lengths
                .borrow()
                .get(&module_id)
                .cloned();
        }
        Some(db.query(ComptimeArrayLengthsQuery(module_id)))
    };
    let program_comptime_values = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            if !executable_program_comptime_values
                .borrow()
                .contains_key(&module_id)
            {
                let values = signature_comptime_values(db, module_id, fact_mode.program_signatures);
                executable_program_comptime_values
                    .borrow_mut()
                    .insert(module_id, values);
            }
            return executable_program_comptime_values
                .borrow()
                .get(&module_id)
                .cloned();
        }
        if let Some(signatures) = fact_mode.program_signatures {
            if !executable_program_comptime_values
                .borrow()
                .contains_key(&module_id)
            {
                let array_lengths = program_comptime_array_lengths(module_id)?;
                let enum_values = with_comptime_input_and_program_signatures(
                    db,
                    module_id,
                    Some(signatures),
                    |input, module| {
                        let mut enum_values =
                            nia_comptime_check::compute_module_comptime_enum_values(
                                input,
                                array_lengths.clone(),
                            );
                        enum_values.diagnostics.extend(module.diagnostics.clone());
                        enum_values
                    },
                );
                let values = with_comptime_input_and_program_signatures(
                    db,
                    module_id,
                    Some(signatures),
                    |input, module| {
                        let mut values = nia_comptime_check::compute_module_comptime_values(
                            input,
                            array_lengths,
                            enum_values,
                        );
                        values.diagnostics.extend(module.diagnostics.clone());
                        values
                    },
                );
                executable_program_comptime_values
                    .borrow_mut()
                    .insert(module_id, values);
            }
            return executable_program_comptime_values
                .borrow()
                .get(&module_id)
                .cloned();
        }
        Some(db.query(ComptimeValuesQuery(module_id)))
    };
    let program_comptime_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(signature_comptime_module_lowering(db, module_id).module);
        }
        Some(db.query(ComptimeModuleQuery(module_id)).module)
    };
    let program_visible_extensions =
        |module_id| Some(db.query(VisibleExtensionsQuery(module_id)).methods.clone());
    let run_body_check = |inputs: &BodyCheckResolutionInputs,
                          body_comptime: nia_body_check::BodyComptime<'_>,
                          comptime_module: &nia_comptime_ir::ResolvedComptimeModule,
                          filter: nia_body_check::BodyCheckFilter<'_>| {
        nia_body_check::check_module_bodies_with_program_signatures_and_layouts_with_timings(
            nia_body_check::BodyCheckInput {
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
                signatures: nia_body_check::BodyLocalSignatures::from_item_signatures(&signatures),
                comptime_signatures: &signatures,
                normalization: &normalization,
                seed_interner: seed_interner.clone(),
                target: &db.query(CompilerTargetQuery),
                comptime: body_comptime,
                comptime_module,
                layouts: &layouts,
                extensions: &empty_extensions,
                lazy_extensions: Some(&lazy_extensions),
                program_extension_methods,
                extension_interner: None,
                program: nia_body_check::BodyProgramContext {
                    defs: Some(&program_defs),
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
                program_comptime: nia_body_check::ProgramComptimeMaps {
                    values: &program_comptime_values,
                    array_lengths: &program_comptime_array_lengths,
                    module: &program_comptime_module,
                },
                filter,
                product,
            },
            db.context().timings(),
        )
    };
    let body_check = run_body_check(inputs, body_comptime, comptime_module, filter);
    let stored_inputs = match filter {
        nia_body_check::BodyCheckFilter::ReachableItems {
            globals,
            already_checked_functions,
            already_checked_globals,
            ..
        } => {
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
                    active_item_tree: db.query_shared(FullActiveModuleItemTreeQuery(module_id)),
                    defs: &defs,
                    type_resolution: &type_resolution,
                    lowered: &lowered,
                },
            )
        }
        nia_body_check::BodyCheckFilter::ReachableFunctions(_)
        | nia_body_check::BodyCheckFilter::All => filtered_inputs,
    };
    BodyCheckWithResolutionInputs {
        body_check,
        inputs: stored_inputs,
        comptime: filtered_comptime_inputs.map(BodyCheckComptimeInputs::into_check),
    }
}

pub(super) fn executable_layouts_for_reachable_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    array_length_cache: Option<
        &RefCell<HashMap<ModuleId, nia_comptime_check::ComptimeArrayLengths>>,
    >,
    program_signatures_override: Option<&ProgramExecutableSignatures>,
) -> nia_layout::Layouts {
    time_module_provider(db, "executable_layouts", module_id, || {
        let defs = db.query_shared(FullModuleDefsQuery(module_id));
        let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
        let type_lowering = db.query(TypeLoweringQuery(module_id));
        let type_normalization = db.query(LayoutTypeNormalizationQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let program_struct = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .structs
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramStructSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let program_union = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .unions
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramUnionSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let program_enum = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .enums
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramEnumSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let program_type_alias = |def_id: GlobalDefId| {
            db.query_shared(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .type_aliases
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramTypeAliasSignature {
                signature,
                interner: signature_type_interner(
                    db,
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ),
            })
        };
        let executable_array_lengths = |id: nia_ids::GlobalConstExprId| {
            if let Some(array_length_cache) = array_length_cache {
                if !array_length_cache.borrow().contains_key(&id.module_id) {
                    let has_reachable_body_items = has_reachable_executable_body_items(
                        db,
                        id.module_id,
                        reachable_functions,
                        reachable_globals,
                    );
                    let array_lengths = if has_reachable_body_items {
                        with_comptime_input_and_program_signatures(
                            db,
                            id.module_id,
                            program_signatures_override,
                            |input, module| {
                                let mut array_lengths =
                                    nia_comptime_check::compute_module_comptime_array_lengths(
                                        input,
                                    );
                                array_lengths.diagnostics.extend(module.diagnostics.clone());
                                array_lengths
                            },
                        )
                    } else {
                        with_type_signature_comptime_input(
                            db,
                            id.module_id,
                            program_signatures_override,
                            |input, module| {
                                let mut array_lengths =
                                    nia_comptime_check::compute_module_comptime_array_lengths(
                                        input,
                                    );
                                array_lengths.diagnostics.extend(module.diagnostics.clone());
                                array_lengths
                            },
                        )
                    };
                    array_length_cache
                        .borrow_mut()
                        .insert(id.module_id, array_lengths);
                }
                return array_length_cache
                    .borrow()
                    .get(&id.module_id)
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied());
            }
            Some(db.query(ComptimeArrayLengthsQuery(id.module_id)))
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        let (layout_interner, roots) =
            time_module_provider(db, "executable_layouts.roots", module_id, || {
                let mut layout_interner = type_normalization.interner.clone();
                let roots = executable_layout_roots(
                    module_id,
                    &mut layout_interner,
                    &item_signatures,
                    &program_struct,
                    &program_union,
                    type_lowering
                        .versioned_type_uses_from_active_item_tree(&active_item_tree)
                        .into_iter()
                        .map(|(_, ty)| ty),
                    reachable_functions,
                    reachable_globals,
                );
                (layout_interner, roots)
            });
        let layouts = time_module_provider(db, "executable_layouts.compute", module_id, || {
            let symbols = db.context().symbols();
            nia_layout::compute_layouts_for_roots_with_program_context(
                nia_layout::LayoutComputationInput {
                    defs: &defs,
                    interner: &layout_interner,
                    signatures: &item_signatures,
                    normalized: &type_normalization.normalized,
                    array_lengths: &executable_array_lengths,
                    target: nia_layout::TargetDataLayout::LP64,
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
        layouts
    })
}

pub(super) fn executable_program_layouts<'a>(
    db: &'a QueryDb<CompilerContext>,
    cache: &'a RefCell<HashMap<ModuleId, nia_layout::Layouts>>,
    reachable_functions: &'a HashSet<GlobalDefId>,
    reachable_globals: &'a HashSet<GlobalDefId>,
    array_length_cache: Option<
        &'a RefCell<HashMap<ModuleId, nia_comptime_check::ComptimeArrayLengths>>,
    >,
    program_signatures_override: Option<&'a ProgramExecutableSignatures>,
) -> impl Fn(ModuleId) -> Option<nia_layout::Layouts> + 'a {
    move |module_id| {
        if let Some(layouts) = cache.borrow().get(&module_id).cloned() {
            return Some(layouts);
        }
        let has_reachable_body_items = has_reachable_executable_body_items(
            db,
            module_id,
            reachable_functions,
            reachable_globals,
        );
        let layouts = if has_reachable_body_items {
            executable_layouts_for_reachable_items(
                db,
                module_id,
                reachable_functions,
                reachable_globals,
                array_length_cache,
                program_signatures_override,
            )
        } else {
            signature_layouts_for_types(db, module_id, program_signatures_override)
        };
        cache.borrow_mut().insert(module_id, layouts.clone());
        Some(layouts)
    }
}

fn has_reachable_executable_body_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> bool {
    reachable_functions
        .iter()
        .any(|def_id| def_id.module_id == module_id)
        || reachable_globals
            .iter()
            .any(|def_id| def_id.module_id == module_id && is_runtime_global_def(db, *def_id))
}

pub(super) fn executable_reachable_body_modules(
    db: &QueryDb<CompilerContext>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> HashSet<ModuleId> {
    reachable_functions
        .iter()
        .map(|def_id| def_id.module_id)
        .chain(
            reachable_globals
                .iter()
                .filter(|def_id| is_runtime_global_def(db, **def_id))
                .map(|def_id| def_id.module_id),
        )
        .collect()
}

fn is_runtime_global_def(db: &QueryDb<CompilerContext>, def_id: GlobalDefId) -> bool {
    db.query(ModuleDefsQuery(def_id.module_id))
        .defs
        .get(def_id.def_id)
        .is_some_and(|def| def.kind == DefKind::Global)
}

fn rooted_layouts_for_checked_module(
    db: &QueryDb<CompilerContext>,
    module: &CheckedModule,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> nia_layout::Layouts {
    if module.executable_type_only {
        return module.layouts.clone();
    }
    let item_signatures = db.query(ItemSignaturesQuery(module.id));
    let roots = checked_module_layout_roots(module);
    let array_lengths = &module.comptime.array_lengths;
    let symbols = db.context().symbols();
    let local_array_lengths = |id| array_lengths.get(&id).copied();
    let layout_query = |module_id| {
        program_layouts_override
            .and_then(|program_layouts| program_layouts(module_id))
            .or_else(|| Some(db.query(LayoutsQuery(module_id))))
    };
    let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
        program_array_lengths_override
            .and_then(|array_lengths| array_lengths(id))
            .or_else(|| {
                Some(db.query(ComptimeArrayLengthsQuery(id.module_id)))
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied())
            })
    };
    nia_layout::compute_layouts_for_roots_with_program_context(
        nia_layout::LayoutComputationInput {
            defs: &module.defs,
            interner: &module.type_normalization.interner,
            signatures: &item_signatures,
            normalized: &module.type_normalization.normalized,
            array_lengths: &local_array_lengths,
            target: nia_layout::TargetDataLayout::LP64,
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
    )
}

fn executable_layout_roots(
    module_id: ModuleId,
    interner: &mut nia_ty::TyInterner,
    signatures: &ItemSignatures,
    program_struct: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    program_union: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    type_uses: impl IntoIterator<Item = InternedTyId>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> CollectedLayoutRoots {
    let mut roots = LayoutRootCollector::with_program(interner, program_struct, program_union);
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

fn checked_module_layout_roots(module: &CheckedModule) -> CollectedLayoutRoots {
    let mut interner = module.type_normalization.interner.clone();
    let mut roots = LayoutRootCollector::new(&mut interner);
    collect_semantic_layout_roots(&module.semantic_facts, &mut roots);
    roots.finish()
}

pub(super) fn provide_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> CheckedModule {
    time_module_provider(db, "checked_module", module_id, || {
        checked_module_with_body_and_flow_check(
            db,
            module_id,
            db.query(BodyCheckQuery(module_id)),
            db.query(FlowCheckQuery(module_id)),
            None,
        )
    })
}

fn checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: nia_body_check::BodyCheck,
    flow_check: nia_flow_check::FlowCheck,
    layouts: Option<nia_layout::Layouts>,
) -> CheckedModule {
    let path = db.query(ModulePathQuery(module_id));
    CheckedModule {
        id: module_id,
        path,
        defs: db.query(FullModuleDefsQuery(module_id)),
        type_resolution: db.query(TypeResolutionQuery(module_id)),
        type_lowering: db.query(TypeLoweringQuery(module_id)),
        value_resolution: db.query(ValueResolutionQuery(module_id)),
        local_resolution: db.query(LocalResolutionQuery(module_id)),
        type_normalization: db.query(TypeNormalizationQuery(module_id)),
        comptime: db.query(ComptimeQuery(module_id)),
        static_check: db.query(StaticCheckQuery(module_id)),
        layouts: layouts.unwrap_or_else(|| db.query(LayoutsQuery(module_id))),
        abi_check: db.query(AbiCheckQuery(module_id)),
        flow_check,
        body_ir: body_check.ir,
        semantic_uses: db.query(SemanticUseTableQuery(module_id)),
        semantic_facts: body_check.facts,
        executable_reachable_globals: None,
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: false,
        body_diagnostics: body_check.diagnostics,
    }
}

pub(super) fn executable_checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: BodyCheckWithResolutionInputs,
    flow_check: nia_flow_check::FlowCheck,
    layouts: nia_layout::Layouts,
) -> CheckedModule {
    let BodyCheckWithResolutionInputs {
        body_check,
        inputs: body_inputs,
        comptime,
    } = body_check;
    CheckedModule {
        id: module_id,
        path: db.query(ModulePathQuery(module_id)),
        defs: db.query(FullModuleDefsQuery(module_id)),
        type_resolution: db.query(TypeResolutionQuery(module_id)),
        type_lowering: db.query(TypeLoweringQuery(module_id)),
        value_resolution: body_inputs.values,
        local_resolution: body_inputs.locals,
        type_normalization: db.query(TypeNormalizationQuery(module_id)),
        comptime: comptime.unwrap_or_else(|| db.query(ComptimeQuery(module_id))),
        static_check: nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        },
        layouts,
        abi_check: nia_abi_check::AbiCheck {
            diagnostics: Vec::new(),
        },
        flow_check,
        body_ir: body_check.ir,
        semantic_uses: body_inputs.semantic_uses,
        semantic_facts: body_check.facts,
        executable_reachable_globals: None,
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: false,
        body_diagnostics: body_check.diagnostics,
    }
}

pub(super) fn executable_signature_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    layouts: nia_layout::Layouts,
    program_signatures: &ProgramExecutableSignatures,
) -> CheckedModule {
    let type_resolution = db.query(SignatureTypeResolutionQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_lowering = db.query(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_normalization = db.query(SignatureTypeNormalizationQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let (array_lengths, enum_values) = with_type_signature_comptime_input(
        db,
        module_id,
        Some(program_signatures),
        |input, module| {
            let mut array_lengths =
                nia_comptime_check::compute_module_comptime_array_lengths(input);
            array_lengths.diagnostics.extend(module.diagnostics.clone());
            let mut enum_values = nia_comptime_check::compute_module_comptime_enum_values(
                input,
                array_lengths.clone(),
            );
            enum_values.diagnostics.extend(module.diagnostics.clone());
            (array_lengths, enum_values)
        },
    );
    let mut comptime_diagnostics = array_lengths.diagnostics.clone();
    comptime_diagnostics.extend(enum_values.diagnostics.clone());
    CheckedModule {
        id: module_id,
        path: db.query(ModulePathQuery(module_id)),
        defs: db.query(ModuleDefsQuery(module_id)),
        type_resolution,
        type_lowering,
        value_resolution: ValueResolution {
            node_names: HashMap::new(),
            node_qualified_values: HashMap::new(),
            node_builtin_associated_values: HashMap::new(),
            node_variant_enums: HashMap::new(),
            node_qualified_type_prefixes: HashMap::new(),
            diagnostics: Vec::new(),
        },
        local_resolution: nia_local_resolve::LocalResolution {
            locals: nia_local_resolve::LocalMap::default(),
            node_local_defs: HashMap::new(),
            node_uses: HashMap::new(),
            diagnostics: Vec::new(),
        },
        type_normalization: type_normalization.clone(),
        comptime: ComptimeCheck {
            interner: enum_values.interner,
            values: HashMap::new(),
            typed_values: HashMap::new(),
            enum_values: enum_values.values,
            typed_enum_values: enum_values.typed_values,
            array_lengths: array_lengths.values,
            diagnostics: comptime_diagnostics,
        },
        static_check: nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        },
        layouts,
        abi_check: nia_abi_check::AbiCheck {
            diagnostics: Vec::new(),
        },
        flow_check: nia_flow_check::FlowCheck {
            diagnostics: Vec::new(),
        },
        body_ir: nia_body_ir::BodyIr {
            interner: type_normalization.interner,
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        },
        semantic_uses: nia_sema_ir::SemanticUseTable::default(),
        semantic_facts: nia_sema_ir::SemanticFacts::default(),
        executable_reachable_globals: Some(HashSet::new()),
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: true,
        body_diagnostics: Vec::new(),
    }
}

pub(super) fn extend_module_functions_from_filtered_value_refs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    mut module_functions: HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> HashSet<GlobalDefId> {
    let active_item_tree =
        time_module_provider(db, "extend_value_refs.active_item_tree", module_id, || {
            db.query_shared(FullActiveModuleItemTreeQuery(module_id))
        });
    let defs = time_module_provider(db, "extend_value_refs.defs", module_id, || {
        db.query(FullModuleDefsQuery(module_id))
    });
    let signatures = time_module_provider(db, "extend_value_refs.signatures", module_id, || {
        db.query_shared(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ))
    });
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let public_surfaces =
        time_module_provider(db, "extend_value_refs.public_surfaces", module_id, || {
            db.query(PublicSurfacesQuery)
        });
    let using_scope = time_module_provider(
        db,
        "extend_value_refs.module_using_scope",
        module_id,
        || db.query(ModuleUsingScopeQuery(module_id)),
    );
    let graph = time_module_provider(db, "extend_value_refs.module_graph", module_id, || {
        db.query(ModuleGraphQuery)
    });

    loop {
        let filter = nia_body_check::BodyCheckFilter::ReachableItems {
            functions: &module_functions,
            globals: module_globals,
            already_checked_functions: checked_functions,
            already_checked_globals: None,
        };
        let filtered_active_item_tree =
            time_module_provider(db, "extend_value_refs.filter_item_tree", module_id, || {
                active_item_tree_for_body_check_filter(module_id, &defs, &active_item_tree, filter)
            });
        let values =
            time_module_provider(db, "extend_value_refs.value_resolution", module_id, || {
                nia_value_resolve::resolve_module_values_from_active_item_tree(
                    &filtered_active_item_tree,
                    &defs,
                    nia_value_resolve::ProgramDefsContext {
                        defs: Some(&program_defs),
                        graph: Some(&graph),
                    },
                    &public_surfaces.surfaces,
                    &using_scope,
                )
            });
        let local_refs = LocalExecutableValueRefs {
            module_id,
            defs: &defs,
            values: &values,
            signatures: &signatures.functions,
        };
        let mut changed = false;
        time_module_provider(db, "extend_value_refs.scan_refs", module_id, || {
            changed |= extend_local_executable_functions_from_value_refs(
                &mut module_functions,
                &filtered_active_item_tree,
                &local_refs,
                checked_functions,
            );
        });
        if !changed {
            break;
        }
    }
    module_functions
}

pub(super) fn executable_value_ref_edges_from_reachable_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    module_functions: &HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
) -> ExecutableValueRefEdges {
    let active_item_tree = time_module_provider(
        db,
        "executable_value_refs.active_item_tree",
        module_id,
        || db.query_shared(FullActiveModuleItemTreeQuery(module_id)),
    );
    let defs = time_module_provider(db, "executable_value_refs.defs", module_id, || {
        db.query(FullModuleDefsQuery(module_id))
    });
    let program_defs = |module_id| Some(db.query_shared(FullModuleDefsQuery(module_id)));
    let public_surfaces = time_module_provider(
        db,
        "executable_value_refs.public_surfaces",
        module_id,
        || db.query(PublicSurfacesQuery),
    );
    let using_scope = time_module_provider(
        db,
        "executable_value_refs.module_using_scope",
        module_id,
        || db.query(ModuleUsingScopeQuery(module_id)),
    );
    let graph = time_module_provider(db, "executable_value_refs.module_graph", module_id, || {
        db.query(ModuleGraphQuery)
    });

    let mut scan_functions = module_functions.clone();
    let mut edges = ExecutableValueRefEdges::default();
    loop {
        let filter = nia_body_check::BodyCheckFilter::ReachableItems {
            functions: &scan_functions,
            globals: module_globals,
            already_checked_functions: None,
            already_checked_globals: None,
        };
        let filtered_active_item_tree = time_module_provider(
            db,
            "executable_value_refs.filter_item_tree",
            module_id,
            || active_item_tree_for_body_check_filter(module_id, &defs, &active_item_tree, filter),
        );
        let values = time_module_provider(
            db,
            "executable_value_refs.value_resolution",
            module_id,
            || {
                nia_value_resolve::resolve_module_values_from_active_item_tree(
                    &filtered_active_item_tree,
                    &defs,
                    nia_value_resolve::ProgramDefsContext {
                        defs: Some(&program_defs),
                        graph: Some(&graph),
                    },
                    &public_surfaces.surfaces,
                    &using_scope,
                )
            },
        );
        let before_local = scan_functions.len();
        time_module_provider(db, "executable_value_refs.scan_refs", module_id, || {
            extend_executable_edges_from_value_refs(
                db,
                module_id,
                &filtered_active_item_tree,
                &values,
                &mut edges,
                &mut scan_functions,
            );
        });
        if scan_functions.len() == before_local {
            break;
        }
    }
    edges
}

fn extend_local_executable_functions_from_value_refs(
    module_functions: &mut HashSet<GlobalDefId>,
    active_item_tree: &ActiveModuleItemTree,
    refs: &LocalExecutableValueRefs<'_>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> bool {
    let mut keys = HashSet::new();
    collect_active_item_tree_node_keys(active_item_tree, &mut keys);
    let mut changed = false;
    for key in keys {
        if let Some(def_id) =
            refs.values
                .node_names
                .get(&key)
                .and_then(|resolution| match resolution {
                    nia_value_resolve::ValueNameResolution::Def(def_id) => Some(*def_id),
                    nia_value_resolve::ValueNameResolution::External(_)
                    | nia_value_resolve::ValueNameResolution::Module
                    | nia_value_resolve::ValueNameResolution::LocalDeferred
                    | nia_value_resolve::ValueNameResolution::Error => None,
                })
        {
            changed |=
                insert_local_executable_function(module_functions, refs, def_id, checked_functions);
        }
        if let Some(global_id) = refs.values.node_qualified_values.get(&key)
            && global_id.module_id == refs.module_id
        {
            changed |= insert_local_executable_function(
                module_functions,
                refs,
                global_id.def_id,
                checked_functions,
            );
        }
    }
    changed
}

fn extend_executable_edges_from_value_refs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    active_item_tree: &ActiveModuleItemTree,
    values: &ValueResolution,
    edges: &mut ExecutableValueRefEdges,
    scan_functions: &mut HashSet<GlobalDefId>,
) {
    let mut keys = HashSet::new();
    collect_active_item_tree_node_keys(active_item_tree, &mut keys);
    for key in keys {
        if let Some(global_id) = values
            .node_names
            .get(&key)
            .and_then(|resolution| match resolution {
                nia_value_resolve::ValueNameResolution::Def(def_id) => Some(GlobalDefId {
                    module_id,
                    def_id: *def_id,
                }),
                nia_value_resolve::ValueNameResolution::External(global_id) => Some(*global_id),
                nia_value_resolve::ValueNameResolution::Module
                | nia_value_resolve::ValueNameResolution::LocalDeferred
                | nia_value_resolve::ValueNameResolution::Error => None,
            })
        {
            insert_executable_value_ref_edge(db, global_id, edges, scan_functions);
        }
        if let Some(global_id) = values.node_qualified_values.get(&key).copied() {
            insert_executable_value_ref_edge(db, global_id, edges, scan_functions);
        }
    }
}

fn insert_executable_value_ref_edge(
    db: &QueryDb<CompilerContext>,
    global_id: GlobalDefId,
    edges: &mut ExecutableValueRefEdges,
    scan_functions: &mut HashSet<GlobalDefId>,
) {
    let defs = db.query_shared(FullModuleDefsQuery(global_id.module_id));
    let Some(def) = defs.defs.get(global_id.def_id) else {
        return;
    };
    match def.kind {
        DefKind::Function | DefKind::Method | DefKind::TraitMethod => {
            let signatures = db.query_shared(SignatureItemSignaturesQuery(
                global_id.module_id,
                nia_item_tree::SignatureItemSet::Functions,
            ));
            let Some(signature) = signatures.functions.get(&global_id.def_id) else {
                return;
            };
            if signature.is_comptime || !signature.has_body {
                return;
            }
            edges.functions.insert(global_id);
            scan_functions.insert(global_id);
        }
        DefKind::Global => {
            edges.globals.insert(global_id);
        }
        DefKind::Comptime
        | DefKind::Struct
        | DefKind::StructField
        | DefKind::Union
        | DefKind::UnionField
        | DefKind::Enum
        | DefKind::EnumVariant
        | DefKind::TypeAlias
        | DefKind::Trait
        | DefKind::TraitAssociatedType
        | DefKind::Module => {}
    }
}

fn insert_local_executable_function(
    module_functions: &mut HashSet<GlobalDefId>,
    refs: &LocalExecutableValueRefs<'_>,
    def_id: DefId,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> bool {
    let Some(def) = refs.defs.defs.get(def_id) else {
        return false;
    };
    if !matches!(
        def.kind,
        DefKind::Function | DefKind::Method | DefKind::TraitMethod
    ) {
        return false;
    }
    let Some(signature) = refs.signatures.get(&def_id) else {
        return false;
    };
    if signature.is_comptime || !signature.has_body {
        return false;
    }
    let global_id = GlobalDefId {
        module_id: refs.module_id,
        def_id,
    };
    if checked_functions.is_some_and(|checked| checked.contains(&global_id)) {
        return false;
    }
    module_functions.insert(global_id)
}

fn collect_active_item_tree_node_keys(
    active_item_tree: &ActiveModuleItemTree,
    keys: &mut HashSet<nia_node_id::VersionedNodeKey>,
) {
    struct Collector<'a> {
        keys: &'a mut HashSet<nia_node_id::VersionedNodeKey>,
    }

    impl<'ast> nia_ast_walk::Visitor<'ast> for Collector<'_> {
        fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
            self.keys.insert(expr.node_key.clone());
            nia_ast_walk::walk_expr(self, expr);
        }

        fn visit_type(&mut self, ty: &'ast nia_ast::TypeRef) {
            self.keys.insert(ty.node_key.clone());
            nia_ast_walk::walk_type(self, ty);
        }
    }

    let mut collector = Collector { keys };
    for item in &active_item_tree.items {
        nia_ast_walk::Visitor::visit_item(&mut collector, &item.to_ast_item());
    }
}

pub(super) fn provide_checked_module_ids(db: &QueryDb<CompilerContext>) -> Vec<ModuleId> {
    time_provider(db.context().timings(), "checked_module_ids", || {
        db.query(SemanticModuleIdsQuery)
    })
}

pub(super) fn extend_module_functions_from_local_static_globals(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    mut module_functions: HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> HashSet<GlobalDefId> {
    let defs = db.query_shared(FullModuleDefsQuery(module_id));
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
    module_functions
}

pub(super) fn filter_checked_module_for_codegen(
    mut module: CheckedModule,
    db: &QueryDb<CompilerContext>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<nia_layout::Layouts>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> CheckedModule {
    module
        .body_ir
        .function_bodies
        .retain(|def_id, _| reachable_functions.contains(def_id));
    module
        .body_ir
        .global_inits
        .retain(|def_id, _| reachable_globals.contains(def_id));
    module.semantic_facts = filter_semantic_facts_for_reachable_items(
        module.semantic_facts,
        reachable_functions,
        reachable_globals,
    );
    module.layouts = rooted_layouts_for_checked_module(
        db,
        &module,
        program_layouts_override,
        program_array_lengths_override,
    );
    module.executable_reachable_globals = Some(reachable_globals.clone());
    module
}

pub(super) fn executable_reachable_aggregate_roots(
    struct_signature: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    union_signature: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    modules: &[CheckedModule],
) -> ExecutableReachableAggregateRoots {
    let mut structs = HashSet::new();
    let mut unions = HashSet::new();
    for module in modules {
        let mut interner = module.type_normalization.interner.clone();
        let mut roots =
            LayoutRootCollector::with_program(&mut interner, struct_signature, union_signature);
        collect_semantic_layout_roots(&module.semantic_facts, &mut roots);
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
) -> nia_flow_check::FlowCheck {
    time_module_provider(db, "executable_flow_check", module_id, || {
        let active_item_tree = db.query_shared(FullActiveModuleItemTreeQuery(module_id));
        let type_lowering = db.query_shared(SignatureTypeLoweringQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        let signatures = db.query_shared(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        nia_flow_check::check_active_module_flow_with_signatures_and_filter(
            &active_item_tree,
            &type_lowering.interner,
            nia_flow_check::FlowCheckSignatures {
                functions: &signatures.functions,
            },
            nia_flow_check::FlowCheckFilter::ReachableFunctions {
                module_id,
                functions: reachable_functions,
            },
        )
    })
}
