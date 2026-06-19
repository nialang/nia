// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet, VecDeque};

use nia_body_ir::{
    BodyIr, PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee,
    TypedExpr, TypedExprKind, TypedInlineAsm, TypedMemoryIntrinsicSource, TypedPattern,
    TypedPatternKind, TypedPlace, TypedStmt, TypedStmtKind, TypedSwitchArmBody,
};
use nia_defs::{DefCollection, DefKind, ExtensionMethods};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_imports::ModuleGraph;
use nia_item_signatures::{ItemSignatures, ProgramSignatureMaps};
use nia_sema_ir::{FunctionSemanticFacts, SemanticFacts};
use nia_ty::{AssociatedTypeBindingTy, TyInterner, TyKind};

#[derive(Debug, Clone, Copy)]
pub struct ReachableModuleInput<'a> {
    pub module_id: ModuleId,
    pub body_ir: &'a BodyIr,
    pub item_signatures: &'a ItemSignatures,
    pub semantic_facts: &'a SemanticFacts,
    pub type_lowering: &'a nia_type_lower::TypeLowering,
    pub type_normalization: &'a nia_type_normalize::TypeNormalization,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutableReachability {
    pub modules: HashSet<ModuleId>,
    pub functions: HashSet<GlobalDefId>,
    pub stats: ExecutableReachabilityStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutableReachabilityStats {
    pub checked_modules: usize,
    pub checked_bodies: usize,
    pub reachable_bodies: usize,
}

pub fn compute_executable_reachability(
    parse_ok: &[ModuleId],
    graph: &ModuleGraph,
    defs_by_id: &HashMap<ModuleId, DefCollection>,
    program_signatures: ProgramSignatureMaps<'_>,
    extension_methods: &ExtensionMethods,
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    let modules_by_id = modules
        .iter()
        .map(|module| (module.module_id, *module))
        .collect::<HashMap<_, _>>();
    let mut reachable_functions = executable_root_functions(graph, defs_by_id);
    let mut reachable_modules = reachable_functions
        .iter()
        .map(|def_id| def_id.module_id)
        .collect::<HashSet<_>>();
    add_reachable_module(graph.root(), &mut reachable_modules, &mut VecDeque::new());

    let parse_ok_set = parse_ok.iter().copied().collect::<HashSet<_>>();
    loop {
        let before = (reachable_functions.len(), reachable_modules.len());
        let mut reachable_traits = HashSet::new();
        let current_reachable_modules = reachable_modules.clone();
        for module in modules_by_id
            .values()
            .filter(|module| current_reachable_modules.contains(&module.module_id))
        {
            let mut pending_modules = VecDeque::new();
            extend_reachable_functions_from_bodies(
                module,
                program_signatures,
                &mut reachable_functions,
                &mut reachable_modules,
                &mut pending_modules,
            );
            collect_reachable_body_trait_ids(module, &reachable_functions, &mut reachable_traits);
            collect_reachable_fact_owner_modules(
                module,
                &reachable_functions,
                &mut reachable_modules,
                &mut pending_modules,
                &mut reachable_traits,
            );
        }
        let mut pending_modules = VecDeque::new();
        extend_reachable_functions_from_traits(
            program_signatures,
            extension_methods,
            &reachable_traits,
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
        if before == (reachable_functions.len(), reachable_modules.len()) {
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
        functions: reachable_functions,
        stats,
    }
}

pub fn filter_semantic_facts_for_reachable_functions(
    facts: SemanticFacts,
    reachable_functions: &HashSet<GlobalDefId>,
) -> SemanticFacts {
    let mut reachable_facts = SemanticFacts {
        global_types: facts.global_types,
        ..Default::default()
    };
    for def_id in reachable_functions {
        let Some(function_facts) = facts.function_facts.get(def_id) else {
            continue;
        };
        reachable_facts
            .local_types
            .extend(function_facts.local_types.clone());
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

fn executable_root_functions(
    graph: &ModuleGraph,
    defs_by_id: &HashMap<ModuleId, DefCollection>,
) -> HashSet<GlobalDefId> {
    let mut roots = HashSet::new();
    if let Some(main) = named_function(defs_by_id, graph.root(), "main") {
        roots.insert(main);
    }
    if let Some(start_module) = freestanding_start_module(graph)
        && let Some(start) = named_function(defs_by_id, start_module, "_start")
    {
        roots.insert(start);
        roots.extend(module_functions(defs_by_id, start_module));
    }
    roots
}

fn module_functions(
    defs_by_id: &HashMap<ModuleId, DefCollection>,
    module_id: ModuleId,
) -> impl Iterator<Item = GlobalDefId> + '_ {
    defs_by_id
        .get(&module_id)
        .into_iter()
        .flat_map(move |defs| {
            defs.defs.iter().filter_map(move |(def_id, def)| {
                (def.kind == DefKind::Function).then_some(GlobalDefId { module_id, def_id })
            })
        })
}

fn named_function(
    defs_by_id: &HashMap<ModuleId, DefCollection>,
    module_id: ModuleId,
    name: &str,
) -> Option<GlobalDefId> {
    defs_by_id.get(&module_id).and_then(|defs| {
        defs.defs.iter().find_map(|(def_id, def)| {
            (def.kind == DefKind::Function && def.name == name)
                .then_some(GlobalDefId { module_id, def_id })
        })
    })
}

fn extend_reachable_functions_from_bodies(
    module: &ReachableModuleInput<'_>,
    program_signatures: ProgramSignatureMaps<'_>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    for def_id in typed_body_callees(module, reachable_functions) {
        add_reachable_function(
            def_id,
            program_signatures,
            reachable_functions,
            reachable_modules,
            pending_modules,
        );
    }
}

fn typed_body_callees(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
) -> Vec<GlobalDefId> {
    let mut refs = TypedBodyRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        if reachable_functions.contains(def_id) {
            collect_typed_body_refs(body, &mut refs);
        }
    }
    refs.functions.into_iter().collect()
}

fn collect_reachable_body_trait_ids(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    traits: &mut HashSet<TraitId>,
) {
    let mut refs = TypedBodyRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        if reachable_functions.contains(def_id) {
            collect_typed_body_refs(body, &mut refs);
        }
    }
    traits.extend(refs.traits);
}

#[derive(Default)]
struct TypedBodyRefs {
    functions: HashSet<GlobalDefId>,
    traits: HashSet<TraitId>,
}

fn collect_typed_body_refs(body: &TypedBody, refs: &mut TypedBodyRefs) {
    for stmt in &body.stmts {
        collect_typed_stmt_refs(stmt, refs);
    }
    if let Some(tail) = body.tail.as_deref() {
        collect_typed_expr_refs(tail, refs);
    }
}

fn collect_typed_stmt_refs(stmt: &TypedStmt, refs: &mut TypedBodyRefs) {
    match &stmt.kind {
        TypedStmtKind::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_typed_expr_refs(value, refs);
            }
        }
        TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => {
            collect_typed_expr_refs(expr, refs);
        }
        TypedStmtKind::Return(value) => {
            if let Some(value) = value {
                collect_typed_expr_refs(value, refs);
            }
        }
        TypedStmtKind::ForIn(for_in) => {
            refs.traits
                .insert(TraitId::Builtin(nia_ty::BuiltinTrait::Iterator));
            collect_typed_expr_refs(&for_in.iter, refs);
            collect_typed_body_refs(&for_in.body, refs);
        }
        TypedStmtKind::While(while_loop) => {
            collect_typed_expr_refs(&while_loop.cond, refs);
            collect_typed_body_refs(&while_loop.body, refs);
        }
        TypedStmtKind::Loop(loop_body) => collect_typed_body_refs(&loop_body.body, refs),
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn collect_typed_expr_refs(expr: &TypedExpr, refs: &mut TypedBodyRefs) {
    match &expr.kind {
        TypedExprKind::Function(def_id)
        | TypedExprKind::FunctionInstance { def_id, .. }
        | TypedExprKind::Field { field: def_id, .. } => {
            refs.functions.insert(*def_id);
        }
        TypedExprKind::Range(range) => {
            if let Some(start) = range.start.as_deref() {
                collect_typed_expr_refs(start, refs);
            }
            if let Some(end) = range.end.as_deref() {
                collect_typed_expr_refs(end, refs);
            }
        }
        TypedExprKind::InlineAsm(asm) => collect_typed_inline_asm_refs(asm, refs),
        TypedExprKind::MemoryIntrinsic(memory) => {
            collect_typed_expr_refs(&memory.dest, refs);
            match &memory.source {
                TypedMemoryIntrinsicSource::Slice(source)
                | TypedMemoryIntrinsicSource::Byte(source) => collect_typed_expr_refs(source, refs),
            }
        }
        TypedExprKind::Atomic(atomic) => collect_typed_atomic_refs(atomic, refs),
        TypedExprKind::LoadUnaligned { ptr, .. } => collect_typed_expr_refs(ptr, refs),
        TypedExprKind::Splat { value } | TypedExprKind::Bitmask { vector: value } => {
            collect_typed_expr_refs(value, refs);
        }
        TypedExprKind::ExtractElement { vector, index } => {
            collect_typed_expr_refs(vector, refs);
            collect_typed_expr_refs(index, refs);
        }
        TypedExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            collect_typed_expr_refs(vector, refs);
            collect_typed_expr_refs(index, refs);
            collect_typed_expr_refs(value, refs);
        }
        TypedExprKind::BitIntrinsic { value, .. }
        | TypedExprKind::StaticArrayPointer { array: value, .. }
        | TypedExprKind::Unary { expr: value, .. }
        | TypedExprKind::OptionalSome { expr: value }
        | TypedExprKind::ErrorOk { expr: value }
        | TypedExprKind::ErrorErr { expr: value }
        | TypedExprKind::Try { expr: value }
        | TypedExprKind::Discard(value)
        | TypedExprKind::Cast { expr: value, .. }
        | TypedExprKind::TraitObjectUpcast { expr: value, .. }
        | TypedExprKind::TraitObjectCoercion { expr: value, .. } => {
            collect_typed_expr_refs(value, refs);
        }
        TypedExprKind::ArrayLiteral { elems } => match elems {
            TypedArrayElements::List(elems) => {
                for elem in elems {
                    collect_typed_expr_refs(elem, refs);
                }
            }
            TypedArrayElements::Repeat { value, .. } => collect_typed_expr_refs(value, refs),
        },
        TypedExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_typed_expr_refs(&field.value, refs);
            }
        }
        TypedExprKind::UnionLiteral { field, .. } => {
            collect_typed_expr_refs(&field.value, refs);
        }
        TypedExprKind::Binary { lhs, rhs, .. } => {
            collect_typed_expr_refs(lhs, refs);
            collect_typed_expr_refs(rhs, refs);
        }
        TypedExprKind::Assign { place, rhs, .. } => {
            collect_typed_place_refs(place, refs);
            collect_typed_expr_refs(rhs, refs);
        }
        TypedExprKind::Call { callee, args } => {
            collect_typed_callee_refs(callee, refs);
            for arg in args {
                collect_typed_expr_refs(arg, refs);
            }
        }
        TypedExprKind::Index { lhs, index } => {
            collect_typed_expr_refs(lhs, refs);
            collect_typed_expr_refs(index, refs);
        }
        TypedExprKind::Slice { lhs, range, .. } => {
            collect_typed_expr_refs(lhs, refs);
            if let Some(start) = range.start.as_deref() {
                collect_typed_expr_refs(start, refs);
            }
            if let Some(end) = range.end.as_deref() {
                collect_typed_expr_refs(end, refs);
            }
        }
        TypedExprKind::Block(body) => collect_typed_body_refs(body, refs),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_typed_expr_refs(cond, refs);
            collect_typed_body_refs(then_branch, refs);
            if let Some(else_branch) = else_branch.as_deref() {
                collect_typed_expr_refs(else_branch, refs);
            }
        }
        TypedExprKind::Switch(switch) => {
            collect_typed_expr_refs(&switch.target, refs);
            for arm in &switch.arms {
                for pattern in &arm.patterns {
                    collect_typed_switch_pattern_refs(pattern, refs);
                }
                match &arm.body {
                    TypedSwitchArmBody::Expr(expr) => collect_typed_expr_refs(expr, refs),
                    TypedSwitchArmBody::Stmt(stmt) => collect_typed_stmt_refs(stmt, refs),
                    TypedSwitchArmBody::Block(body) => collect_typed_body_refs(body, refs),
                }
            }
        }
        TypedExprKind::IfPattern(if_pattern) => {
            collect_typed_expr_refs(&if_pattern.target, refs);
            for arm in &if_pattern.arms {
                collect_typed_pattern_refs(&arm.pattern, refs);
                collect_typed_body_refs(&arm.body, refs);
            }
            if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                collect_typed_expr_refs(else_branch, refs);
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
        | TypedExprKind::Local(_)
        | TypedExprKind::Global(_)
        | TypedExprKind::EnumVariant(_)
        | TypedExprKind::BuiltinValue(_)
        | TypedExprKind::Trap => {}
    }
}

fn collect_typed_callee_refs(callee: &TypedCallee, refs: &mut TypedBodyRefs) {
    match callee {
        TypedCallee::Function(def_id) | TypedCallee::FunctionInstance { def_id, .. } => {
            refs.functions.insert(*def_id);
        }
        TypedCallee::Method {
            def_id, receiver, ..
        } => {
            refs.functions.insert(*def_id);
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::TraitMethod {
            trait_id,
            method_id,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert(TraitId::Source(*trait_id));
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::TraitAssociatedFunction {
            trait_id,
            method_id,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert(TraitId::Source(*trait_id));
        }
        TypedCallee::DynamicTraitMethod {
            trait_id,
            method_id,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert(*trait_id);
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::BuiltinMethod { receiver, .. } | TypedCallee::FunctionPointer(receiver) => {
            collect_typed_expr_refs(receiver, refs);
        }
        TypedCallee::BuiltinOperator(operator) => {
            refs.traits.insert(TraitId::Builtin(operator.trait_id));
        }
        TypedCallee::BuiltinPlaceMethod(method) => {
            refs.traits.insert(TraitId::Builtin(method.trait_id));
            collect_typed_expr_refs(&method.receiver, refs);
        }
    }
}

fn collect_typed_pattern_refs(pattern: &TypedPattern, refs: &mut TypedBodyRefs) {
    match &pattern.kind {
        TypedPatternKind::OptionalSome(pattern)
        | TypedPatternKind::ErrorOk(pattern)
        | TypedPatternKind::ErrorErr(pattern) => collect_typed_pattern_refs(pattern, refs),
        TypedPatternKind::Expr(expr) => collect_typed_expr_refs(expr, refs),
        TypedPatternKind::Range { start, end, .. } => {
            collect_typed_expr_refs(start, refs);
            collect_typed_expr_refs(end, refs);
        }
        TypedPatternKind::Wildcard
        | TypedPatternKind::Bind { .. }
        | TypedPatternKind::OptionalNull => {}
    }
}

fn collect_typed_switch_pattern_refs(
    pattern: &nia_body_ir::TypedSwitchPattern,
    refs: &mut TypedBodyRefs,
) {
    match &pattern.kind {
        nia_body_ir::TypedSwitchPatternKind::Expr(expr) => collect_typed_expr_refs(expr, refs),
        nia_body_ir::TypedSwitchPatternKind::Range { start, end, .. } => {
            collect_typed_expr_refs(start, refs);
            collect_typed_expr_refs(end, refs);
        }
        nia_body_ir::TypedSwitchPatternKind::Wildcard
        | nia_body_ir::TypedSwitchPatternKind::CheckedInt { .. }
        | nia_body_ir::TypedSwitchPatternKind::CheckedIntRange { .. } => {}
    }
}

fn collect_typed_atomic_refs(atomic: &TypedAtomic, refs: &mut TypedBodyRefs) {
    match atomic {
        TypedAtomic::Load { ptr, .. } => collect_typed_expr_refs(ptr, refs),
        TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
            collect_typed_expr_refs(ptr, refs);
            collect_typed_expr_refs(value, refs);
        }
        TypedAtomic::Cmpxchg {
            ptr,
            expected,
            desired,
            ..
        } => {
            collect_typed_expr_refs(ptr, refs);
            collect_typed_expr_refs(expected, refs);
            collect_typed_expr_refs(desired, refs);
        }
        TypedAtomic::Fence { .. } => {}
    }
}

fn collect_typed_inline_asm_refs(asm: &TypedInlineAsm, refs: &mut TypedBodyRefs) {
    for input in &asm.inputs {
        collect_typed_expr_refs(&input.value, refs);
    }
    for output in &asm.outputs {
        collect_typed_place_refs(&output.place, refs);
    }
}

fn collect_typed_place_refs(place: &TypedPlace, refs: &mut TypedBodyRefs) {
    match &place.base {
        PlaceBase::Deref(expr) => collect_typed_expr_refs(expr, refs),
        PlaceBase::Local(_) | PlaceBase::Global(_) | PlaceBase::Error => {}
    }
    for elem in &place.elems {
        match elem {
            PlaceElem::Index(expr) => collect_typed_expr_refs(expr, refs),
            PlaceElem::Field(_) | PlaceElem::Error => {}
        }
    }
}

fn extend_reachable_functions_from_traits(
    program_signatures: ProgramSignatureMaps<'_>,
    extension_methods: &ExtensionMethods,
    reachable_traits: &HashSet<TraitId>,
    reachable_modules: &HashSet<ModuleId>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let mut modules = reachable_modules.clone();
    for trait_id in reachable_traits {
        let TraitId::Source(trait_def) = trait_id else {
            continue;
        };
        if !reachable_modules.contains(&trait_def.module_id) {
            continue;
        }
        let Some(trait_signature) = program_signatures.traits.get(trait_def) else {
            continue;
        };
        for method in &trait_signature.signature.methods {
            if method.has_default {
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
        if method
            .trait_id
            .is_some_and(|trait_id| reachable_traits.contains(&trait_id))
        {
            add_reachable_function(
                method.def_id,
                program_signatures,
                reachable_functions,
                &mut modules,
                pending_modules,
            );
        }
    }
}

fn add_reachable_function(
    def_id: GlobalDefId,
    program_signatures: ProgramSignatureMaps<'_>,
    reachable_functions: &mut HashSet<GlobalDefId>,
    reachable_modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
) {
    let Some(signature) = program_signatures.functions.get(&def_id) else {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        return;
    };
    if signature.signature.is_comptime || !signature.signature.has_body {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
        return;
    }
    if reachable_functions.insert(def_id) {
        add_reachable_module(def_id.module_id, reachable_modules, pending_modules);
    }
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
    reachable_functions: &HashSet<GlobalDefId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
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
            pending_modules,
            traits,
            &mut type_ids,
        );
    }
    collect_module_signature_owner_type_ids(module.item_signatures, &mut type_ids);
    collect_ty_ids_owner_modules(
        type_ids,
        &module.body_ir.interner,
        &module.type_lowering.interner,
        &module.type_normalization.interner,
        modules,
        pending_modules,
        traits,
    );
}

fn collect_module_signature_owner_type_ids(
    signatures: &ItemSignatures,
    type_ids: &mut Vec<InternedTyId>,
) {
    for signature in signatures.structs.values() {
        type_ids.extend(signature.fields.iter().map(|field| field.ty));
        collect_where_predicate_type_ids(&signature.where_predicates, type_ids);
    }
    for signature in signatures.unions.values() {
        type_ids.extend(signature.fields.iter().map(|field| field.ty));
        collect_where_predicate_type_ids(&signature.where_predicates, type_ids);
    }
    for signature in signatures.type_aliases.values() {
        type_ids.push(signature.target);
    }
    for signature in signatures.enums.values() {
        type_ids.push(signature.backing_type);
    }
    for signature in &signatures.trait_impls {
        type_ids.push(signature.target_ty);
        if let Some(trait_ty) = signature.trait_ty {
            type_ids.push(trait_ty);
        }
        collect_where_predicate_type_ids(&signature.where_predicates, type_ids);
        type_ids.extend(
            signature
                .associated_types
                .iter()
                .map(|associated| associated.ty),
        );
    }
    for signature in signatures.globals.values() {
        if let Some(ty) = signature.explicit_type {
            type_ids.push(ty);
        }
    }
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
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
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
        if let nia_sema_ir::BuiltinValue::Layout { ty, .. } = value {
            type_ids.push(*ty);
        }
    }
    for call in facts.node_resolved_calls.values() {
        collect_resolved_call_owner_modules(call, modules, pending_modules, traits, type_ids);
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
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
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
            collect_trait_id_owner_module(
                TraitId::Source(*trait_id),
                modules,
                pending_modules,
                traits,
            );
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
            collect_trait_id_owner_module(
                TraitId::Source(*trait_id),
                modules,
                pending_modules,
                traits,
            );
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
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            type_ids.push(*object_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(params.iter().copied());
            type_ids.push(*return_type);
        }
        nia_sema_ir::ResolvedCall::BuiltinTraitMethod { trait_id, .. } => {
            traits.insert(TraitId::Builtin(*trait_id));
        }
        nia_sema_ir::ResolvedCall::BuiltinMethod { self_ty, .. } => {
            type_ids.push(*self_ty);
        }
        nia_sema_ir::ResolvedCall::BuiltinPlaceMethod {
            trait_id,
            self_ty,
            trait_args,
            ..
        } => {
            traits.insert(TraitId::Builtin(*trait_id));
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::FunctionPointer => {}
    }
}

fn collect_ty_ids_owner_modules(
    tys: impl IntoIterator<Item = InternedTyId>,
    body_interner: &TyInterner,
    type_lowering_interner: &TyInterner,
    normalization_interner: &TyInterner,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    let mut pending = tys.into_iter().collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    while let Some(ty_id) = pending.pop_front() {
        add_reachable_module(ty_id.interner_id, modules, pending_modules);
        if !seen.insert(ty_id) {
            continue;
        }
        let Some(ty) = body_interner
            .get(ty_id)
            .or_else(|| type_lowering_interner.get(ty_id))
            .or_else(|| normalization_interner.get(ty_id))
        else {
            continue;
        };
        collect_ty_owner_modules(ty, &mut pending, modules, pending_modules, traits);
    }
}

fn collect_ty_owner_modules(
    ty: &TyKind,
    type_ids: &mut VecDeque<InternedTyId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    match ty {
        TyKind::Nominal { def_id, args } => {
            add_reachable_module(def_id.module_id, modules, pending_modules);
            type_ids.extend(args.iter().copied());
        }
        TyKind::Pointer { elem, .. }
        | TyKind::VolatilePointer { elem, .. }
        | TyKind::Slice { elem, .. }
        | TyKind::SlicePointee { elem }
        | TyKind::Optional { elem } => {
            type_ids.push_back(*elem);
        }
        TyKind::Array { len, elem } => {
            type_ids.push_back(*elem);
            collect_array_len_owner_modules(len, type_ids);
        }
        TyKind::Range { bound, .. } => {
            if let Some(bound) = bound {
                type_ids.push_back(*bound);
            }
        }
        TyKind::FunctionPointer {
            params,
            return_type,
            ..
        } => {
            type_ids.extend(params.iter().copied());
            type_ids.push_back(*return_type);
        }
        TyKind::ErrorUnion { error, value } => {
            type_ids.push_back(*error);
            type_ids.push_back(*value);
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
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            type_ids.extend(trait_args.iter().copied());
            collect_associated_binding_owner_modules(
                associated_type_bindings,
                type_ids,
                modules,
                pending_modules,
                traits,
            );
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            ..
        } => {
            type_ids.push_back(*self_ty);
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            type_ids.extend(trait_args.iter().copied());
        }
        TyKind::BuiltinTrait { args, .. } => type_ids.extend(args.iter().copied()),
        TyKind::Error
        | TyKind::ComptimeOnly
        | TyKind::Primitive(_)
        | TyKind::Vector { .. }
        | TyKind::GenericParam(_) => {}
    }
}

fn collect_array_len_owner_modules(
    len: &nia_ty::ArrayLenTy,
    type_ids: &mut VecDeque<InternedTyId>,
) {
    if let nia_ty::ArrayLenTy::Builtin { ty, .. } = len {
        type_ids.push_back(*ty);
    }
}

fn collect_trait_id_owner_module(
    trait_id: TraitId,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    traits.insert(trait_id);
    if let TraitId::Source(def_id) = trait_id {
        add_reachable_module(def_id.module_id, modules, pending_modules);
    }
}

fn collect_associated_binding_owner_modules(
    bindings: &[AssociatedTypeBindingTy],
    type_ids: &mut VecDeque<InternedTyId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut HashSet<TraitId>,
) {
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, modules, pending_modules, traits);
        }
        type_ids.extend(binding.trait_args.iter().copied());
        type_ids.push_back(binding.ty);
    }
}
