// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::ModuleLowerer;
use nia_backend_ir::{
    BackendFunction, BackendFunctionInstance, BackendTraitObjectVtable,
    BackendTraitObjectVtableEntry, BackendTraitObjectVtableFunction, BackendTraitObjectVtableKey,
};
use nia_function_ir::{FunctionBody, FunctionCallee, FunctionExpr, FunctionExprKind, FunctionOp};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_ty::TyKind;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn collect_trait_object_vtables(
        &mut self,
        out: &mut Vec<BackendTraitObjectVtable>,
        functions: &[BackendFunction],
        function_instances: &[BackendFunctionInstance],
    ) {
        let mut seen = HashSet::new();
        for function in functions {
            if let Some(body) = &function.function_body {
                self.collect_trait_object_vtables_from_body(body, out, &mut seen);
            }
        }
        for instance in function_instances {
            if let Some(body) = &instance.function_body {
                self.collect_trait_object_vtables_from_body(body, out, &mut seen);
            }
        }
    }

    fn collect_trait_object_vtables_from_body(
        &mut self,
        body: &FunctionBody,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        for block in &body.blocks {
            for op in &block.ops {
                match op {
                    FunctionOp::Binding(binding) => {
                        if let Some(value) = &binding.value {
                            self.collect_trait_object_vtables_from_expr(value, out, seen);
                        }
                    }
                    FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                        self.collect_trait_object_vtables_from_expr(value, out, seen);
                    }
                    FunctionOp::Defer(defer) => {
                        for block in &defer.blocks {
                            for op in &block.ops {
                                match op {
                                    FunctionOp::Binding(binding) => {
                                        if let Some(value) = &binding.value {
                                            self.collect_trait_object_vtables_from_expr(
                                                value, out, seen,
                                            );
                                        }
                                    }
                                    FunctionOp::StoreLocal { value, .. }
                                    | FunctionOp::Expr(value) => {
                                        self.collect_trait_object_vtables_from_expr(
                                            value, out, seen,
                                        );
                                    }
                                    FunctionOp::Defer(_) => {}
                                }
                            }
                        }
                    }
                }
            }
            match &block.terminator {
                nia_function_ir::FunctionTerminator::If { cond, .. }
                | nia_function_ir::FunctionTerminator::Switch { target: cond, .. }
                | nia_function_ir::FunctionTerminator::Return {
                    value: Some(cond), ..
                }
                | nia_function_ir::FunctionTerminator::Tail {
                    value: Some(cond), ..
                } => self.collect_trait_object_vtables_from_expr(cond, out, seen),
                nia_function_ir::FunctionTerminator::Loop { header, .. } => match header {
                    nia_function_ir::FunctionForHeader::Condition(cond) => {
                        self.collect_trait_object_vtables_from_expr(cond, out, seen);
                    }
                    nia_function_ir::FunctionForHeader::CStyle { cond: Some(cond) } => {
                        self.collect_trait_object_vtables_from_expr(cond, out, seen);
                    }
                    _ => {}
                },
                _ => {}
            }
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
            | FunctionExprKind::CStringPointer { array: inner, .. }
            | FunctionExprKind::Unary { expr: inner, .. } => {
                self.collect_trait_object_vtables_from_expr(inner, out, seen);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                self.collect_trait_object_vtables_from_expr(lhs, out, seen);
                self.collect_trait_object_vtables_from_expr(rhs, out, seen);
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
            FunctionExprKind::Error
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Local(_)
            | FunctionExprKind::Global(_)
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::EnumVariant(_)
            | FunctionExprKind::BuiltinValue(_) => {}
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
            | FunctionCallee::FunctionPointer(receiver) => {
                self.collect_trait_object_vtables_from_expr(receiver, out, seen);
            }
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::BuiltinOperator(_) => {}
        }
    }

    fn collect_trait_object_vtables_from_place(
        &mut self,
        place: &nia_function_ir::FunctionPlace,
        out: &mut Vec<BackendTraitObjectVtable>,
        seen: &mut HashSet<BackendTraitObjectVtableKey>,
    ) {
        if let nia_function_ir::FunctionPlaceBase::Deref(expr) = &place.base {
            self.collect_trait_object_vtables_from_expr(expr, out, seen);
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
        }) = self.interner.get(key.object_ty).cloned()
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
        if let Some(vtable) = self.trait_object_vtables.get(&key) {
            return Some(vtable);
        }
        let vtable = self.build_trait_object_vtable(key.clone(), span);
        self.trait_object_vtables.insert(key, vtable.clone());
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
        let Some(program_trait) = self.input.program_traits.get(&source_trait_id) else {
            visiting.pop();
            return;
        };
        let trait_signature = program_trait.signature.clone();
        let trait_args = trait_args
            .iter()
            .map(|arg| nia_ty::import_type_into(&mut self.interner, &program_trait.interner, *arg))
            .collect::<Vec<_>>();
        for method in &trait_signature.methods {
            let slot = *next_slot;
            *next_slot += 1;
            let method_id = GlobalDefId {
                module_id: source_trait_id.module_id,
                def_id: method.def_id,
            };
            let Some((def_id, args)) = self
                .resolve_trait_method_impl(
                    source_trait_id,
                    &trait_args,
                    method_id,
                    &method.name,
                    self_ty,
                )
                .or_else(|| {
                    if self.trait_method_has_default(method_id) {
                        let mut args = vec![self_ty];
                        args.extend(trait_args.iter().copied());
                        Some((method_id, args))
                    } else {
                        None
                    }
                })
            else {
                continue;
            };
            let function = if args.is_empty() {
                BackendTraitObjectVtableFunction::Function(def_id)
            } else {
                BackendTraitObjectVtableFunction::FunctionInstance { def_id, args }
            };
            entries.push(BackendTraitObjectVtableEntry {
                trait_id,
                method_id,
                method_name: method.name.clone(),
                slot,
                function,
            });
        }
        let substitutions = self.generic_substitutions(&trait_signature.generics, &trait_args);
        for supertrait in &trait_signature.supertraits {
            let supertrait =
                nia_ty::import_type_into(&mut self.interner, &program_trait.interner, *supertrait);
            let supertrait = self.instantiate_ty(supertrait, &substitutions);
            let Some(TyKind::Nominal {
                def_id: supertrait_id,
                args: supertrait_args,
            }) = self.interner.get(supertrait).cloned()
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
    fn get(&self, key: &BackendTraitObjectVtableKey) -> Option<BackendTraitObjectVtable> {
        self.vtables.get(key).cloned().flatten()
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
    use nia_ids::{ModuleId, TyInternerIndex};
    use nia_span::Span;

    #[test]
    fn trait_object_vtable_cache_reuses_positive_entries() {
        let mut cache = TraitObjectVtableCache::default();
        let key = test_key(0);
        let vtable = BackendTraitObjectVtable {
            key: key.clone(),
            trait_id: nia_ids::TraitId::Source(GlobalDefId {
                module_id: ModuleId(0),
                def_id: nia_defs::DefId(0),
            }),
            trait_args: Vec::new(),
            entries: Vec::new(),
            span: Span::default(),
        };

        cache.insert(key.clone(), Some(vtable.clone()));

        assert_eq!(cache.get(&key), Some(vtable));
    }

    #[test]
    fn trait_object_vtable_cache_remembers_missing_entries() {
        let mut cache = TraitObjectVtableCache::default();
        let key = test_key(0);

        cache.insert(key.clone(), None);

        assert!(cache.vtables.contains_key(&key));
        assert_eq!(cache.get(&key), None);
    }

    fn test_key(index: u32) -> BackendTraitObjectVtableKey {
        BackendTraitObjectVtableKey {
            self_ty: test_ty(index),
            object_ty: test_ty(index + 1),
        }
    }

    fn test_ty(index: u32) -> InternedTyId {
        InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(index))
    }
}
