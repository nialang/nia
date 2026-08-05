use super::*;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExprTraversal {
    pub(crate) places: bool,
    pub(crate) static_array_pointer: bool,
}

impl ExprTraversal {
    pub(crate) const fn all() -> Self {
        Self {
            places: true,
            static_array_pointer: true,
        }
    }

    pub(crate) const fn values_only() -> Self {
        Self {
            places: false,
            static_array_pointer: true,
        }
    }

    pub(crate) const fn values_without_static_array_pointer() -> Self {
        Self {
            places: false,
            static_array_pointer: false,
        }
    }
}

pub(crate) fn rewrite_blocks_exprs<F>(
    blocks: &mut [FunctionBlock],
    traversal: ExprTraversal,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    let mut changed = false;
    for block in blocks {
        for op in &mut block.ops {
            changed |= rewrite_op_exprs(op, traversal, rewrite_expr);
        }
        changed |= rewrite_terminator_exprs(&mut block.terminator, rewrite_expr);
    }
    changed
}

pub(crate) fn rewrite_op_exprs<F>(
    op: &mut FunctionOp,
    traversal: ExprTraversal,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    match op {
        FunctionOp::Binding(binding) => {
            if let Some(value) = binding.value.as_mut() {
                rewrite_expr(value)
            } else {
                false
            }
        }
        FunctionOp::StoreLocal { value, .. } | FunctionOp::Expr(value) => rewrite_expr(value),
        FunctionOp::MemoryIntrinsic(memory) => {
            let mut changed = rewrite_expr(&mut memory.dest);
            changed |= match &mut memory.source {
                FunctionMemoryIntrinsicSource::Slice(source)
                | FunctionMemoryIntrinsicSource::Byte(source) => rewrite_expr(source),
            };
            changed
        }
        FunctionOp::Defer(body) => rewrite_blocks_exprs(&mut body.blocks, traversal, rewrite_expr),
    }
}

pub(crate) fn rewrite_terminator_exprs<F>(
    terminator: &mut FunctionTerminator,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    match terminator {
        FunctionTerminator::If { cond, .. } => rewrite_expr(cond),
        FunctionTerminator::Switch { target, arms, .. } => {
            let mut changed = rewrite_expr(target);
            for arm in arms {
                changed |= rewrite_expr(&mut arm.pattern);
            }
            changed
        }
        FunctionTerminator::Loop { header, .. } => match header {
            FunctionForHeader::Condition(cond) => rewrite_expr(cond),
            FunctionForHeader::Infinite => false,
        },
        FunctionTerminator::Try { value, .. } => rewrite_expr(value),
        FunctionTerminator::Return { value, .. } | FunctionTerminator::Tail { value, .. } => {
            value.as_mut().is_some_and(rewrite_expr)
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Branch { .. }
        | FunctionTerminator::Next { .. } => false,
    }
}

pub(crate) fn rewrite_expr_children<F>(
    expr: &mut FunctionExpr,
    traversal: ExprTraversal,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    match &mut expr.kind {
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
        | FunctionExprKind::ConstGeneric(_)
        | FunctionExprKind::Local(_)
        | FunctionExprKind::Global(_)
        | FunctionExprKind::GlobalInstance { .. }
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::EnumVariantTag(_)
        | FunctionExprKind::UnionStorageLiteral { .. }
        | FunctionExprKind::BuiltinValue(_) => false,
        FunctionExprKind::EnumVariant { fields, .. } => {
            let mut changed = false;
            for field in fields {
                changed |= rewrite_expr(field);
            }
            changed
        }
        FunctionExprKind::EnumTag { value } | FunctionExprKind::EnumPayloadField { value, .. } => {
            rewrite_expr(value)
        }
        FunctionExprKind::Range(range) => rewrite_range_exprs(range, rewrite_expr),
        FunctionExprKind::InlineAsm(asm) => rewrite_inline_asm_exprs(asm, traversal, rewrite_expr),
        FunctionExprKind::Atomic(atomic) => rewrite_atomic_exprs(atomic, rewrite_expr),
        FunctionExprKind::StaticArrayPointer { array, .. } => {
            traversal.static_array_pointer && rewrite_expr(array)
        }
        FunctionExprKind::ArrayLiteral { elems } => {
            rewrite_array_elements_exprs(elems, rewrite_expr)
        }
        FunctionExprKind::StructLiteral { fields, .. } => {
            let mut changed = false;
            for field in fields {
                changed |= rewrite_expr(&mut field.value);
            }
            changed
        }
        FunctionExprKind::UnionLiteral { field, .. } => rewrite_expr(&mut field.value),
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
        | FunctionExprKind::Field { lhs: expr, .. } => rewrite_expr(expr),
        FunctionExprKind::AddrOf(place) => {
            traversal.places && rewrite_place_exprs(place, rewrite_expr)
        }
        FunctionExprKind::Binary { lhs, rhs, .. } | FunctionExprKind::Index { lhs, index: rhs } => {
            rewrite_expr(lhs) | rewrite_expr(rhs)
        }
        FunctionExprKind::ExtractElement { vector, index } => {
            rewrite_expr(vector) | rewrite_expr(index)
        }
        FunctionExprKind::InsertElement {
            vector,
            index,
            value,
        } => rewrite_expr(vector) | rewrite_expr(index) | rewrite_expr(value),
        FunctionExprKind::Assign { place, rhs, .. } => {
            let mut changed = false;
            if traversal.places {
                changed |= rewrite_place_exprs(place, rewrite_expr);
            }
            changed | rewrite_expr(rhs)
        }
        FunctionExprKind::Call { callee, args } => {
            let mut changed = rewrite_callee_exprs(callee, rewrite_expr);
            for arg in args {
                changed |= rewrite_expr(arg);
            }
            changed
        }
        FunctionExprKind::Slice { lhs, range, .. } => {
            rewrite_expr(lhs) | rewrite_slice_range_exprs(range, rewrite_expr)
        }
    }
}

pub(crate) fn rewrite_atomic_exprs<F>(
    atomic: &mut nia_function_ir::FunctionAtomic,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    match atomic {
        nia_function_ir::FunctionAtomic::Load { ptr, .. } => rewrite_expr(ptr),
        nia_function_ir::FunctionAtomic::Store { ptr, value, .. }
        | nia_function_ir::FunctionAtomic::Rmw { ptr, value, .. } => {
            rewrite_expr(ptr) | rewrite_expr(value)
        }
        nia_function_ir::FunctionAtomic::Cmpxchg {
            ptr,
            expected,
            desired,
            ..
        } => rewrite_expr(ptr) | rewrite_expr(expected) | rewrite_expr(desired),
        nia_function_ir::FunctionAtomic::Fence { .. } => false,
    }
}

pub(crate) fn rewrite_inline_asm_exprs<F>(
    asm: &mut FunctionInlineAsm,
    traversal: ExprTraversal,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    let mut changed = false;
    for input in &mut asm.inputs {
        changed |= rewrite_expr(&mut input.value);
    }
    if traversal.places {
        for output in &mut asm.outputs {
            changed |= rewrite_place_exprs(&mut output.place, rewrite_expr);
        }
    }
    changed
}

pub(crate) fn rewrite_array_elements_exprs<F>(
    elems: &mut FunctionArrayElements,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    match elems {
        FunctionArrayElements::List(elems) => {
            let mut changed = false;
            for elem in elems {
                changed |= rewrite_expr(elem);
            }
            changed
        }
        FunctionArrayElements::Repeat { value, .. } => rewrite_expr(value),
    }
}

pub(crate) fn rewrite_callee_exprs<F>(callee: &mut FunctionCallee, rewrite_expr: &mut F) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    match callee {
        FunctionCallee::Method { receiver, .. }
        | FunctionCallee::TraitMethod { receiver, .. }
        | FunctionCallee::DynamicTraitMethod { receiver, .. }
        | FunctionCallee::BuiltinPlaceMethod { receiver, .. }
        | FunctionCallee::BuiltinMethod { receiver, .. }
        | FunctionCallee::FunctionPointer(receiver) => rewrite_expr(receiver),
        FunctionCallee::Function(_)
        | FunctionCallee::FunctionInstance { .. }
        | FunctionCallee::TraitAssociatedFunction { .. }
        | FunctionCallee::BuiltinOperator(_) => false,
    }
}

pub(crate) fn rewrite_place_exprs<F>(place: &mut FunctionPlace, rewrite_expr: &mut F) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    let mut changed = false;
    if let FunctionPlaceBase::Deref(expr) = &mut place.base {
        changed |= rewrite_expr(expr);
    }
    for elem in &mut place.elems {
        if let FunctionPlaceElem::Index(index) = elem {
            changed |= rewrite_expr(index);
        }
    }
    changed
}

pub(crate) fn rewrite_slice_range_exprs<F>(
    range: &mut FunctionSliceRange,
    rewrite_expr: &mut F,
) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    let mut changed = false;
    if let Some(start) = &mut range.start {
        changed |= rewrite_expr(start);
    }
    if let Some(end) = &mut range.end {
        changed |= rewrite_expr(end);
    }
    changed
}

pub(crate) fn rewrite_range_exprs<F>(range: &mut FunctionRange, rewrite_expr: &mut F) -> bool
where
    F: FnMut(&mut FunctionExpr) -> bool,
{
    let mut changed = false;
    if let Some(start) = &mut range.start {
        changed |= rewrite_expr(start);
    }
    if let Some(end) = &mut range.end {
        changed |= rewrite_expr(end);
    }
    changed
}
