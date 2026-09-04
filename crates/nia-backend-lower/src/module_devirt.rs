// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{BackendOptimizationChange, ModuleLowerer};
use nia_backend_ir::{BackendFunction, BackendFunctionInstance};
use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBody, FunctionCallee, FunctionDeferBody,
    FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm, FunctionOp,
    FunctionPlace, FunctionPlaceBase, FunctionPlaceElem, FunctionTerminator,
};
use nia_ids::{GlobalDefId, TraitId};
use nia_opt::NiaOptimizationLevel;

pub(crate) const DEVIRTUALIZE_DIRECT_TRAIT_CALLS_PASS: &str = "devirtualize-direct-trait-calls";

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn devirtualize_direct_trait_calls(
        &mut self,
        functions: &mut [BackendFunction],
        function_instances: &mut [BackendFunctionInstance],
    ) {
        if self.optimization.level != NiaOptimizationLevel::O3 {
            return;
        }

        for function in functions {
            if let Some(body) = &mut function.function_body
                && self.devirtualize_direct_trait_calls_in_body(body)
            {
                self.record_devirtualization(function.def_id, false, 0);
            }
        }
        for instance in function_instances {
            if let Some(body) = &mut instance.function_body
                && self.devirtualize_direct_trait_calls_in_body(body)
            {
                self.record_devirtualization(instance.def_id, true, instance.args.len());
            }
        }
    }

    fn devirtualize_direct_trait_calls_in_body(&mut self, body: &mut FunctionBody) -> bool {
        self.devirtualize_direct_trait_calls_in_blocks(&mut body.blocks)
    }

    fn devirtualize_direct_trait_calls_in_defer_body(
        &mut self,
        body: &mut FunctionDeferBody,
    ) -> bool {
        self.devirtualize_direct_trait_calls_in_blocks(&mut body.blocks)
    }

    fn devirtualize_direct_trait_calls_in_blocks(&mut self, blocks: &mut [FunctionBlock]) -> bool {
        let mut changed = false;
        for block in blocks {
            for op in &mut block.ops {
                changed |= self.devirtualize_direct_trait_calls_in_op(op);
            }
            changed |= self.devirtualize_direct_trait_calls_in_terminator(&mut block.terminator);
        }
        changed
    }

    fn devirtualize_direct_trait_calls_in_op(&mut self, op: &mut FunctionOp) -> bool {
        match op {
            FunctionOp::Binding(binding) => binding
                .value
                .as_mut()
                .is_some_and(|value| self.devirtualize_direct_trait_calls_in_expr(value)),
            FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => {
                self.devirtualize_direct_trait_calls_in_expr(value)
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                let mut changed = self.devirtualize_direct_trait_calls_in_expr(&mut memory.dest);
                changed |= match &mut memory.source {
                    nia_function_ir::FunctionMemoryIntrinsicSource::Slice(source)
                    | nia_function_ir::FunctionMemoryIntrinsicSource::Byte(source) => {
                        self.devirtualize_direct_trait_calls_in_expr(source)
                    }
                };
                changed
            }
            FunctionOp::Defer(body) => self.devirtualize_direct_trait_calls_in_defer_body(body),
        }
    }

    fn devirtualize_direct_trait_calls_in_terminator(
        &mut self,
        terminator: &mut FunctionTerminator,
    ) -> bool {
        match terminator {
            FunctionTerminator::If { cond, .. } => {
                self.devirtualize_direct_trait_calls_in_expr(cond)
            }
            FunctionTerminator::Switch { target, arms, .. } => {
                let mut changed = self.devirtualize_direct_trait_calls_in_expr(target);
                for arm in arms {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(&mut arm.pattern);
                }
                changed
            }
            FunctionTerminator::Try {
                value,
                error_conversion,
                ..
            } => {
                let mut changed = self.devirtualize_direct_trait_calls_in_expr(value);
                if let Some(conversion) = error_conversion {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(conversion);
                }
                changed
            }
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Condition(cond) => {
                    self.devirtualize_direct_trait_calls_in_expr(cond)
                }
                FunctionForHeader::Infinite => false,
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                value
                    .as_mut()
                    .is_some_and(|value| self.devirtualize_direct_trait_calls_in_expr(value))
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => false,
        }
    }

    fn devirtualize_direct_trait_calls_in_expr(&mut self, expr: &mut FunctionExpr) -> bool {
        let mut changed = false;
        match &mut expr.kind {
            FunctionExprKind::Call { callee, args } => {
                changed |= self.devirtualize_direct_trait_calls_in_callee(callee);
                for arg in args {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(arg);
                }
                if let Some((receiver, def_id, receiver_kind)) =
                    self.direct_trait_call_target(callee)
                {
                    *callee = FunctionCallee::Method {
                        def_id,
                        arg_module_id: self.input.module_id,
                        self_arg: None,
                        args: Vec::new(),
                        const_args: Vec::new(),
                        // The dynamic call already carries the ABI receiver mode selected
                        // by body checking. Devirtualization changes only the dispatch target;
                        // rewriting this metadata would change `&self`/value ABI lowering.
                        receiver_kind,
                        receiver,
                    };
                    changed = true;
                }
            }
            FunctionExprKind::Range(range) => {
                if let Some(start) = &mut range.start {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(start);
                }
                if let Some(end) = &mut range.end {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(end);
                }
            }
            FunctionExprKind::InlineAsm(asm) => {
                changed |= self.devirtualize_direct_trait_calls_in_inline_asm(asm);
            }
            FunctionExprKind::Atomic(atomic) => {
                changed |= self.devirtualize_direct_trait_calls_in_atomic(atomic);
            }
            FunctionExprKind::StaticArrayPointer { array, .. }
            | FunctionExprKind::RangeBound { range: array, .. }
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
            | FunctionExprKind::Cast { expr: array, .. }
            | FunctionExprKind::TraitObjectUpcast { expr: array, .. }
            | FunctionExprKind::TraitObjectCoercion { expr: array, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(array);
            }
            FunctionExprKind::CallableCoercion { state, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(state);
            }
            FunctionExprKind::FunctionCallable { function } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(function);
            }
            FunctionExprKind::ClosureFunctionPointer { .. } => {}
            FunctionExprKind::ArrayLiteral { elems } => match elems {
                FunctionArrayElements::List(elems) => {
                    for elem in elems {
                        changed |= self.devirtualize_direct_trait_calls_in_expr(elem);
                    }
                }
                FunctionArrayElements::Repeat { value, .. } => {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(value);
                }
            },
            FunctionExprKind::Tuple(elems) => {
                for elem in elems {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(elem);
                }
            }
            FunctionExprKind::TupleField { value, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(value);
            }
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(&mut field.value);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(&mut field.value);
            }
            FunctionExprKind::AddrOf(place) => {
                changed |= self.devirtualize_direct_trait_calls_in_place(place);
            }
            FunctionExprKind::Binary { lhs, rhs, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
                changed |= self.devirtualize_direct_trait_calls_in_expr(rhs);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(vector);
                changed |= self.devirtualize_direct_trait_calls_in_expr(index);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(vector);
                changed |= self.devirtualize_direct_trait_calls_in_expr(index);
                changed |= self.devirtualize_direct_trait_calls_in_expr(value);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_place(place);
                changed |= self.devirtualize_direct_trait_calls_in_expr(rhs);
            }
            FunctionExprKind::Field { lhs, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
            }
            FunctionExprKind::Index { lhs, index } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
                changed |= self.devirtualize_direct_trait_calls_in_expr(index);
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(lhs);
                if let Some(start) = &mut range.start {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(start);
                }
                if let Some(end) = &mut range.end {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(end);
                }
            }
            FunctionExprKind::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionExprKind::Error")
            }
            FunctionExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    changed |= self.devirtualize_direct_trait_calls_in_expr(field);
                }
            }
            FunctionExprKind::EnumTag { value }
            | FunctionExprKind::EnumPayloadField { value, .. } => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(value);
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
            | FunctionExprKind::CallerLocation(_)
            | FunctionExprKind::BuiltinValue(_) => {}
            FunctionExprKind::UnionStorageLiteral { relocations, .. } => {
                for relocation in relocations {
                    changed |=
                        self.devirtualize_direct_trait_calls_in_expr(&mut relocation.pointee);
                }
            }
        }
        changed
    }

    fn devirtualize_direct_trait_calls_in_atomic(
        &mut self,
        atomic: &mut nia_function_ir::FunctionAtomic,
    ) -> bool {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ptr, .. } => {
                self.devirtualize_direct_trait_calls_in_expr(ptr)
            }
            nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
                self.devirtualize_direct_trait_calls_in_expr(ptr)
                    | self.devirtualize_direct_trait_calls_in_expr(value)
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.devirtualize_direct_trait_calls_in_expr(ptr)
                    | self.devirtualize_direct_trait_calls_in_expr(expected)
                    | self.devirtualize_direct_trait_calls_in_expr(desired)
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => false,
        }
    }

    fn devirtualize_direct_trait_calls_in_callee(&mut self, callee: &mut FunctionCallee) -> bool {
        match callee {
            FunctionCallee::Tracked { callee, .. } => {
                self.devirtualize_direct_trait_calls_in_callee(callee)
            }
            FunctionCallee::ClosureEntry {
                state: receiver, ..
            }
            | FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::Callable(receiver)
            | FunctionCallee::FunctionPointer(receiver) => {
                self.devirtualize_direct_trait_calls_in_expr(receiver)
            }
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::TraitAssociatedFunction { .. }
            | FunctionCallee::BuiltinOperator(_) => false,
        }
    }

    fn devirtualize_direct_trait_calls_in_place(&mut self, place: &mut FunctionPlace) -> bool {
        let mut changed = false;
        match &mut place.base {
            FunctionPlaceBase::Deref(expr) => {
                changed |= self.devirtualize_direct_trait_calls_in_expr(expr);
            }
            FunctionPlaceBase::Local(_)
            | FunctionPlaceBase::Global(_)
            | FunctionPlaceBase::GlobalInstance { .. } => {}
            FunctionPlaceBase::Error => {
                crate::input::unreachable_invalid_function_ir("FunctionPlaceBase::Error")
            }
        }
        for elem in &mut place.elems {
            if let FunctionPlaceElem::Index(expr) = elem {
                changed |= self.devirtualize_direct_trait_calls_in_expr(expr);
            }
        }
        changed
    }

    fn devirtualize_direct_trait_calls_in_inline_asm(
        &mut self,
        asm: &mut FunctionInlineAsm,
    ) -> bool {
        let mut changed = false;
        for input in &mut asm.inputs {
            changed |= self.devirtualize_direct_trait_calls_in_expr(&mut input.value);
        }
        for output in &mut asm.outputs {
            changed |= self.devirtualize_direct_trait_calls_in_place(&mut output.place);
        }
        changed
    }

    fn direct_trait_call_target(
        &mut self,
        callee: &FunctionCallee,
    ) -> Option<(Box<FunctionExpr>, GlobalDefId, nia_ids::ReceiverKind)> {
        let FunctionCallee::DynamicTraitMethod {
            trait_id: TraitId::Source(trait_def_id),
            method_id,
            method_name,
            trait_args,
            trait_const_args,
            receiver_kind,
            receiver,
            ..
        } = callee
        else {
            return None;
        };
        let FunctionExprKind::TraitObjectCoercion {
            expr,
            target_ty: _,
            self_ty,
        } = &receiver.kind
        else {
            return None;
        };
        let (def_id, args, const_args) = self.resolve_trait_method_impl(
            *trait_def_id,
            trait_args,
            trait_const_args,
            *method_id,
            method_name,
            *self_ty,
        )?;
        // A direct `Method` callee can be emitted here only when resolution
        // selected the non-generic definition itself. Generic impl results
        // need their complete instance arguments and remain dynamic until
        // the normal instantiation path materializes that identity.
        (args.is_empty() && const_args.is_empty()).then(|| (expr.clone(), def_id, *receiver_kind))
    }

    fn record_devirtualization(
        &mut self,
        function: GlobalDefId,
        is_instance: bool,
        type_arg_count: usize,
    ) {
        self.optimization_report
            .changed_passes
            .push(BackendOptimizationChange::Function {
                module_id: self.input.module_id,
                function,
                pass: DEVIRTUALIZE_DIRECT_TRAIT_CALLS_PASS,
                is_instance,
                type_arg_count,
            });
    }
}
