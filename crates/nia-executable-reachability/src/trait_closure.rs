// SPDX-License-Identifier: GPL-3.0-or-later
//! Trait and generic-instance closure for executable reachability.
//!
//! Direct body references seed this closure. It then substitutes concrete
//! generic instances into function/impl predicates, adds the required trait
//! methods and supertraits, and activates their implementation bodies. Both the
//! batch and incremental paths use the same helpers so a body discovered in a
//! later query round receives the same transitive witnesses as an initial root.

use super::*;

#[derive(Clone, Copy)]
struct GenericTraitReachabilityContext<'a> {
    modules_by_id: &'a HashMap<ModuleId, ReachableModuleInput<'a>>,
    program_signatures: ExecutableSignatureIndex<'a>,
    extension_index: &'a dyn ExecutableExtensionLookup,
}

pub(super) fn extend_reachable_functions_from_bodies(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachability: &mut ExecutableReachability,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let refs = typed_executable_refs(module, &reachability.functions, &reachability.globals);
    for instantiation in &refs.generic_instantiations {
        add_reachable_function(
            instantiation.def_id,
            program_signatures,
            reachability,
            pending_modules,
        );
    }
    for def_id in refs.functions {
        add_reachable_function(def_id, program_signatures, reachability, pending_modules);
    }
    for def_id in refs.globals {
        reachability.insert_global_pending(def_id, pending_modules);
    }
}

pub(super) fn collect_reachable_traits_for_modules(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &[ModuleId],
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> ReachableTraitRefs {
    let mut reachable_traits = ReachableTraitRefs::default();
    for module in current_reachable_module_inputs(modules_by_id, current_reachable_modules) {
        collect_reachable_body_trait_ids(
            &module,
            reachable_functions,
            reachable_globals,
            &mut reachable_traits,
        );
    }
    reachable_traits
}

pub(super) fn extend_reachable_traits_from_generic_instances(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &[ModuleId],
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    reachable_functions: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    for def_id in reachable_functions {
        if !module_id_list_contains(current_reachable_modules, def_id.module_id) {
            continue;
        }
        let Some(module) = modules_by_id.get(&def_id.module_id) else {
            continue;
        };
        let mut executable_refs = typed_executable_refs_for_function(module, *def_id);
        for instantiation in executable_refs.generic_instantiations.drain(..) {
            let mut visited = HashSet::default();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                module.type_store,
                GenericTraitReachabilityContext {
                    modules_by_id,
                    program_signatures,
                    extension_index,
                },
                traits,
                &instantiation,
                &mut visited,
                &mut HashSet::new(),
            );
        }
    }
}

pub(super) fn extend_reachable_traits_from_generic_instances_incremental(
    state: &mut IncrementalExecutableReachability,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &[ModuleId],
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
) {
    let pending_functions = state
        .reachability
        .functions
        .iter()
        .copied()
        .filter(|def_id| module_id_list_contains(current_reachable_modules, def_id.module_id))
        .filter(|def_id| !state.scanned_generic_trait_functions.contains(def_id))
        .collect::<Vec<_>>();
    for def_id in pending_functions {
        let Some(module) = modules_by_id.get(&def_id.module_id) else {
            continue;
        };
        state.scanned_generic_trait_functions.insert(def_id);
        let mut executable_refs = typed_executable_refs_for_function(module, def_id);
        for instantiation in executable_refs.generic_instantiations.drain(..) {
            let mut visited = HashSet::default();
            extend_reachable_traits_from_generic_instantiation(
                module.module_id,
                module.type_store,
                GenericTraitReachabilityContext {
                    modules_by_id,
                    program_signatures,
                    extension_index,
                },
                &mut state.reachable_traits,
                &instantiation,
                &mut visited,
                &mut HashSet::new(),
            );
        }
    }
}

fn extend_reachable_traits_from_generic_instantiation(
    use_module_id: ModuleId,
    type_store: &TypeStore,
    context: GenericTraitReachabilityContext<'_>,
    traits: &mut ReachableTraitRefs,
    instantiation: &nia_sema_ir::GenericInstantiation,
    visited: &mut HashSet<ReachableGenericInstantiationKey>,
    active_defs: &mut HashSet<GlobalDefId>,
) {
    let GenericTraitReachabilityContext {
        modules_by_id,
        program_signatures,
        extension_index,
    } = context;
    if !visited.insert(reachable_generic_instantiation_key(instantiation)) {
        return;
    }
    if !active_defs.insert(instantiation.def_id) {
        return;
    }
    extend_reachable_traits_from_trait_default_instantiation(
        use_module_id,
        program_signatures,
        traits,
        instantiation,
    );
    let Some(signature) = (program_signatures.function)(instantiation.def_id) else {
        active_defs.remove(&instantiation.def_id);
        return;
    };
    let append = type_store.append_for_module(use_module_id);
    let types = ReachabilityTypeCx {
        store: type_store,
        append: &append,
    };
    let generics = if instantiation.generics.is_empty() && !instantiation.args.is_empty() {
        &signature.signature.generics
    } else {
        &instantiation.generics
    };
    let mut const_generics = signature
        .signature
        .generic_params
        .iter()
        .filter_map(|param| match param.kind {
            nia_item_signatures::GenericParamSignatureKind::Const { .. } => Some(param.name),
            nia_item_signatures::GenericParamSignatureKind::Type => None,
        })
        .collect::<Vec<_>>();
    extension_index.with_const_generics_for_def(instantiation.def_id, &mut |extension_generics| {
        if !extension_generics.is_empty() {
            const_generics = extension_generics.to_vec();
        }
    });
    let const_generic_set = const_generics.iter().copied().collect::<HashSet<_>>();
    // `GenericInstantiation` stores type and const arguments in separate
    // vectors. Filter the effective declaration-order names by kind before
    // zipping, otherwise an interleaved `N: usize, T` list binds `N` to `T`'s
    // type argument and drops the actual const value.
    let generic_substitutions = generics
        .iter()
        .copied()
        .filter(|name| !const_generic_set.contains(name))
        .zip(instantiation.args.iter().copied())
        .collect::<SymbolMap<_>>();
    let const_substitutions = const_generics
        .iter()
        .copied()
        .zip(instantiation.const_args.iter().cloned())
        .collect::<SymbolMap<_>>();
    let self_ty = instantiation.self_arg;
    let substitutions =
        TypeSubstitutions::local_with_consts(self_ty, &generic_substitutions, &const_substitutions);
    for predicate in &signature.signature.where_predicates {
        let Some(self_ty) = substitute_ty(types, predicate.ty, &substitutions) else {
            continue;
        };
        for bound in &predicate.bounds {
            let Some(trait_ty) = substitute_ty(types, bound.trait_ty, &substitutions) else {
                continue;
            };
            let Some((trait_id, trait_args, trait_const_args)) =
                trait_id_and_args(type_store, trait_ty)
            else {
                continue;
            };
            insert_trait_and_supertrait_methods(
                program_signatures,
                type_store,
                traits,
                TraitMethodExpansionInput {
                    module_id: use_module_id,
                    trait_id,
                    self_ty,
                    trait_args: &trait_args,
                    trait_const_args: &trait_const_args,
                },
            );
        }
    }
    extension_index.with_where_predicates_for_def(instantiation.def_id, &mut |predicates| {
        for predicate in predicates {
            let Some(self_ty) = substitute_ty(types, predicate.ty, &substitutions) else {
                continue;
            };
            for bound in &predicate.bounds {
                let Some(trait_ty) = substitute_ty(types, bound.trait_ty, &substitutions) else {
                    continue;
                };
                let Some((trait_id, trait_args, trait_const_args)) =
                    trait_id_and_args(type_store, trait_ty)
                else {
                    continue;
                };
                insert_trait_and_supertrait_methods(
                    program_signatures,
                    type_store,
                    traits,
                    TraitMethodExpansionInput {
                        module_id: use_module_id,
                        trait_id,
                        self_ty,
                        trait_args: &trait_args,
                        trait_const_args: &trait_const_args,
                    },
                );
            }
        }
    });
    let Some(target_module) = modules_by_id.get(&instantiation.def_id.module_id) else {
        active_defs.remove(&instantiation.def_id);
        return;
    };
    let nested_instantiations = target_module
        .semantic_facts
        .function_facts
        .get(&instantiation.def_id)
        .into_iter()
        .flat_map(|facts| facts.generic_instantiations.iter())
        .chain(
            target_module
                .semantic_facts
                .generic_instantiations
                .iter()
                .filter(|nested| nested.source_def_id == Some(instantiation.def_id)),
        );
    for nested in nested_instantiations {
        let Some(nested_instantiation) =
            instantiate_nested_generic_instantiation(types, nested, &substitutions)
        else {
            continue;
        };
        extend_reachable_traits_from_generic_instantiation(
            use_module_id,
            type_store,
            context,
            traits,
            &nested_instantiation,
            visited,
            active_defs,
        );
    }
    active_defs.remove(&instantiation.def_id);
}

fn extend_reachable_traits_from_trait_default_instantiation(
    use_module_id: ModuleId,
    program_signatures: ExecutableSignatureIndex<'_>,
    traits: &mut ReachableTraitRefs,
    instantiation: &nia_sema_ir::GenericInstantiation,
) {
    let Some((trait_def, trait_signature)) =
        (program_signatures.trait_default_method)(instantiation.def_id)
    else {
        return;
    };
    let Some(_) = trait_signature
        .signature
        .methods
        .iter()
        .find(|method| method.def_id == instantiation.def_id.def_id && method.has_default)
    else {
        return;
    };
    let trait_id = TraitId::Source(trait_def);
    let Some(self_ty) = instantiation.self_arg else {
        return;
    };
    let (trait_args, trait_const_args) = split_trait_generic_args(
        &trait_signature.signature.generic_params,
        &instantiation.args,
        &instantiation.const_args,
    );
    traits.insert_methods_with_const_args(
        use_module_id,
        trait_id,
        trait_signature
            .signature
            .methods
            .iter()
            .map(|method| ReachableTraitMethodName { name: method.name }),
        self_ty,
        &trait_args,
        &trait_const_args,
    );
}

struct TraitMethodExpansionInput<'a> {
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: &'a [InternedTyId],
    trait_const_args: &'a [nia_ty::ConstGenericArg],
}

fn insert_trait_and_supertrait_methods(
    program_signatures: ExecutableSignatureIndex<'_>,
    type_store: &TypeStore,
    traits: &mut ReachableTraitRefs,
    input: TraitMethodExpansionInput<'_>,
) {
    let append = type_store.append_for_module(input.module_id);
    TraitMethodExpansion {
        program_signatures,
        types: ReachabilityTypeCx {
            store: type_store,
            append: &append,
        },
        traits,
        module_id: input.module_id,
        active_traits: HashSet::new(),
    }
    .insert(
        input.trait_id,
        input.self_ty,
        input.trait_args,
        input.trait_const_args,
    );
}

struct TraitMethodExpansion<'a, 'b> {
    program_signatures: ExecutableSignatureIndex<'a>,
    types: ReachabilityTypeCx<'b>,
    traits: &'b mut ReachableTraitRefs,
    module_id: ModuleId,
    active_traits: HashSet<(TraitId, Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>)>,
}

impl TraitMethodExpansion<'_, '_> {
    fn insert(
        &mut self,
        trait_id: TraitId,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
    ) {
        if !self
            .active_traits
            .insert((trait_id, trait_args.to_vec(), trait_const_args.to_vec()))
        {
            return;
        }
        match trait_id {
            TraitId::Builtin(builtin_trait) => {
                self.traits.insert_methods_with_const_args(
                    self.module_id,
                    trait_id,
                    builtin_trait
                        .required_methods()
                        .iter()
                        .filter_map(|method| builtin_trait_method_symbol(*method))
                        .map(|name| ReachableTraitMethodName { name }),
                    self_ty,
                    trait_args,
                    trait_const_args,
                );
                for supertrait in builtin_trait.supertraits() {
                    let supertrait_args = if supertrait.preserves_trait_args {
                        trait_args
                    } else {
                        &[]
                    };
                    self.insert(
                        TraitId::Builtin(supertrait.trait_id),
                        self_ty,
                        supertrait_args,
                        &[],
                    );
                }
            }
            TraitId::Source(trait_def) => {
                let Some(trait_signature) = (self.program_signatures.trait_)(trait_def) else {
                    self.active_traits.remove(&(
                        trait_id,
                        trait_args.to_vec(),
                        trait_const_args.to_vec(),
                    ));
                    return;
                };
                self.traits.insert_methods_with_const_args(
                    self.module_id,
                    trait_id,
                    trait_signature
                        .signature
                        .methods
                        .iter()
                        .map(|method| ReachableTraitMethodName { name: method.name }),
                    self_ty,
                    trait_args,
                    trait_const_args,
                );
                for supertrait in &trait_signature.signature.supertraits {
                    let (generic_substitutions, const_substitutions) = split_generic_substitutions(
                        &trait_signature.signature.generic_params,
                        trait_args,
                        trait_const_args,
                    );
                    let substitutions = TypeSubstitutions::local_with_consts(
                        Some(self_ty),
                        &generic_substitutions,
                        &const_substitutions,
                    );
                    let Some(supertrait_ty) =
                        substitute_ty(self.types, supertrait.ty, &substitutions)
                    else {
                        continue;
                    };
                    let Some((supertrait_id, supertrait_args, supertrait_const_args)) =
                        trait_id_and_args(self.types.store, supertrait_ty)
                    else {
                        continue;
                    };
                    self.insert(
                        supertrait_id,
                        self_ty,
                        &supertrait_args,
                        &supertrait_const_args,
                    );
                }
            }
        }
        self.active_traits
            .remove(&(trait_id, trait_args.to_vec(), trait_const_args.to_vec()));
    }
}

fn split_generic_substitutions(
    generic_params: &[nia_item_signatures::GenericParamSignature],
    type_args: &[InternedTyId],
    const_args: &[nia_ty::ConstGenericArg],
) -> (SymbolMap<InternedTyId>, SymbolMap<nia_ty::ConstGenericArg>) {
    let mut types = SymbolMap::default();
    let mut consts = SymbolMap::default();
    let mut type_index = 0;
    let mut const_index = 0;
    for param in generic_params {
        match param.kind {
            nia_item_signatures::GenericParamSignatureKind::Type => {
                if let Some(arg) = type_args.get(type_index).copied() {
                    types.insert(param.name, arg);
                }
                type_index += 1;
            }
            nia_item_signatures::GenericParamSignatureKind::Const { .. } => {
                if let Some(arg) = const_args.get(const_index).cloned() {
                    consts.insert(param.name, arg);
                }
                const_index += 1;
            }
        }
    }
    (types, consts)
}

fn split_trait_generic_args(
    generic_params: &[nia_item_signatures::GenericParamSignature],
    type_args: &[InternedTyId],
    const_args: &[nia_ty::ConstGenericArg],
) -> (Vec<InternedTyId>, Vec<nia_ty::ConstGenericArg>) {
    let type_count = generic_params
        .iter()
        .filter(|param| {
            matches!(
                param.kind,
                nia_item_signatures::GenericParamSignatureKind::Type
            )
        })
        .count();
    let const_count = generic_params.len() - type_count;
    (
        type_args.iter().take(type_count).copied().collect(),
        const_args.iter().take(const_count).cloned().collect(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableGenericInstantiationKey {
    def_id: GlobalDefId,
    args: Vec<InternedTyId>,
    const_args: Vec<nia_ty::ConstGenericArg>,
}

fn reachable_generic_instantiation_key(
    instantiation: &nia_sema_ir::GenericInstantiation,
) -> ReachableGenericInstantiationKey {
    ReachableGenericInstantiationKey {
        def_id: instantiation.def_id,
        args: instantiation.args.clone(),
        const_args: instantiation.const_args.clone(),
    }
}

fn instantiate_nested_generic_instantiation(
    types: ReachabilityTypeCx<'_>,
    instantiation: &nia_sema_ir::GenericInstantiation,
    substitutions: &TypeSubstitutions<'_>,
) -> Option<nia_sema_ir::GenericInstantiation> {
    let self_arg = match instantiation.self_arg {
        Some(self_arg) => Some(substitute_ty(types, self_arg, substitutions)?),
        None => None,
    };
    let args = instantiation
        .args
        .iter()
        .map(|arg| substitute_ty(types, *arg, substitutions))
        .collect::<Option<Vec<_>>>()?;
    let const_args = instantiation
        .const_args
        .iter()
        .map(|arg| substitute_const_arg(types, arg, substitutions))
        .collect::<Option<Vec<_>>>()?;
    Some(nia_sema_ir::GenericInstantiation {
        def_id: instantiation.def_id,
        self_arg,
        args,
        const_args,
        generics: instantiation.generics.clone(),
        span: instantiation.span,
        source_def_id: instantiation.source_def_id,
    })
}

fn typed_executable_refs(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> TypedExecutableRefs {
    typed_executable_refs_for_items(module, reachable_functions, reachable_globals)
}

pub(super) fn typed_executable_refs_for_items(
    module: &ReachableModuleInput<'_>,
    functions: &HashSet<GlobalDefId>,
    globals: &HashSet<GlobalDefId>,
) -> TypedExecutableRefs {
    let refs = module.executable_refs.refs_for_items(functions, globals);
    typed_executable_refs_from_executable_refs(refs)
}

fn typed_executable_refs_for_function(
    module: &ReachableModuleInput<'_>,
    def_id: GlobalDefId,
) -> TypedExecutableRefs {
    let refs = module.executable_refs.refs_for_function(def_id);
    typed_executable_refs_from_executable_refs(refs)
}

pub(super) fn collect_reachable_body_trait_ids(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    let refs = typed_executable_refs_for_items(module, reachable_functions, reachable_globals);
    traits.extend(refs.traits);
}

fn typed_executable_refs_from_executable_refs(refs: ExecutableItemRefs) -> TypedExecutableRefs {
    let mut traits = ReachableTraitRefs::default();
    for trait_id in refs.trait_refs.traits {
        traits.insert_trait(trait_id);
    }
    for method in refs.trait_refs.methods {
        traits.insert_method_with_const_args(
            method.module_id,
            method.trait_id,
            method.method_name,
            method.self_ty,
            method.trait_args,
            method.trait_const_args,
        );
    }
    for vtable in refs.trait_refs.vtables {
        traits.insert_vtable_with_const_args(
            vtable.module_id,
            vtable.trait_id,
            vtable.self_ty,
            vtable.trait_args,
            vtable.trait_const_args,
        );
    }
    TypedExecutableRefs {
        functions: refs.functions,
        globals: refs.globals,
        traits,
        generic_instantiations: refs.generic_instantiations,
    }
}

#[derive(Default)]
pub(super) struct TypedExecutableRefs {
    pub(super) functions: HashSet<GlobalDefId>,
    pub(super) globals: HashSet<GlobalDefId>,
    pub(super) traits: ReachableTraitRefs,
    pub(super) generic_instantiations: Vec<nia_sema_ir::GenericInstantiation>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ReachableTraitRefs {
    traits: HashSet<TraitId>,
    methods: Vec<ReachableTraitMethod>,
    method_keys: HashSet<ReachableTraitMethodKey>,
    vtables: Vec<ReachableTraitVtable>,
    vtable_keys: HashSet<ReachableTraitVtableKey>,
}

#[derive(Debug, Clone)]
struct ReachableTraitMethod {
    module_id: ModuleId,
    trait_id: TraitId,
    method_name: SymbolId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitMethodKey {
    module_id: ModuleId,
    trait_id: TraitId,
    method_name: SymbolId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
}

#[derive(Debug, Clone)]
struct ReachableTraitVtable {
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitVtableKey {
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
    trait_const_args: Vec<nia_ty::ConstGenericArg>,
}

pub(super) struct ReachableTraitMethodName {
    pub(super) name: SymbolId,
}

impl ReachableTraitRefs {
    pub(super) fn counts(&self) -> (usize, usize, usize) {
        (self.traits.len(), self.methods.len(), self.vtables.len())
    }

    pub(super) fn extend(&mut self, refs: Self) {
        let ReachableTraitRefs {
            traits,
            methods,
            vtables,
            ..
        } = refs;
        self.traits.extend(traits);
        for method in methods {
            self.insert_method_with_const_args(
                method.module_id,
                method.trait_id,
                method.method_name,
                method.self_ty,
                method.trait_args,
                method.trait_const_args,
            );
        }
        for vtable in vtables {
            self.insert_vtable_with_const_args(
                vtable.module_id,
                vtable.trait_id,
                vtable.self_ty,
                vtable.trait_args,
                vtable.trait_const_args,
            );
        }
    }

    pub(super) fn insert_trait(&mut self, trait_id: TraitId) {
        self.traits.insert(trait_id);
    }

    pub(super) fn insert_method_with_const_args(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<nia_ty::ConstGenericArg>,
    ) {
        self.traits.insert(trait_id);
        if !self.method_keys.insert(ReachableTraitMethodKey {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args: trait_args.clone(),
            trait_const_args: trait_const_args.clone(),
        }) {
            return;
        };
        self.methods.push(ReachableTraitMethod {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            trait_const_args,
        });
    }

    pub(super) fn insert_methods_with_const_args(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        methods: impl IntoIterator<Item = ReachableTraitMethodName>,
        self_ty: InternedTyId,
        trait_args: &[InternedTyId],
        trait_const_args: &[nia_ty::ConstGenericArg],
    ) {
        for method in methods {
            self.insert_method_with_const_args(
                module_id,
                trait_id,
                method.name,
                self_ty,
                trait_args.to_vec(),
                trait_const_args.to_vec(),
            );
        }
    }

    fn insert_vtable_with_const_args(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<nia_ty::ConstGenericArg>,
    ) {
        self.traits.insert(trait_id);
        if !self.vtable_keys.insert(ReachableTraitVtableKey {
            module_id,
            trait_id,
            self_ty,
            trait_args: trait_args.clone(),
            trait_const_args: trait_const_args.clone(),
        }) {
            return;
        }
        self.vtables.push(ReachableTraitVtable {
            module_id,
            trait_id,
            self_ty,
            trait_args,
            trait_const_args,
        });
    }

    fn needs_method(&self, trait_id: TraitId, method_name: &SymbolId) -> bool {
        self.methods
            .iter()
            .any(|method| method.trait_id == trait_id && &method.method_name == method_name)
            || self
                .vtables
                .iter()
                .any(|vtable| vtable.trait_id == trait_id)
    }
}

struct DeferredModuleActivation<'a> {
    reachable_functions: &'a mut HashSet<GlobalDefId>,
    reachable_modules: &'a HashSet<ModuleId>,
    pending_module_set: &'a mut HashSet<ModuleId>,
    pending_modules: &'a mut VecDeque<ModuleId>,
}

impl DeferredModuleActivation<'_> {
    fn is_reachable_module(&self, module_id: ModuleId) -> bool {
        self.reachable_modules.contains(&module_id)
    }

    fn add_function(
        &mut self,
        def_id: GlobalDefId,
        program_signatures: ExecutableSignatureIndex<'_>,
    ) {
        if !reachable_function_has_runtime_body(def_id, program_signatures) {
            self.add_module(def_id.module_id);
            return;
        }
        if self.reachable_functions.insert(def_id) {
            self.add_module(def_id.module_id);
        }
    }

    fn add_module(&mut self, module_id: ModuleId) {
        if !self.reachable_modules.contains(&module_id) && self.pending_module_set.insert(module_id)
        {
            self.pending_modules.push_back(module_id);
        }
    }
}

pub(super) fn extend_reachable_functions_from_traits(
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    reachable_traits: &mut ReachableTraitRefs,
    reachability: &mut ExecutableReachability,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let Some(type_store) = modules_by_id
        .values()
        .next()
        .map(|module| module.type_store)
    else {
        return;
    };
    for vtable in reachable_traits.vtables.clone() {
        insert_trait_and_supertrait_methods(
            program_signatures,
            type_store,
            reachable_traits,
            TraitMethodExpansionInput {
                module_id: vtable.module_id,
                trait_id: vtable.trait_id,
                self_ty: vtable.self_ty,
                trait_args: &vtable.trait_args,
                trait_const_args: &vtable.trait_const_args,
            },
        );
    }
    let reachable_modules = &reachability.modules;
    let mut pending_module_set = HashSet::new();
    let mut deferred_modules = DeferredModuleActivation {
        reachable_functions: &mut reachability.functions,
        reachable_modules,
        pending_module_set: &mut pending_module_set,
        pending_modules,
    };
    for trait_id in &reachable_traits.traits {
        let TraitId::Source(trait_def) = trait_id else {
            continue;
        };
        if !deferred_modules.is_reachable_module(trait_def.module_id) {
            continue;
        }
        let Some(trait_signature) = (program_signatures.trait_)(*trait_def) else {
            continue;
        };
        for method in &trait_signature.signature.methods {
            if method.has_default && reachable_traits.needs_method(*trait_id, &method.name) {
                deferred_modules.add_function(
                    GlobalDefId {
                        module_id: trait_def.module_id,
                        def_id: method.def_id,
                    },
                    program_signatures,
                );
            }
        }
    }
    for vtable in &reachable_traits.vtables {
        extension_index.for_each_method_for_trait(vtable.trait_id, &mut |method| {
            if !with_reachable_extension_method_match(
                ReachableExtensionMatchInput {
                    method,
                    trait_id: vtable.trait_id,
                    self_ty: vtable.self_ty,
                    trait_args: &vtable.trait_args,
                    trait_const_args: &vtable.trait_const_args,
                    use_module_id: vtable.module_id,
                    type_store,
                    extension_index,
                    modules_by_id,
                },
                &mut |_| {
                    deferred_modules.add_function(method.def_id, program_signatures);
                },
            ) {}
        });
    }
    let mut method_index = 0;
    while method_index < reachable_traits.methods.len() {
        let mut discovered_traits = ReachableTraitRefs::default();
        {
            let reachable = &reachable_traits.methods[method_index];
            extension_index.for_each_method_for_trait_method(
                reachable.trait_id,
                &reachable.method_name,
                &mut |method| {
                    if !with_reachable_extension_method_match(
                        ReachableExtensionMatchInput {
                            method,
                            trait_id: reachable.trait_id,
                            self_ty: reachable.self_ty,
                            trait_args: &reachable.trait_args,
                            trait_const_args: &reachable.trait_const_args,
                            use_module_id: reachable.module_id,
                            type_store,
                            extension_index,
                            modules_by_id,
                        },
                        &mut |matched| {
                            deferred_modules.add_function(method.def_id, program_signatures);
                            extend_reachable_trait_methods_from_impl_where_predicates(
                                program_signatures,
                                type_store,
                                &matched,
                                &reachable.method_name,
                                reachable.module_id,
                                &mut discovered_traits,
                            );
                        },
                    ) {}
                },
            );
        }
        method_index += 1;
        reachable_traits.extend(discovered_traits);
    }
}

pub(super) fn extend_reachable_functions_from_traits_incremental(
    state: &mut IncrementalExecutableReachability,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_index: &dyn ExecutableExtensionLookup,
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    if state.trait_function_scan.methods == state.reachable_traits.methods.len()
        && state.trait_function_scan.vtables == state.reachable_traits.vtables.len()
    {
        return;
    }
    let Some(type_store) = modules_by_id
        .values()
        .next()
        .map(|module| module.type_store)
    else {
        return;
    };

    let mut pending_module_set = HashSet::new();
    let mut deferred_modules = DeferredModuleActivation {
        reachable_functions: &mut state.reachability.functions,
        reachable_modules: &state.reachability.modules,
        pending_module_set: &mut pending_module_set,
        pending_modules,
    };
    let mut vtable_index = state
        .trait_function_scan
        .vtables
        .min(state.reachable_traits.vtables.len());
    while vtable_index < state.reachable_traits.vtables.len() {
        let vtable = state.reachable_traits.vtables[vtable_index].clone();
        insert_trait_and_supertrait_methods(
            program_signatures,
            type_store,
            &mut state.reachable_traits,
            TraitMethodExpansionInput {
                module_id: vtable.module_id,
                trait_id: vtable.trait_id,
                self_ty: vtable.self_ty,
                trait_args: &vtable.trait_args,
                trait_const_args: &vtable.trait_const_args,
            },
        );
        add_reachable_default_trait_methods_for_vtable(
            program_signatures,
            &vtable,
            &mut deferred_modules,
        );
        extension_index.for_each_method_for_trait(vtable.trait_id, &mut |method| {
            if !with_reachable_extension_method_match(
                ReachableExtensionMatchInput {
                    method,
                    trait_id: vtable.trait_id,
                    self_ty: vtable.self_ty,
                    trait_args: &vtable.trait_args,
                    trait_const_args: &vtable.trait_const_args,
                    use_module_id: vtable.module_id,
                    type_store,
                    extension_index,
                    modules_by_id,
                },
                &mut |_| {
                    deferred_modules.add_function(method.def_id, program_signatures);
                },
            ) {}
        });
        vtable_index += 1;
    }

    let mut method_index = state
        .trait_function_scan
        .methods
        .min(state.reachable_traits.methods.len());
    while method_index < state.reachable_traits.methods.len() {
        let mut discovered_traits = ReachableTraitRefs::default();
        {
            let reachable = &state.reachable_traits.methods[method_index];
            add_reachable_default_trait_method_for_method(
                program_signatures,
                reachable,
                &mut deferred_modules,
            );
            extension_index.for_each_method_for_trait_method(
                reachable.trait_id,
                &reachable.method_name,
                &mut |method| {
                    if !with_reachable_extension_method_match(
                        ReachableExtensionMatchInput {
                            method,
                            trait_id: reachable.trait_id,
                            self_ty: reachable.self_ty,
                            trait_args: &reachable.trait_args,
                            trait_const_args: &reachable.trait_const_args,
                            use_module_id: reachable.module_id,
                            type_store,
                            extension_index,
                            modules_by_id,
                        },
                        &mut |matched| {
                            deferred_modules.add_function(method.def_id, program_signatures);
                            extend_reachable_trait_methods_from_impl_where_predicates(
                                program_signatures,
                                type_store,
                                &matched,
                                &reachable.method_name,
                                reachable.module_id,
                                &mut discovered_traits,
                            );
                        },
                    ) {}
                },
            );
        }
        method_index += 1;
        state.reachable_traits.extend(discovered_traits);
    }

    state.trait_function_scan.vtables = vtable_index;
    state.trait_function_scan.methods = method_index;
}

fn add_reachable_default_trait_method_for_method(
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable: &ReachableTraitMethod,
    deferred_modules: &mut DeferredModuleActivation<'_>,
) {
    let TraitId::Source(trait_def) = reachable.trait_id else {
        return;
    };
    let Some(trait_signature) = (program_signatures.trait_)(trait_def) else {
        return;
    };
    for method in &trait_signature.signature.methods {
        if method.has_default && method.name == reachable.method_name {
            deferred_modules.add_function(
                GlobalDefId {
                    module_id: trait_def.module_id,
                    def_id: method.def_id,
                },
                program_signatures,
            );
        }
    }
}

fn add_reachable_default_trait_methods_for_vtable(
    program_signatures: ExecutableSignatureIndex<'_>,
    vtable: &ReachableTraitVtable,
    deferred_modules: &mut DeferredModuleActivation<'_>,
) {
    let TraitId::Source(trait_def) = vtable.trait_id else {
        return;
    };
    let Some(trait_signature) = (program_signatures.trait_)(trait_def) else {
        return;
    };
    for method in &trait_signature.signature.methods {
        if method.has_default {
            deferred_modules.add_function(
                GlobalDefId {
                    module_id: trait_def.module_id,
                    def_id: method.def_id,
                },
                program_signatures,
            );
        }
    }
}

pub(super) fn add_reachable_function(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachability: &mut ExecutableReachability,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    if !reachable_function_has_runtime_body(def_id, program_signatures) {
        reachability.insert_module_pending(def_id.module_id, pending_modules);
        return;
    }
    reachability.insert_function_pending(def_id, pending_modules);
}

fn reachable_function_has_runtime_body(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'_>,
) -> bool {
    (program_signatures.function)(def_id)
        .map(|signature| signature.signature.has_body)
        .or_else(|| {
            (program_signatures.trait_default_method)(def_id).map(|(_, trait_signature)| {
                trait_signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| method.def_id == def_id.def_id && method.has_default)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{DefId, GlobalDefId, ModuleIdAllocator};
    use nia_symbol::stable_hash;
    use nia_ty::{ConstGenericArg, ConstGenericValue, PrimitiveTy, TypeStore};

    #[test]
    fn trait_const_arguments_are_part_of_reachability_identity() {
        let module_id = ModuleIdAllocator::new().allocate();
        let store = TypeStore::new();
        let bool_ty = store
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Bool);
        let trait_id = TraitId::Source(GlobalDefId {
            module_id,
            def_id: DefId(1),
        });
        let method_name = SymbolId::from_stable_hash(stable_hash("value"));
        let const_arg = |value| ConstGenericArg {
            ty: bool_ty,
            value: ConstGenericValue::Bool(value),
        };

        let mut refs = ReachableTraitRefs::default();
        refs.insert_method_with_const_args(
            module_id,
            trait_id,
            method_name,
            bool_ty,
            Vec::new(),
            vec![const_arg(true)],
        );
        refs.insert_method_with_const_args(
            module_id,
            trait_id,
            method_name,
            bool_ty,
            Vec::new(),
            vec![const_arg(false)],
        );

        assert_eq!(refs.methods.len(), 2);
    }
}
