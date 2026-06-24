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
use nia_item_signatures::{ItemSignatures, ProgramFunctionSignature, ProgramTraitSignature};
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
    modules: &[ReachableModuleInput<'_>],
) -> ExecutableReachability {
    let modules_by_id = modules
        .iter()
        .map(|module| (module.module_id, *module))
        .collect::<HashMap<_, _>>();
    let mut reachable_functions = executable_root_functions(graph, root_defs);
    let mut reachable_modules = reachable_functions
        .iter()
        .map(|def_id| def_id.module_id)
        .collect::<HashSet<_>>();
    add_reachable_module(graph.entry(), &mut reachable_modules, &mut VecDeque::new());

    let parse_ok_set = parse_ok.iter().copied().collect::<HashSet<_>>();
    loop {
        let before = (reachable_functions.len(), reachable_modules.len());
        let mut reachable_traits = ReachableTraitRefs::default();
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

#[derive(Debug, Clone, Copy)]
pub struct ExecutableSignatureIndex<'a> {
    pub functions: &'a HashMap<GlobalDefId, ProgramFunctionSignature>,
    pub traits: &'a HashMap<GlobalDefId, ProgramTraitSignature>,
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
            collect_typed_body_refs(module, body, &mut refs);
        }
    }
    refs.functions.into_iter().collect()
}

fn collect_reachable_body_trait_ids(
    module: &ReachableModuleInput<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut refs = TypedBodyRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        if reachable_functions.contains(def_id) {
            collect_typed_body_refs(module, body, &mut refs);
        }
    }
    traits.extend(refs.traits);
}

#[derive(Default)]
struct TypedBodyRefs {
    functions: HashSet<GlobalDefId>,
    traits: ReachableTraitRefs,
}

#[derive(Default)]
struct ReachableTraitRefs {
    traits: HashSet<TraitId>,
    methods: HashSet<ReachableTraitMethod>,
    vtable_traits: HashSet<TraitId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReachableTraitMethod {
    trait_id: TraitId,
    method_name: String,
}

impl ReachableTraitRefs {
    fn extend(&mut self, refs: Self) {
        self.traits.extend(refs.traits);
        self.methods.extend(refs.methods);
        self.vtable_traits.extend(refs.vtable_traits);
    }

    fn insert_trait(&mut self, trait_id: TraitId) {
        self.traits.insert(trait_id);
    }

    fn insert_method(&mut self, trait_id: TraitId, method_name: impl Into<String>) {
        self.traits.insert(trait_id);
        self.methods.insert(ReachableTraitMethod {
            trait_id,
            method_name: method_name.into(),
        });
    }

    fn insert_vtable_trait(&mut self, trait_id: TraitId) {
        self.traits.insert(trait_id);
        self.vtable_traits.insert(trait_id);
    }

    fn needs_method(&self, trait_id: TraitId, method_name: &str) -> bool {
        self.vtable_traits.contains(&trait_id)
            || self.methods.contains(&ReachableTraitMethod {
                trait_id,
                method_name: method_name.to_string(),
            })
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
                TraitId::Builtin(nia_ty::BuiltinTrait::Iterator),
                nia_ids::BuiltinTraitMethod::IteratorNext.name(),
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
        TypedExprKind::Function(def_id)
        | TypedExprKind::FunctionInstance { def_id, .. }
        | TypedExprKind::Field { field: def_id, .. } => {
            refs.functions.insert(*def_id);
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
            ..
        } => {
            collect_typed_expr_refs(module, value, refs);
            collect_trait_object_vtable_ref(module, *target_ty, refs);
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
            collect_typed_callee_refs(module, callee, refs);
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
        | TypedExprKind::Local(_)
        | TypedExprKind::Global(_)
        | TypedExprKind::EnumVariant(_)
        | TypedExprKind::BuiltinValue(_)
        | TypedExprKind::Trap => {}
    }
}

fn collect_typed_callee_refs(
    module: &ReachableModuleInput<'_>,
    callee: &TypedCallee,
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
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits
                .insert_method(TraitId::Source(*trait_id), method_name.clone());
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::TraitAssociatedFunction {
            trait_id,
            method_id,
            method_name,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits
                .insert_method(TraitId::Source(*trait_id), method_name.clone());
        }
        TypedCallee::DynamicTraitMethod {
            trait_id,
            method_id,
            method_name,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.traits.insert_method(*trait_id, method_name.clone());
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::BuiltinMethod { receiver, .. } | TypedCallee::FunctionPointer(receiver) => {
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::BuiltinOperator(operator) => {
            if let Some(method) = operator.method() {
                refs.traits
                    .insert_method(TraitId::Builtin(operator.trait_id), method.name());
            } else {
                refs.traits
                    .insert_trait(TraitId::Builtin(operator.trait_id));
            }
        }
        TypedCallee::BuiltinPlaceMethod(method) => {
            refs.traits
                .insert_method(TraitId::Builtin(method.trait_id), method.method.name());
            collect_typed_expr_refs(module, &method.receiver, refs);
        }
    }
}

fn collect_trait_object_vtable_ref(
    module: &ReachableModuleInput<'_>,
    object_ty: InternedTyId,
    refs: &mut TypedBodyRefs,
) {
    let Some(ty) = module.body_ir.interner.get(object_ty) else {
        return;
    };
    match ty {
        TyKind::TraitObject { trait_id, .. } | TyKind::TraitObjectPointee { trait_id, .. } => {
            refs.traits.insert_vtable_trait(*trait_id);
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
        TypedPatternKind::OptionalSome(pattern)
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
        PlaceBase::Local(_) | PlaceBase::Global(_) | PlaceBase::Error => {}
    }
    for elem in &place.elems {
        match elem {
            PlaceElem::Index(expr) => collect_typed_expr_refs(module, expr, refs),
            PlaceElem::Field(_) | PlaceElem::Error => {}
        }
    }
}

fn extend_reachable_functions_from_traits(
    program_signatures: ExecutableSignatureIndex<'_>,
    extension_methods: &ExtensionMethods,
    reachable_traits: &ReachableTraitRefs,
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
        let Some(trait_signature) = program_signatures.traits.get(trait_def) else {
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
        add_reachable_function(
            method.def_id,
            program_signatures,
            reachable_functions,
            &mut modules,
            pending_modules,
        );
    }
}

fn add_reachable_function(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'_>,
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
            method_name,
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
            traits.insert_method(TraitId::Source(*trait_id), method_name.clone());
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitAssociatedFunction {
            trait_id,
            method_id,
            method_name,
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
            traits.insert_method(TraitId::Source(*trait_id), method_name.clone());
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::DynamicTraitMethod {
            object_ty,
            trait_id,
            method_id,
            method_name,
            trait_args,
            params,
            return_type,
            ..
        } => {
            add_reachable_module(method_id.module_id, modules, pending_modules);
            collect_trait_id_owner_module(*trait_id, modules, pending_modules, traits);
            traits.insert_method(*trait_id, method_name.clone());
            type_ids.push(*object_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(params.iter().copied());
            type_ids.push(*return_type);
        }
        nia_sema_ir::ResolvedCall::BuiltinTraitMethod { trait_id, op } => {
            if let Some(method) = op.method() {
                traits.insert_method(TraitId::Builtin(*trait_id), method.name());
            } else {
                traits.insert_trait(TraitId::Builtin(*trait_id));
            }
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
            traits.insert_method(TraitId::Builtin(*trait_id), method.name());
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
    traits: &mut ReachableTraitRefs,
) {
    let mut pending = tys.into_iter().collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    while let Some(ty_id) = pending.pop_front() {
        add_reachable_module(type_owner(ty_id).module_id(), modules, pending_modules);
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

fn type_owner(ty: InternedTyId) -> nia_ids::TypeOwner {
    ty.owner()
}

fn collect_ty_owner_modules(
    ty: &TyKind,
    type_ids: &mut VecDeque<InternedTyId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut ReachableTraitRefs,
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
    traits: &mut ReachableTraitRefs,
) {
    traits.insert_trait(trait_id);
    if let TraitId::Source(def_id) = trait_id {
        add_reachable_module(def_id.module_id, modules, pending_modules);
    }
}

fn collect_associated_binding_owner_modules(
    bindings: &[AssociatedTypeBindingTy],
    type_ids: &mut VecDeque<InternedTyId>,
    modules: &mut HashSet<ModuleId>,
    pending_modules: &mut VecDeque<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, modules, pending_modules, traits);
        }
        type_ids.extend(binding.trait_args.iter().copied());
        type_ids.push_back(binding.ty);
    }
}
