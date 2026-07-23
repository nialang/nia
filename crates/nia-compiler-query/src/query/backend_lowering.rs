// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_item_signatures::ItemSignatures;

pub(super) struct BackendLoweringInputs {
    symbols: nia_symbol_table::SymbolTable,
    checked_modules: Vec<Arc<CheckedModule>>,
    module_indices: HashMap<ModuleId, usize>,
    active_item_trees: Vec<Arc<ActiveModuleItemTree>>,
    item_signatures: Vec<ItemSignatures>,
    const_array_lengths: Vec<nia_const_check::ConstArrayLengths>,
    const_enum_values: Vec<nia_const_check::ConstEnumValues>,
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
    runtime: RuntimeModel,
}

pub(super) struct BackendLoweringInputsParts {
    pub(super) symbols: nia_symbol_table::SymbolTable,
    pub(super) checked_modules: Vec<Arc<CheckedModule>>,
    pub(super) active_item_trees: Vec<Arc<ActiveModuleItemTree>>,
    pub(super) item_signatures: Vec<ItemSignatures>,
    pub(super) const_array_lengths: Vec<nia_const_check::ConstArrayLengths>,
    pub(super) const_enum_values: Vec<nia_const_check::ConstEnumValues>,
    pub(super) visible_extensions: Vec<Arc<VisibleExtensionsValue>>,
    pub(super) extension_methods: Arc<ExtensionMethodIndexValue>,
    pub(super) function_bodies: Vec<LoweredFunctionBodyHandle>,
    pub(super) static_inits: Vec<StaticInitHandle>,
    pub(super) source_item_plans: Vec<Arc<BackendModuleSourceItemPlan>>,
    pub(super) function_instance_plans: Vec<Arc<BackendModuleFunctionInstancePlan>>,
    pub(super) program_defs: Vec<Arc<DefCollection>>,
    pub(super) non_function_signatures: ProgramExecutableNonFunctionSignatures,
    pub(super) functions: HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub(super) runtime: RuntimeModel,
}

impl BackendLoweringInputs {
    pub(super) fn new(parts: BackendLoweringInputsParts) -> Self {
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
            assert_eq!(
                actual, module_count,
                "Nia ICE: backend {name} must match checked module count"
            );
        }
        let module_indices = parts
            .checked_modules
            .iter()
            .enumerate()
            .map(|(index, module)| (module.id, index))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            module_indices.len(),
            module_count,
            "Nia ICE: backend inputs must have unique module owners"
        );
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
        Self {
            symbols: parts.symbols,
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
        }
    }

    pub(super) fn module_inputs(&self) -> Vec<BackendLowerModuleInput<'_>> {
        (0..self.checked_modules.len())
            .map(|index| self.module_input(index))
            .collect()
    }

    pub(super) fn module_input(&self, index: usize) -> BackendLowerModuleInput<'_> {
        let checked_module = &self.checked_modules[index];
        let source_item_plan = &self.source_item_plans[index];
        let function_instance_plan = &self.function_instance_plans[index];
        BackendLowerModuleInput {
            module_id: checked_module.id,
            module_name: checked_module.path.as_str().to_string(),
            symbols: &self.symbols,
            active_item_tree: self.active_item_trees[index].as_ref(),
            defs: &checked_module.defs,
            extensions: &self.visible_extensions[index].methods,
            values: &checked_module.value_resolution,
            locals: &checked_module.local_resolution,
            type_lowering: &checked_module.type_lowering,
            signatures: &self.item_signatures[index],
            type_normalization: &checked_module.type_normalization,
            semantic_facts: &checked_module.semantic_facts,
            const_array_lengths: &self.const_array_lengths[index],
            const_enum_values: &self.const_enum_values[index],
            layouts: &checked_module.layouts,
            roots: backend_function_roots(self.runtime, checked_module),
            reachable_functions: Some(&source_item_plan.functions),
            reachable_globals: Some(&source_item_plan.globals),
            reachable_structs: Some(&source_item_plan.structs),
            reachable_unions: Some(&source_item_plan.unions),
            function_instance_plan: &function_instance_plan.instances,
            program: self,
        }
    }
}

impl nia_backend_lower::BackendProgramFacts for BackendLoweringInputs {
    fn const_array_lengths(
        &self,
        module_id: ModuleId,
    ) -> Option<&nia_const_check::ConstArrayLengths> {
        self.module_indices
            .get(&module_id)
            .map(|index| &self.const_array_lengths[*index])
    }

    fn function_body_ids(&self) -> &[GlobalDefId] {
        &self.function_body_ids
    }

    fn function_body(&self, def_id: GlobalDefId) -> Option<&nia_function_ir::FunctionBody> {
        self.function_body_indices
            .get(&def_id)
            .and_then(|index| self.function_bodies[*index].value.body())
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
            .map(|index| &self.visible_extensions[*index].methods)
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
    inputs: Arc<Result<BackendLoweringInputs, Vec<Diagnostic>>>,
    finalization: nia_backend_lower::BackendProgramFinalizationContext,
}

impl BackendFinalizationTaskContext {
    pub(super) fn new(
        inputs: Arc<Result<BackendLoweringInputs, Vec<Diagnostic>>>,
        type_store: Arc<nia_ty::TypeStore>,
        optimization: nia_opt::OptimizationPolicy,
        timings: nia_timing::TimingMode,
    ) -> Self {
        let module_inputs = inputs
            .as_ref()
            .as_ref()
            .expect("Nia ICE: backend finalization context requires valid lowering inputs")
            .module_inputs();
        let finalization = nia_backend_lower::BackendProgramFinalizationContext::new(
            &module_inputs,
            type_store,
            optimization,
            timings,
        );
        Self {
            inputs,
            finalization,
        }
    }

    pub(super) fn finalize_module(
        &self,
        position: usize,
        module_id: ModuleId,
        module_plan: nia_backend_lower::BackendModuleItemPlan,
    ) -> nia_backend_lower::BackendModuleFinalization {
        let inputs = self
            .inputs
            .as_ref()
            .as_ref()
            .expect("Nia ICE: backend finalization task requires valid lowering inputs");
        let input = inputs.module_input(position);
        assert_eq!(
            input.module_id, module_id,
            "Nia ICE: backend finalization task position must match module owner"
        );
        self.finalization
            .finalize_module(position, &input, module_plan)
    }
}

fn backend_function_roots(
    runtime: RuntimeModel,
    checked_module: &CheckedModule,
) -> nia_backend_lower::BackendFunctionRoots {
    if checked_module.executable_type_only {
        return nia_backend_lower::BackendFunctionRoots::NoFunctions;
    }
    match runtime {
        RuntimeModel::Bare => nia_backend_lower::BackendFunctionRoots::FunctionBodies,
        RuntimeModel::FreestandingExecutable => {
            nia_backend_lower::BackendFunctionRoots::EntryPoints
        }
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

    #[test]
    fn program_ir_indexes_borrow_query_owned_payloads() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let type_store = TypeStore::new();
        let ty = type_store
            .append_for_module(module_id)
            .intern(TyKind::Primitive(PrimitiveTy::I32));
        let def_id = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let lowered = vec![LoweredFunctionBodyHandle {
            def_id,
            value: Arc::new(LoweredFunctionBodyValue::Body(FunctionBody {
                span: Span::default(),
                locals: Vec::new(),
                scopes: Vec::new(),
                blocks: Vec::new(),
                entry: FunctionBlockId(0),
                ty,
            })),
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
            runtime: RuntimeModel::Bare,
        };
        let indexed = inputs.function_body(def_id).expect("indexed function body");

        assert_eq!(indexed as *const FunctionBody, query_owned);
        let indexed_init = inputs
            .static_init(def_id)
            .expect("indexed static initializer");
        assert!(std::ptr::eq(indexed_init, init.as_ref()));
    }
}
