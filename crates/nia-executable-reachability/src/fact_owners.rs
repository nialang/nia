// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn add_reachable_type_module(module_id: ModuleId, type_modules: &mut HashSet<ModuleId>) {
    type_modules.insert(module_id);
}

pub(super) fn collect_reachable_fact_owner_modules(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    reachable_functions: &HashSet<GlobalDefId>,
    reachable_globals: &HashSet<GlobalDefId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    collect_reachable_fact_owner_modules_for_items(
        module,
        program_signatures,
        reachable_functions,
        reachable_globals,
        type_modules,
        traits,
    );
}

pub(super) fn collect_reachable_fact_owner_modules_for_items(
    module: &ReachableModuleInput<'_>,
    program_signatures: ExecutableSignatureIndex<'_>,
    functions: &HashSet<GlobalDefId>,
    globals: &HashSet<GlobalDefId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut type_ids = Vec::new();
    for def_id in functions
        .iter()
        .filter(|def_id| def_id.module_id == module.module_id)
    {
        let Some(function_facts) = module.semantic_facts.function_facts.get(def_id) else {
            continue;
        };
        collect_function_fact_owner_modules(
            module.module_id,
            function_facts,
            type_modules,
            traits,
            &mut type_ids,
        );
    }
    for def_id in globals
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
        module.type_store,
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
    module_id: ModuleId,
    facts: &FunctionSemanticFacts,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    type_ids: &mut Vec<InternedTyId>,
) {
    type_ids.extend(facts.local_types.values().copied());
    type_ids.extend(facts.node_expr_types.values().copied());
    for instantiation in &facts.generic_instantiations {
        type_ids.extend(instantiation.args.iter().copied());
        type_ids.extend(instantiation.const_args.iter().map(|arg| arg.ty));
    }
    for coercion in facts.node_pointer_array_to_slice_coercions.values() {
        type_ids.extend([coercion.pointer_ty, coercion.array_ty, coercion.slice_ty]);
    }
    for coercion in facts.node_trait_object_coercions.values() {
        type_ids.extend([coercion.source_ty, coercion.target_ty, coercion.self_ty]);
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
        collect_resolved_call_owner_modules(module_id, call, type_modules, traits, type_ids);
    }
    for reference in facts.node_function_references.values() {
        type_ids.extend(reference.args.iter().copied());
        type_ids.extend(reference.const_args.iter().map(|arg| arg.ty));
    }
    for reference in &facts.trait_method_refs {
        traits.insert_method(
            reference.module_id,
            reference.trait_id,
            reference.method_name,
            reference.self_ty,
            reference.trait_args.clone(),
        );
        type_ids.push(reference.self_ty);
        type_ids.extend(reference.trait_args.iter().copied());
    }
}

fn collect_resolved_call_owner_modules(
    module_id: ModuleId,
    call: &nia_sema_ir::ResolvedCall,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    type_ids: &mut Vec<InternedTyId>,
) {
    match call {
        nia_sema_ir::ResolvedCall::BuiltinFunction { .. } => {}
        nia_sema_ir::ResolvedCall::Function(_) => {}
        nia_sema_ir::ResolvedCall::FunctionInstance {
            args, const_args, ..
        } => {
            type_ids.extend(args.iter().copied());
            type_ids.extend(const_args.iter().map(|arg| arg.ty));
        }
        nia_sema_ir::ResolvedCall::Method { args, .. } => {
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitMethod {
            trait_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            collect_trait_id_owner_module(TraitId::Source(*trait_id), type_modules, traits);
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::TraitAssociatedFunction {
            trait_id,
            self_ty,
            trait_args,
            args,
            ..
        } => {
            collect_trait_id_owner_module(TraitId::Source(*trait_id), type_modules, traits);
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::DynamicTraitMethod {
            object_ty,
            trait_id,
            trait_args,
            params,
            return_type,
            ..
        } => {
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            type_ids.push(*object_ty);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(params.iter().copied());
            type_ids.push(*return_type);
        }
        nia_sema_ir::ResolvedCall::BuiltinTraitMethod {
            trait_id,
            op,
            self_ty,
            trait_args,
        } => {
            traits.insert_trait(TraitId::Builtin(*trait_id));
            if let Some(method) = op.method()
                && let Some(method_name) = builtin_trait_method_symbol(method)
            {
                traits.insert_method(
                    module_id,
                    TraitId::Builtin(*trait_id),
                    method_name,
                    *self_ty,
                    trait_args.clone(),
                );
            }
            type_ids.push(*self_ty);
            type_ids.extend(trait_args.iter().copied());
        }
        nia_sema_ir::ResolvedCall::BuiltinMethod { method, self_ty } => {
            if let Some((trait_id, trait_method)) = semantic_builtin_method_trait(*method)
                && let Some(method_name) = builtin_trait_method_symbol(trait_method)
            {
                traits.insert_method(
                    module_id,
                    TraitId::Builtin(trait_id),
                    method_name,
                    *self_ty,
                    Vec::new(),
                );
            }
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

fn semantic_builtin_method_trait(
    method: nia_sema_ir::BuiltinMethod,
) -> Option<(BuiltinTrait, BuiltinTraitMethod)> {
    match method {
        nia_sema_ir::BuiltinMethod::Len => Some((BuiltinTrait::Len, BuiltinTraitMethod::Len)),
        nia_sema_ir::BuiltinMethod::Start => Some((BuiltinTrait::Start, BuiltinTraitMethod::Start)),
        nia_sema_ir::BuiltinMethod::End => Some((BuiltinTrait::End, BuiltinTraitMethod::End)),
        nia_sema_ir::BuiltinMethod::Char => Some((BuiltinTrait::Char, BuiltinTraitMethod::Char)),
        nia_sema_ir::BuiltinMethod::Iter => {
            Some((BuiltinTrait::Iterable, BuiltinTraitMethod::IterableIter))
        }
    }
}

fn collect_ty_ids_owner_modules<'a>(
    tys: impl IntoIterator<Item = InternedTyId>,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_store: &'a TypeStore,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    let mut pending = tys.into_iter().collect::<VecDeque<_>>();
    let mut seen = HashSet::new();
    while let Some(ty_id) = pending.pop_front() {
        if !seen.insert(ty_id) {
            continue;
        }
        let Some(ty) = type_store.get(ty_id) else {
            continue;
        };
        collect_ty_owner_modules(
            ty,
            type_store,
            program_signatures,
            &mut pending,
            type_modules,
            traits,
            &mut seen,
        );
    }
}

fn collect_ty_owner_modules<'a>(
    ty: &TyKind,
    type_store: &'a TypeStore,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_ids: &mut VecDeque<InternedTyId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<InternedTyId>,
) {
    match ty {
        TyKind::Nominal { def_id, args, .. } => {
            add_reachable_type_module(def_id.module_id, type_modules);
            type_ids.extend(args.iter().copied());
            collect_nominal_signature_owner_type_ids(
                *def_id,
                program_signatures,
                type_store,
                type_modules,
                traits,
                seen,
            );
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
            trait_const_args,
            associated_type_bindings,
            ..
        }
        | TyKind::TraitObjectPointee {
            trait_id,
            trait_args,
            trait_const_args,
            associated_type_bindings,
        } => {
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(trait_const_args.iter().map(|arg| arg.ty));
            collect_associated_binding_owner_modules(
                associated_type_bindings,
                type_ids,
                type_modules,
                traits,
            );
        }
        TyKind::Projection {
            self_ty,
            trait_id,
            trait_args,
            trait_const_args,
            ..
        } => {
            type_ids.push_back(*self_ty);
            collect_trait_id_owner_module(*trait_id, type_modules, traits);
            type_ids.extend(trait_args.iter().copied());
            type_ids.extend(trait_const_args.iter().map(|arg| arg.ty));
        }
        TyKind::BuiltinTrait { args, .. } => type_ids.extend(args.iter().copied()),
        TyKind::Error
        | TyKind::ConstOnly
        | TyKind::Primitive(_)
        | TyKind::BuiltinType(_)
        | TyKind::Vector { .. }
        | TyKind::SelfParam
        | TyKind::GenericParam(_) => {}
    }
}

pub(super) fn builtin_trait_method_symbol(method: BuiltinTraitMethod) -> Option<SymbolId> {
    match method {
        BuiltinTraitMethod::Add => Some(known::ADD),
        BuiltinTraitMethod::Sub => Some(known::SUB),
        BuiltinTraitMethod::Mul => Some(known::MUL),
        BuiltinTraitMethod::Div => Some(known::DIV),
        BuiltinTraitMethod::Rem => Some(known::REM),
        BuiltinTraitMethod::Neg => Some(known::NEG),
        BuiltinTraitMethod::Not => Some(known::LOGICAL_NOT),
        BuiltinTraitMethod::BitNot => Some(known::BIT_NOT),
        BuiltinTraitMethod::BitAnd => Some(known::BIT_AND),
        BuiltinTraitMethod::BitOr => Some(known::BIT_OR),
        BuiltinTraitMethod::BitXor => Some(known::BIT_XOR),
        BuiltinTraitMethod::Shl => Some(known::SHL),
        BuiltinTraitMethod::Shr => Some(known::SHR),
        BuiltinTraitMethod::Eq => Some(known::EQ),
        BuiltinTraitMethod::Ne => Some(known::NE),
        BuiltinTraitMethod::Lt => Some(known::LT),
        BuiltinTraitMethod::Le => Some(known::LE),
        BuiltinTraitMethod::Gt => Some(known::GT),
        BuiltinTraitMethod::Ge => Some(known::GE),
        BuiltinTraitMethod::Deref => Some(known::DEREF),
        BuiltinTraitMethod::DerefMut => Some(known::DEREF_MUT),
        BuiltinTraitMethod::Index => Some(known::INDEX),
        BuiltinTraitMethod::IndexMut => Some(known::INDEX_MUT),
        BuiltinTraitMethod::Slice => Some(known::SLICE),
        BuiltinTraitMethod::SliceMut => Some(known::SLICE_MUT),
        BuiltinTraitMethod::Ptr => Some(known::PTR),
        BuiltinTraitMethod::PtrMut => Some(known::PTR_MUT),
        BuiltinTraitMethod::Len => Some(known::LEN),
        BuiltinTraitMethod::Start => Some(known::START),
        BuiltinTraitMethod::End => Some(known::END),
        BuiltinTraitMethod::Char => Some(known::CHAR),
        BuiltinTraitMethod::IteratorNext => Some(known::NEXT),
        BuiltinTraitMethod::IterableIter => Some(known::ITER_METHOD),
    }
}

fn collect_nominal_signature_owner_type_ids<'a>(
    def_id: GlobalDefId,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_store: &'a TypeStore,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<InternedTyId>,
) {
    if let Some(signature) = (program_signatures.struct_)(def_id) {
        collect_ty_ids_owner_modules_with_store(
            signature.signature.fields.iter().map(|field| field.ty),
            program_signatures,
            type_store,
            type_modules,
            traits,
            seen,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            program_signatures,
            type_store,
            type_modules,
            traits,
            seen,
        );
    }
    if let Some(signature) = (program_signatures.union)(def_id) {
        collect_ty_ids_owner_modules_with_store(
            signature.signature.fields.iter().map(|field| field.ty),
            program_signatures,
            type_store,
            type_modules,
            traits,
            seen,
        );
        collect_owned_where_predicate_type_ids_deque(
            &signature.signature.where_predicates,
            program_signatures,
            type_store,
            type_modules,
            traits,
            seen,
        );
    }
}

fn collect_ty_ids_owner_modules_with_store<'a>(
    tys: impl IntoIterator<Item = InternedTyId>,
    program_signatures: ExecutableSignatureIndex<'a>,
    type_store: &'a TypeStore,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<InternedTyId>,
) {
    let mut pending = tys.into_iter().collect::<VecDeque<_>>();
    while let Some(ty_id) = pending.pop_front() {
        if !seen.insert(ty_id) {
            continue;
        }
        let Some(ty) = type_store.get(ty_id) else {
            continue;
        };
        collect_ty_owner_modules(
            ty,
            type_store,
            program_signatures,
            &mut pending,
            type_modules,
            traits,
            seen,
        );
    }
}

fn collect_owned_where_predicate_type_ids_deque(
    predicates: &[nia_defs::WherePredicateSignature],
    program_signatures: ExecutableSignatureIndex<'_>,
    type_store: &TypeStore,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
    seen: &mut HashSet<InternedTyId>,
) {
    let mut collected = Vec::new();
    collect_where_predicate_type_ids(predicates, &mut collected);
    collect_ty_ids_owner_modules_with_store(
        collected,
        program_signatures,
        type_store,
        type_modules,
        traits,
        seen,
    );
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
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    traits.insert_trait(trait_id);
    if let TraitId::Source(def_id) = trait_id {
        add_reachable_type_module(def_id.module_id, type_modules);
    }
}

fn collect_associated_binding_owner_modules(
    bindings: &[AssociatedTypeBindingTy],
    type_ids: &mut VecDeque<InternedTyId>,
    type_modules: &mut HashSet<ModuleId>,
    traits: &mut ReachableTraitRefs,
) {
    for binding in bindings {
        if let Some(trait_id) = binding.trait_id {
            collect_trait_id_owner_module(trait_id, type_modules, traits);
        }
        type_ids.extend(binding.trait_args.iter().copied());
        type_ids.extend(binding.trait_const_args.iter().map(|arg| arg.ty));
        type_ids.push_back(binding.ty);
    }
}
