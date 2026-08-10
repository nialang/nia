use super::*;

#[derive(Debug, Clone, Copy)]
enum LocalUseKind {
    Place,
    Read,
    Referenced,
}

struct LocalUseCollector<'a> {
    kind: LocalUseKind,
    locals: &'a mut HashSet<LocalId>,
}

impl<'a> LocalUseCollector<'a> {
    fn new(kind: LocalUseKind, locals: &'a mut HashSet<LocalId>) -> Self {
        Self { kind, locals }
    }

    fn collect_body(&mut self, body: &FunctionBody) {
        self.collect_blocks(&body.blocks);
    }

    fn collect_blocks(&mut self, blocks: &[FunctionBlock]) {
        for block in blocks {
            for op in &block.ops {
                self.collect_op(op);
            }
            self.collect_terminator(&block.terminator);
        }
    }

    fn collect_op(&mut self, op: &FunctionOp) {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &binding.value {
                    self.collect_expr(value);
                }
            }
            FunctionOp::StoreLocal {
                local_id, value, ..
            } => {
                if matches!(self.kind, LocalUseKind::Place | LocalUseKind::Referenced) {
                    self.locals.insert(*local_id);
                }
                self.collect_expr(value);
            }
            FunctionOp::Expr(expr) => self.collect_expr(expr),
            FunctionOp::MemoryIntrinsic(memory) => {
                self.collect_expr(&memory.dest);
                match &memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => self.collect_expr(source),
                }
            }
            FunctionOp::Defer(body) => self.collect_blocks(&body.blocks),
        }
    }

    fn collect_terminator(&mut self, terminator: &FunctionTerminator) {
        match terminator {
            FunctionTerminator::If { cond, .. } => self.collect_expr(cond),
            FunctionTerminator::Switch { target, arms, .. } => {
                self.collect_expr(target);
                for arm in arms {
                    self.collect_expr(&arm.pattern);
                }
            }
            FunctionTerminator::Try {
                value,
                error_conversion,
                success_local,
                ..
            } => {
                self.collect_expr(value);
                if let Some(conversion) = error_conversion {
                    self.collect_expr(conversion);
                }
                if matches!(self.kind, LocalUseKind::Referenced) {
                    self.locals.insert(*success_local);
                }
            }
            FunctionTerminator::Loop { header, .. } => match header {
                FunctionForHeader::Condition(cond) => self.collect_expr(cond),
                FunctionForHeader::Infinite => {}
            },
            FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
                if let Some(value) = value {
                    self.collect_expr(value);
                }
            }
            FunctionTerminator::Error { .. }
            | FunctionTerminator::Branch { .. }
            | FunctionTerminator::Next { .. } => {}
        }
    }

    fn collect_expr(&mut self, expr: &FunctionExpr) {
        match &expr.kind {
            FunctionExprKind::Local(local_id) => {
                if matches!(self.kind, LocalUseKind::Read | LocalUseKind::Referenced) {
                    self.locals.insert(*local_id);
                }
            }
            FunctionExprKind::ConstGeneric(_) => {}
            FunctionExprKind::Range(range) => self.collect_range(range),
            FunctionExprKind::InlineAsm(asm) => self.collect_inline_asm(asm),
            FunctionExprKind::Atomic(atomic) => self.collect_atomic(atomic),
            FunctionExprKind::StaticArrayPointer { array, .. } => self.collect_expr(array),
            FunctionExprKind::ArrayLiteral { elems } => self.collect_array_elements(elems),
            FunctionExprKind::Tuple(elems) => {
                for elem in elems {
                    self.collect_expr(elem);
                }
            }
            FunctionExprKind::StructLiteral { fields, .. } => {
                for field in fields {
                    self.collect_expr(&field.value);
                }
            }
            FunctionExprKind::UnionLiteral { field, .. } => self.collect_expr(&field.value),
            FunctionExprKind::Unary { expr, .. }
            | FunctionExprKind::OptionalSome { expr }
            | FunctionExprKind::ErrorOk { expr }
            | FunctionExprKind::ErrorErr { expr }
            | FunctionExprKind::TaggedUnionTag { expr }
            | FunctionExprKind::TaggedUnionPayload { expr }
            | FunctionExprKind::Try { expr }
            | FunctionExprKind::LoadUnaligned { ptr: expr, .. }
            | FunctionExprKind::Splat { value: expr }
            | FunctionExprKind::Bitmask { vector: expr }
            | FunctionExprKind::BitIntrinsic { value: expr, .. }
            | FunctionExprKind::CharFromU32 { value: expr }
            | FunctionExprKind::Discard(expr)
            | FunctionExprKind::Cast { expr, .. }
            | FunctionExprKind::TraitObjectUpcast { expr, .. }
            | FunctionExprKind::TraitObjectCoercion { expr, .. }
            | FunctionExprKind::RangeBound { range: expr, .. }
            | FunctionExprKind::Field { lhs: expr, .. }
            | FunctionExprKind::TupleField { value: expr, .. } => self.collect_expr(expr),
            FunctionExprKind::CallableCoercion { state, .. } => self.collect_expr(state),
            FunctionExprKind::AddrOf(place) => self.collect_place(place),
            FunctionExprKind::Binary { lhs, rhs, .. }
            | FunctionExprKind::Index { lhs, index: rhs } => {
                self.collect_expr(lhs);
                self.collect_expr(rhs);
            }
            FunctionExprKind::ExtractElement { vector, index } => {
                self.collect_expr(vector);
                self.collect_expr(index);
            }
            FunctionExprKind::InsertElement {
                vector,
                index,
                value,
            } => {
                self.collect_expr(vector);
                self.collect_expr(index);
                self.collect_expr(value);
            }
            FunctionExprKind::Assign { place, rhs, .. } => {
                self.collect_place(place);
                self.collect_expr(rhs);
            }
            FunctionExprKind::Call { callee, args } => {
                self.collect_callee(callee);
                for arg in args {
                    self.collect_expr(arg);
                }
            }
            FunctionExprKind::Slice { lhs, range, .. } => {
                self.collect_expr(lhs);
                self.collect_slice_range(range);
            }
            FunctionExprKind::EnumVariant { fields, .. } => {
                for field in fields {
                    self.collect_expr(field);
                }
            }
            FunctionExprKind::EnumTag { value }
            | FunctionExprKind::EnumPayloadField { value, .. } => self.collect_expr(value),
            FunctionExprKind::Error
            | FunctionExprKind::Trap
            | FunctionExprKind::Integer(_)
            | FunctionExprKind::Float(_)
            | FunctionExprKind::String(_)
            | FunctionExprKind::ByteString(_)
            | FunctionExprKind::Char(_)
            | FunctionExprKind::ByteChar(_)
            | FunctionExprKind::Bool(_)
            | FunctionExprKind::Null
            | FunctionExprKind::Global(_)
            | FunctionExprKind::GlobalInstance { .. }
            | FunctionExprKind::Function(_)
            | FunctionExprKind::FunctionInstance { .. }
            | FunctionExprKind::ClosureFunctionPointer { .. }
            | FunctionExprKind::EnumVariantTag(_)
            | FunctionExprKind::BuiltinValue(_) => {}
            FunctionExprKind::UnionStorageLiteral { relocations, .. } => {
                for relocation in relocations {
                    self.collect_expr(&relocation.pointee);
                }
            }
        }
    }

    fn collect_atomic(&mut self, atomic: &nia_function_ir::FunctionAtomic) {
        match atomic {
            nia_function_ir::FunctionAtomic::Load { ptr, .. } => self.collect_expr(ptr),
            nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
            | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
                self.collect_expr(ptr);
                self.collect_expr(value);
            }
            nia_function_ir::FunctionAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.collect_expr(ptr);
                self.collect_expr(expected);
                self.collect_expr(desired);
            }
            nia_function_ir::FunctionAtomic::Fence { .. } => {}
        }
    }

    fn collect_inline_asm(&mut self, asm: &FunctionInlineAsm) {
        for input in &asm.inputs {
            self.collect_expr(&input.value);
        }
        for output in &asm.outputs {
            self.collect_place(&output.place);
        }
    }

    fn collect_array_elements(&mut self, elems: &FunctionArrayElements) {
        match elems {
            FunctionArrayElements::List(elems) => {
                for elem in elems {
                    self.collect_expr(elem);
                }
            }
            FunctionArrayElements::Repeat { value, .. } => self.collect_expr(value),
        }
    }

    fn collect_callee(&mut self, callee: &FunctionCallee) {
        match callee {
            FunctionCallee::ClosureEntry {
                state: receiver, ..
            }
            | FunctionCallee::Method { receiver, .. }
            | FunctionCallee::TraitMethod { receiver, .. }
            | FunctionCallee::DynamicTraitMethod { receiver, .. }
            | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
            | FunctionCallee::BuiltinMethod { receiver, .. }
            | FunctionCallee::Callable(receiver)
            | FunctionCallee::FunctionPointer(receiver) => self.collect_expr(receiver),
            FunctionCallee::Function(_)
            | FunctionCallee::FunctionInstance { .. }
            | FunctionCallee::TraitAssociatedFunction { .. }
            | FunctionCallee::BuiltinOperator(_) => {}
        }
    }

    fn collect_place(&mut self, place: &FunctionPlace) {
        match &place.base {
            FunctionPlaceBase::Local(local_id) => {
                self.locals.insert(*local_id);
            }
            FunctionPlaceBase::Deref(expr) => {
                if matches!(self.kind, LocalUseKind::Read | LocalUseKind::Referenced) {
                    self.collect_expr(expr);
                }
            }
            FunctionPlaceBase::Global(_)
            | FunctionPlaceBase::GlobalInstance { .. }
            | FunctionPlaceBase::Error => {}
        }
        for elem in &place.elems {
            if let FunctionPlaceElem::Index(index) = elem {
                self.collect_expr(index);
            }
        }
    }

    fn collect_slice_range(&mut self, range: &FunctionSliceRange) {
        if let Some(start) = &range.start {
            self.collect_expr(start);
        }
        if let Some(end) = &range.end {
            self.collect_expr(end);
        }
    }

    fn collect_range(&mut self, range: &FunctionRange) {
        if let Some(start) = &range.start {
            self.collect_expr(start);
        }
        if let Some(end) = &range.end {
            self.collect_expr(end);
        }
    }
}

pub(crate) fn collect_place_locals_in_body(body: &FunctionBody) -> HashSet<LocalId> {
    let mut locals = HashSet::new();
    LocalUseCollector::new(LocalUseKind::Place, &mut locals).collect_body(body);
    locals
}

pub(crate) fn collect_read_locals(body: &FunctionBody) -> HashSet<LocalId> {
    let mut locals = HashSet::new();
    LocalUseCollector::new(LocalUseKind::Read, &mut locals).collect_body(body);
    locals
}

pub(crate) fn collect_read_locals_in_current_op(op: &FunctionOp) -> HashSet<LocalId> {
    let mut locals = HashSet::new();
    let mut collector = LocalUseCollector::new(LocalUseKind::Read, &mut locals);
    match op {
        FunctionOp::StoreLocal { value, .. } => collector.collect_expr(value),
        other => collector.collect_op(other),
    }
    locals
}

pub(crate) fn collect_referenced_locals(body: &FunctionBody) -> HashSet<LocalId> {
    let mut refs = HashSet::new();
    LocalUseCollector::new(LocalUseKind::Referenced, &mut refs).collect_body(body);
    refs
}

pub(crate) fn is_noop_local_store(op: &FunctionOp) -> bool {
    matches!(
        op,
        FunctionOp::StoreLocal {
            local_id,
            value:
                FunctionExpr {
                    kind: FunctionExprKind::Local(value_local),
                    ..
                },
            ..
        } if local_id == value_local
    )
}
