// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_ast::{BindingItem, FunctionItem};
use std::collections::VecDeque;

#[derive(Clone, Default)]
pub(in crate::query) struct ExecutableValueRefEdges {
    pub(in crate::query) functions: HashSet<GlobalDefId>,
    pub(in crate::query) globals: HashSet<GlobalDefId>,
}

#[derive(Clone, Default)]
pub(in crate::query) struct ExecutableValueRefIndex {
    pub(super) functions: HashMap<GlobalDefId, ExecutableValueRefEdges>,
    pub(super) globals: HashMap<GlobalDefId, ExecutableValueRefEdges>,
}

fn walk_executable_value_ref_closure(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    functions: &mut HashSet<GlobalDefId>,
    globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
    mut on_function: impl FnMut(GlobalDefId) -> bool,
    mut on_global: impl FnMut(GlobalDefId) -> bool,
) -> bool {
    let mut changed = false;
    let mut pending_functions = functions.iter().copied().collect::<VecDeque<_>>();
    let mut scanned_functions = HashSet::with_capacity(functions.len());
    for global in globals {
        let edges = db.query(ExecutableValueRefEdgesQuery(*global));
        changed |= visit_executable_value_ref_edges(
            module_id,
            functions,
            &mut pending_functions,
            checked_functions,
            &edges,
            &mut on_function,
            &mut on_global,
        );
    }
    while let Some(function) = pending_functions.pop_front() {
        if !scanned_functions.insert(function) {
            continue;
        }
        let edges = db.query(ExecutableValueRefEdgesQuery(function));
        changed |= visit_executable_value_ref_edges(
            module_id,
            functions,
            &mut pending_functions,
            checked_functions,
            &edges,
            &mut on_function,
            &mut on_global,
        );
    }
    changed
}

fn visit_executable_value_ref_edges(
    module_id: ModuleId,
    functions: &mut HashSet<GlobalDefId>,
    pending_functions: &mut VecDeque<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
    edges: &ExecutableValueRefEdges,
    on_function: &mut impl FnMut(GlobalDefId) -> bool,
    on_global: &mut impl FnMut(GlobalDefId) -> bool,
) -> bool {
    let mut changed = false;
    for global_id in &edges.functions {
        changed |= on_function(*global_id);
        if global_id.module_id == module_id
            && checked_functions.is_none_or(|checked| !checked.contains(global_id))
            && functions.insert(*global_id)
        {
            pending_functions.push_back(*global_id);
            changed = true;
        }
    }
    for global_id in &edges.globals {
        changed |= on_global(*global_id);
    }
    changed
}

impl ExecutableValueRefEdges {
    fn insert_edge(&mut self, db: &QueryDb<CompilerContext>, global_id: GlobalDefId) -> bool {
        let defs = db.get(FullModuleDefsQuery(global_id.module_id));
        let Some(def) = defs.defs.get(global_id.def_id) else {
            return false;
        };
        match def.kind {
            DefKind::Function | DefKind::Method | DefKind::TraitMethod => {
                let signatures = db.get(SignatureItemSignaturesQuery(
                    global_id.module_id,
                    nia_item_tree::SignatureItemSet::Functions,
                ));
                let Some(signature) = signatures.functions.get(&global_id.def_id) else {
                    return false;
                };
                if signature.is_const || !signature.has_body {
                    return false;
                }
                self.functions.insert(global_id)
            }
            DefKind::Global => self.globals.insert(global_id),
            DefKind::Const
            | DefKind::Struct
            | DefKind::StructField
            | DefKind::Union
            | DefKind::UnionField
            | DefKind::Enum
            | DefKind::EnumVariant
            | DefKind::TypeAlias
            | DefKind::Trait
            | DefKind::TraitAssociatedType
            | DefKind::Module => false,
        }
    }
}

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
) -> Option<nia_const_ir::ResolvedConstExpr> {
    let defs = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.defs",
        global_id.module_id,
        || db.query(FullModuleDefsQuery(global_id.module_id)),
    );
    let source_path = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.source_path",
        global_id.module_id,
        || db.query(ModulePathQuery(global_id.module_id)),
    );
    let active_item_tree = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.active_item_tree",
        global_id.module_id,
        || db.query(FullActiveModuleItemTreeQuery(global_id.module_id)),
    );
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
    let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
    let public_surfaces = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.public_surfaces",
        global_id.module_id,
        || db.query(PublicSurfacesQuery),
    );
    let using_scope = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.module_using_scope",
        global_id.module_id,
        || db.query(ModuleUsingScopeQuery(global_id.module_id)),
    );
    let source_version = db.query(ModuleSourceVersionQuery(global_id.module_id));
    let origins = db.query(ModuleOriginsQuery(global_id.module_id));
    let lowered = time_module_provider(
        db,
        "executable_body_check.const_eval.global_initializer.type_lowering",
        global_id.module_id,
        || db.get(TypeLoweringQuery(global_id.module_id)),
    );
    let type_resolution = db.get(TypeResolutionQuery(global_id.module_id));
    let signatures = db.query(ItemSignaturesQuery(global_id.module_id));
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
            let visible_extensions = || db.query(VisibleExtensionsQuery(global_id.module_id));
            let associated_values =
                LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
            nia_value_resolve::resolve_module_values_from_exprs_with_associated_values_and_symbols(
                lowered.const_exprs.iter().filter_map(|(id, expr)| {
                    needed_const_exprs.contains(id).then_some(expr.clone())
                }),
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.get(ModuleGraphQuery)),
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
                    || db.query(VisibleExtensionsQuery(global_id.module_id)),
                )
            };
            let associated_values =
                LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
            let symbols = db.context().symbols();
            nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
                &filtered_active_item_tree,
                &defs,
                nia_value_resolve::ProgramDefsContext {
                    defs: Some(&program_defs),
                    graph: Some(&db.get(ModuleGraphQuery)),
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
) -> Option<nia_const_ir::ResolvedConstExpr> {
    if fact_mode.signature_facts_for(global_id.module_id) {
        let lowering = signature_const_module_lowering(db, global_id.module_id);
        let module = &lowering.module;
        return module
            .global_initializers()
            .get(&global_id)
            .or_else(|| module.deferred_global_initializers().get(&global_id))
            .cloned();
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
) -> BodyCheckConstInputs {
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
    let program_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(
                signature_const_module_lowering(db, module_id)
                    .module
                    .clone(),
            );
        }
        Some(db.get(ConstModuleQuery(module_id)).module.clone())
    };
    let program_source_path = |module_id| Some(db.query(ModulePathQuery(module_id)));
    let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.get(TypeNormalizationQuery(module_id)))
    };
    let local_trait_impls = fact_mode
        .non_function_signatures
        .is_none()
        .then(|| db.get(VisibleTraitImplsQuery(module_id)));
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
        Some(
            db.query(VisibleTraitImplsQuery(requested_module_id))
                .trait_impls
                .clone(),
        )
    };
    let program_is_enum = |def_id: GlobalDefId| {
        fact_mode
            .non_function_signatures
            .is_some_and(|signatures| signatures.enums.contains_key(&def_id))
            || db
                .get(SignatureItemSignaturesQuery(
                    def_id.module_id,
                    nia_item_tree::SignatureItemSet::Types,
                ))
                .enums
                .contains_key(&def_id.def_id)
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.get(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.get(ItemSignaturesQuery(module_id)))
    };
    let value_signatures_for_module = |module_id| {
        Some(db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Values,
        )))
    };
    let local_visible_extensions = db.get(VisibleExtensionsQuery(module_id));
    let visible_extensions_for_module = |requested_module_id| {
        if requested_module_id == module_id {
            return Some(local_visible_extensions.methods.clone());
        }
        Some(
            db.query(VisibleExtensionsQuery(requested_module_id))
                .methods
                .clone(),
        )
    };
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
    BodyCheckConstInputs {
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
    layouts: Option<Arc<nia_layout::Layouts>>,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>>>,
    non_function_signatures_override: Option<&ProgramExecutableNonFunctionSignatures>,
) -> nia_body_check::BodyCheck {
    body_check_with_filter_and_layouts_with_inputs(
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
            program_function_signature_cache: None,
            product: nia_body_check::BodyCheckProduct::Full,
            prechecked: None,
        },
    )
    .body_check
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
    pub program_function_signature_cache:
        Option<&'a RefCell<HashMap<GlobalDefId, ProgramFunctionSignature>>>,
    pub product: nia_body_check::BodyCheckProduct,
    pub prechecked: Option<nia_body_check::PrecheckedBodyCheck>,
}

pub(super) fn body_check_with_filter_and_layouts_with_inputs(
    db: &QueryDb<CompilerContext>,
    input: ExecutableBodyCheckInput<'_>,
) -> BodyCheckWithResolutionInputs {
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
        program_function_signature_cache,
        product,
        prechecked,
    } = input;
    let source_version = db.query(ModuleSourceVersionQuery(module_id));
    let origins = db.query(ModuleOriginsQuery(module_id));
    let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
    let defs = db.get(FullModuleDefsQuery(module_id));
    let program_defs = |module_id| Some(db.get(FullModuleDefsQuery(module_id)));
    let type_resolution = db.get(TypeResolutionQuery(module_id));
    let lowered = db.get(TypeLoweringQuery(module_id));
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
    let normalization = db.get(TypeNormalizationQuery(module_id));
    let extension_method_normalization = |module_id| {
        Some(db.get(SignatureTypeNormalizationQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Traits,
        )))
    };
    let mut filtered_const_inputs = None;
    let full_const_values;
    let full_const_array_lengths;
    let full_const_typed_facts;
    let full_const_module;
    let (body_const, const_module) = match filter {
        nia_body_check::BodyCheckFilter::All => {
            full_const_values = db.get(ConstValuesQuery(module_id));
            full_const_array_lengths = db.get(ConstArrayLengthsQuery(module_id));
            full_const_typed_facts = db.get(ConstTypedFactsQuery(module_id));
            full_const_module = db.get(ConstModuleQuery(module_id));
            (
                nia_body_check::BodyConst::from_phases(
                    &full_const_values,
                    &full_const_array_lengths,
                    &full_const_typed_facts,
                ),
                &full_const_module.module,
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
                            normalization: &normalization,
                            lowered: &lowered,
                            resolution: inputs,
                        },
                        fact_mode,
                        global_initializer_cache,
                        const_module_cache,
                    )
                },
            ));
            let filtered = filtered_const_inputs
                .as_ref()
                .expect("filtered const inputs must be initialized");
            (
                nia_body_check::BodyConst::from_phases(
                    &filtered.values,
                    &filtered.array_lengths,
                    &filtered.typed_facts,
                ),
                &filtered.module.module,
            )
        }
    };
    let layouts = layouts.unwrap_or_else(|| db.get(LayoutsQuery(module_id)));
    let program_layouts = |module_id| {
        program_layouts_override
            .and_then(|program_layouts| program_layouts(module_id))
            .or_else(|| Some(db.get(SignatureLayoutsQuery(module_id))))
    };
    let empty_extensions = nia_defs::VisibleExtensionMethods::default();
    let lazy_extensions = || {
        let extensions = db.query(VisibleExtensionsQuery(module_id));
        extensions.methods.clone()
    };
    let empty_program_extension_methods = nia_defs::ExtensionMethods::default();
    let program_extension_methods = &empty_program_extension_methods;
    let program_extension_method_by_id =
        |def_id: GlobalDefId| db.query(ExtensionMethodByIdQuery(def_id)).method.clone();
    let program_extension_methods_named =
        |name: &SymbolId| db.query(ExtensionMethodsNamedQuery(*name)).methods.clone();
    let program_type_normalization = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.get(SignatureTypeNormalizationQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.get(TypeNormalizationQuery(module_id)))
    };
    let local_function_signatures = db.get(SignatureItemSignaturesQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Functions,
    ));
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
        db.get(SignatureItemSignaturesQuery(
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
            signature
        })
    };
    let program_global_signature = |def_id: GlobalDefId| {
        db.get(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Values,
        ))
        .globals
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramGlobalSignature { signature })
    };
    let program_const_signature = |def_id: GlobalDefId| {
        db.get(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Values,
        ))
        .consts
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramConstSignature { signature })
    };
    let program_struct_signature = |def_id: GlobalDefId| {
        db.get(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .structs
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramStructSignature { signature })
    };
    let program_union_signature = |def_id: GlobalDefId| {
        db.get(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .unions
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramUnionSignature { signature })
    };
    let program_enum_signature = |def_id: GlobalDefId| {
        db.get(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .enums
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramEnumSignature { signature })
    };
    let program_trait_signature = |def_id: GlobalDefId| {
        db.get(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Traits,
        ))
        .traits
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTraitSignature { signature })
    };
    let program_type_alias_signature = |def_id: GlobalDefId| {
        db.get(SignatureItemSignaturesQuery(
            def_id.module_id,
            nia_item_tree::SignatureItemSet::Types,
        ))
        .type_aliases
        .get(&def_id.def_id)
        .cloned()
        .map(|signature| ProgramTypeAliasSignature { signature })
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
        visible_trait_impls = db.query(VisibleTraitImplsQuery(module_id));
        ProgramSignatureContext::new_indexed(
            &program_signature_lookup,
            &visible_trait_impls.trait_impls,
            &visible_trait_impls.trait_impl_index,
        )
    };
    let item_signatures_for_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(db.get(SignatureItemSignaturesQuery(
                module_id,
                nia_item_tree::SignatureItemSet::Types,
            )));
        }
        Some(db.get(ItemSignaturesQuery(module_id)))
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
            {
                let array_lengths =
                    signature_const_array_lengths(db, module_id, fact_mode.non_function_signatures);
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
                executable_program_const_array_lengths
                    .borrow_mut()
                    .insert(module_id, Arc::new(array_lengths));
            }
            return executable_program_const_array_lengths
                .borrow()
                .get(&module_id)
                .cloned();
        }
        Some(db.get(ConstArrayLengthsQuery(module_id)))
    };
    let program_const_values = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            if !executable_program_const_values
                .borrow()
                .contains_key(&module_id)
            {
                let values =
                    signature_const_values(db, module_id, fact_mode.non_function_signatures);
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
                executable_program_const_values
                    .borrow_mut()
                    .insert(module_id, Arc::new(values));
            }
            return executable_program_const_values
                .borrow()
                .get(&module_id)
                .cloned();
        }
        Some(db.get(ConstValuesQuery(module_id)))
    };
    let program_const_module = |module_id| {
        if fact_mode.signature_facts_for(module_id) {
            return Some(
                signature_const_module_lowering(db, module_id)
                    .module
                    .clone(),
            );
        }
        Some(db.get(ConstModuleQuery(module_id)).module.clone())
    };
    let program_visible_extensions =
        |module_id| Some(db.query(VisibleExtensionsQuery(module_id)).methods.clone());
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
                    normalization: &normalization,
                    seed,
                    target: &db.query(CompilerTargetQuery),
                    const_eval: body_const,
                    const_module,
                    layouts: &layouts,
                    extensions: &empty_extensions,
                    lazy_extensions: Some(&lazy_extensions),
                    program_extension_methods,
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
        (nia_body_check::BodyCheckProduct::FactsOnly, _) => filtered_inputs,
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
                    active_item_tree: db.query(FullActiveModuleItemTreeQuery(module_id)),
                    defs: &defs,
                    type_resolution: &type_resolution,
                    lowered: &lowered,
                },
            )
        }
        (
            nia_body_check::BodyCheckProduct::Full,
            nia_body_check::BodyCheckFilter::ReachableFunctions(_)
            | nia_body_check::BodyCheckFilter::All,
        ) => filtered_inputs,
    };
    BodyCheckWithResolutionInputs {
        body_check,
        inputs: stored_inputs,
        const_eval: filtered_const_inputs.map(BodyCheckConstInputs::into_check),
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
) -> nia_layout::Layouts {
    time_module_provider(db, "executable_layouts", module_id, || {
        let defs = db.get(FullModuleDefsQuery(module_id));
        let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
        let type_lowering = db.get(TypeLoweringQuery(module_id));
        let type_normalization = db.get(LayoutTypeNormalizationQuery(module_id));
        let item_signatures = db.query(ItemSignaturesQuery(module_id));
        let program_struct = |def_id: GlobalDefId| {
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .structs
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramStructSignature { signature })
        };
        let program_union = |def_id: GlobalDefId| {
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .unions
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramUnionSignature { signature })
        };
        let program_enum = |def_id: GlobalDefId| {
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .enums
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramEnumSignature { signature })
        };
        let program_type_alias = |def_id: GlobalDefId| {
            db.get(SignatureItemSignaturesQuery(
                def_id.module_id,
                nia_item_tree::SignatureItemSet::Types,
            ))
            .type_aliases
            .get(&def_id.def_id)
            .cloned()
            .map(|signature| ProgramTypeAliasSignature { signature })
        };
        let load_filtered_array_lengths = |target_module_id| {
            let has_reachable_body_items = reachable_body_modules_override
                .map(|modules| modules.contains(target_module_id))
                .unwrap_or_else(|| {
                    has_reachable_executable_body_items(
                        db,
                        target_module_id,
                        reachable_functions,
                        reachable_globals,
                    )
                });
            if has_reachable_body_items {
                with_const_input_and_program_facts(
                    db,
                    target_module_id,
                    non_function_signatures_override,
                    |module_id| {
                        reachable_body_modules_override
                            .map(|modules| !modules.contains(module_id))
                            .unwrap_or_else(|| {
                                !has_reachable_executable_body_items(
                                    db,
                                    module_id,
                                    reachable_functions,
                                    reachable_globals,
                                )
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
                array_length_cache
                    .borrow_mut()
                    .insert(module_id, load_filtered_array_lengths(module_id));
            }
            array_length_cache
                .borrow()
                .get(&module_id)
                .cloned()
                .expect("local executable array lengths must be cached")
        } else {
            load_filtered_array_lengths(module_id)
        };
        let signature_array_lengths = RefCell::new(HashMap::new());
        let executable_array_lengths = |id: nia_ids::GlobalConstExprId| {
            if id.module_id == module_id {
                return local_array_lengths.values.get(&id).copied();
            }
            if !signature_array_lengths.borrow().contains_key(&id.module_id) {
                let array_lengths = with_type_signature_const_input(
                    db,
                    id.module_id,
                    non_function_signatures_override,
                    |input, module| {
                        let mut array_lengths =
                            nia_const_check::compute_module_const_array_lengths(input);
                        array_lengths.diagnostics.extend(module.diagnostics.clone());
                        array_lengths
                    },
                );
                signature_array_lengths
                    .borrow_mut()
                    .insert(id.module_id, array_lengths);
            }
            signature_array_lengths
                .borrow()
                .get(&id.module_id)
                .and_then(|array_lengths| array_lengths.values.get(&id).copied())
        };
        time_module_provider(db, "executable_layouts.compute", module_id, || {
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
        })
    })
}

pub(super) fn executable_program_layouts<'a>(
    db: &'a QueryDb<CompilerContext>,
    cache: &'a RefCell<HashMap<ModuleId, Arc<nia_layout::Layouts>>>,
    reachable_functions: &'a HashSet<GlobalDefId>,
    reachable_globals: &'a HashSet<GlobalDefId>,
    array_length_cache: Option<&'a RefCell<HashMap<ModuleId, nia_const_check::ConstArrayLengths>>>,
    non_function_signatures_override: Option<&'a ProgramExecutableNonFunctionSignatures>,
    reachable_body_modules_override: Option<ReachableBodyModules<'a>>,
) -> impl Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>> + 'a {
    move |module_id| {
        if let Some(layouts) = cache.borrow().get(&module_id).cloned() {
            return Some(layouts);
        }
        let has_reachable_body_items = reachable_body_modules_override
            .map(|modules| modules.contains(module_id))
            .unwrap_or_else(|| {
                has_reachable_executable_body_items(
                    db,
                    module_id,
                    reachable_functions,
                    reachable_globals,
                )
            });
        let layouts = Arc::new(if has_reachable_body_items {
            executable_layouts_for_reachable_items(
                db,
                module_id,
                reachable_functions,
                reachable_globals,
                array_length_cache,
                non_function_signatures_override,
                reachable_body_modules_override,
            )
        } else {
            signature_layouts_for_types(db, module_id, non_function_signatures_override)
        });
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

fn is_runtime_global_def(db: &QueryDb<CompilerContext>, def_id: GlobalDefId) -> bool {
    db.query(ModuleDefsQuery(def_id.module_id))
        .defs
        .get(def_id.def_id)
        .is_some_and(|def| def.kind == DefKind::Global)
}

fn rooted_layouts_for_checked_module(
    db: &QueryDb<CompilerContext>,
    module: &CheckedModule,
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> Arc<nia_layout::Layouts> {
    if module.executable_type_only {
        return module.layouts.clone();
    }
    let item_signatures = db.query(ItemSignaturesQuery(module.id));
    let roots = checked_module_layout_roots(&db.context().type_store, module);
    let array_lengths = &module.const_eval.array_lengths;
    let symbols = db.context().symbols();
    let local_array_lengths = |id| array_lengths.get(&id).copied();
    let layout_query = |module_id| {
        program_layouts_override
            .and_then(|program_layouts| program_layouts(module_id))
            .or_else(|| Some(db.get(LayoutsQuery(module_id))))
    };
    let program_array_lengths = |id: nia_ids::GlobalConstExprId| {
        program_array_lengths_override
            .and_then(|array_lengths| array_lengths(id))
            .or_else(|| {
                Some(db.get(ConstArrayLengthsQuery(id.module_id)))
                    .and_then(|array_lengths| array_lengths.values.get(&id).copied())
            })
    };
    Arc::new(nia_layout::compute_layouts_for_roots_with_program_context(
        nia_layout::LayoutComputationInput {
            type_store: &db.context().type_store,
            defs: &module.defs,
            signatures: &item_signatures,
            root_types: &[],
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
    ))
}

struct ExecutableLayoutModule<'a> {
    module_id: ModuleId,
    signatures: &'a ItemSignatures,
    program_struct: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    program_union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
}

fn executable_layout_roots(
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

pub(super) fn provide_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
) -> CheckedModule {
    time_module_provider(db, "checked_module", module_id, || {
        checked_module_with_body_and_flow_check(
            db,
            module_id,
            db.get(BodyCheckQuery(module_id)),
            db.get(FlowCheckQuery(module_id)),
            None,
        )
    })
}

fn checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: Arc<nia_body_check::BodyCheck>,
    flow_check: Arc<nia_flow_check::FlowCheck>,
    layouts: Option<Arc<nia_layout::Layouts>>,
) -> CheckedModule {
    let path = db.query(ModulePathQuery(module_id));
    CheckedModule {
        id: module_id,
        path,
        defs: db.query(FullModuleDefsQuery(module_id)),
        type_resolution: db.get(TypeResolutionQuery(module_id)),
        type_lowering: db.get(TypeLoweringQuery(module_id)),
        value_resolution: db.get(ValueResolutionQuery(module_id)),
        local_resolution: db.get(LocalResolutionQuery(module_id)),
        type_normalization: db.get(TypeNormalizationQuery(module_id)),
        const_eval: db.get(ConstQuery(module_id)),
        static_check: db.get(StaticCheckQuery(module_id)),
        layouts: layouts.unwrap_or_else(|| db.get(LayoutsQuery(module_id))),
        abi_check: db.get(AbiCheckQuery(module_id)),
        flow_check,
        body_ir: Arc::clone(&body_check.ir),
        semantic_uses: db.get(SemanticUseTableQuery(module_id)),
        semantic_facts: Arc::clone(&body_check.facts),
        provider_demands: Arc::clone(&body_check.provider_demands),
        executable_reachable_globals: None,
        executable_reachable_structs: None,
        executable_reachable_unions: None,
        executable_type_only: false,
        body_diagnostics: Arc::clone(&body_check.diagnostics),
    }
}

pub(super) fn executable_checked_module_with_body_and_flow_check(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    body_check: BodyCheckWithResolutionInputs,
    flow_check: nia_flow_check::FlowCheck,
    layouts: Arc<nia_layout::Layouts>,
) -> CheckedModule {
    let BodyCheckWithResolutionInputs {
        body_check,
        inputs: body_inputs,
        const_eval,
    } = body_check;
    CheckedModule {
        id: module_id,
        path: db.query(ModulePathQuery(module_id)),
        defs: db.query(FullModuleDefsQuery(module_id)),
        type_resolution: db.get(TypeResolutionQuery(module_id)),
        type_lowering: db.get(TypeLoweringQuery(module_id)),
        value_resolution: body_inputs.values,
        local_resolution: body_inputs.locals,
        type_normalization: db.get(TypeNormalizationQuery(module_id)),
        const_eval: const_eval
            .map(Arc::new)
            .unwrap_or_else(|| db.get(ConstQuery(module_id))),
        static_check: Arc::new(nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        }),
        layouts,
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
        body_diagnostics: body_check.diagnostics,
    }
}

pub(super) fn executable_signature_checked_module(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    layouts: Arc<nia_layout::Layouts>,
    program_signatures: &ProgramExecutableNonFunctionSignatures,
) -> CheckedModule {
    let type_resolution = db.get(SignatureTypeResolutionQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_lowering = db.get(SignatureTypeLoweringQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
    let type_normalization = db.get(SignatureTypeNormalizationQuery(
        module_id,
        nia_item_tree::SignatureItemSet::Types,
    ));
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
    );
    let mut const_diagnostics = array_lengths.diagnostics.clone();
    const_diagnostics.extend(enum_values.diagnostics.clone());
    CheckedModule {
        id: module_id,
        path: db.query(ModulePathQuery(module_id)),
        defs: db.query(ModuleDefsQuery(module_id)),
        type_resolution,
        type_lowering,
        value_resolution: Arc::new(ValueResolution {
            node_names: HashMap::new(),
            node_qualified_values: HashMap::new(),
            node_builtin_associated_values: HashMap::new(),
            node_variant_enums: HashMap::new(),
            node_qualified_type_prefixes: HashMap::new(),
            diagnostics: Vec::new(),
        }),
        local_resolution: Arc::new(nia_local_resolve::LocalResolution {
            locals: nia_local_resolve::LocalMap::default(),
            node_local_defs: HashMap::new(),
            node_uses: HashMap::new(),
            diagnostics: Vec::new(),
        }),
        type_normalization,
        const_eval: Arc::new(ConstCheck {
            values: Arc::new(HashMap::new()),
            typed_values: Arc::new(HashMap::new()),
            enum_values: enum_values.values,
            typed_enum_values: enum_values.typed_values,
            array_lengths: array_lengths.values,
            diagnostics: const_diagnostics,
        }),
        static_check: Arc::new(nia_static_check::StaticCheck {
            diagnostics: Vec::new(),
        }),
        layouts,
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
        body_diagnostics: Arc::new(Vec::new()),
    }
}

pub(super) fn extend_module_functions_from_filtered_value_refs(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    mut module_functions: HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
    checked_functions: Option<&HashSet<GlobalDefId>>,
) -> HashSet<GlobalDefId> {
    time_module_provider(db, "extend_value_refs.scan_refs", module_id, || {
        walk_executable_value_ref_closure(
            db,
            module_id,
            &mut module_functions,
            module_globals,
            checked_functions,
            |_| false,
            |_| false,
        );
    });
    module_functions
}

pub(super) fn executable_value_ref_edges_from_reachable_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    module_functions: &HashSet<GlobalDefId>,
    module_globals: &HashSet<GlobalDefId>,
) -> ExecutableValueRefEdges {
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
        );
    });
    all_edges
}

pub(in crate::query) fn provide_executable_value_ref_edges(
    db: &QueryDb<CompilerContext>,
    owner: GlobalDefId,
) -> ExecutableValueRefEdges {
    time_module_provider(db, "executable_value_ref_edges", owner.module_id, || {
        let Some(item_input) = db.query(ExecutableValueRefItemQuery(owner)) else {
            return ExecutableValueRefEdges::default();
        };
        let active_item_tree = executable_value_ref_active_item_tree(&item_input);
        let defs = db.get(ModuleDefsQuery(owner.module_id));
        let program_defs = |module_id| Some(db.get(ModuleDefsQuery(module_id)));
        let graph = QueryModuleGraphLookup::new(db);
        let public_surfaces = QueryPublicSurfaceLookup::new(db);
        let using_scope = QueryUsingScopeLookup::new(db, owner.module_id);
        let visible_extensions = || db.query(VisibleExtensionsQuery(owner.module_id));
        let associated_values =
            LazyAssociatedValueResolver::new(&db.context().type_store, &visible_extensions);
        let symbols = db.context().symbols();
        let values = nia_value_resolve::resolve_module_values_from_active_item_tree_with_associated_values_and_symbols(
            &active_item_tree,
            &defs,
            nia_value_resolve::ProgramDefsContext {
                defs: Some(&program_defs),
                graph: Some(&graph),
            },
            &public_surfaces,
            &using_scope,
            Some(&associated_values),
            Some(&symbols),
        );
        let locals =
            nia_local_resolve::resolve_module_locals_from_filtered_active_item_tree_with_origins_and_symbols(
                &active_item_tree,
                &item_input.active_item_tree,
                &defs,
                &values,
                None,
                &nia_node_id::NodeOriginTable::default(),
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
        );
        index
            .functions
            .remove(&owner)
            .or_else(|| index.globals.remove(&owner))
            .unwrap_or_default()
    })
}

fn executable_value_ref_active_item_tree(
    input: &ExecutableValueRefItemInput,
) -> ActiveModuleItemTree {
    let mut item = input.active_item_tree.items[input.item_index].clone();
    match &mut item.kind {
        nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
            item_trait
                .methods
                .retain(|method| method.function.node_key == input.owner_node_key);
        }
        nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
            extend
                .methods
                .retain(|method| method.function.node_key == input.owner_node_key);
            extend
                .associated_values
                .retain(|value| value.binding.node_key == input.owner_node_key);
        }
        _ => {}
    }
    ActiveModuleItemTree::new(vec![item], input.active_item_tree.inactive_spans.clone())
}

fn collect_executable_value_ref_index_for_items(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    items: &[nia_item_tree::ItemTreeNode],
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    index: &mut ExecutableValueRefIndex,
) {
    for item in items {
        match &item.kind {
            nia_item_tree::ItemTreeNodeKind::Function(function) => {
                collect_executable_value_ref_index_for_function(
                    db, module_id, defs, values, locals, function, index,
                );
            }
            nia_item_tree::ItemTreeNodeKind::Binding(binding) => {
                collect_executable_value_ref_index_for_binding(
                    db, module_id, defs, values, locals, binding, index,
                );
            }
            nia_item_tree::ItemTreeNodeKind::Trait(item_trait) => {
                for method in &item_trait.methods {
                    collect_executable_value_ref_index_for_function(
                        db,
                        module_id,
                        defs,
                        values,
                        locals,
                        &method.function,
                        index,
                    );
                }
            }
            nia_item_tree::ItemTreeNodeKind::Extend(extend) => {
                for associated_value in &extend.associated_values {
                    collect_executable_value_ref_index_for_binding(
                        db,
                        module_id,
                        defs,
                        values,
                        locals,
                        &associated_value.binding,
                        index,
                    );
                }
                for method in &extend.methods {
                    collect_executable_value_ref_index_for_function(
                        db,
                        module_id,
                        defs,
                        values,
                        locals,
                        &method.function,
                        index,
                    );
                }
            }
            nia_item_tree::ItemTreeNodeKind::Module(_)
            | nia_item_tree::ItemTreeNodeKind::Using(_)
            | nia_item_tree::ItemTreeNodeKind::Struct(_)
            | nia_item_tree::ItemTreeNodeKind::Union(_)
            | nia_item_tree::ItemTreeNodeKind::Enum(_)
            | nia_item_tree::ItemTreeNodeKind::TypeAlias(_) => {}
        }
    }
}

fn collect_executable_value_ref_index_for_function(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    function: &FunctionItem,
    index: &mut ExecutableValueRefIndex,
) {
    if function.is_const || function.body.is_none() {
        return;
    }
    let Some(def_id) = defs.def_nodes.get(&function.node_key) else {
        return;
    };
    let owner = GlobalDefId { module_id, def_id };
    let edges = index.functions.entry(owner).or_default();
    collect_executable_value_ref_edges_from_function(
        db, module_id, function, values, locals, edges,
    );
}

fn collect_executable_value_ref_index_for_binding(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    defs: &DefCollection,
    values: &ValueResolution,
    locals: &LocalResolution,
    binding: &BindingItem,
    index: &mut ExecutableValueRefIndex,
) {
    if binding.is_const() || binding.value.is_none() {
        return;
    }
    let Some(def_id) = defs.def_nodes.get(&binding.node_key) else {
        return;
    };
    let owner = GlobalDefId { module_id, def_id };
    let edges = index.globals.entry(owner).or_default();
    collect_executable_value_ref_edges_from_binding(db, module_id, binding, values, locals, edges);
}

fn collect_executable_value_ref_edges_from_function(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    function: &FunctionItem,
    values: &ValueResolution,
    locals: &LocalResolution,
    edges: &mut ExecutableValueRefEdges,
) {
    let mut collector = ExecutableValueRefCollector::new(db, module_id, values, locals, edges);
    nia_ast_walk::Visitor::visit_function(&mut collector, function);
}

fn collect_executable_value_ref_edges_from_binding(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    binding: &BindingItem,
    values: &ValueResolution,
    locals: &LocalResolution,
    edges: &mut ExecutableValueRefEdges,
) {
    let mut collector = ExecutableValueRefCollector::new(db, module_id, values, locals, edges);
    if let Some(ty) = &binding.ty {
        nia_ast_walk::Visitor::visit_type(&mut collector, ty);
    }
    if let Some(value) = &binding.value {
        nia_ast_walk::Visitor::visit_expr(&mut collector, value);
    }
}

struct ExecutableValueRefCollector<'a> {
    db: &'a QueryDb<CompilerContext>,
    module_id: ModuleId,
    values: &'a ValueResolution,
    locals: &'a LocalResolution,
    edges: &'a mut ExecutableValueRefEdges,
}

impl<'a> ExecutableValueRefCollector<'a> {
    fn new(
        db: &'a QueryDb<CompilerContext>,
        module_id: ModuleId,
        values: &'a ValueResolution,
        locals: &'a LocalResolution,
        edges: &'a mut ExecutableValueRefEdges,
    ) -> Self {
        Self {
            db,
            module_id,
            values,
            locals,
            edges,
        }
    }
}

impl<'ast> nia_ast_walk::Visitor<'ast> for ExecutableValueRefCollector<'_> {
    fn visit_expr(&mut self, expr: &'ast nia_ast::Expr) {
        collect_executable_value_ref_edge_for_key(
            self.db,
            self.module_id,
            self.values,
            self.locals,
            self.edges,
            &expr.node_key,
        );
        nia_ast_walk::walk_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast nia_ast::TypeRef) {
        collect_executable_value_ref_edge_for_key(
            self.db,
            self.module_id,
            self.values,
            self.locals,
            self.edges,
            &ty.node_key,
        );
        nia_ast_walk::walk_type(self, ty);
    }
}

fn collect_executable_value_ref_edge_for_key(
    db: &QueryDb<CompilerContext>,
    module_id: ModuleId,
    values: &ValueResolution,
    locals: &LocalResolution,
    edges: &mut ExecutableValueRefEdges,
    key: &nia_node_id::VersionedNodeKey,
) {
    match locals.node_uses.get(key) {
        Some(nia_local_resolve::LocalUse::Static(global_id)) => {
            edges.insert_edge(db, *global_id);
            return;
        }
        Some(nia_local_resolve::LocalUse::Local(_)) => return,
        Some(nia_local_resolve::LocalUse::ModuleValue)
        | Some(nia_local_resolve::LocalUse::Module)
        | Some(nia_local_resolve::LocalUse::TypePrefix)
        | Some(nia_local_resolve::LocalUse::Unresolved)
        | None => {}
    }
    if let Some(global_id) = values
        .node_names
        .get(key)
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
        edges.insert_edge(db, global_id);
    }
    if let Some(global_id) = values.node_qualified_values.get(key).copied() {
        edges.insert_edge(db, global_id);
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
    let defs = db.get(FullModuleDefsQuery(module_id));
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
    program_layouts_override: Option<&dyn Fn(ModuleId) -> Option<Arc<nia_layout::Layouts>>>,
    program_array_lengths_override: Option<&dyn Fn(nia_ids::GlobalConstExprId) -> Option<u64>>,
) -> CheckedModule {
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
    type_store: &nia_ty::TypeStore,
    struct_signature: &dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    union_signature: &dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    modules: &[CheckedModule],
) -> ExecutableReachableAggregateRoots {
    let mut structs = HashSet::new();
    let mut unions = HashSet::new();
    for module in modules {
        let mut roots = LayoutRootCollector::with_program(
            type_store,
            module.id,
            struct_signature,
            union_signature,
        );
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
        let active_item_tree = db.query(FullActiveModuleItemTreeQuery(module_id));
        let signatures = db.get(SignatureItemSignaturesQuery(
            module_id,
            nia_item_tree::SignatureItemSet::Functions,
        ));
        nia_flow_check::check_active_module_flow_with_signatures_and_filter(
            &active_item_tree,
            db.context().type_store(),
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
