// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use nia_body_ir::{
    BodyIr, PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee,
    TypedExpr, TypedExprKind, TypedInlineAsm, TypedMemoryIntrinsicSource, TypedPattern,
    TypedPatternKind, TypedPlace, TypedStmt, TypedStmtKind, TypedSwitchArmBody,
};
use nia_defs::ExtensionMethods;
use nia_ids::{GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_imports::ModuleGraph;
use nia_item_signatures::{
    ProgramFunctionSignature, ProgramStructSignature, ProgramTraitImplSignature,
    ProgramTraitSignature, ProgramUnionSignature,
};
use nia_sema_ir::{FunctionSemanticFacts, SemanticFacts};
use nia_static_ir::StaticInit;
use nia_ty::{AssociatedTypeBindingTy, TyInterner, TyKind};

#[derive(Debug, Clone, Copy)]
pub struct ReachableModuleInput<'a> {
    pub module_id: ModuleId,
    pub body_ir: &'a BodyIr,
    pub semantic_facts: &'a SemanticFacts,
    pub type_lowering: &'a nia_type_lower::TypeLowering,
    pub type_normalization: &'a nia_type_normalize::TypeNormalization,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableReachability {
    pub modules: HashSet<ModuleId>,
    pub type_modules: HashSet<ModuleId>,
    pub functions: HashSet<GlobalDefId>,
    pub globals: HashSet<GlobalDefId>,
    pub stats: ExecutableReachabilityStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutableReachabilityStats {
    pub checked_modules: usize,
    pub checked_bodies: usize,
    pub reachable_bodies: usize,
}

#[derive(Clone, Copy)]
pub struct ExecutableRootDefs<'a> {
    pub named_function: &'a dyn Fn(ModuleId, &str) -> Option<GlobalDefId>,
    pub module_functions: &'a dyn Fn(ModuleId) -> Vec<GlobalDefId>,
}

impl std::fmt::Debug for ExecutableRootDefs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutableRootDefs")
            .field("named_function", &true)
            .field("module_functions", &true)
            .finish()
    }
}

pub fn compute_executable_reachability(
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    compute_executable_reachability_with_seed(
        None,
        parse_ok,
        graph,
        root_defs,
        program_signatures,
        extension_methods,
        trait_impls,
        modules,
    )
}

pub fn compute_executable_reachability_with_seed(
    seed: Option<&ExecutableReachability>,
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    let modules_by_id = modules
        .iter()
        .map(|module| (module.module_id, *module))
        .collect::<HashMap<_, _>>();
    let mut reachable_functions = HashSet::new();
    let mut reachable_globals = seed.map(|seed| seed.globals.clone()).unwrap_or_default();
    let mut reachable_modules = seed.map(|seed| seed.modules.clone()).unwrap_or_default();
    let mut reachable_type_modules = seed
        .map(|seed| seed.type_modules.clone())
        .unwrap_or_default();
    let mut pending_seed_modules = VecDeque::new();
    for def_id in seed
        .into_iter()
        .flat_map(|seed| seed.functions.iter().copied())
        .chain(executable_root_functions(graph, root_defs))
    {
        add_reachable_function(
            def_id,
            program_signatures,
            &mut reachable_functions,
            &mut reachable_modules,
            &mut pending_seed_modules,
        );
    }
    reachable_modules.extend(reachable_functions.iter().map(|def_id| def_id.module_id));
    add_reachable_module(graph.entry(), &mut reachable_modules, &mut VecDeque::new());

    let parse_ok_set = parse_ok.iter().copied().collect::<HashSet<_>>();
    loop {
        let before = (
            reachable_functions.len(),
            reachable_globals.len(),
            reachable_modules.len(),
            reachable_type_modules.len(),
        );
        let current_reachable_modules = reachable_modules.clone();
        let mut reachable_traits = collect_reachable_traits_for_modules(
            &modules_by_id,
            &current_reachable_modules,
            &reachable_functions,
            &reachable_globals,
        );
        extend_reachable_traits_from_generic_instances(
            &modules_by_id,
            &current_reachable_modules,
            program_signatures,
            extension_methods,
            &reachable_functions,
            &mut reachable_traits,
        );
        for module in modules_by_id
            .values()
            .filter(|module| current_reachable_modules.contains(&module.module_id))
        {
            let mut pending_modules = VecDeque::new();
            extend_reachable_functions_from_bodies(
                module,
                program_signatures,
                &mut reachable_functions,
                &mut reachable_globals,
                &mut reachable_modules,
                &mut pending_modules,
            );
            collect_reachable_fact_owner_modules(
                module,
                program_signatures,
                &reachable_functions,
                &reachable_globals,
                &mut reachable_modules,
                &mut reachable_type_modules,
                &mut pending_modules,
                &mut reachable_traits,
            );
        }
        let mut pending_modules = VecDeque::new();
        extend_reachable_functions_from_traits(
            program_signatures,
            extension_methods,
            trait_impls,
            &modules_by_id,
            &mut reachable_traits,
            &reachable_modules,
            &mut reachable_functions,
            &mut pending_modules,
        );
        while let Some(module_id) = pending_modules.pop_front() {
            if !parse_ok_set.contains(&module_id) {
                continue;
            }
            reachable_modules.insert(module_id);
        }
        if before
            == (
                reachable_functions.len(),
                reachable_globals.len(),
                reachable_modules.len(),
                reachable_type_modules.len(),
            )
        {
            break;
        }
    }

    let stats = ExecutableReachabilityStats {
        checked_modules: modules_by_id.len(),
        checked_bodies: modules_by_id
            .values()
            .map(|module| module.body_ir.function_bodies.len())
            .sum(),
        reachable_bodies: modules_by_id
            .values()
            .map(|module| {
                module
                    .body_ir
                    .function_bodies
                    .keys()
                    .filter(|def_id| reachable_functions.contains(def_id))
                    .count()
            })
            .sum(),
    };

    ExecutableReachability {
        modules: reachable_modules,
        type_modules: reachable_type_modules,
        functions: reachable_functions,
        globals: reachable_globals,
        stats,
    }
}

pub fn filter_semantic_facts_for_reachable_functions(
    facts: SemanticFacts,
    reachable_functions: &HashSet<GlobalDefId>,
) -> SemanticFacts {
    let reachable_globals = facts.global_types.keys().copied().collect::<HashSet<_>>();
    filter_semantic_facts_for_reachable_items(facts, reachable_functions, &reachable_globals)
}

pub fn filter_semantic_facts_for_reachable_items(
    facts: SemanticFacts,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> SemanticFacts {
    let mut reachable_facts = SemanticFacts {
        global_types: facts
            .global_types
            .into_iter()
            .filter(|(def_id, _)| reachable_globals.contains(def_id))
            .collect(),
        ..Default::default()
    };
    for def_id in reachable_functions {
        let Some(function_facts) = facts.function_facts.get(def_id) else {
            continue;
        };
        reachable_facts
            .generic_instantiations
            .extend(function_facts.generic_instantiations.clone());
        reachable_facts
            .node_expr_types
            .extend(function_facts.node_expr_types.clone());
        reachable_facts
            .node_bracket_suffix_resolutions
            .extend(function_facts.node_bracket_suffix_resolutions.clone());
        reachable_facts
            .node_array_to_slice_coercions
            .extend(function_facts.node_array_to_slice_coercions.clone());
        reachable_facts
            .node_pointer_array_to_slice_coercions
            .extend(function_facts.node_pointer_array_to_slice_coercions.clone());
        reachable_facts
            .node_trait_object_coercions
            .extend(function_facts.node_trait_object_coercions.clone());
        reachable_facts
            .node_trait_object_upcasts
            .extend(function_facts.node_trait_object_upcasts.clone());
        reachable_facts
            .node_builtin_values
            .extend(function_facts.node_builtin_values.clone());
        reachable_facts
            .node_array_repeat_counts
            .extend(function_facts.node_array_repeat_counts.clone());
        reachable_facts
            .node_switch_pattern_values
            .extend(function_facts.node_switch_pattern_values.clone());
        reachable_facts
            .node_resolved_calls
            .extend(function_facts.node_resolved_calls.clone());
        reachable_facts
            .node_function_references
            .extend(function_facts.node_function_references.clone());
    }
    reachable_facts.generic_instantiations.extend(
        facts
            .generic_instantiations
            .into_iter()
            .filter(|instantiation| instantiation.source_def_id.is_none()),
    );
    reachable_facts.node_builtin_associated_values = facts.node_builtin_associated_values;
    reachable_facts.function_facts = facts
        .function_facts
        .into_iter()
        .filter(|(def_id, _)| reachable_functions.contains(def_id))
        .collect();
    reachable_facts
}

pub fn extend_executable_reachability_from_checked_module(
    reachability: &mut ExecutableReachability,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    module: ReachableModuleInput<'_>,
    checked_modules: &[ReachableModuleInput<'_>],
) -> bool {
    let before = (
        reachability.functions.len(),
        reachability.globals.len(),
        reachability.modules.len(),
        reachability.type_modules.len(),
    );
    let mut pending_modules = VecDeque::new();
    extend_reachable_functions_from_bodies(
        &module,
        program_signatures,
        &mut reachability.functions,
        &mut reachability.globals,
        &mut reachability.modules,
        &mut pending_modules,
    );
    let mut reachable_traits = ReachableTraitRefs::default();
    collect_reachable_body_trait_ids(
        &module,
        &reachability.functions,
        &reachability.globals,
        &mut reachable_traits,
    );
    let mut modules_by_id = checked_modules
        .iter()
        .map(|checked_module| (checked_module.module_id, *checked_module))
        .collect::<HashMap<_, _>>();
    modules_by_id.insert(module.module_id, module);
    let current_reachable_modules = modules_by_id
        .keys()
        .copied()
        .filter(|module_id| reachability.modules.contains(module_id))
        .collect::<HashSet<_>>();
    extend_reachable_traits_from_generic_instances(
        &modules_by_id,
        &current_reachable_modules,
        program_signatures,
        extension_methods,
        &reachability.functions,
        &mut reachable_traits,
    );
    collect_reachable_fact_owner_modules(
        &module,
        program_signatures,
        &reachability.functions,
        &reachability.globals,
        &mut reachability.modules,
        &mut reachability.type_modules,
        &mut pending_modules,
        &mut reachable_traits,
    );
    extend_reachable_functions_from_traits(
        program_signatures,
        extension_methods,
        trait_impls,
        &modules_by_id,
        &mut reachable_traits,
        &reachability.modules,
        &mut reachability.functions,
        &mut pending_modules,
    );
    before
        != (
            reachability.functions.len(),
            reachability.globals.len(),
            reachability.modules.len(),
            reachability.type_modules.len(),
        )
}

#[derive(Clone, Copy)]
pub struct ExecutableSignatureIndex<'a> {
    pub function: &'a dyn Fn(GlobalDefId) -> Option<ProgramFunctionSignature>,
    pub struct_: &'a dyn Fn(GlobalDefId) -> Option<ProgramStructSignature>,
    pub union: &'a dyn Fn(GlobalDefId) -> Option<ProgramUnionSignature>,
    pub trait_: &'a dyn Fn(GlobalDefId) -> Option<ProgramTraitSignature>,
    pub trait_default_method:
        &'a dyn Fn(GlobalDefId) -> Option<(GlobalDefId, ProgramTraitSignature)>,
}

impl std::fmt::Debug for ExecutableSignatureIndex<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutableSignatureIndex")
            .field("function", &true)
            .field("struct_", &true)
            .field("union", &true)
            .field("trait_", &true)
            .finish()
    }
}

fn executable_root_functions(
    graph: &ModuleGraph,
    root_defs: ExecutableRootDefs<'_>,
) -> HashSet<GlobalDefId> {
    let mut roots = HashSet::new();
    if let Some(main) = (root_defs.named_function)(graph.entry(), "main") {
        roots.insert(main);
    }
    if let Some(start_module) = freestanding_start_module(graph)
        && let Some(start) = (root_defs.named_function)(start_module, "_start")
    {
        roots.insert(start);
        roots.extend((root_defs.module_functions)(start_module));
    }
    roots
}

fn extend_reachable_functions_from_bodies(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_globals: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let refs = typed_body_refs(module, reachable_functions, reachable_globals);
    for def_id in refs.functions {
        add_reachable_function(
            def_id,
            program_signatures,
            reachable_functions,
            reachable_modules,
            pending_modules,
        );
    }
    for def_id in refs.globals {
        if reachable_globals.insert(def_id) {
            add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        }
    }
}

fn collect_reachable_traits_for_modules(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> ReachableTraitRefs {
    let mut reachable_traits = ReachableTraitRefs::default();
    for module in modules_by_id
        .values()
        .filter(|module| current_reachable_modules.contains(&module.module_id))
    {
        collect_reachable_body_trait_ids(
            module,
            reachable_functions,
            reachable_globals,
            &mut reachable_traits,
        );
    }
    reachable_traits
}

fn extend_reachable_traits_from_generic_instances(
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    current_reachable_modules: &HashSet<ModuleId>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    reachable_functions: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    let needed_methods = traits
        .methods
        .iter()
        .map(|method| (method.trait_id, method.method_name.clone()))
        .collect::<HashSet<_>>();
    for module in modules_by_id
        .values()
        .filter(|module| current_reachable_modules.contains(&module.module_id))
    {
        for def_id in reachable_functions
            .iter()
            .filter(|def_id| def_id.module_id == module.module_id)
        {
            let Some(function_facts) = module.semantic_facts.function_facts.get(def_id) else {
                continue;
            };
            for instantiation in &function_facts.generic_instantiations {
                extend_reachable_traits_from_generic_instantiation(
                    module,
                    program_signatures,
                    extension_methods,
                    &needed_methods,
                    traits,
                    instantiation,
                );
            }
        }
    }
}

fn extend_reachable_traits_from_generic_instantiation(
    use_module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    needed_methods: &HashSet<(TraitId, String)>,
    traits: &mut ReachableTraitRefs,
    instantiation: &nia_sema_ir::GenericInstantiation,
) {
    extend_reachable_traits_from_trait_default_instantiation(
        use_module,
        program_signatures,
        needed_methods,
        traits,
        instantiation,
    );
    let Some(signature) = (program_signatures.function)(instantiation.def_id) else {
        return;
    };
    let mut signature_interner = signature.interner.clone();
    let generics = if instantiation.generics.is_empty() && !instantiation.args.is_empty() {
        &signature.signature.generics
    } else {
        &instantiation.generics
    };
    let substitutions = generics
        .iter()
        .cloned()
        .zip(instantiation.args.iter().copied())
        .filter_map(|(generic, arg)| {
            nia_ty::try_import_type_into(&mut signature_interner, &use_module.body_ir.interner, arg)
                .ok()
                .map(|arg| (generic, arg))
        })
        .collect::<HashMap<_, _>>();
    let extension_where_predicates = extension_methods
        .all_methods()
        .find(|method| method.def_id == instantiation.def_id)
        .map(|method| method.where_predicates.as_slice())
        .unwrap_or(&[]);
    for predicate in signature
        .signature
        .where_predicates
        .iter()
        .chain(extension_where_predicates)
    {
        let mut substituted_interner = signature_interner.clone();
        let Some(self_ty) = substitute_ty(&mut substituted_interner, predicate.ty, &substitutions)
        else {
            continue;
        };
        for bound in &predicate.bounds {
            let Some(trait_ty) =
                substitute_ty(&mut substituted_interner, bound.trait_ty, &substitutions)
            else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(&substituted_interner, trait_ty)
            else {
                continue;
            };
            if let TraitId::Source(trait_def) = trait_id
                && let Some(trait_signature) = (program_signatures.trait_)(trait_def)
            {
                for method in &trait_signature.signature.methods {
                    traits.insert_method_with_interner(
                        use_module.module_id,
                        trait_id,
                        method.name.clone(),
                        self_ty,
                        trait_args.clone(),
                        Some(substituted_interner.clone()),
                    );
                }
            }
            for (_, method_name) in needed_methods
                .iter()
                .filter(|(needed_trait_id, _)| *needed_trait_id == trait_id)
            {
                traits.insert_method_with_interner(
                    use_module.module_id,
                    trait_id,
                    method_name.clone(),
                    self_ty,
                    trait_args.clone(),
                    Some(substituted_interner.clone()),
                );
            }
        }
    }
}

fn extend_reachable_traits_from_trait_default_instantiation(
    use_module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    needed_methods: &HashSet<(TraitId, String)>,
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
    let needed_names = needed_methods
        .iter()
        .filter_map(|(needed_trait_id, method_name)| {
            (*needed_trait_id == trait_id).then_some(method_name)
        })
        .collect::<Vec<_>>();
    if needed_names.is_empty() {
        return;
    }
    let Some(self_ty) = instantiation.args.first().copied() else {
        return;
    };
    let mut method_interner = trait_signature.interner.clone();
    let Ok(self_ty) =
        nia_ty::try_import_type_into(&mut method_interner, &use_module.body_ir.interner, self_ty)
    else {
        return;
    };
    let trait_args = instantiation
        .args
        .iter()
        .skip(1)
        .take(trait_signature.signature.generics.len())
        .map(|arg| {
            nia_ty::try_import_type_into(&mut method_interner, &use_module.body_ir.interner, *arg)
        })
        .collect::<Result<Vec<_>, _>>();
    let Ok(trait_args) = trait_args else {
        return;
    };
    for method_name in needed_names {
        traits.insert_method_with_interner(
            use_module.module_id,
            trait_id,
            method_name.clone(),
            self_ty,
            trait_args.clone(),
            Some(method_interner.clone()),
        );
    }
}

fn typed_body_refs(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> TypedBodyRefs {
    let mut refs = TypedBodyRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        if reachable_functions.contains(def_id) {
            collect_typed_body_refs(module, body, &mut refs);
        }
    }
    for (def_id, init) in &module.body_ir.global_inits {
        if reachable_globals.contains(def_id) {
            collect_static_init_refs(init, &mut refs);
        }
    }
    refs
}

fn collect_reachable_body_trait_ids(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut refs = TypedBodyRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        if reachable_functions.contains(def_id) {
            collect_typed_body_refs(module, body, &mut refs);
        }
    }
    for (def_id, init) in &module.body_ir.global_inits {
        if reachable_globals.contains(def_id) {
            collect_static_init_refs(init, &mut refs);
        }
    }
    traits.extend(refs.traits);
}

#[derive(Default)]
struct TypedBodyRefs {
    functions: HashSet<GlobalDefId>,
    globals: HashSet<GlobalDefId>,
    traits: ReachableTraitRefs,
}

#[derive(Debug, Clone, Default)]
struct ReachableTraitRefs {
    traits: HashSet<TraitId>,
    methods: Vec<ReachableTraitMethod>,
    method_keys: HashSet<ReachableTraitMethodKey>,
    vtables: Vec<ReachableTraitVtable>,
}

#[derive(Debug, Clone)]
struct ReachableTraitMethod {
    module_id: ModuleId,
    trait_id: TraitId,
    method_name: String,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
    interner: Option<TyInterner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitMethodKey {
    trait_id: TraitId,
    method_name: String,
    self_ty: TyKind,
    trait_args: Vec<TyKind>,
}

#[derive(Debug, Clone)]
struct ReachableTraitVtable {
    module_id: ModuleId,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: Vec<InternedTyId>,
}

impl ReachableTraitRefs {
    fn extend(&mut self, refs: Self) {
        self.traits.extend(refs.traits);
        for method in refs.methods {
            self.insert_method_with_interner(
                method.module_id,
                method.trait_id,
                method.method_name,
                method.self_ty,
                method.trait_args,
                method.interner,
            );
        }
        self.vtables.extend(refs.vtables);
    }

    fn insert_trait(&mut self, trait_id: TraitId) {
        self.traits.insert(trait_id);
    }

    fn insert_method(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        method_name: impl Into<String>,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    ) {
        self.insert_method_with_interner(
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            None,
        );
    }

    fn insert_method_with_interner(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        method_name: impl Into<String>,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        interner: Option<TyInterner>,
    ) {
        self.traits.insert(trait_id);
        let key_interner = interner.as_ref();
        let key_interner = match key_interner {
            Some(interner) => interner,
            None => {
                return self.methods.push(ReachableTraitMethod {
                    module_id,
                    trait_id,
                    method_name: method_name.into(),
                    self_ty,
                    trait_args,
                    interner,
                });
            }
        };
        let Some(self_ty_key) = key_interner.get(self_ty).cloned() else {
            return;
        };
        let Some(trait_arg_keys) = trait_args
            .iter()
            .map(|arg| key_interner.get(*arg).cloned())
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let method_name = method_name.into();
        if !self.method_keys.insert(ReachableTraitMethodKey {
            trait_id,
            method_name: method_name.clone(),
            self_ty: self_ty_key,
            trait_args: trait_arg_keys,
        }) {
            return;
        }
        self.methods.push(ReachableTraitMethod {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            interner,
        });
    }

    fn insert_vtable(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    ) {
        self.traits.insert(trait_id);
        self.vtables.push(ReachableTraitVtable {
            module_id,
            trait_id,
            self_ty,
            trait_args,
        });
    }

    fn needs_method(&self, trait_id: TraitId, method_name: &str) -> bool {
        self.methods
            .iter()
            .any(|method| method.trait_id == trait_id && method.method_name == method_name)
            || self
                .vtables
                .iter()
                .any(|vtable| vtable.trait_id == trait_id)
    }
}

fn collect_typed_body_refs(
    module: &ReachableModuleInput<'_>,
    body: &TypedBody,
    refs: &mut TypedBodyRefs,
) {
    for stmt in &body.stmts {
        collect_typed_stmt_refs(module, stmt, refs);
    }
    if let Some(tail) = body.tail.as_deref() {
        collect_typed_expr_refs(module, tail, refs);
    }
}

fn collect_typed_stmt_refs(
    module: &ReachableModuleInput<'_>,
    stmt: &TypedStmt,
    refs: &mut TypedBodyRefs,
) {
    match &stmt.kind {
        TypedStmtKind::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_typed_expr_refs(module, value, refs);
            }
        }
        TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => {
            collect_typed_expr_refs(module, expr, refs);
        }
        TypedStmtKind::Return(value) => {
            if let Some(value) = value {
                collect_typed_expr_refs(module, value, refs);
            }
        }
        TypedStmtKind::ForIn(for_in) => {
            refs.traits.insert_method(
                module.module_id,
                TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
                nia_ids::BuiltinTraitMethod::IteratorNext.name(),
                for_in.iter.ty,
                Vec::new(),
            );
            collect_typed_expr_refs(module, &for_in.iter, refs);
            collect_typed_body_refs(module, &for_in.body, refs);
        }
        TypedStmtKind::While(while_loop) => {
            collect_typed_expr_refs(module, &while_loop.cond, refs);
            collect_typed_body_refs(module, &while_loop.body, refs);
        }
        TypedStmtKind::Loop(loop_body) => collect_typed_body_refs(module, &loop_body.body, refs),
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn collect_typed_expr_refs(
    module: &ReachableModuleInput<'_>,
    expr: &TypedExpr,
    refs: &mut TypedBodyRefs,
) {
    match &expr.kind {
        TypedExprKind::Function(def_id) | TypedExprKind::FunctionInstance { def_id, .. } => {
            refs.functions.insert(*def_id);
        }
        TypedExprKind::Field { lhs, .. } => {
            collect_typed_expr_refs(module, lhs, refs);
        }
        TypedExprKind::Range(range) => {
            if let Some(start) = range.start.as_deref() {
                collect_typed_expr_refs(module, start, refs);
            }
            if let Some(end) = range.end.as_deref() {
                collect_typed_expr_refs(module, end, refs);
            }
        }
        TypedExprKind::InlineAsm(asm) => collect_typed_inline_asm_refs(module, asm, refs),
        TypedExprKind::MemoryIntrinsic(memory) => {
            collect_typed_expr_refs(module, &memory.dest, refs);
            match &memory.source {
                TypedMemoryIntrinsicSource::Slice(source)
                | TypedMemoryIntrinsicSource::Byte(source) => {
                    collect_typed_expr_refs(module, source, refs)
                }
            }
        }
        TypedExprKind::Atomic(atomic) => collect_typed_atomic_refs(module, atomic, refs),
        TypedExprKind::LoadUnaligned { ptr, .. } => collect_typed_expr_refs(module, ptr, refs),
        TypedExprKind::Splat { value } | TypedExprKind::Bitmask { vector: value } => {
            collect_typed_expr_refs(module, value, refs);
        }
        TypedExprKind::ExtractElement { vector, index } => {
            collect_typed_expr_refs(module, vector, refs);
            collect_typed_expr_refs(module, index, refs);
        }
        TypedExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            collect_typed_expr_refs(module, vector, refs);
            collect_typed_expr_refs(module, index, refs);
            collect_typed_expr_refs(module, value, refs);
        }
        TypedExprKind::BitIntrinsic { value, .. }
        | TypedExprKind::CharFromU32 { value }
        | TypedExprKind::StaticArrayPointer { array: value, .. }
        | TypedExprKind::Unary { expr: value, .. }
        | TypedExprKind::OptionalSome { expr: value }
        | TypedExprKind::ErrorOk { expr: value }
        | TypedExprKind::ErrorErr { expr: value }
        | TypedExprKind::Try { expr: value }
        | TypedExprKind::Discard(value)
        | TypedExprKind::Cast { expr: value, .. }
        | TypedExprKind::TraitObjectUpcast { expr: value, .. } => {
            collect_typed_expr_refs(module, value, refs);
        }
        TypedExprKind::TraitObjectCoercion {
            expr: value,
            target_ty,
            self_ty,
        } => {
            collect_typed_expr_refs(module, value, refs);
            collect_trait_object_vtable_ref(module, *target_ty, *self_ty, refs);
        }
        TypedExprKind::ArrayLiteral { elems } => match elems {
            TypedArrayElements::List(elems) => {
                for elem in elems {
                    collect_typed_expr_refs(module, elem, refs);
                }
            }
            TypedArrayElements::Repeat { value, .. } => {
                collect_typed_expr_refs(module, value, refs)
            }
        },
        TypedExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_typed_expr_refs(module, &field.value, refs);
            }
        }
        TypedExprKind::UnionLiteral { field, .. } => {
            collect_typed_expr_refs(module, &field.value, refs);
        }
        TypedExprKind::Binary { lhs, rhs, .. } => {
            collect_typed_expr_refs(module, lhs, refs);
            collect_typed_expr_refs(module, rhs, refs);
        }
        TypedExprKind::Assign { place, rhs, .. } => {
            collect_typed_place_refs(module, place, refs);
            collect_typed_expr_refs(module, rhs, refs);
        }
        TypedExprKind::Call { callee, args } => {
            collect_typed_callee_refs(module, callee, args, refs);
            for arg in args {
                collect_typed_expr_refs(module, arg, refs);
            }
        }
        TypedExprKind::Index { lhs, index } => {
            collect_typed_expr_refs(module, lhs, refs);
            collect_typed_expr_refs(module, index, refs);
        }
        TypedExprKind::Slice { lhs, range, .. } => {
            collect_typed_expr_refs(module, lhs, refs);
            if let Some(start) = range.start.as_deref() {
                collect_typed_expr_refs(module, start, refs);
            }
            if let Some(end) = range.end.as_deref() {
                collect_typed_expr_refs(module, end, refs);
            }
        }
        TypedExprKind::Block(body) => collect_typed_body_refs(module, body, refs),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_typed_expr_refs(module, cond, refs);
            collect_typed_body_refs(module, then_branch, refs);
            if let Some(else_branch) = else_branch.as_deref() {
                collect_typed_expr_refs(module, else_branch, refs);
            }
        }
        TypedExprKind::Switch(switch) => {
            collect_typed_expr_refs(module, &switch.target, refs);
            for arm in &switch.arms {
                for pattern in &arm.patterns {
                    collect_typed_switch_pattern_refs(module, pattern, refs);
                }
                match &arm.body {
                    TypedSwitchArmBody::Expr(expr) => collect_typed_expr_refs(module, expr, refs),
                    TypedSwitchArmBody::Stmt(stmt) => collect_typed_stmt_refs(module, stmt, refs),
                    TypedSwitchArmBody::Block(body) => collect_typed_body_refs(module, body, refs),
                }
            }
        }
        TypedExprKind::IfPattern(if_pattern) => {
            collect_typed_expr_refs(module, &if_pattern.target, refs);
            for arm in &if_pattern.arms {
                collect_typed_pattern_refs(module, &arm.pattern, refs);
                collect_typed_body_refs(module, &arm.body, refs);
            }
            if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                collect_typed_expr_refs(module, else_branch, refs);
            }
        }
        TypedExprKind::Error
        | TypedExprKind::Integer(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::String(_)
        | TypedExprKind::ByteString(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::ByteChar(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Null
        | TypedExprKind::Local(_) => {}
        TypedExprKind::Global(def_id) => {
            refs.globals.insert(*def_id);
        }
        TypedExprKind::EnumVariant(_) | TypedExprKind::BuiltinValue(_) | TypedExprKind::Trap => {}
    }
}

fn collect_typed_callee_refs(
    module: &ReachableModuleInput<'_>,
    callee: &TypedCallee,
    args: &[TypedExpr],
    refs: &mut TypedBodyRefs,
) {
    match callee {
        TypedCallee::Function(def_id) | TypedCallee::FunctionInstance { def_id, .. } => {
            refs.functions.insert(*def_id);
        }
        TypedCallee::Method {
            def_id, receiver, ..
        } => {
            refs.functions.insert(*def_id);
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::TraitMethod {
            trait_id,
            method_id,
            method_name,
            self_ty,
            trait_args,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert_method(
                module.module_id,
                TraitId::Source(*trait_id),
                method_name.clone(),
                *self_ty,
                trait_args.clone(),
            );
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::TraitAssociatedFunction {
            trait_id,
            method_id,
            method_name,
            self_ty,
            trait_args,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert_method(
                module.module_id,
                TraitId::Source(*trait_id),
                method_name.clone(),
                *self_ty,
                trait_args.clone(),
            );
        }
        TypedCallee::DynamicTraitMethod {
            trait_id,
            method_id,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert_trait(*trait_id);
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::BuiltinMethod { receiver, .. } | TypedCallee::FunctionPointer(receiver) => {
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::BuiltinOperator(operator) => {
            if let Some(method) = operator.method() {
                if let Some(receiver) = args.first() {
                    refs.traits.insert_method(
                        module.module_id,
                        TraitId::Builtin(operator.trait_id),
                        method.name(),
                        receiver.ty,
                        Vec::new(),
                    );
                } else {
                    refs.traits
                        .insert_trait(TraitId::Builtin(operator.trait_id));
                }
            } else {
                refs.traits
                    .insert_trait(TraitId::Builtin(operator.trait_id));
            }
        }
        TypedCallee::BuiltinPlaceMethod(method) => {
            refs.traits.insert_method(
                module.module_id,
                TraitId::Builtin(method.trait_id),
                method.method.name(),
                method.self_ty,
                method.trait_args.clone(),
            );
            collect_typed_expr_refs(module, &method.receiver, refs);
        }
    }
}

fn collect_trait_object_vtable_ref(
    module: &ReachableModuleInput<'_>,
    object_ty: InternedTyId,
    self_ty: InternedTyId,
    refs: &mut TypedBodyRefs,
) {
    let Some(ty) = module.body_ir.interner.get(object_ty) else {
        return;
    };
    match ty {
        TyKind::TraitObject {
            trait_id,
            trait_args,
            ..
        }
        | TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            ..
        } => {
            refs.traits
                .insert_vtable(module.module_id, *trait_id, self_ty, trait_args.clone());
        }
        _ => {}
    }
}

fn collect_typed_pattern_refs(
    module: &ReachableModuleInput<'_>,
    pattern: &TypedPattern,
    refs: &mut TypedBodyRefs,
) {
    match &pattern.kind {
        TypedPatternKind::Pointer(pattern)
        | TypedPatternKind::MutPointer(pattern)
        | TypedPatternKind::OptionalSome(pattern)
        | TypedPatternKind::ErrorOk(pattern)
        | TypedPatternKind::ErrorErr(pattern) => collect_typed_pattern_refs(module, pattern, refs),
        TypedPatternKind::Expr(expr) => collect_typed_expr_refs(module, expr, refs),
        TypedPatternKind::Range { start, end, .. } => {
            collect_typed_expr_refs(module, start, refs);
            collect_typed_expr_refs(module, end, refs);
        }
        TypedPatternKind::Wildcard
        | TypedPatternKind::Bind { .. }
        | TypedPatternKind::OptionalNull => {}
    }
}

fn collect_typed_switch_pattern_refs(
    module: &ReachableModuleInput<'_>,
    pattern: &nia_body_ir::TypedSwitchPattern,
    refs: &mut TypedBodyRefs,
) {
    match &pattern.kind {
        nia_body_ir::TypedSwitchPatternKind::Expr(expr) => {
            collect_typed_expr_refs(module, expr, refs)
        }
        nia_body_ir::TypedSwitchPatternKind::Range { start, end, .. } => {
            collect_typed_expr_refs(module, start, refs);
            collect_typed_expr_refs(module, end, refs);
        }
        nia_body_ir::TypedSwitchPatternKind::Wildcard
        | nia_body_ir::TypedSwitchPatternKind::CheckedInt { .. }
        | nia_body_ir::TypedSwitchPatternKind::CheckedIntRange { .. } => {}
    }
}

fn collect_typed_atomic_refs(
    module: &ReachableModuleInput<'_>,
    atomic: &TypedAtomic,
    refs: &mut TypedBodyRefs,
) {
    match atomic {
        TypedAtomic::Load { ptr, .. } => collect_typed_expr_refs(module, ptr, refs),
        TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
            collect_typed_expr_refs(module, ptr, refs);
            collect_typed_expr_refs(module, value, refs);
        }
        TypedAtomic::Cmpxchg {
            ptr,
            expected,
            desired,
            ..
        } => {
            collect_typed_expr_refs(module, ptr, refs);
            collect_typed_expr_refs(module, expected, refs);
            collect_typed_expr_refs(module, desired, refs);
        }
        TypedAtomic::Fence { .. } => {}
    }
}

fn collect_typed_inline_asm_refs(
    module: &ReachableModuleInput<'_>,
    asm: &TypedInlineAsm,
    refs: &mut TypedBodyRefs,
) {
    for input in &asm.inputs {
        collect_typed_expr_refs(module, &input.value, refs);
    }
    for output in &asm.outputs {
        collect_typed_place_refs(module, &output.place, refs);
    }
}

fn collect_typed_place_refs(
    module: &ReachableModuleInput<'_>,
    place: &TypedPlace,
    refs: &mut TypedBodyRefs,
) {
    match &place.base {
        PlaceBase::Deref(expr) => collect_typed_expr_refs(module, expr, refs),
        PlaceBase::Global(def_id) => {
            refs.globals.insert(*def_id);
        }
        PlaceBase::Local(_) | PlaceBase::Error => {}
    }
    for elem in &place.elems {
        match elem {
            PlaceElem::Index(expr) => collect_typed_expr_refs(module, expr, refs),
            PlaceElem::Field(_) | PlaceElem::Error => {}
        }
    }
}

fn collect_static_init_refs(init: &StaticInit, refs: &mut TypedBodyRefs) {
    match init {
        StaticInit::Array(elems) => {
            for elem in elems {
                collect_static_init_refs(elem, refs);
            }
        }
        StaticInit::Repeat { value, count } => {
            if *count != 0 {
                collect_static_init_refs(value, refs);
            }
        }
        StaticInit::Struct(fields) => {
            for field in fields {
                collect_static_init_refs(&field.value, refs);
            }
        }
        StaticInit::AddrOfGlobal { global, .. } => {
            refs.globals.insert(*global);
        }
        StaticInit::AddrOfFunction { function, .. } => {
            refs.functions.insert(*function);
        }
        StaticInit::StaticArrayPointer { array_init, .. } => {
            collect_static_init_refs(array_init, refs);
        }
        StaticInit::Zero
        | StaticInit::Int(_)
        | StaticInit::Float(_)
        | StaticInit::Bool(_)
        | StaticInit::Char(_)
        | StaticInit::Byte(_)
        | StaticInit::Chars(_)
        | StaticInit::Bytes(_)
        | StaticInit::NullPtr => {}
    }
}

fn extend_reachable_functions_from_traits(
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    trait_impls: &[ProgramTraitImplSignature],
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    reachable_traits: &mut ReachableTraitRefs,
    reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let mut modules = reachable_modules.clone();
    for trait_id in &reachable_traits.traits {
        let TraitId::Source(trait_def) = trait_id else {
            continue;
        };
        if !reachable_modules.contains(&trait_def.module_id) {
            continue;
        }
        let Some(trait_signature) = (program_signatures.trait_)(*trait_def) else {
            continue;
        };
        for method in &trait_signature.signature.methods {
            if method.has_default && reachable_traits.needs_method(*trait_id, &method.name) {
                add_reachable_function(
                    GlobalDefId {
                        module_id: trait_def.module_id,
                        def_id: method.def_id,
                    },
                    program_signatures,
                    reachable_functions,
                    &mut modules,
                    pending_modules,
                );
            }
        }
    }
    for method in extension_methods.all_methods() {
        let Some(trait_id) = method.trait_id else {
            continue;
        };
        if !reachable_traits.needs_method(trait_id, &method.name) {
            continue;
        }
        let needs_body = reachable_extension_method_needs_body(
            method,
            trait_impls,
            modules_by_id,
            reachable_traits,
        );
        if !needs_body {
            continue;
        }
        add_reachable_function(
            method.def_id,
            program_signatures,
            reachable_functions,
            &mut modules,
            pending_modules,
        );
    }
    let mut method_index = 0;
    while method_index < reachable_traits.methods.len() {
        let reachable = reachable_traits.methods[method_index].clone();
        method_index += 1;
        for method in extension_methods.all_methods() {
            let Some(trait_id) = method.trait_id else {
                continue;
            };
            if trait_id != reachable.trait_id || method.name != reachable.method_name {
                continue;
            }
            let Some(matched) = reachable_extension_method_match(
                method,
                trait_id,
                reachable.self_ty,
                &reachable.trait_args,
                reachable.module_id,
                reachable.interner.as_ref(),
                trait_impls,
                modules_by_id,
            ) else {
                continue;
            };
            add_reachable_function(
                method.def_id,
                program_signatures,
                reachable_functions,
                &mut modules,
                pending_modules,
            );
            extend_reachable_trait_methods_from_impl_where_predicates(
                program_signatures,
                &matched,
                &reachable.method_name,
                reachable.module_id,
                reachable_traits,
            );
        }
    }
}

fn reachable_extension_method_needs_body(
    method: &nia_defs::ExtensionMethod,
    trait_impls: &[ProgramTraitImplSignature],
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
    reachable_traits: &ReachableTraitRefs,
) -> bool {
    let Some(method_trait_id) = method.trait_id else {
        return false;
    };
    if reachable_traits.vtables.iter().any(|vtable| {
        reachable_extension_method_match(
            method,
            method_trait_id,
            vtable.self_ty,
            &vtable.trait_args,
            vtable.module_id,
            None,
            trait_impls,
            modules_by_id,
        )
        .is_some()
    }) {
        return true;
    }
    reachable_traits.methods.iter().any(|reachable| {
        reachable.trait_id == method_trait_id
            && reachable.method_name == method.name
            && reachable_extension_method_match(
                method,
                method_trait_id,
                reachable.self_ty,
                &reachable.trait_args,
                reachable.module_id,
                reachable.interner.as_ref(),
                trait_impls,
                modules_by_id,
            )
            .is_some()
    })
}

#[derive(Debug)]
struct ReachableExtensionMethodMatch<'a> {
    impl_signature: &'a ProgramTraitImplSignature,
    interner: TyInterner,
    substitutions: HashMap<String, InternedTyId>,
}

fn reachable_extension_method_match<'a>(
    method: &nia_defs::ExtensionMethod,
    trait_id: TraitId,
    self_ty: InternedTyId,
    trait_args: &[InternedTyId],
    use_module_id: ModuleId,
    use_interner_override: Option<&TyInterner>,
    trait_impls: &'a [ProgramTraitImplSignature],
    modules_by_id: &HashMap<ModuleId, ReachableModuleInput<'_>>,
) -> Option<ReachableExtensionMethodMatch<'a>> {
    if method.trait_args.len() != trait_args.len() {
        return None;
    }
    let Some(impl_signature) = trait_impls.iter().find(|impl_signature| {
        impl_signature.module_id == method.def_id.module_id
            && impl_signature.impl_id == method.impl_id
            && impl_signature.trait_id == trait_id
    }) else {
        return None;
    };
    if impl_signature.trait_args.len() != trait_args.len() {
        return None;
    }
    let mut interner = if let Some(interner) = use_interner_override {
        interner.clone()
    } else if let Some(use_module) = modules_by_id.get(&use_module_id) {
        use_module.body_ir.interner.clone()
    } else {
        return None;
    };
    let Ok(target_ty) = nia_ty::try_import_type_into(
        &mut interner,
        &impl_signature.interner,
        impl_signature.target_ty,
    ) else {
        return None;
    };
    let Ok(imported_trait_args) = impl_signature
        .trait_args
        .iter()
        .map(|arg| nia_ty::try_import_type_into(&mut interner, &impl_signature.interner, *arg))
        .collect::<Result<Vec<_>, _>>()
    else {
        return None;
    };
    let mut substitutions = HashMap::new();
    if !match_type_pattern(&interner, target_ty, self_ty, &mut substitutions) {
        return None;
    }
    if !imported_trait_args
        .iter()
        .zip(trait_args)
        .all(|(pattern, actual)| {
            match_type_pattern(&interner, *pattern, *actual, &mut substitutions)
        })
    {
        return None;
    }
    Some(ReachableExtensionMethodMatch {
        impl_signature,
        interner,
        substitutions,
    })
}

fn extend_reachable_trait_methods_from_impl_where_predicates(
    program_signatures: ExecutableSignatureIndex<'_>,
    matched: &ReachableExtensionMethodMatch<'_>,
    fallback_method_name: &str,
    module_id: ModuleId,
    traits: &mut ReachableTraitRefs,
) {
    for predicate in &matched.impl_signature.where_predicates {
        let mut interner = matched.interner.clone();
        let Ok(predicate_ty) = nia_ty::try_import_type_into(
            &mut interner,
            &matched.impl_signature.interner,
            predicate.ty,
        ) else {
            continue;
        };
        let Some(self_ty) = substitute_ty(&mut interner, predicate_ty, &matched.substitutions)
        else {
            continue;
        };
        for bound in &predicate.bounds {
            let Ok(trait_ty) = nia_ty::try_import_type_into(
                &mut interner,
                &matched.impl_signature.interner,
                bound.trait_ty,
            ) else {
                continue;
            };
            let Some(trait_ty) = substitute_ty(&mut interner, trait_ty, &matched.substitutions)
            else {
                continue;
            };
            let Some((trait_id, trait_args)) = trait_id_and_args(&interner, trait_ty) else {
                continue;
            };
            if let TraitId::Source(trait_def) = trait_id
                && let Some(trait_signature) = (program_signatures.trait_)(trait_def)
            {
                for method in &trait_signature.signature.methods {
                    traits.insert_method_with_interner(
                        module_id,
                        trait_id,
                        method.name.clone(),
                        self_ty,
                        trait_args.clone(),
                        Some(interner.clone()),
                    );
                }
                continue;
            }
            traits.insert_method_with_interner(
                module_id,
                trait_id,
                fallback_method_name.to_string(),
                self_ty,
                trait_args,
                Some(interner.clone()),
            );
        }
    }
}

fn add_reachable_function(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let has_runtime_body = (program_signatures.function)(def_id)
        .map(|signature| !signature.signature.is_comptime && signature.signature.has_body)
        .or_else(|| {
            (program_signatures.trait_default_method)(def_id).map(|(_, trait_signature)| {
                trait_signature
                    .signature
                    .methods
                    .iter()
                    .any(|method| method.def_id == def_id.def_id && method.has_default)
            })
        });
    if !has_runtime_body.unwrap_or(false) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        return;
    }
    if reachable_functions.insert(def_id) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
    }
}

fn match_type_pattern(
    interner: &TyInterner,
    pattern: InternedTyId,
    actual: InternedTyId,
    substitutions: &mut HashMap<String, InternedTyId>,
) -> bool {
    let Some(pattern_ty) = interner.get(pattern) else {
        return false;
    };
    match pattern_ty {
        TyKind::GenericParam(name) => {
            if let Some(existing) = substitutions.get(name).copied() {
                types_equivalent(interner, existing, actual)
            } else {
                substitutions.insert(name.clone(), actual);
                true
            }
        }
        TyKind::Primitive(pattern_primitive) => {
            matches!(interner.get(actual), Some(TyKind::Primitive(actual_primitive)) if pattern_primitive == actual_primitive)
        }
        TyKind::Vector {
            elem: pattern_elem,
            lanes: pattern_lanes,
        } => {
            matches!(interner.get(actual), Some(TyKind::Vector { elem, lanes }) if elem == pattern_elem && lanes == pattern_lanes)
        }
        TyKind::Pointer { is_readonly, elem } => match interner.get(actual) {
            Some(TyKind::Pointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::VolatilePointer { is_readonly, elem } => match interner.get(actual) {
            Some(TyKind::VolatilePointer {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::Slice { is_readonly, elem } => match interner.get(actual) {
            Some(TyKind::Slice {
                is_readonly: actual_readonly,
                elem: actual_elem,
            }) if is_readonly == actual_readonly => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::SlicePointee { elem } => match interner.get(actual) {
            Some(TyKind::SlicePointee { elem: actual_elem }) => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::Array { len, elem } => match interner.get(actual) {
            Some(TyKind::Array {
                len: actual_len,
                elem: actual_elem,
            }) if len == actual_len => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::Range { kind, bound } => match interner.get(actual) {
            Some(TyKind::Range {
                kind: actual_kind,
                bound: actual_bound,
            }) if kind == actual_kind => match (bound, actual_bound) {
                (Some(bound), Some(actual_bound)) => {
                    match_type_pattern(interner, *bound, *actual_bound, substitutions)
                }
                (None, None) => true,
                _ => false,
            },
            _ => false,
        },
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => match interner.get(actual) {
            Some(TyKind::FunctionPointer {
                params: actual_params,
                return_type: actual_return,
                is_variadic: actual_variadic,
            }) if is_variadic == actual_variadic && params.len() == actual_params.len() => {
                params
                    .iter()
                    .zip(actual_params)
                    .all(|(param, actual_param)| {
                        match_type_pattern(interner, *param, *actual_param, substitutions)
                    })
                    && match_type_pattern(interner, *return_type, *actual_return, substitutions)
            }
            _ => false,
        },
        TyKind::Optional { elem } => match interner.get(actual) {
            Some(TyKind::Optional { elem: actual_elem }) => {
                match_type_pattern(interner, *elem, *actual_elem, substitutions)
            }
            _ => false,
        },
        TyKind::ErrorUnion { error, value } => match interner.get(actual) {
            Some(TyKind::ErrorUnion {
                error: actual_error,
                value: actual_value,
            }) => {
                match_type_pattern(interner, *error, *actual_error, substitutions)
                    && match_type_pattern(interner, *value, *actual_value, substitutions)
            }
            _ => false,
        },
        TyKind::Nominal { def_id, args } => match interner.get(actual) {
            Some(TyKind::Nominal {
                def_id: actual_def_id,
                args: actual_args,
            }) if def_id == actual_def_id && args.len() == actual_args.len() => {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(interner, *arg, *actual_arg, substitutions)
                })
            }
            _ => false,
        },
        TyKind::BuiltinTrait { trait_id, args } => match interner.get(actual) {
            Some(TyKind::BuiltinTrait {
                trait_id: actual_trait_id,
                args: actual_args,
            }) if trait_id == actual_trait_id && args.len() == actual_args.len() => {
                args.iter().zip(actual_args).all(|(arg, actual_arg)| {
                    match_type_pattern(interner, *arg, *actual_arg, substitutions)
                })
            }
            _ => false,
        },
        TyKind::TraitObject { .. }
        | TyKind::TraitObjectPointee { .. }
        | TyKind::Projection { .. } => types_equivalent(interner, pattern, actual),
        TyKind::Error | TyKind::ComptimeOnly => true,
    }
}

fn types_equivalent(interner: &TyInterner, left: InternedTyId, right: InternedTyId) -> bool {
    left == right || interner.get(left) == interner.get(right)
}

fn trait_id_and_args(
    interner: &TyInterner,
    ty: InternedTyId,
) -> Option<(TraitId, Vec<InternedTyId>)> {
    match interner.get(ty)? {
        TyKind::Nominal { def_id, args } => Some((TraitId::Source(*def_id), args.clone())),
        TyKind::BuiltinTrait { trait_id, args } => {
            Some((TraitId::Builtin(*trait_id), args.clone()))
        }
        _ => None,
    }
}

fn substitute_ty(
    interner: &mut TyInterner,
    ty: InternedTyId,
    substitutions: &HashMap<String, InternedTyId>,
) -> Option<InternedTyId> {
    let kind = interner.get(ty)?.clone();
    match kind {
        TyKind::GenericParam(name) => substitutions.get(&name).copied().or(Some(ty)),
        TyKind::Pointer { is_readonly, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Pointer { is_readonly, elem }))
        }
        TyKind::VolatilePointer { is_readonly, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::VolatilePointer { is_readonly, elem }))
        }
        TyKind::Slice { is_readonly, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Slice { is_readonly, elem }))
        }
        TyKind::SlicePointee { elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::SlicePointee { elem }))
        }
        TyKind::Array { len, elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Array { len, elem }))
        }
        TyKind::Range { kind, bound } => {
            let bound = match bound {
                Some(bound) => Some(substitute_ty(interner, bound, substitutions)?),
                None => None,
            };
            Some(interner.intern(TyKind::Range { kind, bound }))
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => {
            let params = params
                .into_iter()
                .map(|param| substitute_ty(interner, param, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let return_type = substitute_ty(interner, return_type, substitutions)?;
            Some(interner.intern(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic,
            }))
        }
        TyKind::Optional { elem } => {
            let elem = substitute_ty(interner, elem, substitutions)?;
            Some(interner.intern(TyKind::Optional { elem }))
        }
        TyKind::ErrorUnion { error, value } => {
            let error = substitute_ty(interner, error, substitutions)?;
            let value = substitute_ty(interner, value, substitutions)?;
            Some(interner.intern(TyKind::ErrorUnion { error, value }))
        }
        TyKind::Nominal { def_id, args } => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            Some(interner.intern(TyKind::Nominal { def_id, args }))
        }
        TyKind::BuiltinTrait { trait_id, args } => {
            let args = args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            Some(interner.intern(TyKind::BuiltinTrait { trait_id, args }))
        }
        TyKind::TraitObject {
            is_readonly,
            trait_id,
            trait_args,
            associated_type_bindings,
        } => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let associated_type_bindings = substitute_associated_type_bindings(
                interner,
                associated_type_bindings,
                substitutions,
            )?;
            Some(interner.intern(TyKind::TraitObject {
                is_readonly,
                trait_id,
                trait_args,
                associated_type_bindings,
            }))
        }
        TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            associated_type_bindings,
        } => {
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let associated_type_bindings = substitute_associated_type_bindings(
                interner,
                associated_type_bindings,
                substitutions,
            )?;
            Some(interner.intern(TyKind::TraitObjectPointee {
                trait_id,
                trait_args,
                associated_type_bindings,
            }))
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            name,
        } => {
            let self_ty = substitute_ty(interner, self_ty, substitutions)?;
            let trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            Some(interner.intern(TyKind::Projection {
                self_ty,
                trait_id,
                trait_args,
                name,
            }))
        }
        TyKind::Error | TyKind::ComptimeOnly | TyKind::Primitive(_) | TyKind::Vector { .. } => {
            Some(ty)
        }
    }
}

fn substitute_associated_type_bindings(
    interner: &mut TyInterner,
    bindings: Vec<AssociatedTypeBindingTy>,
    substitutions: &HashMap<String, InternedTyId>,
) -> Option<Vec<AssociatedTypeBindingTy>> {
    bindings
        .into_iter()
        .map(|binding| {
            let trait_args = binding
                .trait_args
                .into_iter()
                .map(|arg| substitute_ty(interner, arg, substitutions))
                .collect::<Option<Vec<_>>>()?;
            let ty = substitute_ty(interner, binding.ty, substitutions)?;
            Some(AssociatedTypeBindingTy {
                trait_id: binding.trait_id,
                trait_args,
                name: binding.name,
                ty,
            })
        })
        .collect()
}

fn add_reachable_module(
    module_id: ModuleId,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    if reachable_modules.insert(module_id) {
        pending_modules.push_back(module_id);
    }
}

fn add_reachable_type_module(module_id: ModuleId, type_modules: &mut HashSet<ModuleId>) {
    type_modules.insert(module_id);
}

fn freestanding_start_module(graph: &ModuleGraph) -> Option<ModuleId> {
    graph.module_id_for_module_path(&nia_imports::ModulePath {
        package: nia_imports::STD_MODULE_MAP_NAME.to_string(),
        segments: vec![
            "start".to_string(),
            "freestanding".to_string(),
            "linux".to_string(),
            "x86_64".to_string(),
        ],
    })
}

fn collect_reachable_fact_owner_modules(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    modules: &mut HashSet<ModuleId>,
    type_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut type_ids = Vec::new();
    for def_id in reachable_functions
        .iter()
        .filter(|def_id| def_id.module_id == module.module_id)
    {
        let Some(function_facts) = module.semantic_facts.function_facts.get(def_id) else {
            continue;
        };
        collect_function_fact_owner_modules(
            function_facts,
            modules,
            type_modules,
            pending_modules,
            traits,
            &mut type_ids,
        );
    }
    for def_id in reachable_globals
        .iter()
        .filter(|def_id| def_id.module_id == module.module_id)
    {
        if let Some(ty) = module.semantic_facts.global_types.get(def_id) {
            type_ids.push(*ty);
        }
    }
    collect_ty_ids_owner_modules(
        type_ids,
        program_signatures,
        &module.body_ir.interner,
        &module.type_lowering.interner,
        &module.type_normalization.interner,
        modules,
        type_modules,
        traits,
    );
}

fn collect_where_predicate_type_ids(
    predicates: &[nia_defs::WherePredicateSignature],
    type_ids: &mut Vec<InternedTyId>,
) {
    for predicate in predicates {
        type_ids.push(predicate.ty);
        for bound in &predicate.bounds {
            type_ids.push(bound.trait_ty);
            type_ids.extend(
                bound
                    .associated_type_bindings
                    .iter()
                    .map(|binding| binding.ty),
            );
        }
    }
}

fn collect_function_fact_owner_modules(
    facts: &FunctionSemanticFacts,
    modules: &mut HashSet<ModuleId>,
    type_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut ReachableTraitRefs,
    type_ids: &mut Vec<InternedTyId>,
) {
    type_ids.extend(facts.local_types.values().copied());
    type_ids.extend(facts.node_expr_types.values().copied());
    for instantiation in &facts.generic_instantiations {
        add_reachable_module(instantiation.def_id.module_id, modules, pending_modules);
        type_ids.extend(instantiation.args.iter().copied());
    }
    for coercion in facts.node_array_to_slice_coercions.values() {
        type_ids.extend([coercion.array_ty, coercion.slice_ty]);
    }
    for coercion in facts.node_pointer_array_to_slice_coercions.values() {
        type_ids.extend([coercion.pointer_ty, coercion.array_ty, coercion.slice_ty]);
    }
    for coercion in facts.node_trait_object_coercions.values() {
        type_ids.extend([coercion.source_ty, coercion.target_ty]);
    }
    for upcast in facts.node_trait_object_upcasts.values() {
        type_ids.extend([upcast.source_ty, upcast.target_ty]);
    }
    for value in facts.node_builtin_values.values() {
        match value {
            nia_sema_ir::BuiltinValue::Layout { ty, .. }
            | nia_sema_ir::BuiltinValue::FieldOffset { ty, .. } => type_ids.push(*ty),
            _ => {}
        }
    }
    for call in facts.node_resolved_calls.values() {
        collect_resolved_call_owner_modules(
            call,
            modules,
            type_modules,
            pending_modules,
            traits,
            type_ids,
        );
    }
    for reference in facts.node_function_references.values() {
        add_reachable_module(reference.def_id.module_id, modules, pending_modules);
        add_reachable_module(reference.arg_module_id, modules, pending_modules);
        type_ids.extend(reference.args.iter().copied());
    }
}

fn collect_resolved_call_owner_modules(
    call: &nia_sema_ir::ResolvedCall,
    modules: &mut HashSet<ModuleId>,
    type_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut ReachableTraitRefs,
    type_ids: &mut Vec<InternedTyId>,
) {
    match call {
        nia_sema_ir::ResolvedCall::Function(def_id) => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
        }
        nia_sema_ir::ResolvedCall::FunctionInstance {
            def_id,
            arg_module_id,
            args,
        } => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
            add_reachable_module(*arg_module_id, modules, pending_modules);
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::Method { def_id, args, .. } => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitMethod {
            trait_id,
            method_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            add_reachable_module(method_id.module_id, modules, pending_modules);
            collect_trait_id_owner_module(TraitId::Source(*trait_id), type_modules, traits);
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitAssociatedFunction {
            trait_id,
            method_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            add_reachable_module(method_id.module_id, modules, pending_modules);
            collect_trait_id_owner_module(TraitId::Source(*trait_id), type_modules, traits);
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::DynamicTraitMethod {
            object_ty,
            trait_id,
            method_id,
            trait_args,
            params,
            return_type,
            ..
        } => {
            add_reachable_module(method_id.module_id, modules, pending_modules);
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            type_ids.push(*object_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(params.iter().copied());
            type_ids.push(*return_type);
        }
        nia_sema_ir::ResolvedCall::BuiltinTraitMethod { trait_id, op } => {
            let _ = op;
            traits.insert_trait(TraitId::Builtin(*trait_id));
        }
        nia_sema_ir::ResolvedCall::BuiltinMethod { self_ty, .. } => {
            type_ids.push(*self_ty);
        }
        nia_sema_ir::ResolvedCall::BuiltinPlaceMethod {
            trait_id,
            method,
            self_ty,
            trait_args,
            ..
        } => {
            let _ = method;
            traits.insert_trait(TraitId::Builtin(*trait_id));
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::FunctionPointer => {}
    }
}

fn collect_ty_ids_owner_modules<'a>(
    tys: impl IntoIterator<Item = InternedTyId>,
    program_signatures: ExecutableSignatureIndex<'a>,
    body_interner: &TyInterner,
    type_lowering_interner: &TyInterner,
    normalization_interner: &TyInterner,
    modules: &mut HashSet<ModuleId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut pending = tys
        .into_iter()
        .map(|ty| PendingTy {
            ty,
            interner: None,
            owned_interner: None,
        })
        .collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    while let Some(pending_ty) = pending.pop_front() {
        let ty_id = pending_ty.ty;
        add_reachable_type_module(type_owner(ty_id).module_id(), type_modules);
        let interner_id = pending_ty
            .interner
            .map(TyInterner::interner_id)
            .or_else(|| {
                pending_ty
                    .owned_interner
                    .as_ref()
                    .map(TyInterner::interner_id)
            });
        if !seen.insert((ty_id, interner_id)) {
            continue;
        }
        let ty = if let Some(interner) = pending_ty.interner {
            interner.get(ty_id)
        } else if let Some(interner) = pending_ty.owned_interner.as_ref() {
            interner.get(ty_id)
        } else {
            body_interner
                .get(ty_id)
                .or_else(|| type_lowering_interner.get(ty_id))
                .or_else(|| normalization_interner.get(ty_id))
        };
        let Some(ty) = ty else { continue };
        collect_ty_owner_modules(
            ty,
            program_signatures,
            &mut pending,
            modules,
            type_modules,
            traits,
        );
    }
}

#[derive(Clone)]
struct PendingTy<'a> {
    ty: InternedTyId,
    interner: Option<&'a TyInterner>,
    owned_interner: Option<TyInterner>,
}

fn type_owner(ty: InternedTyId) -> nia_ids::TypeOwner {
    ty.owner()
}

fn collect_ty_owner_modules<'a>(
    ty: &TyKind,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_ids: &mut VecDeque<PendingTy<'a>>,
    modules: &mut HashSet<ModuleId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    match ty {
        TyKind::Nominal { def_id, args } => {
            add_reachable_type_module(def_id.module_id, type_modules);
            push_tys(type_ids, args.iter().copied());
            collect_nominal_signature_owner_type_ids(*def_id, program_signatures, type_ids);
        }
        TyKind::Pointer { elem, .. }
        | TyKind::VolatilePointer { elem, .. }
        | TyKind::Slice { elem, .. }
        | TyKind::SlicePointee { elem }
        | TyKind::Optional { elem } => {
            push_ty(type_ids, *elem);
        }
        TyKind::Array { len, elem } => {
            push_ty(type_ids, *elem);
            collect_array_len_owner_modules(len, type_ids);
        }
        TyKind::Range { bound, .. } => {
            if let Some(bound) = bound {
                push_ty(type_ids, *bound);
            }
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            ..
        } => {
            push_tys(type_ids, params.iter().copied());
            push_ty(type_ids, *return_type);
        }
        TyKind::ErrorUnion { error, value } => {
            push_ty(type_ids, *error);
            push_ty(type_ids, *value);
        }
        TyKind::TraitObject {
            trait_id,
            trait_args,
            associated_type_bindings,
            ..
        }
        | TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            associated_type_bindings,
        } => {
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            push_tys(type_ids, trait_args.iter().copied());
            collect_associated_binding_owner_modules(
                associated_type_bindings,
                type_ids,
                modules,
                type_modules,
                traits,
            );
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            ..
        } => {
            push_ty(type_ids, *self_ty);
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            push_tys(type_ids, trait_args.iter().copied());
        }
        TyKind::BuiltinTrait { args, .. } => push_tys(type_ids, args.iter().copied()),
        TyKind::Error
        | TyKind::ComptimeOnly
        | TyKind::Primitive(_)
        | TyKind::Vector { .. }
        | TyKind::GenericParam(_) => {}
    }
}

fn collect_nominal_signature_owner_type_ids<'a>(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_ids: &mut VecDeque<PendingTy<'a>>,
) {
    if let Some(signature) = (program_signatures.struct_)(def_id) {
        push_owned_program_tys(
            type_ids,
            signature.signature.fields.iter().map(|field| field.ty),
            &signature.interner,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            type_ids,
            &signature.interner,
        );
    }
    if let Some(signature) = (program_signatures.union)(def_id) {
        push_owned_program_tys(
            type_ids,
            signature.signature.fields.iter().map(|field| field.ty),
            &signature.interner,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            type_ids,
            &signature.interner,
        );
    }
}

fn collect_owned_where_predicate_type_ids_deque(
    predicates: &[nia_defs::WherePredicateSignature],
    type_ids: &mut VecDeque<PendingTy<'_>>,
    interner: &TyInterner,
) {
    let mut collected = Vec::new();
    collect_where_predicate_type_ids(predicates, &mut collected);
    push_owned_program_tys(type_ids, collected, interner);
}

fn collect_array_len_owner_modules(
    len: &nia_ty::ArrayLenTy,
    type_ids: &mut VecDeque<PendingTy<'_>>,
) {
    if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
        push_ty(type_ids, *ty);
    }
}

fn collect_trait_id_owner_module(
    trait_id: TraitId,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    traits.insert_trait(trait_id);
    if let TraitId::Source(def_id) = trait_id {
        add_reachable_type_module(def_id.module_id, type_modules);
    }
}

fn collect_associated_binding_owner_modules<'a>(
    bindings: &[AssociatedTypeBindingTy],
    type_ids: &mut VecDeque<PendingTy<'a>>,
    modules: &mut HashSet<ModuleId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    let _ = modules;
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, type_modules, traits);
        }
        push_tys(type_ids, binding.trait_args.iter().copied());
        push_ty(type_ids, binding.ty);
    }
}

fn push_ty(type_ids: &mut VecDeque<PendingTy<'_>>, ty: InternedTyId) {
    type_ids.push_back(PendingTy {
        ty,
        interner: None,
        owned_interner: None,
    });
}

fn push_tys(type_ids: &mut VecDeque<PendingTy<'_>>, tys: impl IntoIterator<Item = InternedTyId>) {
    type_ids.extend(tys.into_iter().map(|ty| PendingTy {
        ty,
        interner: None,
        owned_interner: None,
    }));
}

fn push_owned_program_tys(
    type_ids: &mut VecDeque<PendingTy<'_>>,
    tys: impl IntoIterator<Item = InternedTyId>,
    interner: &TyInterner,
) {
    type_ids.extend(tys.into_iter().map(|ty| PendingTy {
        ty,
        interner: None,
        owned_interner: Some(interner.clone()),
    }));
}
