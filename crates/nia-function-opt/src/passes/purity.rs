use super::*;

pub(crate) fn remove_pure_expr_ops(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        let before = block.ops.len();
        block.ops.retain(|op| !is_pure_expr_op(op));
        changed |= block.ops.len() != before;
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= remove_pure_expr_ops(&mut body.blocks);
            }
        }
    }
    changed
}

pub(crate) fn is_pure_expr_op(op: &FunctionOp) -> bool {
    matches!(op, FunctionOp::Expr(expr) if is_pure_discardable_expr(expr))
}

/// Classifies whether evaluating an expression can be discarded without
/// changing observable behavior.
///
/// The walk is intentionally conservative: loads and aggregate projections
/// are pure only when every nested operand is pure, while address-taking,
/// calls, atomics, inline assembly, assignments, and traps always retain the
/// operation. This keeps cleanup independent of target-specific alias facts.
pub(crate) fn is_pure_discardable_expr(expr: &FunctionExpr) -> bool {
    match &expr.kind {
        FunctionExprKind::Error => false,
        FunctionExprKind::Trap => false,
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
        | FunctionExprKind::Global(_)
        | FunctionExprKind::GlobalInstance { .. }
        | FunctionExprKind::Function(_)
        | FunctionExprKind::FunctionInstance { .. }
        | FunctionExprKind::ClosureFunctionPointer { .. }
        | FunctionExprKind::EnumVariantTag(_)
        | FunctionExprKind::BuiltinValue(_)
        | FunctionExprKind::CallerLocation(_) => true,
        FunctionExprKind::UnionStorageLiteral { relocations, .. } => relocations
            .iter()
            .all(|relocation| is_pure_discardable_expr(&relocation.pointee)),
        FunctionExprKind::EnumVariant { fields, .. } => fields.iter().all(is_pure_discardable_expr),
        FunctionExprKind::EnumTag { value } | FunctionExprKind::EnumPayloadField { value, .. } => {
            is_pure_discardable_expr(value)
        }
        FunctionExprKind::Discard(expr) => is_pure_discardable_expr(expr),
        FunctionExprKind::RangeBound { range, .. } => is_pure_discardable_expr(range),
        FunctionExprKind::Range(range) => {
            range.start.as_deref().is_none_or(is_pure_discardable_expr)
                && range.end.as_deref().is_none_or(is_pure_discardable_expr)
        }
        FunctionExprKind::ArrayLiteral { elems } => is_pure_discardable_array_elements(elems),
        FunctionExprKind::Tuple(elems) => elems.iter().all(is_pure_discardable_expr),
        FunctionExprKind::TupleField { value, .. } => is_pure_discardable_expr(value),
        FunctionExprKind::StructLiteral { fields, .. } => fields
            .iter()
            .all(|field| is_pure_discardable_expr(&field.value)),
        FunctionExprKind::UnionLiteral { field, .. } => is_pure_discardable_expr(&field.value),
        FunctionExprKind::OptionalSome { expr }
        | FunctionExprKind::ErrorOk { expr }
        | FunctionExprKind::ErrorErr { expr }
        | FunctionExprKind::TaggedUnionTag { expr }
        | FunctionExprKind::TaggedUnionPayload { expr }
        | FunctionExprKind::Unary { expr, .. }
        | FunctionExprKind::Splat { value: expr }
        | FunctionExprKind::Bitmask { vector: expr }
        | FunctionExprKind::BitIntrinsic { value: expr, .. }
        | FunctionExprKind::CharFromU32 { value: expr }
        | FunctionExprKind::Cast { expr, .. } => is_pure_discardable_expr(expr),
        FunctionExprKind::Binary { lhs, rhs, .. } | FunctionExprKind::Index { lhs, index: rhs } => {
            is_pure_discardable_expr(lhs) && is_pure_discardable_expr(rhs)
        }
        FunctionExprKind::ExtractElement { vector, index } => {
            is_pure_discardable_expr(vector) && is_pure_discardable_expr(index)
        }
        FunctionExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            is_pure_discardable_expr(vector)
                && is_pure_discardable_expr(index)
                && is_pure_discardable_expr(value)
        }
        FunctionExprKind::Field { lhs, .. } => is_pure_discardable_expr(lhs),
        FunctionExprKind::Slice { lhs, range, .. } => {
            is_pure_discardable_expr(lhs)
                && range.start.as_deref().is_none_or(is_pure_discardable_expr)
                && range.end.as_deref().is_none_or(is_pure_discardable_expr)
        }
        FunctionExprKind::InlineAsm(_)
        | FunctionExprKind::Atomic(_)
        | FunctionExprKind::LoadUnaligned { .. }
        | FunctionExprKind::StaticArrayPointer { .. }
        | FunctionExprKind::AddrOf(_)
        | FunctionExprKind::Assign { .. }
        | FunctionExprKind::Try { .. }
        | FunctionExprKind::TraitObjectUpcast { .. }
        | FunctionExprKind::TraitObjectCoercion { .. }
        | FunctionExprKind::CallableCoercion { .. }
        | FunctionExprKind::FunctionCallable { .. }
        | FunctionExprKind::Call { .. } => false,
    }
}

pub(crate) fn is_pure_discardable_array_elements(elems: &FunctionArrayElements) -> bool {
    match elems {
        FunctionArrayElements::List(elems) => elems.iter().all(is_pure_discardable_expr),
        FunctionArrayElements::Repeat { value, .. } => is_pure_discardable_expr(value),
    }
}
