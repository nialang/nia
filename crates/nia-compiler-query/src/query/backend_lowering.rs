// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_item_signatures::ItemSignatures;

pub(super) struct BackendLoweringInputs {
    symbols: nia_symbol_table::SymbolTable,
    source_identities: HashMap<ModuleId, nia_source::SourceIdentity>,
    symbol_package_identities: HashMap<ModuleId, String>,
    checked_modules: Vec<Arc<CheckedModule>>,
    module_indices: HashMap<ModuleId, usize>,
    active_item_trees: Vec<Arc<ActiveModuleItemTree>>,
    item_signatures: Vec<ItemSignatures>,
    const_array_lengths: Vec<Arc<HashMap<GlobalConstExprId, u64>>>,
    const_enum_values: Vec<Arc<HashMap<DefId, nia_const_check::ConstValue>>>,
    visible_extensions: Vec<Arc<VisibleExtensionsValue>>,
    extension_methods: Arc<ExtensionMethodIndexValue>,
    function_bodies: Vec<LoweredFunctionBodyHandle>,
    function_body_ids: Vec<GlobalDefId>,
    function_body_indices: HashMap<GlobalDefId, usize>,
    static_inits: Vec<StaticInitHandle>,
    static_init_ids: Vec<GlobalDefId>,
    static_init_indices: HashMap<GlobalDefId, usize>,
    source_item_plans: Vec<Arc<BackendModuleSourceItemPlan>>,
    function_instance_plans: Vec<Arc<BackendModuleFunctionInstancePlan>>,
    program_defs: Vec<Arc<DefCollection>>,
    non_function_signatures: ProgramExecutableNonFunctionSignatures,
    functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    runtime: RuntimeSpec,
}

pub(super) struct BackendLoweringInputsParts {
    pub(super) symbols: nia_symbol_table::SymbolTable,
    pub(super) source_identities: HashMap<ModuleId, nia_source::SourceIdentity>,
    pub(super) symbol_package_identities: HashMap<ModuleId, String>,
    pub(super) checked_modules: Vec<Arc<CheckedModule>>,
    pub(super) active_item_trees: Vec<Arc<ActiveModuleItemTree>>,
    pub(super) item_signatures: Vec<ItemSignatures>,
    pub(super) const_array_lengths: Vec<Arc<HashMap<GlobalConstExprId, u64>>>,
    pub(super) const_enum_values: Vec<Arc<HashMap<DefId, nia_const_check::ConstValue>>>,
    pub(super) visible_extensions: Vec<Arc<VisibleExtensionsValue>>,
    pub(super) extension_methods: Arc<ExtensionMethodIndexValue>,
    pub(super) function_bodies: Vec<LoweredFunctionBodyHandle>,
    pub(super) static_inits: Vec<StaticInitHandle>,
    pub(super) source_item_plans: Vec<Arc<BackendModuleSourceItemPlan>>,
    pub(super) function_instance_plans: Vec<Arc<BackendModuleFunctionInstancePlan>>,
    pub(super) program_defs: Vec<Arc<DefCollection>>,
    pub(super) non_function_signatures: ProgramExecutableNonFunctionSignatures,
    pub(super) functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub(super) runtime: RuntimeSpec,
}

impl BackendLoweringInputs {
    pub(super) fn new(parts: BackendLoweringInputsParts) -> QueryResult<Self> {
        let module_count = parts.checked_modules.len();
        for (name, actual) in [
            ("active item trees", parts.active_item_trees.len()),
            ("item signatures", parts.item_signatures.len()),
            ("const array lengths", parts.const_array_lengths.len()),
            ("const enum values", parts.const_enum_values.len()),
            ("visible extensions", parts.visible_extensions.len()),
            ("source item plans", parts.source_item_plans.len()),
            (
                "function instance plans",
                parts.function_instance_plans.len(),
            ),
            ("program definitions", parts.program_defs.len()),
        ] {
            if actual != module_count {
                return Err(QueryError::internal(format!(
                    "backend {name} count {actual} does not match checked module count {module_count}"
                )));
            }
        }
        let module_indices = parts
            .checked_modules
            .iter()
            .enumerate()
            .map(|(index, module)| (module.id, index))
            .collect::<HashMap<_, _>>();
        if module_indices.len() != module_count {
            return Err(QueryError::internal(
                "backend lowering inputs contain duplicate module owners",
            ));
        }
        let function_body_ids = parts
            .function_bodies
            .iter()
            .filter(|body| body.value.body().is_some())
            .map(|body| body.def_id)
            .collect::<Vec<_>>();
        let function_body_indices = parts
            .function_bodies
            .iter()
            .enumerate()
            .map(|(index, body)| (body.def_id, index))
            .collect();
        let static_init_ids = parts
            .static_inits
            .iter()
            .filter(|init| init.value.as_ref().is_some())
            .map(|init| init.def_id)
            .collect::<Vec<_>>();
        let static_init_indices = parts
            .static_inits
            .iter()
            .enumerate()
            .map(|(index, init)| (init.def_id, index))
            .collect();
        Ok(Self {
            symbols: parts.symbols,
            source_identities: parts.source_identities,
            symbol_package_identities: parts.symbol_package_identities,
            checked_modules: parts.checked_modules,
            module_indices,
            active_item_trees: parts.active_item_trees,
            item_signatures: parts.item_signatures,
            const_array_lengths: parts.const_array_lengths,
            const_enum_values: parts.const_enum_values,
            visible_extensions: parts.visible_extensions,
            extension_methods: parts.extension_methods,
            function_bodies: parts.function_bodies,
            function_body_ids,
            function_body_indices,
            static_inits: parts.static_inits,
            static_init_ids,
            static_init_indices,
            source_item_plans: parts.source_item_plans,
            function_instance_plans: parts.function_instance_plans,
            program_defs: parts.program_defs,
            non_function_signatures: parts.non_function_signatures,
            functions: parts.functions,
            runtime: parts.runtime,
        })
    }

    pub(super) fn module_inputs(&self) -> QueryResult<Vec<BackendLowerModuleInput<'_>>> {
        (0..self.checked_modules.len())
            .map(|index| self.module_input(index))
            .collect()
    }

    pub(super) fn module_input(&self, index: usize) -> QueryResult<BackendLowerModuleInput<'_>> {
        let Some(checked_module) = self.checked_modules.get(index) else {
            return Err(QueryError::internal(format!(
                "backend module input position {index} is out of bounds"
            )));
        };
        let Some(source_item_plan) = self.source_item_plans.get(index) else {
            return Err(QueryError::internal(format!(
                "backend source item plan position {index} is missing"
            )));
        };
        let Some(function_instance_plan) = self.function_instance_plans.get(index) else {
            return Err(QueryError::internal(format!(
                "backend function instance plan position {index} is missing"
            )));
        };
        let Some(active_item_tree) = self.active_item_trees.get(index) else {
            return Err(QueryError::internal(format!(
                "backend active item tree position {index} is missing"
            )));
        };
        let Some(signatures) = self.item_signatures.get(index) else {
            return Err(QueryError::internal(format!(
                "backend item signatures position {index} is missing"
            )));
        };
        let Some(const_array_lengths) = self.const_array_lengths.get(index) else {
            return Err(QueryError::internal(format!(
                "backend const array lengths position {index} is missing"
            )));
        };
        let Some(const_enum_values) = self.const_enum_values.get(index) else {
            return Err(QueryError::internal(format!(
                "backend const enum values position {index} is missing"
            )));
        };
        let Some(visible_extensions) = self.visible_extensions.get(index) else {
            return Err(QueryError::internal(format!(
                "backend visible extensions position {index} is missing"
            )));
        };
        let Some(symbol_package_identity) = self.symbol_package_identities.get(&checked_module.id)
        else {
            return Err(QueryError::internal(format!(
                "backend module {:?} is missing package symbol identity",
                checked_module.id
            )));
        };
        Ok(BackendLowerModuleInput {
            module_id: checked_module.id,
            source_identity: checked_module.path.identity(),
            symbol_package_identity: symbol_package_identity.clone(),
            module_name: checked_module.path.as_str().to_string(),
            symbols: &self.symbols,
            active_item_tree: active_item_tree.as_ref(),
            defs: &checked_module.defs,
            extensions: &visible_extensions.methods,
            values: &checked_module.value_resolution,
            locals: &checked_module.local_resolution,
            type_lowering: &checked_module.type_lowering,
            signatures,
            type_normalization: &checked_module.type_normalization,
            semantic_facts: &checked_module.semantic_facts,
            const_array_lengths: const_array_lengths.as_ref(),
            const_enum_values: const_enum_values.as_ref(),
            layouts: &checked_module.layouts,
            roots: backend_function_roots(&self.runtime, checked_module),
            reachable_functions: Some(&source_item_plan.functions),
            reachable_globals: Some(&source_item_plan.globals),
            reachable_structs: Some(&source_item_plan.structs),
            reachable_unions: Some(&source_item_plan.unions),
            function_instance_plan: &function_instance_plan.instances,
            program: self,
        })
    }
}

impl nia_backend_lower::BackendProgramFacts for BackendLoweringInputs {
    fn source_identities(&self) -> &HashMap<ModuleId, nia_source::SourceIdentity> {
        &self.source_identities
    }

    fn symbol_package_identities(&self) -> &HashMap<ModuleId, String> {
        &self.symbol_package_identities
    }

    fn const_array_lengths(&self, module_id: ModuleId) -> Option<&HashMap<GlobalConstExprId, u64>> {
        self.module_indices
            .get(&module_id)
            .map(|index| self.const_array_lengths[*index].as_ref())
    }

    fn function_body_ids(&self) -> &[GlobalDefId] {
        &self.function_body_ids
    }

    fn function_body(&self, def_id: GlobalDefId) -> Option<&nia_function_ir::FunctionBody> {
        self.function_body_indices
            .get(&def_id)
            .and_then(|index| self.function_bodies[*index].value.body())
    }

    fn closure_entries(&self, def_id: GlobalDefId) -> &[nia_function_ir::FunctionClosureEntry] {
        self.function_body_indices
            .get(&def_id)
            .map(|index| self.function_bodies[*index].value.closure_entries())
            .unwrap_or_default()
    }

    fn static_init_ids(&self) -> &[GlobalDefId] {
        &self.static_init_ids
    }

    fn static_init(&self, def_id: GlobalDefId) -> Option<&nia_static_ir::StaticInit> {
        self.static_init_indices
            .get(&def_id)
            .and_then(|index| self.static_inits[*index].value.as_ref().as_deref())
    }

    fn extension_methods(&self) -> &nia_defs::ExtensionMethods {
        &self.extension_methods.methods
    }

    fn extensions(&self, module_id: ModuleId) -> Option<&nia_defs::VisibleExtensionMethods> {
        self.module_indices
            .get(&module_id)
            .map(|index| self.visible_extensions[*index].methods.as_ref())
    }

    fn defs(&self, module_id: ModuleId) -> Option<&DefCollection> {
        self.module_indices
            .get(&module_id)
            .map(|index| self.program_defs[*index].as_ref())
    }

    fn normalized_type(&self, ty: InternedTyId) -> Option<InternedTyId> {
        self.checked_modules
            .iter()
            .filter_map(|module| {
                module
                    .type_normalization
                    .normalized
                    .get(&ty)
                    .copied()
                    .map(|normalized| (module.id, normalized))
            })
            .min_by_key(|(module_id, _)| *module_id)
            .map(|(_, normalized)| normalized)
    }

    fn normalized_type_from_module(
        &self,
        module_id: ModuleId,
        ty: InternedTyId,
    ) -> Option<InternedTyId> {
        self.module_indices.get(&module_id).and_then(|index| {
            self.checked_modules[*index]
                .type_normalization
                .normalized
                .get(&ty)
                .copied()
        })
    }

    fn functions(&self) -> &HashMap<GlobalDefId, ProgramFunctionSignature> {
        &self.functions
    }

    fn structs(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramStructSignature> {
        &self.non_function_signatures.structs
    }

    fn unions(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramUnionSignature> {
        &self.non_function_signatures.unions
    }

    fn enums(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramEnumSignature> {
        &self.non_function_signatures.enums
    }

    fn traits(&self) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramTraitSignature> {
        &self.non_function_signatures.traits
    }

    fn type_aliases(
        &self,
    ) -> &HashMap<GlobalDefId, nia_item_signatures::ProgramTypeAliasSignature> {
        &self.non_function_signatures.type_aliases
    }

    fn trait_impls(&self) -> &[nia_item_signatures::ProgramTraitImplSignature] {
        &self.non_function_signatures.trait_impls
    }

    fn trait_impl_index(&self) -> &nia_item_signatures::ProgramTraitImplIndex {
        &self.non_function_signatures.trait_impl_index
    }
}

pub(super) struct BackendFinalizationTaskContext {
    inputs: Arc<BackendLoweringInputs>,
    finalization: nia_backend_lower::BackendProgramFinalizationContext,
}

impl BackendFinalizationTaskContext {
    pub(super) fn new(
        inputs: Arc<ProgramBackendLoweringInputs>,
        type_store: Arc<nia_ty::TypeStore>,
        optimization: nia_opt::OptimizationPolicy,
        timings: nia_timing::TimingMode,
    ) -> QueryResult<Self> {
        let Some(inputs) = inputs.semantic.as_ref().map(Arc::clone) else {
            return Err(QueryError::internal(
                "backend finalization context requires valid lowering inputs",
            ));
        };
        let module_inputs = inputs.module_inputs()?;
        let finalization = nia_backend_lower::BackendProgramFinalizationContext::new(
            &module_inputs,
            type_store,
            optimization,
            timings,
        );
        Ok(Self {
            inputs,
            finalization,
        })
    }

    pub(super) fn finalize_module(
        &self,
        position: usize,
        module_id: ModuleId,
        module_plan: nia_backend_lower::BackendModuleItemPlan,
    ) -> QueryResult<nia_backend_lower::BackendModuleFinalization> {
        let input = self.inputs.module_input(position)?;
        if input.module_id != module_id {
            return Err(QueryError::internal(format!(
                "backend finalization task position {position} belongs to {:?}, not {module_id:?}",
                input.module_id
            )));
        }
        Ok(self
            .finalization
            .finalize_module(position, &input, module_plan)?)
    }
}

fn backend_function_roots(
    runtime: &RuntimeSpec,
    checked_module: &CheckedModule,
) -> nia_backend_lower::BackendFunctionRoots {
    if checked_module.executable_type_only {
        return nia_backend_lower::BackendFunctionRoots::NoFunctions;
    }
    match runtime {
        RuntimeSpec::Bare => nia_backend_lower::BackendFunctionRoots::FunctionBodies,
        RuntimeSpec::Source(_) => nia_backend_lower::BackendFunctionRoots::EntryPoints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_backend_lower::BackendProgramFacts;
    use nia_function_ir::{FunctionBlockId, FunctionBody};
    use nia_ids::{DefId, ModuleIdAllocator};
    use nia_span::Span;
    use nia_ty::{PrimitiveTy, TyKind, TypeStore};

    trait TestTypeStoreAppend {
        fn test_intern(&self, kind: TyKind) -> nia_ids::InternedTyId;
    }

    impl TestTypeStoreAppend for nia_ty::TypeStoreAppend {
        fn test_intern(&self, kind: TyKind) -> nia_ids::InternedTyId {
            self.intern(kind)
                .expect("intern backend-lowering test type")
        }
    }

    #[test]
    fn program_ir_indexes_borrow_query_owned_payloads() {
        let module_ids = ModuleIdAllocator::new().expect("create module ID allocator");
        let module_id = module_ids.allocate().expect("allocate module ID");
        let type_store = TypeStore::new().expect("create type store");
        let ty = type_store
            .append_for_module(module_id)
            .test_intern(TyKind::Primitive(PrimitiveTy::I32));
        let def_id = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let lowered = vec![LoweredFunctionBodyHandle {
            def_id,
            value: Arc::new(LoweredFunctionBodyValue::Body(
                nia_function_lower::LoweredFunctionBody {
                    body: FunctionBody {
                        span: Span::default(),
                        locals: Vec::new(),
                        scopes: Vec::new(),
                        blocks: Vec::new(),
                        entry: FunctionBlockId(0),
                        ty,
                    },
                    closure_entries: Vec::new(),
                },
            )),
        }];

        let init = Arc::new(nia_static_ir::StaticInit::Bytes(vec![1, 2, 3]));
        let static_inits = vec![StaticInitHandle {
            def_id,
            value: Arc::new(Some(Arc::clone(&init))),
        }];
        let query_owned =
            lowered[0].value.body().expect("query-owned function body") as *const FunctionBody;

        let inputs = BackendLoweringInputs {
            symbols: nia_symbol_table::SymbolTable::new(),
            source_identities: HashMap::new(),
            symbol_package_identities: HashMap::new(),
            checked_modules: Vec::new(),
            module_indices: HashMap::new(),
            active_item_trees: Vec::new(),
            item_signatures: Vec::new(),
            const_array_lengths: Vec::new(),
            const_enum_values: Vec::new(),
            visible_extensions: Vec::new(),
            extension_methods: Arc::new(ExtensionMethodIndexQueryValue {
                methods: nia_defs::ExtensionMethods::default(),
            }),
            function_body_ids: vec![def_id],
            function_body_indices: HashMap::from([(def_id, 0)]),
            function_bodies: lowered,
            static_init_ids: vec![def_id],
            static_init_indices: HashMap::from([(def_id, 0)]),
            static_inits,
            source_item_plans: Vec::new(),
            function_instance_plans: Vec::new(),
            program_defs: Vec::new(),
            non_function_signatures: ProgramExecutableNonFunctionSignatures {
                globals: HashMap::new(),
                consts: HashMap::new(),
                structs: HashMap::new(),
                unions: HashMap::new(),
                enums: HashMap::new(),
                type_aliases: HashMap::new(),
                traits: HashMap::new(),
                trait_impls: Vec::new(),
                trait_impl_index: nia_item_signatures::ProgramTraitImplIndex::default(),
                trait_method_index: nia_program_signatures::ProgramTraitMethodIndex::default(),
            },
            functions: HashMap::new(),
            runtime: RuntimeSpec::Bare,
        };
        let indexed = inputs.function_body(def_id).expect("indexed function body");

        assert_eq!(indexed as *const FunctionBody, query_owned);
        let indexed_init = inputs
            .static_init(def_id)
            .expect("indexed static initializer");
        assert!(std::ptr::eq(indexed_init, init.as_ref()));
    }
}
