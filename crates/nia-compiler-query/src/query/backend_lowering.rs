// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use nia_item_signatures::ItemSignatures;

pub(super) struct BackendLoweringIndexes<'a> {
    pub(super) program_extensions: HashMap<ModuleId, &'a nia_defs::VisibleExtensionMethods>,
    pub(super) program_type_normalizations:
        HashMap<ModuleId, &'a HashMap<InternedTyId, InternedTyId>>,
    pub(super) program_function_bodies: HashMap<GlobalDefId, &'a nia_function_ir::FunctionBody>,
    pub(super) program_const: HashMap<ModuleId, &'a nia_const_check::ConstArrayLengths>,
}

pub(super) fn build_backend_lowering_indexes<'a>(
    visible_extension_modules: &'a [(ModuleId, Arc<VisibleExtensionsValue>)],
    checked_modules: &'a [Arc<CheckedModule>],
    const_array_lengths: &'a [nia_const_check::ConstArrayLengths],
    function_bodies: &'a [Arc<LoweredFunctionBodies>],
) -> BackendLoweringIndexes<'a> {
    let program_extensions = visible_extension_modules
        .iter()
        .map(|(module_id, visible_extensions)| (*module_id, &visible_extensions.methods))
        .collect::<HashMap<_, _>>();
    let program_function_bodies = function_bodies
        .iter()
        .flat_map(|lowered| lowered.bodies.iter().map(|(def_id, body)| (*def_id, body)))
        .collect::<HashMap<_, _>>();
    let program_const = checked_modules
        .iter()
        .zip(const_array_lengths.iter())
        .map(|(checked_module, array_lengths)| (checked_module.id, array_lengths))
        .collect::<HashMap<_, _>>();
    let program_type_normalizations = checked_modules
        .iter()
        .map(|checked_module| {
            (
                checked_module.id,
                &checked_module.type_normalization.normalized,
            )
        })
        .collect::<HashMap<_, _>>();

    BackendLoweringIndexes {
        program_extensions,
        program_type_normalizations,
        program_function_bodies,
        program_const,
    }
}

pub(super) struct BackendLoweringModuleInputsInput<'a> {
    pub(super) symbols: &'a dyn nia_symbol::SymbolText,
    pub(super) checked_modules: &'a [Arc<CheckedModule>],
    pub(super) runtime: RuntimeModel,
    pub(super) active_item_trees: &'a [Arc<ActiveModuleItemTree>],
    pub(super) item_signatures: &'a [ItemSignatures],
    pub(super) const_array_lengths: &'a [nia_const_check::ConstArrayLengths],
    pub(super) const_enum_values: &'a [nia_const_check::ConstEnumValues],
    pub(super) visible_extensions: &'a [Arc<VisibleExtensionsValue>],
    pub(super) function_bodies: &'a [Arc<LoweredFunctionBodies>],
    pub(super) extension_methods: &'a nia_defs::ExtensionMethods,
    pub(super) program_defs: &'a dyn Fn(ModuleId) -> Option<Arc<DefCollection>>,
    pub(super) program_signatures: ProgramCodegenSignatures<'a>,
    pub(super) indexes: &'a BackendLoweringIndexes<'a>,
}

pub(super) fn build_backend_lowering_module_inputs<'a>(
    input: BackendLoweringModuleInputsInput<'a>,
) -> Vec<BackendLowerModuleInput<'a>> {
    input
        .checked_modules
        .iter()
        .zip(input.active_item_trees.iter())
        .zip(input.item_signatures.iter())
        .zip(input.const_array_lengths.iter())
        .zip(input.const_enum_values.iter())
        .zip(input.visible_extensions.iter())
        .zip(input.function_bodies.iter())
        .map(
            |(
                (
                    (
                        (
                            ((checked_module, active_item_tree), item_signatures),
                            const_array_lengths,
                        ),
                        const_enum_values,
                    ),
                    visible_extensions,
                ),
                function_bodies,
            )| {
                BackendLowerModuleInput {
                    module_id: checked_module.id,
                    module_name: checked_module.path.as_str().to_string(),
                    symbols: input.symbols,
                    active_item_tree: active_item_tree.as_ref(),
                    defs: &checked_module.defs,
                    extensions: &visible_extensions.methods,
                    values: &checked_module.value_resolution,
                    locals: &checked_module.local_resolution,
                    type_lowering: &checked_module.type_lowering,
                    signatures: item_signatures,
                    type_normalization: &checked_module.type_normalization,
                    body_ir: &checked_module.body_ir,
                    semantic_facts: &checked_module.semantic_facts,
                    const_array_lengths,
                    const_enum_values,
                    program_const: &input.indexes.program_const,
                    layouts: &checked_module.layouts,
                    function_bodies: &function_bodies.bodies,
                    roots: backend_function_roots(input.runtime, checked_module),
                    reachable_globals: checked_module.executable_reachable_globals.as_ref(),
                    reachable_structs: checked_module.executable_reachable_structs.as_deref(),
                    reachable_unions: checked_module.executable_reachable_unions.as_deref(),
                    program_function_bodies: &input.indexes.program_function_bodies,
                    program_extension_methods: input.extension_methods,
                    program_extensions: &input.indexes.program_extensions,
                    program_defs: input.program_defs,
                    program_type_normalizations: &input.indexes.program_type_normalizations,
                    program_functions: input.program_signatures.functions,
                    program_structs: input.program_signatures.structs,
                    program_unions: input.program_signatures.unions,
                    program_enums: input.program_signatures.enums,
                    program_traits: input.program_signatures.traits,
                    program_type_aliases: input.program_signatures.type_aliases,
                    trait_impls: input.program_signatures.trait_impls,
                    trait_impl_index: input.program_signatures.trait_impl_index,
                }
            },
        )
        .collect()
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
    use nia_function_ir::{FunctionBlockId, FunctionBody};
    use nia_ids::{DefId, ModuleIdAllocator};
    use nia_span::Span;
    use nia_ty::{PrimitiveTy, TyKind, TypeStore};

    #[test]
    fn program_function_body_index_borrows_query_owned_bodies() {
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
        let lowered = vec![Arc::new(LoweredFunctionBodies {
            bodies: HashMap::from([(
                def_id,
                FunctionBody {
                    span: Span::default(),
                    locals: Vec::new(),
                    scopes: Vec::new(),
                    blocks: Vec::new(),
                    entry: FunctionBlockId(0),
                    ty,
                },
            )]),
            diagnostics: Vec::new(),
        })];

        let indexes = build_backend_lowering_indexes(&[], &[], &[], &lowered);
        let query_owned = lowered[0]
            .bodies
            .get(&def_id)
            .expect("query-owned function body");
        let indexed = indexes
            .program_function_bodies
            .get(&def_id)
            .copied()
            .expect("indexed function body");

        assert!(std::ptr::eq(indexed, query_owned));
    }
}
