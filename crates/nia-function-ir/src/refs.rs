// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{BTreeSet, HashSet};

use crate::{
    FunctionArrayElements, FunctionAtomic, FunctionBlock, FunctionBody, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm,
    FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace, FunctionPlaceBase, FunctionPlaceElem,
    FunctionTerminator,
};
use nia_ids::{GlobalDefId, InternedTyId, ModuleId};
use nia_span::Span;
use nia_ty::{ConstGenericArg, TraitId, TyKind};

/// A reference to one concrete generic function instance found in a body.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionInstanceRef {
    /// Definition being instantiated.
    pub def_id: GlobalDefId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Optional receiver substitution.
    pub self_arg: Option<InternedTyId>,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
    /// Source span of the instantiation use.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Complete identity of a monomorphized function body.
///
/// Every field participates in reachability, caching, and backend instance
/// lookup: `arg_module_id` identifies the module that owns the argument type
/// store, `self_arg` identifies an implicit receiver type, and `const_args`
/// distinguishes const-generic bodies that otherwise share the same
/// definition and type arguments. Callers must use this key instead of a
/// definition-only identity when preserving or deduplicating instances.
pub struct FunctionInstanceKey {
    /// Definition being instantiated.
    pub def_id: GlobalDefId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Optional receiver substitution.
    pub self_arg: Option<InternedTyId>,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
}

/// A reference to one concrete generic global instance found in a body.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalInstanceRef {
    /// Definition being instantiated.
    pub def_id: GlobalDefId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
    /// Source span of the instantiation use.
    pub span: Span,
}

/// Hashable identity of one concrete generic global instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlobalInstanceKey {
    /// Definition being instantiated.
    pub def_id: GlobalDefId,
    /// Module supplying generic argument resolution context.
    pub arg_module_id: ModuleId,
    /// Type arguments in canonical order.
    pub args: Vec<InternedTyId>,
    /// Const arguments paired with `args`.
    pub const_args: Vec<ConstGenericArg>,
}

/// Identity of a concrete trait-object vtable referenced by a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraitObjectVtableRef {
    /// Concrete receiver type.
    pub self_ty: InternedTyId,
    /// Complete erased object type.
    pub object_ty: InternedTyId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Dynamic-dispatch identity available without a concrete receiver type.
pub struct DynamicTraitCallRef {
    /// Erased receiver view used at the call site.
    pub object_ty: InternedTyId,
    /// Trait segment containing the selected method slot.
    pub trait_id: TraitId,
}

impl GlobalInstanceRef {
    /// Projects the cache/deduplication key from this source reference.
    pub fn key(&self) -> GlobalInstanceKey {
        GlobalInstanceKey {
            def_id: self.def_id,
            arg_module_id: self.arg_module_id,
            args: self.args.clone(),
            const_args: self.const_args.clone(),
        }
    }
}

impl FunctionInstanceRef {
    /// Projects the cache/deduplication key from this source reference.
    pub fn key(&self) -> FunctionInstanceKey {
        FunctionInstanceKey {
            def_id: self.def_id,
            arg_module_id: self.arg_module_id,
            self_arg: self.self_arg,
            args: self.args.clone(),
            const_args: self.const_args.clone(),
        }
    }
}

/// Deduplicated identities discovered while walking one function body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FunctionBodyRefs {
    /// Whether traversal encountered a recovery/error node in the body.
    ///
    /// References collected before and after the malformed node remain
    /// available so consumers can report the structural failure without
    /// losing otherwise useful dependency information.
    pub invalid_ir: bool,
    /// Modules whose definitions or type stores are needed.
    pub modules: BTreeSet<ModuleId>,
    /// Monomorphic functions referenced by value or call.
    pub functions: BTreeSet<GlobalDefId>,
    /// Monomorphic globals referenced by value or place.
    pub globals: BTreeSet<GlobalDefId>,
    /// Concrete function instances, retaining each use span.
    pub function_instances: Vec<FunctionInstanceRef>,
    /// Concrete global instances, retaining each use span.
    pub global_instances: Vec<GlobalInstanceRef>,
    /// Interned types required by expressions and locals.
    pub types: BTreeSet<InternedTyId>,
    /// Concrete trait-object vtables referenced by coercion.
    pub trait_object_vtables: BTreeSet<TraitObjectVtableRef>,
    /// Erased object and trait identities used by dynamic calls.
    ///
    /// Unlike a coercion, a call does not name the concrete `self` type, and an
    /// upcast may dispatch through a supertrait segment in a source vtable.
    /// Codegen readiness uses both identities to wait for every candidate table
    /// and its target declarations before validating the call contract.
    pub dynamic_trait_calls: HashSet<DynamicTraitCallRef>,
}

impl FunctionBodyRefs {
    /// Merges references from another body walk into this accumulator.
    pub fn extend(&mut self, other: Self) {
        self.invalid_ir |= other.invalid_ir;
        self.modules.extend(other.modules);
        self.functions.extend(other.functions);
        self.globals.extend(other.globals);
        self.function_instances.extend(other.function_instances);
        self.global_instances.extend(other.global_instances);
        self.types.extend(other.types);
        self.trait_object_vtables.extend(other.trait_object_vtables);
        self.dynamic_trait_calls.extend(other.dynamic_trait_calls);
    }
}

impl FunctionBody {
    /// Walks all nested expressions and collects backend-relevant identities.
    pub fn value_refs(&self, types: &nia_ty::TypeStore) -> FunctionBodyRefs {
        let mut refs = FunctionBodyRefs::default();
        collect_function_refs_from_body(self, types, &mut refs);
        refs
    }
}

fn collect_function_refs_from_body(
    body: &FunctionBody,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    refs.types.insert(body.ty);
    refs.types.extend(body.locals.iter().map(|local| local.ty));
    for block in &body.blocks {
        collect_function_refs_from_block(block, types, refs);
    }
}

fn collect_function_refs_from_defer_body(
    body: &FunctionDeferBody,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    for block in &body.blocks {
        collect_function_refs_from_block(block, types, refs);
    }
}

fn collect_function_refs_from_block(
    block: &FunctionBlock,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    for op in &block.ops {
        collect_function_refs_from_op(op, types, refs);
    }
    collect_function_refs_from_terminator(&block.terminator, types, refs);
}

fn collect_function_refs_from_op(
    op: &FunctionOp,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_function_refs_from_expr(value, types, refs);
            }
        }
        FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
            collect_function_refs_from_expr(value, types, refs);
        }
        FunctionOp::MemoryIntrinsic(memory) => {
            refs.types.insert(memory.elem_ty);
            collect_function_refs_from_expr(&memory.dest, types, refs);
            match &memory.source {
                FunctionMemoryIntrinsicSource::Slice(source)
                | FunctionMemoryIntrinsicSource::Byte(source) => {
                    collect_function_refs_from_expr(source, types, refs);
                }
            }
        }
        FunctionOp::Defer(body) => collect_function_refs_from_defer_body(body, types, refs),
    }
}

fn collect_function_refs_from_terminator(
    terminator: &FunctionTerminator,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    match terminator {
        FunctionTerminator::If { cond, .. } => collect_function_refs_from_expr(cond, types, refs),
        FunctionTerminator::Switch { target, arms, .. } => {
            collect_function_refs_from_expr(target, types, refs);
            for arm in arms {
                collect_function_refs_from_expr(&arm.pattern, types, refs);
            }
        }
        FunctionTerminator::Try {
            value,
            error_conversion,
            ..
        } => {
            collect_function_refs_from_expr(value, types, refs);
            if let Some(conversion) = error_conversion {
                collect_function_refs_from_expr(conversion, types, refs);
            }
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(expr) => {
                collect_function_refs_from_expr(expr, types, refs)
            }
            FunctionForHeader::Infinite => {}
        },
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            if let Some(value) = value {
                collect_function_refs_from_expr(value, types, refs);
            }
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => {}
    }
}

fn collect_function_refs_from_expr(
    expr: &FunctionExpr,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    refs.types.insert(expr.ty);
    match &expr.kind {
        FunctionExprKind::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        FunctionExprKind::FunctionInstance {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } => {
            collect_function_instance_types(*self_arg, args, const_args, refs);
            refs.function_instances.push(FunctionInstanceRef {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                self_arg: *self_arg,
                args: args.clone(),
                const_args: const_args.clone(),
                span: expr.span,
            });
        }
        FunctionExprKind::Range(range) => {
            if let Some(start) = &range.start {
                collect_function_refs_from_expr(start, types, refs);
            }
            if let Some(end) = &range.end {
                collect_function_refs_from_expr(end, types, refs);
            }
        }
        FunctionExprKind::InlineAsm(asm) => collect_function_refs_from_inline_asm(asm, types, refs),
        FunctionExprKind::Atomic(atomic) => collect_function_refs_from_atomic(atomic, types, refs),
        FunctionExprKind::TraitObjectUpcast {
            expr: inner,
            source_ty,
            target_ty,
        } => {
            refs.types.extend([*source_ty, *target_ty]);
            collect_function_refs_from_expr(inner, types, refs);
        }
        FunctionExprKind::TraitObjectCoercion {
            expr: inner,
            target_ty,
            self_ty,
        } => {
            refs.types.extend([*self_ty, *target_ty]);
            refs.trait_object_vtables.insert(TraitObjectVtableRef {
                self_ty: *self_ty,
                object_ty: *target_ty,
            });
            collect_function_refs_from_expr(inner, types, refs);
        }
        FunctionExprKind::CallableCoercion { state, .. } => {
            collect_function_refs_from_expr(state, types, refs);
        }
        FunctionExprKind::FunctionCallable { function } => {
            collect_function_refs_from_expr(function, types, refs);
        }
        FunctionExprKind::StaticArrayPointer {
            allocation, array, ..
        } => {
            refs.modules.insert(allocation.module_id());
            collect_function_refs_from_expr(array, types, refs);
        }
        FunctionExprKind::RangeBound { range: array, .. }
        | FunctionExprKind::Unary { expr: array, .. }
        | FunctionExprKind::OptionalSome { expr: array }
        | FunctionExprKind::ErrorOk { expr: array }
        | FunctionExprKind::ErrorErr { expr: array }
        | FunctionExprKind::TaggedUnionTag { expr: array }
        | FunctionExprKind::TaggedUnionPayload { expr: array }
        | FunctionExprKind::Try { expr: array }
        | FunctionExprKind::LoadUnaligned { ptr: array, .. }
        | FunctionExprKind::Splat { value: array }
        | FunctionExprKind::Bitmask { vector: array }
        | FunctionExprKind::BitIntrinsic { value: array, .. }
        | FunctionExprKind::CharFromU32 { value: array }
        | FunctionExprKind::Discard(array)
        | FunctionExprKind::Cast { expr: array, .. } => {
            collect_function_refs_from_expr(array, types, refs);
        }
        FunctionExprKind::ArrayLiteral { elems } => match elems {
            FunctionArrayElements::List(elems) => {
                for elem in elems {
                    collect_function_refs_from_expr(elem, types, refs);
                }
            }
            FunctionArrayElements::Repeat { value, .. } => {
                collect_function_refs_from_expr(value, types, refs)
            }
        },
        FunctionExprKind::Tuple(elems) => {
            for elem in elems {
                collect_function_refs_from_expr(elem, types, refs);
            }
        }
        FunctionExprKind::TupleField { value, .. } => {
            collect_function_refs_from_expr(value, types, refs);
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_function_refs_from_expr(&field.value, types, refs);
            }
        }
        FunctionExprKind::UnionLiteral { field, .. } => {
            collect_function_refs_from_expr(&field.value, types, refs);
        }
        FunctionExprKind::UnionStorageLiteral { relocations, .. } => {
            for relocation in relocations {
                refs.modules.insert(relocation.allocation.module_id());
                collect_function_refs_from_expr(&relocation.pointee, types, refs);
            }
        }
        FunctionExprKind::AddrOf(place) => collect_function_refs_from_place(place, types, refs),
        FunctionExprKind::Binary { lhs, rhs, .. } => {
            collect_function_refs_from_expr(lhs, types, refs);
            collect_function_refs_from_expr(rhs, types, refs);
        }
        FunctionExprKind::ExtractElement { vector, index } => {
            collect_function_refs_from_expr(vector, types, refs);
            collect_function_refs_from_expr(index, types, refs);
        }
        FunctionExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            collect_function_refs_from_expr(vector, types, refs);
            collect_function_refs_from_expr(index, types, refs);
            collect_function_refs_from_expr(value, types, refs);
        }
        FunctionExprKind::Assign { place, rhs, .. } => {
            collect_function_refs_from_place(place, types, refs);
            collect_function_refs_from_expr(rhs, types, refs);
        }
        FunctionExprKind::Call { callee, args } => {
            collect_function_refs_from_callee(expr.span, callee, types, refs);
            for arg in args {
                collect_function_refs_from_expr(arg, types, refs);
            }
        }
        FunctionExprKind::Field { lhs, .. } => collect_function_refs_from_expr(lhs, types, refs),
        FunctionExprKind::Index { lhs, index } => {
            collect_function_refs_from_expr(lhs, types, refs);
            collect_function_refs_from_expr(index, types, refs);
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            collect_function_refs_from_expr(lhs, types, refs);
            if let Some(start) = &range.start {
                collect_function_refs_from_expr(start, types, refs);
            }
            if let Some(end) = &range.end {
                collect_function_refs_from_expr(end, types, refs);
            }
        }
        FunctionExprKind::EnumVariant { fields, .. } => {
            for field in fields {
                collect_function_refs_from_expr(field, types, refs);
            }
        }
        FunctionExprKind::EnumTag { value } | FunctionExprKind::EnumPayloadField { value, .. } => {
            collect_function_refs_from_expr(value, types, refs);
        }
        FunctionExprKind::Error => refs.invalid_ir = true,
        FunctionExprKind::Integer(_)
        | FunctionExprKind::Float(_)
        | FunctionExprKind::String(_)
        | FunctionExprKind::ByteString(_)
        | FunctionExprKind::Char(_)
        | FunctionExprKind::ByteChar(_)
        | FunctionExprKind::Bool(_)
        | FunctionExprKind::Null
        | FunctionExprKind::ConstGeneric(_)
        | FunctionExprKind::Local(_)
        | FunctionExprKind::ClosureFunctionPointer { .. }
        | FunctionExprKind::EnumVariantTag(_)
        | FunctionExprKind::BuiltinValue(_)
        | FunctionExprKind::CallerLocation(_)
        | FunctionExprKind::Trap => {}
        FunctionExprKind::Global(def_id) => {
            refs.globals.insert(*def_id);
        }
        FunctionExprKind::GlobalInstance {
            def_id,
            arg_module_id,
            args,
            const_args,
        } => {
            collect_instance_types(args, const_args, refs);
            refs.global_instances.push(GlobalInstanceRef {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                args: args.clone(),
                const_args: const_args.clone(),
                span: expr.span,
            });
        }
    }
}

fn collect_function_refs_from_atomic(
    atomic: &FunctionAtomic,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    match atomic {
        FunctionAtomic::Load { ty, ptr, .. } => {
            refs.types.insert(*ty);
            collect_function_refs_from_expr(ptr, types, refs);
        }
        FunctionAtomic::Store { ty, ptr, value, .. }
        | FunctionAtomic::Rmw { ty, ptr, value, .. } => {
            refs.types.insert(*ty);
            collect_function_refs_from_expr(ptr, types, refs);
            collect_function_refs_from_expr(value, types, refs);
        }
        FunctionAtomic::Cmpxchg {
            ty,
            ptr,
            expected,
            desired,
            ..
        } => {
            refs.types.insert(*ty);
            collect_function_refs_from_expr(ptr, types, refs);
            collect_function_refs_from_expr(expected, types, refs);
            collect_function_refs_from_expr(desired, types, refs);
        }
        FunctionAtomic::Fence { .. } => {}
    }
}

fn collect_function_refs_from_callee(
    span: Span,
    callee: &FunctionCallee,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    match callee {
        FunctionCallee::Tracked { callee, .. } => {
            collect_function_refs_from_callee(span, callee, types, refs)
        }
        FunctionCallee::ClosureEntry { state, .. } => {
            collect_function_refs_from_expr(state, types, refs);
        }
        FunctionCallee::Function(def_id) => {
            refs.functions.insert(*def_id);
        }
        FunctionCallee::FunctionInstance {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
        } => {
            collect_function_instance_types(*self_arg, args, const_args, refs);
            refs.function_instances.push(FunctionInstanceRef {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                self_arg: *self_arg,
                args: args.clone(),
                const_args: const_args.clone(),
                span,
            });
        }
        FunctionCallee::Method {
            def_id,
            arg_module_id,
            self_arg,
            args,
            const_args,
            receiver,
            ..
        } => {
            collect_function_instance_types(*self_arg, args, const_args, refs);
            if self_arg.is_none() && args.is_empty() && const_args.is_empty() {
                refs.functions.insert(*def_id);
            } else {
                refs.function_instances.push(FunctionInstanceRef {
                    def_id: *def_id,
                    arg_module_id: *arg_module_id,
                    self_arg: *self_arg,
                    args: args.clone(),
                    const_args: const_args.clone(),
                    span,
                });
            }
            collect_function_refs_from_expr(receiver, types, refs);
        }
        FunctionCallee::TraitMethod {
            self_ty,
            trait_args,
            trait_const_args,
            args,
            const_args,
            receiver,
            ..
        } => {
            refs.types.insert(*self_ty);
            refs.types.extend(trait_args.iter().copied());
            refs.types.extend(trait_const_args.iter().map(|arg| arg.ty));
            collect_instance_types(args, const_args, refs);
            collect_function_refs_from_expr(receiver, types, refs);
        }
        FunctionCallee::TraitAssociatedFunction {
            self_ty,
            trait_args,
            trait_const_args,
            args,
            const_args,
            ..
        } => {
            refs.types.insert(*self_ty);
            refs.types.extend(trait_args.iter().copied());
            refs.types.extend(trait_const_args.iter().map(|arg| arg.ty));
            collect_instance_types(args, const_args, refs);
        }
        FunctionCallee::DynamicTraitMethod {
            object_ty,
            trait_id,
            trait_args,
            trait_const_args,
            params,
            return_type,
            receiver,
            ..
        } => {
            refs.types.extend([*object_ty, *return_type]);
            refs.dynamic_trait_calls.insert(DynamicTraitCallRef {
                object_ty: *object_ty,
                trait_id: *trait_id,
            });
            refs.types.extend(trait_args.iter().copied());
            refs.types.extend(trait_const_args.iter().map(|arg| arg.ty));
            refs.types.extend(params.iter().copied());
            collect_function_refs_from_expr(receiver, types, refs);
        }
        FunctionCallee::BuiltinPlaceMethod {
            self_ty,
            trait_args,
            receiver,
            ..
        } => {
            refs.types.insert(*self_ty);
            refs.types.extend(trait_args.iter().copied());
            collect_function_refs_from_expr(receiver, types, refs);
        }
        FunctionCallee::BuiltinMethod {
            method,
            self_ty,
            receiver,
        } => {
            refs.types.insert(*self_ty);
            let receiver_is_unevaluated = matches!(
                (method, types.get(*self_ty)),
                (
                    crate::FunctionBuiltinMethod::SliceLen,
                    Some(TyKind::Array { .. })
                )
            );
            if !receiver_is_unevaluated {
                collect_function_refs_from_expr(receiver, types, refs);
            }
        }
        FunctionCallee::Callable(receiver) | FunctionCallee::FunctionPointer(receiver) => {
            collect_function_refs_from_expr(receiver, types, refs);
        }
        FunctionCallee::BuiltinOperator(_) => {}
    }
}

fn collect_function_refs_from_place(
    place: &FunctionPlace,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    refs.types.insert(place.ty);
    match &place.base {
        FunctionPlaceBase::Deref(expr) => collect_function_refs_from_expr(expr, types, refs),
        FunctionPlaceBase::Local(_) => {}
        FunctionPlaceBase::Global(def_id) => {
            refs.globals.insert(*def_id);
        }
        FunctionPlaceBase::GlobalInstance {
            def_id,
            arg_module_id,
            args,
            const_args,
        } => {
            collect_instance_types(args, const_args, refs);
            refs.global_instances.push(GlobalInstanceRef {
                def_id: *def_id,
                arg_module_id: *arg_module_id,
                args: args.clone(),
                const_args: const_args.clone(),
                span: place.span,
            });
        }
        FunctionPlaceBase::Error => refs.invalid_ir = true,
    }
    for elem in &place.elems {
        match elem {
            FunctionPlaceElem::Index(expr) => collect_function_refs_from_expr(expr, types, refs),
            FunctionPlaceElem::Field(_) | FunctionPlaceElem::TupleField(_) => {}
            FunctionPlaceElem::Error => refs.invalid_ir = true,
        }
    }
}

fn collect_function_instance_types(
    self_arg: Option<InternedTyId>,
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
    refs: &mut FunctionBodyRefs,
) {
    refs.types.extend(self_arg);
    collect_instance_types(args, const_args, refs);
}

fn collect_instance_types(
    args: &[InternedTyId],
    const_args: &[ConstGenericArg],
    refs: &mut FunctionBodyRefs,
) {
    refs.types.extend(args.iter().copied());
    refs.types.extend(const_args.iter().map(|arg| arg.ty));
}

fn collect_function_refs_from_inline_asm(
    asm: &FunctionInlineAsm,
    types: &nia_ty::TypeStore,
    refs: &mut FunctionBodyRefs,
) {
    for input in &asm.inputs {
        collect_function_refs_from_expr(&input.value, types, refs);
    }
    for output in &asm.outputs {
        collect_function_refs_from_place(&output.place, types, refs);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::*;
    use crate::{
        AtomicOrder, FunctionAsmInput, FunctionAsmOutput, FunctionBlockId, FunctionScope,
        FunctionScopeId,
    };
    use nia_ids::{DefId, ModuleIdAllocator};
    use nia_ty::{ConstGenericArg, ConstGenericValue, IntConst, PrimitiveTy, TypeStore};

    fn expr(ty: InternedTyId, kind: FunctionExprKind) -> FunctionExpr {
        FunctionExpr {
            span: Span::default(),
            ty,
            kind,
        }
    }

    fn global(module_id: ModuleId, def_id: u64) -> GlobalDefId {
        GlobalDefId {
            module_id,
            def_id: DefId(def_id),
        }
    }

    #[test]
    fn traverses_nested_typed_value_references() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let arg_module_id = module_ids.allocate();
        let types = TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize);
        let vtable_self_ty = types
            .append_for_module(module_id)
            .primitive(PrimitiveTy::I32);
        let object_ty = types
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Bool);
        let function = global(module_id, 1);
        let nested_function = global(module_id, 2);
        let function_instance = global(module_id, 3);
        let defer_function_instance = global(module_id, 4);
        let global_value = global(module_id, 5);
        let global_place = global(module_id, 6);
        let global_instance = global(module_id, 7);
        let dynamic_trait = TraitId::Source(global(module_id, 8));
        let dynamic_method = global(module_id, 9);

        let instance_expr = expr(
            ty,
            FunctionExprKind::FunctionInstance {
                def_id: function_instance,
                arg_module_id,
                self_arg: Some(ty),
                args: vec![ty],
                const_args: Vec::new(),
            },
        );
        let call = expr(
            ty,
            FunctionExprKind::Call {
                callee: FunctionCallee::Function(function),
                args: vec![expr(
                    ty,
                    FunctionExprKind::Atomic(FunctionAtomic::Load {
                        ty,
                        ptr: Box::new(expr(ty, FunctionExprKind::Global(global_value))),
                        order: AtomicOrder::Acquire,
                    }),
                )],
            },
        );
        let asm = expr(
            ty,
            FunctionExprKind::InlineAsm(FunctionInlineAsm {
                code: String::new(),
                inputs: vec![FunctionAsmInput {
                    constraint: "r".to_string(),
                    value: expr(ty, FunctionExprKind::Global(global_value)),
                    span: Span::default(),
                }],
                outputs: vec![FunctionAsmOutput {
                    constraint: "=r".to_string(),
                    place: FunctionPlace {
                        span: Span::default(),
                        ty,
                        base: FunctionPlaceBase::Global(global_place),
                        elems: vec![FunctionPlaceElem::Index(Box::new(expr(
                            ty,
                            FunctionExprKind::Function(nested_function),
                        )))],
                    },
                    span: Span::default(),
                }],
                clobbers: Vec::new(),
                options: Vec::new(),
            }),
        );
        let defer = FunctionDeferBody {
            span: Span::default(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span: Span::default(),
                ops: Vec::new(),
                terminator: FunctionTerminator::Return {
                    value: Some(expr(
                        ty,
                        FunctionExprKind::Call {
                            callee: FunctionCallee::FunctionInstance {
                                def_id: defer_function_instance,
                                arg_module_id,
                                self_arg: None,
                                args: vec![ty],
                                const_args: Vec::new(),
                            },
                            args: vec![expr(
                                ty,
                                FunctionExprKind::AddrOf(FunctionPlace {
                                    span: Span::default(),
                                    ty,
                                    base: FunctionPlaceBase::GlobalInstance {
                                        def_id: global_instance,
                                        arg_module_id,
                                        args: vec![ty],
                                        const_args: Vec::new(),
                                    },
                                    elems: Vec::new(),
                                }),
                            )],
                        },
                    )),
                    span: Span::default(),
                },
            }],
            entry: FunctionBlockId(0),
        };
        let body = FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span: Span::default(),
                ops: vec![
                    FunctionOp::Expr(instance_expr),
                    FunctionOp::Expr(call),
                    FunctionOp::Expr(expr(
                        ty,
                        FunctionExprKind::Call {
                            callee: FunctionCallee::DynamicTraitMethod {
                                object_ty,
                                trait_id: dynamic_trait,
                                method_id: dynamic_method,
                                method_name: nia_symbol::SymbolId::EMPTY,
                                trait_args: Vec::new(),
                                trait_const_args: Vec::new(),
                                slot: 0,
                                params: Vec::new(),
                                return_type: ty,
                                receiver_kind: nia_ids::ReceiverKind::RefReadOnly,
                                receiver: Box::new(expr(
                                    object_ty,
                                    FunctionExprKind::Global(global_value),
                                )),
                            },
                            args: Vec::new(),
                        },
                    )),
                    FunctionOp::Expr(asm),
                    FunctionOp::Expr(expr(
                        object_ty,
                        FunctionExprKind::TraitObjectCoercion {
                            expr: Box::new(expr(
                                vtable_self_ty,
                                FunctionExprKind::Global(global_value),
                            )),
                            target_ty: object_ty,
                            self_ty: vtable_self_ty,
                        },
                    )),
                    FunctionOp::Defer(defer),
                ],
                terminator: FunctionTerminator::Tail {
                    value: None,
                    span: Span::default(),
                },
            }],
            entry: FunctionBlockId(0),
            ty,
        };

        let refs = body.value_refs(&types);

        assert_eq!(refs.functions, BTreeSet::from([function, nested_function]));
        assert_eq!(refs.globals, BTreeSet::from([global_value, global_place]));
        assert_eq!(refs.types, BTreeSet::from([ty, vtable_self_ty, object_ty]));
        assert_eq!(
            refs.trait_object_vtables,
            BTreeSet::from([TraitObjectVtableRef {
                self_ty: vtable_self_ty,
                object_ty,
            }])
        );
        assert_eq!(
            refs.dynamic_trait_calls,
            HashSet::from([DynamicTraitCallRef {
                object_ty,
                trait_id: dynamic_trait,
            }])
        );
        assert_eq!(
            refs.function_instances
                .iter()
                .map(FunctionInstanceRef::key)
                .collect::<HashSet<_>>(),
            HashSet::from([
                FunctionInstanceKey {
                    def_id: function_instance,
                    arg_module_id,
                    self_arg: Some(ty),
                    args: vec![ty],
                    const_args: Vec::new(),
                },
                FunctionInstanceKey {
                    def_id: defer_function_instance,
                    arg_module_id,
                    self_arg: None,
                    args: vec![ty],
                    const_args: Vec::new(),
                },
            ])
        );
        assert_eq!(
            refs.global_instances
                .iter()
                .map(GlobalInstanceRef::key)
                .collect::<HashSet<_>>(),
            HashSet::from([GlobalInstanceKey {
                def_id: global_instance,
                arg_module_id,
                args: vec![ty],
                const_args: Vec::new(),
            }])
        );
    }

    #[test]
    fn trait_callees_retain_method_const_argument_types() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let receiver_ty = append.primitive(PrimitiveTy::U8);
        let method_const_ty = append.primitive(PrimitiveTy::Usize);
        let associated_const_ty = append.primitive(PrimitiveTy::Bool);
        let trait_id = global(module_id, 1);
        let method_id = global(module_id, 2);
        let associated_id = global(module_id, 3);
        let const_arg = |ty, value| ConstGenericArg {
            ty,
            value: ConstGenericValue::Int(IntConst::unsigned(value)),
        };
        let receiver = || expr(receiver_ty, FunctionExprKind::Global(global(module_id, 4)));
        let body = FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span: Span::default(),
                ops: vec![
                    FunctionOp::Expr(expr(
                        receiver_ty,
                        FunctionExprKind::Call {
                            callee: FunctionCallee::TraitMethod {
                                trait_id,
                                method_id,
                                method_name: nia_symbol::SymbolId::EMPTY,
                                self_ty: receiver_ty,
                                trait_args: Vec::new(),
                                trait_const_args: Vec::new(),
                                args: vec![receiver_ty],
                                const_args: vec![const_arg(method_const_ty, 4)],
                                receiver_kind: nia_ids::ReceiverKind::RefReadOnly,
                                receiver: Box::new(receiver()),
                            },
                            args: Vec::new(),
                        },
                    )),
                    FunctionOp::Expr(expr(
                        receiver_ty,
                        FunctionExprKind::Call {
                            callee: FunctionCallee::TraitAssociatedFunction {
                                trait_id,
                                method_id: associated_id,
                                method_name: nia_symbol::SymbolId::EMPTY,
                                self_ty: receiver_ty,
                                trait_args: Vec::new(),
                                trait_const_args: Vec::new(),
                                args: vec![receiver_ty],
                                const_args: vec![const_arg(associated_const_ty, 8)],
                            },
                            args: Vec::new(),
                        },
                    )),
                ],
                terminator: FunctionTerminator::Tail {
                    value: None,
                    span: Span::default(),
                },
            }],
            entry: FunctionBlockId(0),
            ty: receiver_ty,
        };

        let refs = body.value_refs(&types);

        assert!(refs.types.contains(&method_const_ty));
        assert!(refs.types.contains(&associated_const_ty));
    }

    #[test]
    fn array_len_does_not_retain_unevaluated_receiver_value() {
        let module_id = ModuleIdAllocator::new().allocate();
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let elem_ty = append.primitive(PrimitiveTy::U8);
        let usize_ty = append.primitive(PrimitiveTy::Usize);
        let array_ty = append.intern(TyKind::Array {
            len: nia_ty::ArrayLenTy::ConstValue(4),
            elem: elem_ty,
        });
        let slice_ty = append.intern(TyKind::Slice {
            is_readonly: true,
            elem: elem_ty,
        });
        let array_global = global(module_id, 1);
        let slice_global = global(module_id, 2);
        let len_call = |self_ty, global| {
            expr(
                usize_ty,
                FunctionExprKind::Call {
                    callee: FunctionCallee::BuiltinMethod {
                        method: crate::FunctionBuiltinMethod::SliceLen,
                        self_ty,
                        receiver: Box::new(expr(self_ty, FunctionExprKind::Global(global))),
                    },
                    args: Vec::new(),
                },
            )
        };
        let body = FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span: Span::default(),
                ops: vec![FunctionOp::Expr(len_call(array_ty, array_global))],
                terminator: FunctionTerminator::Return {
                    value: Some(len_call(slice_ty, slice_global)),
                    span: Span::default(),
                },
            }],
            entry: FunctionBlockId(0),
            ty: usize_ty,
        };

        let refs = body.value_refs(&types);

        assert_eq!(refs.globals, BTreeSet::from([slice_global]));
        assert!(refs.types.contains(&array_ty));
        assert!(refs.types.contains(&slice_ty));
    }

    #[test]
    fn instance_keys_ignore_reference_spans() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let def_id = global(module_id, 1);
        let types = TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize);
        let mut reference = FunctionInstanceRef {
            def_id,
            arg_module_id: module_id,
            self_arg: None,
            args: vec![ty],
            const_args: Vec::new(),
            span: Span::new(1, 2),
        };
        let key = reference.key();
        reference.span = Span::new(3, 4);

        assert_eq!(reference.key(), key);
    }

    #[test]
    fn union_storage_relocation_pointee_participates_in_reachability() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let types = TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize);
        let pointee_global = global(module_id, 1);
        let body = FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span: Span::default(),
                ops: vec![FunctionOp::Expr(expr(
                    ty,
                    FunctionExprKind::UnionStorageLiteral {
                        bytes: vec![None; 8],
                        relocations: vec![crate::FunctionUnionRelocation {
                            offset: 0,
                            width: 8,
                            allocation: crate::PromotedAllocationId::new(
                                module_id,
                                Span::new(4, 7),
                            ),
                            pointee: Box::new(expr(ty, FunctionExprKind::Global(pointee_global))),
                        }],
                    },
                ))],
                terminator: FunctionTerminator::Tail {
                    value: None,
                    span: Span::default(),
                },
            }],
            entry: FunctionBlockId(0),
            ty,
        };

        let refs = body.value_refs(&types);

        assert_eq!(refs.globals, BTreeSet::from([pointee_global]));
        assert_eq!(refs.modules, BTreeSet::from([module_id]));
        assert!(refs.types.contains(&ty));
    }

    #[test]
    fn error_nodes_are_reported_without_panicking_reference_traversal() {
        let module_id = ModuleIdAllocator::new().allocate();
        let types = TypeStore::new();
        let ty = types
            .append_for_module(module_id)
            .primitive(PrimitiveTy::Usize);
        let body = FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks: vec![FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span: Span::default(),
                ops: vec![FunctionOp::Expr(expr(ty, FunctionExprKind::Error))],
                terminator: FunctionTerminator::Tail {
                    value: Some(expr(
                        ty,
                        FunctionExprKind::AddrOf(FunctionPlace {
                            span: Span::default(),
                            ty,
                            base: FunctionPlaceBase::Local(nia_ids::LocalId(0)),
                            elems: vec![FunctionPlaceElem::Error],
                        }),
                    )),
                    span: Span::default(),
                },
            }],
            entry: FunctionBlockId(0),
            ty,
        };

        let refs = body.value_refs(&types);

        assert!(refs.invalid_ir);
        assert!(refs.types.contains(&ty));
    }
}
