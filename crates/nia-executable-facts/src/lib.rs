// SPDX-License-Identifier: GPL-3.0-or-later

use nia_body_ir::{
    BodyIr, PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee,
    TypedExpr, TypedExprKind, TypedInlineAsm, TypedMemoryIntrinsicSource, TypedPattern,
    TypedPatternKind, TypedPlace, TypedStmt, TypedStmtKind, TypedSwitchArmBody,
};
use nia_defs::{DefCollection, DefKind};
use nia_ids::{BuiltinTrait, BuiltinTraitMethod, GlobalDefId, InternedTyId, ModuleId, TraitId};
use nia_sema_ir::{GenericInstantiation, ResolvedCall, SemanticFacts};
use nia_static_ir::StaticInit;
use nia_symbol::{SymbolId, known};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy)]
pub struct ReachableModuleInput<'a> {
    pub module_id: ModuleId,
    pub defs: &'a DefCollection,
    pub body_ir: &'a BodyIr,
    pub executable_refs: &'a ExecutableModuleRefs,
    pub semantic_facts: &'a SemanticFacts,
    pub type_lowering: &'a nia_type_lower::TypeLowering,
    pub type_normalization: &'a nia_type_normalize::TypeNormalization,
}

#[derive(Debug, Clone, Default)]
pub struct ExecutableItemRefs {
    pub functions: HashSet<GlobalDefId>,
    pub globals: HashSet<GlobalDefId>,
    pub trait_refs: ExecutableTraitRefs,
    pub generic_instantiations: Vec<GenericInstantiation>,
}

impl ExecutableItemRefs {
    pub fn extend(&mut self, refs: Self) {
        self.functions.extend(refs.functions);
        self.globals.extend(refs.globals);
        self.trait_refs.extend(refs.trait_refs);
        self.generic_instantiations
            .extend(refs.generic_instantiations);
    }

    pub fn extend_ref(&mut self, refs: &Self) {
        self.functions.extend(refs.functions.iter().copied());
        self.globals.extend(refs.globals.iter().copied());
        self.trait_refs.extend_ref(&refs.trait_refs);
        self.generic_instantiations
            .extend(refs.generic_instantiations.iter().cloned());
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExecutableModuleRefs {
    pub functions: HashMap<GlobalDefId, ExecutableItemRefs>,
    pub globals: HashMap<GlobalDefId, ExecutableItemRefs>,
}

impl ExecutableModuleRefs {
    pub fn extend(&mut self, refs: Self) {
        for (def_id, refs) in refs.functions {
            self.functions.entry(def_id).or_default().extend(refs);
        }
        for (def_id, refs) in refs.globals {
            self.globals.entry(def_id).or_default().extend(refs);
        }
    }

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

    pub fn refs_for_function(&self, def_id: GlobalDefId) -> ExecutableItemRefs {
        let mut refs = ExecutableItemRefs::default();
        if let Some(function_refs) = self.functions.get(&def_id) {
            refs.extend_ref(function_refs);
        }
        refs
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExecutableTraitRefs {
    pub traits: HashSet<TraitId>,
    pub methods: Vec<ExecutableTraitMethodRef>,
    pub vtables: Vec<ExecutableTraitVtableRef>,
}

#[derive(Debug, Clone)]
pub struct ExecutableTraitMethodRef {
    pub module_id: ModuleId,
    pub trait_id: TraitId,
    pub method_name: SymbolId,
    pub self_ty: InternedTyId,
    pub trait_args: Vec<InternedTyId>,
}

#[derive(Debug, Clone)]
pub struct ExecutableTraitVtableRef {
    pub module_id: ModuleId,
    pub trait_id: TraitId,
    pub self_ty: InternedTyId,
    pub trait_args: Vec<InternedTyId>,
}

impl ExecutableTraitRefs {
    pub fn extend(&mut self, refs: Self) {
        self.traits.extend(refs.traits);
        self.methods.extend(refs.methods);
        self.vtables.extend(refs.vtables);
    }

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
        self.traits.insert(trait_id);
        self.methods.push(ExecutableTraitMethodRef {
            module_id,
            trait_id,
            method_name,
            self_ty,
            trait_args,
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
        self.vtables.push(ExecutableTraitVtableRef {
            module_id,
            trait_id,
            self_ty,
            trait_args,
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
        BuiltinTraitMethod::Ptr => known::PTR,
        BuiltinTraitMethod::PtrMut => known::PTR_MUT,
        BuiltinTraitMethod::Len => known::LEN,
        BuiltinTraitMethod::Start => known::START,
        BuiltinTraitMethod::End => known::END,
        BuiltinTraitMethod::Char => known::CHAR,
        BuiltinTraitMethod::IterableIter => known::ITER_METHOD,
        BuiltinTraitMethod::IteratorNext => known::NEXT,
    }
}

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
                collect_static_init_refs(init, &mut refs);
            }
        }
    } else {
        for (def_id, init) in &module.body_ir.global_inits {
            if !globals.contains(def_id) {
                continue;
            }
            collect_static_init_refs(init, &mut refs);
        }
    }
    refs
}

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
        collect_static_init_refs(init, &mut global_refs);
        refs.globals.insert(*def_id, global_refs);
    }
    refs
}

pub fn executable_module_refs_from_semantic_facts(
    module: &ReachableModuleInput<'_>,
) -> ExecutableModuleRefs {
    let mut refs = ExecutableModuleRefs::default();
    for (def_id, facts) in &module.semantic_facts.function_facts {
        let function_refs = refs.functions.entry(*def_id).or_default();
        collect_local_static_globals_owned_by_function(module, *def_id, function_refs);
        for call in facts.node_resolved_calls.values() {
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
            function_refs.trait_refs.insert_method(
                reference.module_id,
                reference.trait_id,
                reference.method_name,
                reference.self_ty,
                reference.trait_args.clone(),
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
        ResolvedCall::Method { def_id, args, .. } => {
            refs.functions.insert(*def_id);
            if !args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *def_id,
                    self_arg: None,
                    args: args.clone(),
                    const_args: Vec::new(),
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
            args,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
                *self_ty,
                trait_args.clone(),
            );
            if !args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *method_id,
                    self_arg: Some(*self_ty),
                    args: args.clone(),
                    const_args: Vec::new(),
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
            args,
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
                *self_ty,
                trait_args.clone(),
            );
            if !args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *method_id,
                    self_arg: Some(*self_ty),
                    args: args.clone(),
                    const_args: Vec::new(),
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
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method(
                module.module_id,
                *trait_id,
                *method_name,
                *object_ty,
                trait_args.clone(),
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
        ResolvedCall::BuiltinFunction { .. } | ResolvedCall::FunctionPointer => {}
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
        TypedExprKind::Switch(switch) => {
            collect_typed_expr_refs(module, &switch.target, refs);
            for arm in &switch.arms {
                for pattern in &arm.patterns {
                    collect_typed_switch_pattern_refs(module, pattern, refs);
                }
                match &arm.body {
                    TypedSwitchArmBody::Expr(expr) => collect_typed_expr_refs(module, expr, refs),
                    TypedSwitchArmBody::Stmt(stmt) => collect_typed_stmt_refs(module, stmt, refs),
                    TypedSwitchArmBody::Block(body) => {
                        collect_typed_executable_refs(module, body, refs)
                    }
                }
            }
        }
        TypedExprKind::IfPattern(if_pattern) => {
            collect_typed_expr_refs(module, &if_pattern.target, refs);
            for arm in &if_pattern.arms {
                collect_typed_pattern_refs(module, &arm.pattern, refs);
                collect_typed_executable_refs(module, &arm.body, refs);
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
        | TypedExprKind::ConstGeneric(_)
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
    refs: &mut ExecutableItemRefs,
) {
    match callee {
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
            receiver,
            ..
        } => {
            refs.functions.insert(*def_id);
            if !method_args.is_empty() {
                refs.generic_instantiations.push(GenericInstantiation {
                    def_id: *def_id,
                    self_arg: None,
                    args: method_args.clone(),
                    const_args: Vec::new(),
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
            receiver,
            ..
        } => {
            refs.functions.insert(*method_id);
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
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
            refs.trait_refs.insert_method(
                module.module_id,
                TraitId::Source(*trait_id),
                *method_name,
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
            refs.trait_refs.insert_trait(*trait_id);
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
        TypedCallee::FunctionPointer(receiver) => {
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
    let Some(ty) = module.body_ir.interner.get(object_ty) else {
        return;
    };
    match ty {
        nia_ty::TyKind::TraitObject {
            trait_id,
            trait_args,
            ..
        }
        | nia_ty::TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            ..
        } => {
            refs.trait_refs
                .insert_vtable(module.module_id, *trait_id, self_ty, trait_args.clone());
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
    refs: &mut ExecutableItemRefs,
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
            PlaceElem::Field(_) | PlaceElem::Error => {}
        }
    }
}

fn collect_static_init_refs(init: &StaticInit, refs: &mut ExecutableItemRefs) {
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

fn builtin_method_trait(
    method: nia_body_ir::BuiltinMethod,
) -> Option<(BuiltinTrait, BuiltinTraitMethod)> {
    match method {
        nia_body_ir::BuiltinMethod::Len => Some((BuiltinTrait::Len, BuiltinTraitMethod::Len)),
        nia_body_ir::BuiltinMethod::Start => Some((BuiltinTrait::Start, BuiltinTraitMethod::Start)),
        nia_body_ir::BuiltinMethod::End => Some((BuiltinTrait::End, BuiltinTraitMethod::End)),
        nia_body_ir::BuiltinMethod::Char => Some((BuiltinTrait::Char, BuiltinTraitMethod::Char)),
        nia_body_ir::BuiltinMethod::Iter => {
            Some((BuiltinTrait::Iterable, BuiltinTraitMethod::IterableIter))
        }
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
