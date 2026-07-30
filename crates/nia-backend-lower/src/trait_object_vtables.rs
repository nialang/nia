// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::ModuleLowerer;
use nia_backend_ir::{
    BackendTraitObjectVtable, BackendTraitObjectVtableEntry, BackendTraitObjectVtableFunction,
    BackendTraitObjectVtableKey,
};
use nia_function_ir::{
    FunctionBody, FunctionCallee, FunctionDeferBody, FunctionExpr, FunctionExprKind,
    FunctionForHeader, FunctionMemoryIntrinsicSource, FunctionOp, FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_ty::TyKind;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn collect_trait_object_vtables_from_concrete_body(
        &mut self,
        body: &FunctionBody,
    ) -> Vec<BackendTraitObjectVtable> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.collect_trait_object_vtables_from_body(body, &mut out, &mut seen);
        out
    }

    fn collect_trait_object_vtables_from_body(
        &mut self,
        body: &FunctionBody,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        self.collect_trait_object_vtables_from_blocks(&body.blocks, out, seen);
    }

    fn collect_trait_object_vtables_from_defer_body(
        &mut self,
        body: &FunctionDeferBody,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        self.collect_trait_object_vtables_from_blocks(&body.blocks, out, seen);
    }

    fn collect_trait_object_vtables_from_blocks(
        &mut self,
        blocks: &[nia_function_ir::FunctionBlock],
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        for block in blocks {
            self.collect_trait_object_vtables_from_ops(&block.ops, out, seen);
            self.collect_trait_object_vtables_from_terminator(&block.terminator, out, seen);
        }
    }

    fn collect_trait_object_vtables_from_ops(
        &mut self,
        ops: &[FunctionOp],
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        for op in ops {
            self.collect_trait_object_vtables_from_op(op, out, seen);
        }
    }

    fn collect_trait_object_vtables_from_op(
        &mut self,
        op: &FunctionOp,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &binding.value {
                    self.collect_trait_object_vtables_from_expr(value, out, seen);
                }
            }
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.collect_trait_object_vtables_from_expr(value, out, seen);
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                self.collect_trait_object_vtables_from_expr(&memory.dest, out, seen);
                match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => {
                        self.collect_trait_object_vtables_from_expr(source, out, seen);
                    }
                }
            }
            FunctionOp::Defer(defer) => {
                self.collect_trait_object_vtables_from_defer_body(defer, out, seen);
            }
        }
    }

    fn collect_trait_object_vtables_from_terminator(
        &mut self,
        terminator: &FunctionTerminator,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        match terminator {
            FunctionTerminator::If { cond, .. } => {
                self.collect_trait_object_vtables_from_expr(cond, out, seen);
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                self.collect_trait_object_vtables_from_expr(target, out, seen);
                for arm in arms {
                    self.collect_trait_object_vtables_from_expr(&arm.pattern, out, seen);
                }
            }
            FunctionTerminator::Try { value, .. } => {
                self.collect_trait_object_vtables_from_expr(value, out, seen);
            }
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Condition(cond) => {
                    self.collect_trait_object_vtables_from_expr(cond, out, seen);
                }
                FunctionForHeader::Infinite => {}
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.collect_trait_object_vtables_from_expr(value, out, seen);
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
    }

    fn collect_trait_object_vtables_from_expr(
        &mut self,
        expr: &FunctionExpr,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        match &expr.kind {
            FunctionExprKind::TraitObjectCoercion {
                expr: inner,
                target_ty,
                self_ty,
            } => {
                self.collect_trait_object_vtables_from_expr(inner, out, seen);
                let key = BackendTraitObjectVtableKey {
                    self_ty: *self_ty,
                    object_ty: *target_ty,
                };
                if seen.insert(key.clone()) {
                    let cached = self.cached_trait_object_vtable(key, expr.span);
                    if let Some(vtable) = cached {
                        out.push(vtable);
                    }
                }
            }
            FunctionExprKind::Discard(inner)
            | FunctionExprKind::Cast { expr: inner, .. }
            | FunctionExprKind::TraitObjectUpcast { expr: inner, .. }
            | FunctionExprKind::RangeBound { range: inner, .. }
            | FunctionExprKind::StaticArrayPointer { array: inner, .. }
            | FunctionExprKind::OptionalSome { expr: inner }
            | FunctionExprKind::ErrorOk { expr: inner }
            | FunctionExprKind::ErrorErr { expr: inner }
            | FunctionExprKind::TaggedUnionTag { expr: inner }
            | FunctionExprKind::TaggedUnionPayload { expr: inner }
            | FunctionExprKind::Try { expr: inner }
            | FunctionExprKind::LoadUnaligned { ptr: inner, .. }
            | FunctionExprKind::Splat { value: inner }
            | FunctionExprKind::Bitmask { vector: inner }
            | FunctionExprKind::BitIntrinsic { value: inner, .. }
            | FunctionExprKind::CharFromU32 { value: inner }
            | FunctionExprKind::Unary { expr: inner, .. } => {
                self.collect_trait_object_vtables_from_expr(inner, out, seen);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.collect_trait_object_vtables_from_expr(lhs, out, seen);
                self.collect_trait_object_vtables_from_expr(rhs, out, seen);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.collect_trait_object_vtables_from_expr(vector, out, seen);
                self.collect_trait_object_vtables_from_expr(index, out, seen);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.collect_trait_object_vtables_from_expr(vector, out, seen);
                self.collect_trait_object_vtables_from_expr(index, out, seen);
                self.collect_trait_object_vtables_from_expr(value, out, seen);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.collect_trait_object_vtables_from_place(place, out, seen);
                self.collect_trait_object_vtables_from_expr(rhs, out, seen);
            }
            FunctionExprKind::Call { callee, args } => {
                self.collect_trait_object_vtables_from_callee(callee, out, seen);
                for arg in args {
                    self.collect_trait_object_vtables_from_expr(arg, out, seen);
                }
            }
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                nia_function_ir::FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        self.collect_trait_object_vtables_from_expr(elem, out, seen);
                    }
                }
                nia_function_ir::FunctionArrayElements::Repeat { value, .. } => {
                    self.collect_trait_object_vtables_from_expr(value, out, seen);
                }
            },
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_trait_object_vtables_from_expr(&field.value, out, seen);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                self.collect_trait_object_vtables_from_expr(&field.value, out, seen);
            }
            FunctionExprKind::AddrOf(place) => {
                self.collect_trait_object_vtables_from_place(place, out, seen);
            }
            FunctionExprKind::Field { lhs, .. } => {
                self.collect_trait_object_vtables_from_expr(lhs, out, seen);
            }
            FunctionExprKind::Index { lhs, index } => {
                self.collect_trait_object_vtables_from_expr(lhs, out, seen);
                self.collect_trait_object_vtables_from_expr(index, out, seen);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.collect_trait_object_vtables_from_expr(lhs, out, seen);
                if let Some(start) = &range.start {
                    self.collect_trait_object_vtables_from_expr(start, out, seen);
                }
                if let Some(end) = &range.end {
                    self.collect_trait_object_vtables_from_expr(end, out, seen);
                }
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &range.start {
                    self.collect_trait_object_vtables_from_expr(start, out, seen);
                }
                if let Some(end) = &range.end {
                    self.collect_trait_object_vtables_from_expr(end, out, seen);
                }
            }
            FunctionExprKind::InlineAsm(asm) => {
                for input in &asm.inputs {
                    self.collect_trait_object_vtables_from_expr(&input.value, out, seen);
                }
            }
            FunctionExprKind::Atomic(atomic) => {
                self.collect_trait_object_vtables_from_atomic(atomic, out, seen);
            }
            FunctionExprKind::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionExprKind::Error")
            }
            FunctionExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    self.collect_trait_object_vtables_from_expr(field, out, seen);
                }
            }
            FunctionExprKind::EnumTag { value }
            | FunctionExprKind::EnumPayloadField { value, .. } => {
                self.collect_trait_object_vtables_from_expr(value, out, seen);
            }
            FunctionExprKind::Trap
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Null
            | FunctionExprKind::ConstGeneric(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::GlobalInstance { .. }
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::EnumVariantTag(_)
            | FunctionExprKind::BuiltinValue(_) => {}
        }
    }

    fn collect_trait_object_vtables_from_atomic(
        &mut self,
        atomic: &nia_function_ir::FunctionAtomic,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ptr, .. } => {
                self.collect_trait_object_vtables_from_expr(ptr, out, seen)
            }
            nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
                self.collect_trait_object_vtables_from_expr(ptr, out, seen);
                self.collect_trait_object_vtables_from_expr(value, out, seen);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.collect_trait_object_vtables_from_expr(ptr, out, seen);
                self.collect_trait_object_vtables_from_expr(expected, out, seen);
                self.collect_trait_object_vtables_from_expr(desired, out, seen);
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => {}
        }
    }

    fn collect_trait_object_vtables_from_callee(
        &mut self,
        callee: &FunctionCallee,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        match callee {
            FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::FunctionPointer(receiver) => {
                self.collect_trait_object_vtables_from_expr(receiver, out, seen);
            }
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::TraitAssociatedFunction { .. }
            | FunctionCallee::BuiltinOperator(_) => {}
        }
    }

    fn collect_trait_object_vtables_from_place(
        &mut self,
        place: &nia_function_ir::FunctionPlace,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        match &place.base {
            nia_function_ir::FunctionPlaceBase::Deref(expr) => {
                self.collect_trait_object_vtables_from_expr(expr, out, seen);
            }
            nia_function_ir::FunctionPlaceBase::Local(_)
            | nia_function_ir::FunctionPlaceBase::Global(_)
            | nia_function_ir::FunctionPlaceBase::GlobalInstance { .. } => {}
            nia_function_ir::FunctionPlaceBase::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionPlaceBase::Error")
            }
        }
        for elem in &place.elems {
            if let nia_function_ir::FunctionPlaceElem::Index(expr) = elem {
                self.collect_trait_object_vtables_from_expr(expr, out, seen);
            }
        }
    }

    fn build_trait_object_vtable(
        &mut self,
        key: BackendTraitObjectVtableKey,
        span: nia_span::Span,
    ) -> Option<BackendTraitObjectVtable> {
        let Some(TyKind::TraitObject {
            trait_id,
            trait_args,
            ..
        }) = self.ty_kind(key.object_ty).cloned()
        else {
            return None;
        };
        let mut entries = Vec::new();
        let mut next_slot = 0;
        self.push_trait_object_vtable_entries(
            key.self_ty,
            trait_id,
            &trait_args,
            &mut entries,
            &mut next_slot,
            &mut Vec::new(),
        );
        Some(BackendTraitObjectVtable {
            key,
            trait_id,
            trait_args,
            entries,
            span,
        })
    }

    fn cached_trait_object_vtable(
        &mut self,
        key: BackendTraitObjectVtableKey,
        span: nia_span::Span,
    ) -> Option<BackendTraitObjectVtable> {
        if let Some(vtable) = self.trait_context.trait_object_vtables.get(&key) {
            return vtable;
        }
        let vtable = self.build_trait_object_vtable(key.clone(), span);
        self.trait_context
            .trait_object_vtables
            .insert(key, vtable.clone());
        vtable
    }

    fn push_trait_object_vtable_entries(
        &mut self,
        self_ty: InternedTyId,
        trait_id: nia_ids::TraitId,
        trait_args: &[InternedTyId],
        entries: &mut Vec<BackendTraitObjectVtableEntry>,
        next_slot: &mut usize,
        visiting: &mut Vec<nia_ids::TraitId>,
    ) {
        if visiting.contains(&trait_id) {
            return;
        }
        visiting.push(trait_id);
        let nia_ids::TraitId::Source(source_trait_id) = trait_id else {
            visiting.pop();
            return;
        };
        let Some(program_trait) = self.input.program.traits().get(&source_trait_id) else {
            visiting.pop();
            return;
        };
        let trait_signature = program_trait.signature.clone();
        for method in &trait_signature.methods {
            let slot = *next_slot;
            *next_slot += 1;
            let method_id = GlobalDefId {
                module_id: source_trait_id.module_id,
                def_id: method.def_id,
            };
            let Some((def_id, arg_module_id, self_arg, args)) = self
                .resolve_trait_method_impl(
                    source_trait_id,
                    trait_args,
                    method_id,
                    &method.name,
                    self_ty,
                )
                .map(|(def_id, args)| (def_id, self.input.module_id, None, args))
                .or_else(|| {
                    if self.trait_method_has_default(method_id) {
                        // A default method is defined and type-checked in the
                        // trait's module.  Using the module currently
                        // materializing a vtable creates duplicate instances
                        // for the same concrete `(self, object)` pair when a
                        // facade and its consumer both reference the vtable.
                        Some((
                            method_id,
                            source_trait_id.module_id,
                            Some(self_ty),
                            trait_args.to_vec(),
                        ))
                    } else {
                        None
                    }
                })
            else {
                continue;
            };
            let function = if self_arg.is_none() && args.is_empty() {
                BackendTraitObjectVtableFunction::Function(def_id)
            } else {
                let args = self.canonicalize_instance_args(&args);
                BackendTraitObjectVtableFunction::FunctionInstance {
                    def_id,
                    arg_module_id,
                    self_arg,
                    args,
                    const_args: Vec::new(),
                }
            };
            entries.push(BackendTraitObjectVtableEntry {
                trait_id,
                method_id,
                method_name: method.name,
                slot,
                function,
            });
        }
        let substitutions =
            ModuleLowerer::generic_substitutions(&trait_signature.generics, trait_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait =
                self.normalized_type_from_module(source_trait_id.module_id, supertrait.ty);
            let supertrait = self.instantiate_ty(supertrait, &substitutions);
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
                ..
            }) = self.ty_kind(supertrait).cloned()
            else {
                continue;
            };
            self.push_trait_object_vtable_entries(
                self_ty,
                nia_ids::TraitId::Source(supertrait_id),
                &supertrait_args,
                entries,
                next_slot,
                visiting,
            );
        }
        visiting.pop();
    }
}

#[derive(Debug, Default)]
pub(crate) struct TraitObjectVtableCache {
    vtables: HashMap<BackendTraitObjectVtableKey, Option<BackendTraitObjectVtable>>,
}

impl TraitObjectVtableCache {
    fn get(&self, key: &BackendTraitObjectVtableKey) -> Option<Option<BackendTraitObjectVtable>> {
        self.vtables.get(key).cloned()
    }

    fn insert(
        &mut self,
        key: BackendTraitObjectVtableKey,
        vtable: Option<BackendTraitObjectVtable>,
    ) {
        self.vtables.insert(key, vtable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_ids::{ModuleIdAllocator, TypeStoreIndex};
    use nia_span::Span;

    #[test]
    fn trait_object_vtable_cache_reuses_positive_entries() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let mut cache = TraitObjectVtableCache::default();
        let key = test_key(0);
        let vtable = BackendTraitObjectVtable {
            key: key.clone(),
            trait_id: nia_ids::TraitId::Source(GlobalDefId {
                module_id,
                def_id: nia_defs::DefId(0),
            }),
            trait_args: Vec::new(),
            entries: Vec::new(),
            span: Span::default(),
        };

        cache.insert(key.clone(), Some(vtable.clone()));

        assert_eq!(cache.get(&key), Some(Some(vtable)));
    }

    #[test]
    fn trait_object_vtable_cache_remembers_missing_entries() {
        let mut cache = TraitObjectVtableCache::default();
        let key = test_key(0);

        cache.insert(key.clone(), None);

        assert_eq!(cache.get(&key), Some(None));
        assert_eq!(cache.get(&test_key(2)), None);
    }

    fn test_key(index: u32) -> BackendTraitObjectVtableKey {
        BackendTraitObjectVtableKey {
            self_ty: test_ty(index),
            object_ty: test_ty(index + 1),
        }
    }

    fn test_ty(index: u32) -> InternedTyId {
        static TYPE_STORE: std::sync::OnceLock<nia_ty::TypeStore> = std::sync::OnceLock::new();
        let type_store = TYPE_STORE.get_or_init(nia_ty::TypeStore::new);
        InternedTyId::new(type_store.id(), TypeStoreIndex::from_store_index(index))
    }
}
