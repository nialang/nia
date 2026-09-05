// SPDX-License-Identifier: GPL-3.0-or-later
//! Executable-reference extraction and reachable semantic-fact filtering.
//!
//! This crate converts typed bodies or retained semantic facts into the same
//! per-item dependency schema. Reachability consumes that schema to follow
//! functions, globals, trait dispatch, vtables, and generic instantiations.
//! Typed IR is authoritative after body checking; semantic facts provide the
//! equivalent dependency view for query paths that have not materialized a
//! typed body.

use nia_body_ir::{
    BodyIr, PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee,
    TypedExpr, TypedExprKind, TypedInlineAsm, TypedMatchArmBody, TypedMemoryIntrinsicSource,
    TypedPattern, TypedPatternKind, TypedPlace, TypedStmt, TypedStmtKind,
};
use nia_defs::{DefCollection, DefKind};
use nia_function_ir::FunctionBodyRefs;
use nia_ids::{BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_sema_ir::{GenericInstantiation, ResolvedCall, SemanticFacts};
use nia_static_ir::StaticInit;
use nia_symbol::{SymbolId, known};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy)]
/// Borrowed module products required to extract executable dependencies.
pub struct ReachableModuleInput<'a> {
    /// Session-local identity of the module being inspected.
    pub module_id: ModuleId,
    /// Definition ownership used to discover function-local statics.
    pub defs: &'a DefCollection,
    /// Type store used to decompose trait-object instance identities.
    pub type_store: &'a nia_ty::TypeStore,
    /// Typed function bodies and lowered static initializers.
    pub body_ir: &'a BodyIr,
    /// Previously merged per-item dependency facts.
    pub executable_refs: &'a ExecutableModuleRefs,
    /// Semantic checking facts used by the pre-Body-IR extraction path.
    pub semantic_facts: &'a SemanticFacts,
}

#[derive(Debug, Clone, Default)]
/// Direct executable dependencies collected for one or more items.
pub struct ExecutableItemRefs {
    /// Referenced source function definitions.
    pub functions: HashSet<GlobalDefId>,
    /// Referenced static/global definitions.
    pub globals: HashSet<GlobalDefId>,
    /// Trait declarations, method instances, and vtable instances.
    pub trait_refs: ExecutableTraitRefs,
    /// Concrete generic functions requested by the item.
    pub generic_instantiations: Vec<GenericInstantiation>,
}

impl ExecutableItemRefs {
    /// Moves every dependency from `refs` into this aggregate.
    pub fn extend(&mut self, refs: Self) {
        self.functions.extend(refs.functions);
        self.globals.extend(refs.globals);
        self.trait_refs.extend(refs.trait_refs);
        self.generic_instantiations
            .extend(refs.generic_instantiations);
    }

    /// Clones every dependency from `refs` into this aggregate.
    pub fn extend_ref(&mut self, refs: &Self) {
        self.functions.extend(refs.functions.iter().copied());
        self.globals.extend(refs.globals.iter().copied());
        self.trait_refs.extend_ref(&refs.trait_refs);
        self.generic_instantiations
            .extend(refs.generic_instantiations.iter().cloned());
    }
}

/// Projects the canonical function-body reference aggregate into executable
/// reachability facts.
pub fn executable_item_refs_from_function_body_refs(refs: &FunctionBodyRefs) -> ExecutableItemRefs {
    ExecutableItemRefs {
        functions: refs.functions.iter().copied().collect(),
        globals: refs.globals.iter().copied().collect(),
        generic_instantiations: refs
            .function_instances
            .iter()
            .map(|instance| GenericInstantiation {
                def_id: instance.def_id,
                self_arg: instance.self_arg,
                args: instance.args.clone(),
                const_args: instance.const_args.clone(),
                generics: Vec::new(),
                span: instance.span,
                source_def_id: None,
            })
            .collect(),
        ..ExecutableItemRefs::default()
    }
}

#[derive(Debug, Clone, Default)]
/// Direct dependency facts indexed by their owning function or global.
pub struct ExecutableModuleRefs {
    /// Function dependencies keyed by source function identity.
    pub functions: HashMap<GlobalDefId, ExecutableItemRefs>,
    /// Static-initializer dependencies keyed by global identity.
    pub globals: HashMap<GlobalDefId, ExecutableItemRefs>,
}

impl ExecutableModuleRefs {
    /// Merges another module index, unioning entries with the same owner.
    pub fn extend(&mut self, refs: Self) {
        for (def_id, refs) in refs.functions {
            self.functions.entry(def_id).or_default().extend(refs);
        }
        for (def_id, refs) in refs.globals {
            self.globals.entry(def_id).or_default().extend(refs);
        }
    }

    /// Unions dependencies for the selected function and global owners.
    pub fn refs_for_items(
        &self,
        functions: &HashSet<GlobalDefId>,
        globals: &HashSet<GlobalDefId>,
    ) -> ExecutableItemRefs {
        let mut refs = ExecutableItemRefs::default();
        for def_id in functions {
            if let Some(function_refs) = self.functions.get(def_id) {
                refs.extend_ref(function_refs);
            }
        }
        for def_id in globals {
            if let Some(global_refs) = self.globals.get(def_id) {
                refs.extend_ref(global_refs);
            }
        }
        refs
    }

    /// Returns a cloned dependency aggregate for one function.
    pub fn refs_for_function(&self, def_id: GlobalDefId) -> ExecutableItemRefs {
        let mut refs = ExecutableItemRefs::default();
        if let Some(function_refs) = self.functions.get(&def_id) {
            refs.extend_ref(function_refs);
        }
        refs
    }
}

#[derive(Debug, Clone, Default)]
/// Trait-related executable instances required by selected items.
pub struct ExecutableTraitRefs {
    /// Referenced trait declarations, including builtin traits.
    pub traits: HashSet<TraitId>,
    /// Concrete trait method dispatch instances.
    pub methods: Vec<ExecutableTraitMethodRef>,
    /// Concrete trait-object vtable instances.
    pub vtables: Vec<ExecutableTraitVtableRef>,
}

#[derive(Debug, Clone)]
/// One trait method instance required by executable code.
pub struct ExecutableTraitMethodRef {
    /// Module supplying resolution and generic context.
    pub module_id: ModuleId,
    /// Source or builtin trait containing the method.
    pub trait_id: TraitId,
    /// Stable method name used during implementation lookup.
    pub method_name: SymbolId,
    /// Concrete receiver type.
    pub self_ty: InternedTyId,
    /// Type arguments identifying the trait instance.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments are part of the trait instance identity used by reachability.
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
}

#[derive(Debug, Clone)]
/// One concrete trait-object vtable required by executable code.
pub struct ExecutableTraitVtableRef {
    /// Module supplying resolution and generic context.
    pub module_id: ModuleId,
    /// Source or builtin trait represented by the object.
    pub trait_id: TraitId,
    /// Concrete type stored behind the trait object.
    pub self_ty: InternedTyId,
    /// Type arguments identifying the trait instance.
    pub trait_args: Vec<InternedTyId>,
    /// Const arguments are part of the trait-object instance identity.
    pub trait_const_args: Vec<nia_ty::ConstGenericArg>,
}

impl ExecutableTraitRefs {
    /// Moves all trait dependencies from `refs` into this aggregate.
    pub fn extend(&mut self, refs: Self) {
        self.traits.extend(refs.traits);
        self.methods.extend(refs.methods);
        self.vtables.extend(refs.vtables);
    }

    /// Clones all trait dependencies from `refs` into this aggregate.
    pub fn extend_ref(&mut self, refs: &Self) {
        self.traits.extend(refs.traits.iter().copied());
        self.methods.extend(refs.methods.iter().cloned());
        self.vtables.extend(refs.vtables.iter().cloned());
    }

    fn insert_trait(&mut self, trait_id: TraitId) {
        self.traits.insert(trait_id);
    }

    fn insert_method(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
    ) {
        self.insert_method_with_const_args(
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            Vec::new(),
        );
    }

    fn insert_method_with_const_args(
        &mut self,
        module_id: ModuleId,
        trait_id: TraitId,
        method_name: SymbolId,
        self_ty: InternedTyId,
        trait_args: Vec<InternedTyId>,
        trait_const_args: Vec<nia_ty::ConstGenericArg>,
    ) {
        self.traits.insert(trait_id);
        self.methods.push(ExecutableTraitMethodRef {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
            trait_const_args,
        });
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
        self.vtables.push(ExecutableTraitVtableRef {
            module_id,
            trait_id,
            self_ty,
            trait_args,
            trait_const_args,
        });
    }
}

fn builtin_trait_method_symbol(method: BuiltinTraitMethod) -> SymbolId {
    match method {
        BuiltinTraitMethod::Add => known::ADD,
        BuiltinTraitMethod::Sub => known::SUB,
        BuiltinTraitMethod::Mul => known::MUL,
        BuiltinTraitMethod::Div => known::DIV,
        BuiltinTraitMethod::Rem => known::REM,
        BuiltinTraitMethod::Neg => known::NEG,
        BuiltinTraitMethod::Not => known::LOGICAL_NOT,
        BuiltinTraitMethod::BitNot => known::BIT_NOT,
        BuiltinTraitMethod::BitAnd => known::BIT_AND,
        BuiltinTraitMethod::BitOr => known::BIT_OR,
        BuiltinTraitMethod::BitXor => known::BIT_XOR,
        BuiltinTraitMethod::Shl => known::SHL,
        BuiltinTraitMethod::Shr => known::SHR,
        BuiltinTraitMethod::Eq => known::EQ,
        BuiltinTraitMethod::Ne => known::NE,
        BuiltinTraitMethod::Lt => known::LT,
        BuiltinTraitMethod::Le => known::LE,
        BuiltinTraitMethod::Gt => known::GT,
        BuiltinTraitMethod::Ge => known::GE,
        BuiltinTraitMethod::Deref => known::DEREF,
        BuiltinTraitMethod::DerefMut => known::DEREF_MUT,
        BuiltinTraitMethod::Index => known::INDEX,
        BuiltinTraitMethod::IndexMut => known::INDEX_MUT,
        BuiltinTraitMethod::Slice => known::SLICE,
        BuiltinTraitMethod::SliceMut => known::SLICE_MUT,
        BuiltinTraitMethod::IterableIter => known::ITER_METHOD,
        BuiltinTraitMethod::IteratorNext => known::NEXT,
    }
}

/// Collects direct dependencies for selected typed functions and globals.
///
/// The smaller of each selected set and its owner table drives iteration, so
/// sparse queries avoid scanning an entire module while dense queries avoid
/// repeated hash lookups. Missing selected ids contribute no dependencies.
pub fn executable_refs_for_items(
    module: &ReachableModuleInput<'_>,
    functions: &HashSet<GlobalDefId>,
    globals: &HashSet<GlobalDefId>,
) -> ExecutableItemRefs {
    let mut refs = ExecutableItemRefs::default();
    if functions.len() <= module.body_ir.function_bodies.len() {
        for def_id in functions {
            if let Some(body) = module.body_ir.function_bodies.get(def_id) {
                collect_typed_executable_refs(module, body, &mut refs);
                collect_local_static_globals_owned_by_function(module, *def_id, &mut refs);
            }
        }
    } else {
        for (def_id, body) in &module.body_ir.function_bodies {
            if !functions.contains(def_id) {
                continue;
            }
            collect_typed_executable_refs(module, body, &mut refs);
            collect_local_static_globals_owned_by_function(module, *def_id, &mut refs);
        }
    }
    if globals.len() <= module.body_ir.global_inits.len() {
        for def_id in globals {
            if let Some(init) = module.body_ir.global_inits.get(def_id) {
                collect_static_init_refs(module.module_id, init, &mut refs);
            }
        }
    } else {
        for (def_id, init) in &module.body_ir.global_inits {
            if !globals.contains(def_id) {
                continue;
            }
            collect_static_init_refs(module.module_id, init, &mut refs);
        }
    }
    refs
}

/// Builds a complete per-item dependency index from typed Body IR.
pub fn executable_module_refs_from_typed_ir(
    module: &ReachableModuleInput<'_>,
) -> ExecutableModuleRefs {
    let mut refs = ExecutableModuleRefs::default();
    for (def_id, body) in &module.body_ir.function_bodies {
        let mut function_refs = ExecutableItemRefs::default();
        collect_typed_executable_refs(module, body, &mut function_refs);
        collect_local_static_globals_owned_by_function(module, *def_id, &mut function_refs);
        refs.functions.insert(*def_id, function_refs);
    }
    for (def_id, init) in &module.body_ir.global_inits {
        let mut global_refs = ExecutableItemRefs::default();
        collect_static_init_refs(module.module_id, init, &mut global_refs);
        refs.globals.insert(*def_id, global_refs);
    }
    refs
}

/// Builds a per-function dependency index from semantic checking facts.
///
/// Array-repeat count queries are not executable calls even when represented
/// by a resolved-call fact, so they are deliberately excluded here.
pub fn executable_module_refs_from_semantic_facts(
    module: &ReachableModuleInput<'_>,
) -> ExecutableModuleRefs {
    let mut refs = ExecutableModuleRefs::default();
    for (def_id, facts) in &module.semantic_facts.function_facts {
        let function_refs = refs.functions.entry(*def_id).or_default();
        collect_local_static_globals_owned_by_function(module, *def_id, function_refs);
        function_refs
            .globals
            .extend(facts.global_value_uses.iter().copied());
        function_refs
            .generic_instantiations
            .extend(facts.generic_instantiations.iter().cloned());
        for (key, call) in &facts.node_resolved_calls {
            if facts.node_array_repeat_counts.contains_key(&key) {
                continue;
            }
            collect_resolved_call_refs(module, call, function_refs);
        }
        for reference in facts.node_function_references.values() {
            function_refs.functions.insert(reference.def_id);
            if !reference.args.is_empty() || !reference.const_args.is_empty() {
                function_refs
                    .generic_instantiations
                    .push(GenericInstantiation {
                        def_id: reference.def_id,
                        self_arg: None,
                        args: reference.args.clone(),
                        const_args: reference.const_args.clone(),
                        generics: Vec::new(),
                        span: nia_span::Span::default(),
                        source_def_id: None,
                    });
            }
        }
        for reference in &facts.trait_method_refs {
            function_refs.trait_refs.insert_method_with_const_args(
                reference.module_id,
                reference.trait_id,
                reference.method_name,
                reference.self_ty,
                reference.trait_args.clone(),
                reference.trait_const_args.clone(),
            );
        }
        for coercion in facts.node_trait_object_coercions.values().copied() {
            collect_trait_object_vtable_ref(
                module,
                coercion.target_ty,
                coercion.self_ty,
                function_refs,
            );
        }
    }
    refs
}

fn collect_resolved_call_refs(
    module: &ReachableModuleInput<'_>,
    call: &ResolvedCall,
    refs: &mut ExecutableItemRefs,
) {
    match call {
        ResolvedCall::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        ResolvedCall::FunctionInstance {
            def_id,
            args,
            const_args,
            ..
        } => {
            refs.functions.insert(*def_id);
            refs.generic_instantiations.push(GenericInstantiation {
                def_id: *def_id,
                self_arg: None,
                args: args.clone(),
                const_args: const_args.clone(),
                generics: Vec::new(),
                span: nia_span::Span::default(),
                source_def_id: None,
            });
        }
        ResolvedCall::Method {
            def_id,
            args,
            const_args,
            ..
        } => {
            refs.functions.insert(*def_id);
            if !args.is_empty() || !const_args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *def_id,
                    self_arg: None,
                    args: args.clone(),
                    const_args: const_args.clone(),
                    generics: Vec::new(),
                    span: nia_span::Span::default(),
                    source_def_id: None,
                });
            }
        }
        ResolvedCall::TraitMethod {
            trait_id,
            method_id,
            method_name,
            self_ty,
            trait_args,
            trait_const_args,
            args,
            const_args,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method_with_const_args(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
                *self_ty,
                trait_args.clone(),
                trait_const_args.clone(),
            );
            if !args.is_empty() || !const_args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *method_id,
                    self_arg: Some(*self_ty),
                    args: args.clone(),
                    const_args: const_args.clone(),
                    generics: Vec::new(),
                    span: nia_span::Span::default(),
                    source_def_id: None,
                });
            }
        }
        ResolvedCall::TraitAssociatedFunction {
            trait_id,
            method_id,
            method_name,
            self_ty,
            trait_args,
            trait_const_args,
            args,
            const_args,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method_with_const_args(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
                *self_ty,
                trait_args.clone(),
                trait_const_args.clone(),
            );
            if !args.is_empty() || !const_args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *method_id,
                    self_arg: Some(*self_ty),
                    args: args.clone(),
                    const_args: const_args.clone(),
                    generics: Vec::new(),
                    span: nia_span::Span::default(),
                    source_def_id: None,
                });
            }
        }
        ResolvedCall::DynamicTraitMethod {
            trait_id,
            method_id,
            method_name,
            trait_args,
            object_ty,
            trait_const_args,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method_with_const_args(
                module.module_id,
                *trait_id,
                *method_name,
                *object_ty,
                trait_args.clone(),
                trait_const_args.clone(),
            );
        }
        ResolvedCall::BuiltinTraitMethod {
            trait_id,
            op,
            self_ty,
            trait_args,
        } => {
            refs.trait_refs.insert_trait(TraitId::Builtin(*trait_id));
            if let Some(method) = op.method() {
                refs.trait_refs.insert_method(
                    module.module_id,
                    TraitId::Builtin(*trait_id),
                    builtin_trait_method_symbol(method),
                    *self_ty,
                    trait_args.clone(),
                );
            }
        }
        ResolvedCall::BuiltinMethod { method, self_ty } => {
            if let Some((trait_id, trait_method)) = builtin_method_trait(*method) {
                refs.trait_refs.insert_method(
                    module.module_id,
                    TraitId::Builtin(trait_id),
                    builtin_trait_method_symbol(trait_method),
                    *self_ty,
                    Vec::new(),
                );
            }
        }
        ResolvedCall::BuiltinPlaceMethod {
            trait_id,
            method,
            self_ty,
            trait_args,
        } => {
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Builtin(*trait_id),
                builtin_trait_method_symbol(*method),
                *self_ty,
                trait_args.clone(),
            );
        }
        ResolvedCall::BuiltinFunction { .. }
        | ResolvedCall::Closure
        | ResolvedCall::Callable
        | ResolvedCall::FunctionPointer => {}
    }
}

fn collect_local_static_globals_owned_by_function(
    module: &ReachableModuleInput<'_>,
    function: GlobalDefId,
    refs: &mut ExecutableItemRefs,
) {
    for (def_id, def) in module.defs.defs.iter() {
        if def.kind == DefKind::Global && def.parent == Some(function.def_id) {
            refs.globals.insert(GlobalDefId {
                module_id: module.module_id,
                def_id,
            });
        }
    }
}

fn collect_typed_executable_refs(
    module: &ReachableModuleInput<'_>,
    body: &TypedBody,
    refs: &mut ExecutableItemRefs,
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
    refs: &mut ExecutableItemRefs,
) {
    match &stmt.kind {
        TypedStmtKind::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_typed_expr_refs(module, value, refs);
            }
        }
        TypedStmtKind::PatternBinding(binding) => {
            collect_typed_pattern_refs(module, &binding.pattern, refs);
            collect_typed_expr_refs(module, &binding.value, refs);
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
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Builtin(BuiltinTrait::Iterable),
                builtin_trait_method_symbol(BuiltinTraitMethod::IterableIter),
                for_in.iterable_self_ty,
                Vec::new(),
            );
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Builtin(BuiltinTrait::Iterator),
                builtin_trait_method_symbol(BuiltinTraitMethod::IteratorNext),
                for_in.iterator_ty,
                Vec::new(),
            );
            // Item patterns are evaluated at each successful iteration. Keep
            // their expression/range operands in the reachability graph just
            // like binding, if-pattern, and match patterns; otherwise a
            // referenced function or global could be pruned as unreachable.
            collect_typed_pattern_refs(module, &for_in.pattern, refs);
            collect_typed_expr_refs(module, &for_in.iter, refs);
            collect_typed_executable_refs(module, &for_in.body, refs);
        }
        TypedStmtKind::While(while_loop) => {
            collect_typed_expr_refs(module, &while_loop.cond, refs);
            collect_typed_executable_refs(module, &while_loop.body, refs);
        }
        TypedStmtKind::Loop(loop_body) => {
            collect_typed_executable_refs(module, &loop_body.body, refs)
        }
        TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn collect_typed_expr_refs(
    module: &ReachableModuleInput<'_>,
    expr: &TypedExpr,
    refs: &mut ExecutableItemRefs,
) {
    match &expr.kind {
        TypedExprKind::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        TypedExprKind::FunctionInstance {
            def_id,
            args,
            const_args,
            ..
        } => {
            refs.functions.insert(*def_id);
            refs.generic_instantiations.push(GenericInstantiation {
                def_id: *def_id,
                self_arg: None,
                args: args.clone(),
                const_args: const_args.clone(),
                generics: Vec::new(),
                span: nia_span::Span::default(),
                source_def_id: None,
            });
        }
        TypedExprKind::FunctionCallable { function } => {
            collect_typed_expr_refs(module, function, refs);
        }
        TypedExprKind::Field { lhs, .. } => {
            collect_typed_expr_refs(module, lhs, refs);
        }
        TypedExprKind::TupleField { lhs, .. } => collect_typed_expr_refs(module, lhs, refs),
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
        | TypedExprKind::Try { expr: value, .. }
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
        TypedExprKind::CallableCoercion { state, .. } => {
            collect_typed_expr_refs(module, state, refs);
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
        TypedExprKind::Tuple(elems) => {
            for elem in elems {
                collect_typed_expr_refs(module, elem, refs);
            }
        }
        TypedExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_typed_expr_refs(module, &capture.value, refs);
            }
            collect_typed_executable_refs(module, body, refs);
        }
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
        TypedExprKind::Block(body) => collect_typed_executable_refs(module, body, refs),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_typed_expr_refs(module, cond, refs);
            collect_typed_executable_refs(module, then_branch, refs);
            if let Some(else_branch) = else_branch.as_deref() {
                collect_typed_expr_refs(module, else_branch, refs);
            }
        }
        TypedExprKind::Match(matched) => {
            collect_typed_expr_refs(module, &matched.target, refs);
            for arm in &matched.arms {
                for pattern in &arm.patterns {
                    collect_typed_pattern_refs(module, pattern, refs);
                }
                match &arm.body {
                    TypedMatchArmBody::Expr(expr) => collect_typed_expr_refs(module, expr, refs),
                    TypedMatchArmBody::Stmt(stmt) => collect_typed_stmt_refs(module, stmt, refs),
                    TypedMatchArmBody::Block(body) => {
                        collect_typed_executable_refs(module, body, refs)
                    }
                }
            }
        }
        TypedExprKind::IfPattern(if_pattern) => {
            collect_typed_expr_refs(module, &if_pattern.target, refs);
            collect_typed_pattern_refs(module, &if_pattern.pattern, refs);
            collect_typed_executable_refs(module, &if_pattern.then_branch, refs);
            if let Some(else_branch) = if_pattern.else_branch.as_deref() {
                collect_typed_expr_refs(module, else_branch, refs);
            }
        }
        TypedExprKind::IfPatternChain(chain) => {
            for clause in &chain.clauses {
                match clause {
                    nia_body_ir::TypedIfPatternClause::Pattern { target, pattern } => {
                        collect_typed_expr_refs(module, target, refs);
                        collect_typed_pattern_refs(module, pattern, refs);
                    }
                    nia_body_ir::TypedIfPatternClause::Condition(condition) => {
                        collect_typed_expr_refs(module, condition, refs)
                    }
                }
            }
            collect_typed_executable_refs(module, &chain.then_branch, refs);
            if let Some(else_branch) = chain.else_branch.as_deref() {
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
        | TypedExprKind::ConstGeneric(_)
        | TypedExprKind::Local(_)
        | TypedExprKind::EnumConstructor(_)
        | TypedExprKind::ClosureFunctionPointer { .. } => {}
        TypedExprKind::UnionStorageLiteral { relocations, .. } => {
            for relocation in relocations {
                collect_typed_expr_refs(module, &relocation.pointee, refs);
            }
        }
        TypedExprKind::Global(def_id) => {
            refs.globals.insert(*def_id);
        }
        TypedExprKind::EnumVariant { fields, .. } => {
            for field in fields {
                collect_typed_expr_refs(module, field, refs);
            }
        }
        TypedExprKind::BuiltinValue(_) | TypedExprKind::CallerLocation(_) | TypedExprKind::Trap => {
        }
    }
}

fn collect_typed_callee_refs(
    module: &ReachableModuleInput<'_>,
    callee: &TypedCallee,
    args: &[TypedExpr],
    refs: &mut ExecutableItemRefs,
) {
    match callee {
        TypedCallee::Tracked { callee, .. } => {
            collect_typed_callee_refs(module, callee, args, refs)
        }
        TypedCallee::Closure(callee) => collect_typed_expr_refs(module, callee, refs),
        TypedCallee::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        TypedCallee::FunctionInstance {
            def_id,
            args,
            const_args,
            ..
        } => {
            refs.functions.insert(*def_id);
            refs.generic_instantiations.push(GenericInstantiation {
                def_id: *def_id,
                self_arg: None,
                args: args.clone(),
                const_args: const_args.clone(),
                generics: Vec::new(),
                span: nia_span::Span::default(),
                source_def_id: None,
            });
        }
        TypedCallee::Method {
            def_id,
            args: method_args,
            const_args: method_const_args,
            receiver,
            ..
        } => {
            refs.functions.insert(*def_id);
            if !method_args.is_empty() || !method_const_args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *def_id,
                    self_arg: None,
                    args: method_args.clone(),
                    const_args: method_const_args.clone(),
                    generics: Vec::new(),
                    span: nia_span::Span::default(),
                    source_def_id: None,
                });
            }
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::TraitMethod {
            trait_id,
            method_id,
            method_name,
            self_ty,
            trait_args,
            trait_const_args,
            args: method_args,
            const_args: method_const_args,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method_with_const_args(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
                *self_ty,
                trait_args.clone(),
                trait_const_args.clone(),
            );
            if !method_args.is_empty() || !method_const_args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *method_id,
                    self_arg: Some(*self_ty),
                    args: method_args.clone(),
                    const_args: method_const_args.clone(),
                    generics: Vec::new(),
                    span: nia_span::Span::default(),
                    source_def_id: None,
                });
            }
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::TraitAssociatedFunction {
            trait_id,
            method_id,
            method_name,
            self_ty,
            trait_args,
            trait_const_args,
            args: method_args,
            const_args: method_const_args,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method_with_const_args(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
                *self_ty,
                trait_args.clone(),
                trait_const_args.clone(),
            );
            if !method_args.is_empty() || !method_const_args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *method_id,
                    self_arg: Some(*self_ty),
                    args: method_args.clone(),
                    const_args: method_const_args.clone(),
                    generics: Vec::new(),
                    span: nia_span::Span::default(),
                    source_def_id: None,
                });
            }
        }
        TypedCallee::DynamicTraitMethod {
            trait_id,
            method_id,
            method_name,
            object_ty,
            trait_args,
            trait_const_args,
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method_with_const_args(
                module.module_id,
                *trait_id,
                *method_name,
                *object_ty,
                trait_args.clone(),
                trait_const_args.clone(),
            );
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::BuiltinMethod {
            method,
            self_ty,
            receiver,
        } => {
            if let Some((trait_id, trait_method)) = builtin_method_trait(*method) {
                refs.trait_refs.insert_method(
                    module.module_id,
                    TraitId::Builtin(trait_id),
                    builtin_trait_method_symbol(trait_method),
                    *self_ty,
                    Vec::new(),
                );
            }
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::Callable(receiver) | TypedCallee::FunctionPointer(receiver) => {
            collect_typed_expr_refs(module, receiver, refs);
        }
        TypedCallee::BuiltinOperator(operator) => {
            if let Some(method) = operator.method() {
                if let Some(receiver) = args.first() {
                    refs.trait_refs.insert_method(
                        module.module_id,
                        TraitId::Builtin(operator.trait_id),
                        builtin_trait_method_symbol(method),
                        receiver.ty,
                        Vec::new(),
                    );
                } else {
                    refs.trait_refs
                        .insert_trait(TraitId::Builtin(operator.trait_id));
                }
            } else {
                refs.trait_refs
                    .insert_trait(TraitId::Builtin(operator.trait_id));
            }
        }
        TypedCallee::BuiltinPlaceMethod(method) => {
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Builtin(method.trait_id),
                builtin_trait_method_symbol(method.method),
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
    refs: &mut ExecutableItemRefs,
) {
    let Some(ty) = module.type_store.get(object_ty) else {
        return;
    };
    match ty {
        nia_ty::TyKind::TraitObject {
            trait_id,
            trait_args,
            trait_const_args,
            ..
        }
        | nia_ty::TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            ..
        } => {
            refs.trait_refs.insert_vtable_with_const_args(
                module.module_id,
                *trait_id,
                self_ty,
                trait_args.clone(),
                trait_const_args.clone(),
            );
        }
        _ => {}
    }
}

fn collect_typed_pattern_refs(
    module: &ReachableModuleInput<'_>,
    pattern: &TypedPattern,
    refs: &mut ExecutableItemRefs,
) {
    match &pattern.kind {
        TypedPatternKind::Pointer(pattern)
        | TypedPatternKind::MutPointer(pattern)
        | TypedPatternKind::OptionalSome(pattern)
        | TypedPatternKind::ErrorOk(pattern)
        | TypedPatternKind::ErrorErr(pattern) => collect_typed_pattern_refs(module, pattern, refs),
        TypedPatternKind::Nominal { fields, .. } => {
            for field in fields {
                collect_typed_pattern_refs(module, field, refs);
            }
        }
        TypedPatternKind::Tuple(patterns) => {
            for pattern in patterns {
                collect_typed_pattern_refs(module, pattern, refs);
            }
        }
        TypedPatternKind::Expr(expr) => collect_typed_expr_refs(module, expr, refs),
        TypedPatternKind::Range { start, end, .. } => {
            collect_typed_expr_refs(module, start, refs);
            collect_typed_expr_refs(module, end, refs);
        }
        TypedPatternKind::Wildcard
        | TypedPatternKind::Bind { .. }
        | TypedPatternKind::OptionalNull
        | TypedPatternKind::CheckedInt { .. }
        | TypedPatternKind::CheckedIntRange { .. } => {}
    }
}

fn collect_typed_atomic_refs(
    module: &ReachableModuleInput<'_>,
    atomic: &TypedAtomic,
    refs: &mut ExecutableItemRefs,
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
    refs: &mut ExecutableItemRefs,
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
    refs: &mut ExecutableItemRefs,
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
            PlaceElem::Field(_) | PlaceElem::TupleField(_) | PlaceElem::Error => {}
        }
    }
}

fn collect_static_init_refs(module_id: ModuleId, init: &StaticInit, refs: &mut ExecutableItemRefs) {
    let init_refs = init.value_refs(module_id);
    refs.extend(executable_item_refs_from_function_body_refs(&init_refs));
}

fn builtin_method_trait(
    method: nia_body_ir::BuiltinMethod,
) -> Option<(BuiltinTrait, BuiltinTraitMethod)> {
    match method {
        nia_body_ir::BuiltinMethod::SliceLen
        | nia_body_ir::BuiltinMethod::SlicePtr
        | nia_body_ir::BuiltinMethod::SlicePtrMut
        | nia_body_ir::BuiltinMethod::Start
        | nia_body_ir::BuiltinMethod::End => None,
        nia_body_ir::BuiltinMethod::Iter => {
            Some((BuiltinTrait::Iterable, BuiltinTraitMethod::IterableIter))
        }
    }
}

/// Retains semantic facts owned by reachable functions.
///
/// This compatibility entry point preserves all global facts.
pub fn filter_semantic_facts_for_reachable_functions(
    facts: SemanticFacts,
    reachable_functions: &HashSet<GlobalDefId>,
) -> SemanticFacts {
    let reachable_globals = facts.global_types.keys().copied().collect::<HashSet<_>>();
    filter_semantic_facts_for_reachable_items(facts, reachable_functions, &reachable_globals)
}

/// Retains owner-indexed semantic facts for reachable executable items.
///
/// Module-wide node facts are preserved because their owner cannot be
/// reconstructed from the node key alone. Function and global maps, whose
/// ownership is explicit, are filtered to the supplied sets.
pub fn filter_semantic_facts_for_reachable_items(
    facts: SemanticFacts,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
) -> SemanticFacts {
    let node_store = facts.node_store().clone();
    let mut reachable_facts = SemanticFacts::with_node_store(&node_store);
    reachable_facts.global_types = facts
        .global_types
        .into_iter()
        .filter(|(def_id, _)| reachable_globals.contains(def_id))
        .collect();
    reachable_facts.const_types = facts.const_types;
    reachable_facts.generic_instantiations.extend(
        facts
            .generic_instantiations
            .into_iter()
            .filter(|instantiation| instantiation.source_def_id.is_none()),
    );
    reachable_facts.node_builtin_associated_values = facts.node_builtin_associated_values;
    reachable_facts.node_expr_types = facts.node_expr_types;
    reachable_facts.node_bracket_suffix_resolutions = facts.node_bracket_suffix_resolutions;
    reachable_facts.node_pointer_array_to_slice_coercions =
        facts.node_pointer_array_to_slice_coercions;
    reachable_facts.node_trait_object_coercions = facts.node_trait_object_coercions;
    reachable_facts.node_trait_object_upcasts = facts.node_trait_object_upcasts;
    reachable_facts.node_builtin_values = facts.node_builtin_values;
    reachable_facts.node_associated_const_projections = facts.node_associated_const_projections;
    reachable_facts.node_array_repeat_counts = facts.node_array_repeat_counts;
    reachable_facts.node_pattern_values = facts.node_pattern_values;
    reachable_facts.node_resolved_calls = facts.node_resolved_calls;
    reachable_facts.node_function_references = facts.node_function_references;
    reachable_facts.function_facts = facts
        .function_facts
        .into_iter()
        .filter(|(def_id, _)| reachable_functions.contains(def_id))
        .collect();
    reachable_facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_body_ir::{
        AtomicOrder, MemoryIntrinsicOp, PlaceBase, TypedAsmInput, TypedAsmOutput, TypedAtomic,
        TypedForIn, TypedInlineAsm, TypedMemoryIntrinsic, TypedMemoryIntrinsicSource, TypedPattern,
        TypedPatternKind, TypedPlace, TypedUnionRelocation,
    };
    use nia_ids::{DefId, ModuleIdAllocator, ReceiverKind};
    use nia_span::Span;

    #[test]
    fn for_item_pattern_function_references_are_reachable() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = nia_ty::TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::Bool);
        let referenced = GlobalDefId {
            module_id,
            def_id: DefId(7),
        };
        let defs = nia_defs::DefCollection {
            module_id,
            defs: Default::default(),
            module_scope: Default::default(),
            scopes: nia_defs::DefScopes {
                struct_members: HashMap::new(),
                union_members: HashMap::new(),
                enum_members: HashMap::new(),
            },
            def_nodes: Default::default(),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
        };
        let body_ir = BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        };
        let executable_refs = ExecutableModuleRefs::default();
        let semantic_facts = nia_sema_ir::SemanticFacts::default();
        let module = ReachableModuleInput {
            module_id,
            defs: &defs,
            type_store: &types,
            body_ir: &body_ir,
            executable_refs: &executable_refs,
            semantic_facts: &semantic_facts,
        };
        let stmt = TypedStmt {
            span: Span::default(),
            kind: TypedStmtKind::ForIn(Box::new(TypedForIn {
                pattern: TypedPattern {
                    ty,
                    span: Span::default(),
                    kind: TypedPatternKind::Expr(Box::new(TypedExpr {
                        span: Span::default(),
                        ty,
                        kind: TypedExprKind::Function(referenced),
                    })),
                },
                item_ty: ty,
                bool_ty: ty,
                iterable_self_ty: ty,
                iterator_ty: ty,
                iter: TypedExpr {
                    span: Span::default(),
                    ty,
                    kind: TypedExprKind::Bool(true),
                },
                body: TypedBody {
                    span: Span::default(),
                    locals: Vec::new(),
                    stmts: Vec::new(),
                    tail: None,
                    ty,
                },
            })),
        };
        let mut refs = ExecutableItemRefs::default();

        collect_typed_stmt_refs(&module, &stmt, &mut refs);

        assert!(refs.functions.contains(&referenced));
    }

    #[test]
    fn typed_function_instance_values_retain_generic_identity() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = nia_ty::TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::I32);
        let function = GlobalDefId {
            module_id,
            def_id: DefId(8),
        };
        let const_arg = nia_ty::ConstGenericArg {
            ty,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(4_u8.into())),
        };
        let defs = nia_defs::DefCollection {
            module_id,
            defs: Default::default(),
            module_scope: Default::default(),
            scopes: nia_defs::DefScopes {
                struct_members: HashMap::new(),
                union_members: HashMap::new(),
                enum_members: HashMap::new(),
            },
            def_nodes: Default::default(),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
        };
        let body_ir = BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        };
        let executable_refs = ExecutableModuleRefs::default();
        let semantic_facts = nia_sema_ir::SemanticFacts::default();
        let module = ReachableModuleInput {
            module_id,
            defs: &defs,
            type_store: &types,
            body_ir: &body_ir,
            executable_refs: &executable_refs,
            semantic_facts: &semantic_facts,
        };
        let span = Span::new(3, 9);
        let stmt = TypedStmt {
            span,
            kind: TypedStmtKind::Expr(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::FunctionInstance {
                    def_id: function,
                    arg_module_id: module_id,
                    args: vec![ty],
                    const_args: vec![const_arg.clone()],
                },
            }),
        };
        let mut refs = ExecutableItemRefs::default();

        collect_typed_stmt_refs(&module, &stmt, &mut refs);

        assert!(refs.functions.contains(&function));
        assert_eq!(
            refs.generic_instantiations,
            vec![GenericInstantiation {
                def_id: function,
                self_arg: None,
                args: vec![ty],
                const_args: vec![const_arg],
                generics: Vec::new(),
                span: Span::default(),
                source_def_id: None,
            }]
        );
    }

    #[test]
    fn typed_method_callees_retain_method_const_arguments() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = nia_ty::TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::I32);
        let method = GlobalDefId {
            module_id,
            def_id: DefId(11),
        };
        let const_arg = nia_ty::ConstGenericArg {
            ty,
            value: nia_ty::ConstGenericValue::Int(nia_ty::IntConst::unsigned(4_u8.into())),
        };
        let defs = nia_defs::DefCollection {
            module_id,
            defs: Default::default(),
            module_scope: Default::default(),
            scopes: nia_defs::DefScopes {
                struct_members: HashMap::new(),
                union_members: HashMap::new(),
                enum_members: HashMap::new(),
            },
            def_nodes: Default::default(),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
        };
        let body_ir = BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        };
        let executable_refs = ExecutableModuleRefs::default();
        let semantic_facts = nia_sema_ir::SemanticFacts::default();
        let module = ReachableModuleInput {
            module_id,
            defs: &defs,
            type_store: &types,
            body_ir: &body_ir,
            executable_refs: &executable_refs,
            semantic_facts: &semantic_facts,
        };
        let receiver = TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Bool(true),
        };
        let callee = TypedCallee::Method {
            def_id: method,
            args: vec![ty],
            const_args: vec![const_arg.clone()],
            receiver_kind: ReceiverKind::Value,
            receiver: Box::new(receiver),
        };
        let mut refs = ExecutableItemRefs::default();

        collect_typed_callee_refs(&module, &callee, &[], &mut refs);

        assert_eq!(refs.generic_instantiations.len(), 1);
        assert_eq!(refs.generic_instantiations[0].def_id, method);
        assert_eq!(refs.generic_instantiations[0].args, vec![ty]);
        assert_eq!(
            refs.generic_instantiations[0].const_args,
            vec![const_arg.clone()]
        );

        let trait_id = GlobalDefId {
            module_id,
            def_id: DefId(12),
        };
        let trait_callee = TypedCallee::TraitMethod {
            trait_id,
            method_id: method,
            implementation_method: None,
            method_name: nia_symbol::known::ADD,
            self_ty: ty,
            trait_args: vec![ty],
            trait_const_args: vec![const_arg.clone()],
            args: vec![ty],
            const_args: vec![const_arg.clone()],
            receiver_kind: ReceiverKind::Value,
            receiver: Box::new(TypedExpr {
                span: Span::default(),
                ty,
                kind: TypedExprKind::Bool(true),
            }),
        };
        collect_typed_callee_refs(&module, &trait_callee, &[], &mut refs);

        let trait_instance = refs
            .generic_instantiations
            .iter()
            .find(|instance| instance.self_arg == Some(ty))
            .expect("trait method generic instance");
        assert_eq!(trait_instance.def_id, method);
        assert_eq!(trait_instance.args, vec![ty]);
        assert_eq!(trait_instance.const_args, vec![const_arg.clone()]);

        let associated_callee = TypedCallee::TraitAssociatedFunction {
            trait_id,
            method_id: method,
            method_name: nia_symbol::known::ADD,
            self_ty: ty,
            trait_args: vec![ty],
            trait_const_args: vec![const_arg.clone()],
            args: vec![ty],
            const_args: vec![const_arg.clone()],
        };
        collect_typed_callee_refs(&module, &associated_callee, &[], &mut refs);
        assert_eq!(
            refs.generic_instantiations
                .iter()
                .filter(|instance| instance.self_arg == Some(ty))
                .count(),
            2
        );
    }

    #[test]
    fn static_function_instance_values_retain_generic_identity() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = nia_ty::TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::I32);
        let function = GlobalDefId {
            module_id,
            def_id: DefId(9),
        };
        let global = GlobalDefId {
            module_id,
            def_id: DefId(10),
        };
        let defs = nia_defs::DefCollection {
            module_id,
            defs: Default::default(),
            module_scope: Default::default(),
            scopes: nia_defs::DefScopes {
                struct_members: HashMap::new(),
                union_members: HashMap::new(),
                enum_members: HashMap::new(),
            },
            def_nodes: Default::default(),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
        };
        let body_ir = BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::from([(
                global,
                std::sync::Arc::new(StaticInit::AddrOfFunction {
                    function,
                    args: vec![ty],
                    const_args: Vec::new(),
                }),
            )]),
        };
        let executable_refs = ExecutableModuleRefs::default();
        let semantic_facts = nia_sema_ir::SemanticFacts::default();
        let module = ReachableModuleInput {
            module_id,
            defs: &defs,
            type_store: &types,
            body_ir: &body_ir,
            executable_refs: &executable_refs,
            semantic_facts: &semantic_facts,
        };
        let refs = executable_refs_for_items(&module, &HashSet::new(), &HashSet::from([global]));

        assert!(refs.functions.is_empty());
        assert_eq!(refs.generic_instantiations.len(), 1);
        assert_eq!(refs.generic_instantiations[0].def_id, function);
        assert_eq!(refs.generic_instantiations[0].args, vec![ty]);
    }

    #[test]
    fn semantic_fact_filter_keeps_only_requested_function_and_global_owners() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = nia_ty::TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::Bool);
        let reachable_function = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let unreachable_function = GlobalDefId {
            module_id,
            def_id: DefId(2),
        };
        let reachable_global = GlobalDefId {
            module_id,
            def_id: DefId(3),
        };
        let unreachable_global = GlobalDefId {
            module_id,
            def_id: DefId(4),
        };
        let mut facts = SemanticFacts::default();
        facts.global_types.insert(reachable_global, ty);
        facts.global_types.insert(unreachable_global, ty);
        facts.function_facts.insert(
            reachable_function,
            nia_sema_ir::FunctionSemanticFacts::default(),
        );
        facts.function_facts.insert(
            unreachable_function,
            nia_sema_ir::FunctionSemanticFacts::default(),
        );
        let reachable_functions = HashSet::from([reachable_function]);
        let reachable_globals = HashSet::from([reachable_global]);

        let filtered = filter_semantic_facts_for_reachable_items(
            facts,
            &reachable_functions,
            &reachable_globals,
        );

        assert_eq!(
            filtered
                .function_facts
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            reachable_functions
        );
        assert_eq!(
            filtered
                .global_types
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            reachable_globals
        );
    }

    #[test]
    fn typed_reference_collection_covers_hidden_expression_containers() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = nia_ty::TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(nia_ty::PrimitiveTy::Bool);
        let function = GlobalDefId {
            module_id,
            def_id: DefId(10),
        };
        let globals = (20..26)
            .map(|id| GlobalDefId {
                module_id,
                def_id: DefId(id),
            })
            .collect::<Vec<_>>();
        let defs = nia_defs::DefCollection {
            module_id,
            defs: Default::default(),
            module_scope: Default::default(),
            scopes: nia_defs::DefScopes {
                struct_members: HashMap::new(),
                union_members: HashMap::new(),
                enum_members: HashMap::new(),
            },
            def_nodes: Default::default(),
            module_usings: Vec::new(),
            diagnostics: Vec::new(),
        };
        let body_ir = BodyIr {
            function_bodies: HashMap::new(),
            global_inits: HashMap::new(),
        };
        let executable_refs = ExecutableModuleRefs::default();
        let semantic_facts = nia_sema_ir::SemanticFacts::default();
        let module = ReachableModuleInput {
            module_id,
            defs: &defs,
            type_store: &types,
            body_ir: &body_ir,
            executable_refs: &executable_refs,
            semantic_facts: &semantic_facts,
        };
        let global_expr = |def_id| TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Global(def_id),
        };
        let place = |def_id| TypedPlace {
            span: Span::default(),
            ty,
            base: PlaceBase::Global(def_id),
            elems: Vec::new(),
        };
        let expr = TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Tuple(vec![
                TypedExpr {
                    span: Span::default(),
                    ty,
                    kind: TypedExprKind::Assign {
                        place: place(globals[0]),
                        op: nia_ast::AssignOp::Assign,
                        rhs: Box::new(global_expr(globals[1])),
                    },
                },
                TypedExpr {
                    span: Span::default(),
                    ty,
                    kind: TypedExprKind::Call {
                        callee: TypedCallee::FunctionPointer(Box::new(TypedExpr {
                            span: Span::default(),
                            ty,
                            kind: TypedExprKind::Function(function),
                        })),
                        args: Vec::new(),
                    },
                },
                TypedExpr {
                    span: Span::default(),
                    ty,
                    kind: TypedExprKind::InlineAsm(TypedInlineAsm {
                        code: String::new(),
                        inputs: vec![TypedAsmInput {
                            constraint: String::new(),
                            value: global_expr(globals[2]),
                            span: Span::default(),
                        }],
                        outputs: vec![TypedAsmOutput {
                            constraint: String::new(),
                            place: place(globals[3]),
                            span: Span::default(),
                        }],
                        clobbers: Vec::new(),
                        options: Vec::new(),
                    }),
                },
                TypedExpr {
                    span: Span::default(),
                    ty,
                    kind: TypedExprKind::UnionStorageLiteral {
                        bytes: Vec::new(),
                        relocations: vec![TypedUnionRelocation {
                            offset: 0,
                            width: 0,
                            allocation: nia_body_ir::PromotedAllocationId::new(
                                module_id,
                                Span::default(),
                            ),
                            pointee: Box::new(global_expr(globals[4])),
                        }],
                    },
                },
                TypedExpr {
                    span: Span::default(),
                    ty,
                    kind: TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                        op: MemoryIntrinsicOp::Copy,
                        elem_ty: ty,
                        dest: Box::new(global_expr(globals[5])),
                        source: TypedMemoryIntrinsicSource::Byte(Box::new(global_expr(globals[0]))),
                    }),
                },
                TypedExpr {
                    span: Span::default(),
                    ty,
                    kind: TypedExprKind::Atomic(TypedAtomic::Load {
                        ty,
                        ptr: Box::new(global_expr(globals[1])),
                        order: AtomicOrder::Acquire,
                    }),
                },
            ]),
        };
        let stmt = TypedStmt {
            span: Span::default(),
            kind: TypedStmtKind::Expr(expr),
        };
        let mut refs = ExecutableItemRefs::default();

        collect_typed_stmt_refs(&module, &stmt, &mut refs);

        assert!(refs.functions.contains(&function));
        assert!((0..6).all(|index| { refs.globals.contains(&globals[index]) }));
    }
}
