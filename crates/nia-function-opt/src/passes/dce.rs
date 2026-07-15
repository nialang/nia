use super::*;

pub(crate) fn remove_noop_local_stores(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        let before = block.ops.len();
        block.ops.retain(|op| !is_noop_local_store(op));
        changed |= block.ops.len() != before;
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                changed |= remove_noop_local_stores(&mut body.blocks);
            }
        }
    }
    changed
}

pub(crate) fn remove_zst_local_runtime_ops(
    body: &mut FunctionBody,
    is_zero_sized: impl Fn(InternedTyId) -> bool + Copy,
) -> bool {
    let zst_locals = body
        .locals
        .iter()
        .filter(|local| is_zero_sized(local.ty))
        .map(|local| local.id)
        .collect::<HashSet<_>>();
    if zst_locals.is_empty() {
        return false;
    }
    remove_zst_local_runtime_ops_in_blocks(&mut body.blocks, &zst_locals, is_zero_sized)
}

pub(crate) fn remove_zst_local_runtime_ops_in_blocks(
    blocks: &mut [FunctionBlock],
    zst_locals: &HashSet<LocalId>,
    is_zero_sized: impl Fn(InternedTyId) -> bool + Copy,
) -> bool {
    let mut changed = false;
    for block in blocks {
        let mut replacement_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op {
                FunctionOp::Binding(binding) if zst_locals.contains(&binding.local_id) => {
                    changed = true;
                    if let Some(value) = binding.value
                        && !is_pure_discardable_expr(&value)
                    {
                        replacement_ops.push(FunctionOp::Expr(value));
                    }
                }
                FunctionOp::StoreLocal {
                    local_id, value, ..
                } if zst_locals.contains(&local_id) => {
                    changed = true;
                    if !is_pure_discardable_expr(&value) {
                        replacement_ops.push(FunctionOp::Expr(value));
                    }
                }
                FunctionOp::Expr(FunctionExpr {
                    span,
                    ty,
                    kind: FunctionExprKind::Assign { place, rhs, .. },
                }) if is_direct_zst_local_place(&place, zst_locals, is_zero_sized) => {
                    changed = true;
                    if !is_pure_discardable_expr(&rhs) {
                        replacement_ops.push(FunctionOp::Expr(FunctionExpr {
                            span,
                            ty,
                            kind: FunctionExprKind::Discard(rhs),
                        }));
                    }
                }
                FunctionOp::Defer(mut body) => {
                    changed |= remove_zst_local_runtime_ops_in_blocks(
                        &mut body.blocks,
                        zst_locals,
                        is_zero_sized,
                    );
                    replacement_ops.push(FunctionOp::Defer(body));
                }
                other => replacement_ops.push(other),
            }
        }
        block.ops = replacement_ops;
    }
    changed
}

pub(crate) fn is_direct_zst_local_place(
    place: &FunctionPlace,
    zst_locals: &HashSet<LocalId>,
    is_zero_sized: impl Fn(InternedTyId) -> bool,
) -> bool {
    matches!(place.base, FunctionPlaceBase::Local(local_id) if zst_locals.contains(&local_id))
        && place.elems.is_empty()
        && is_zero_sized(place.ty)
}

pub(crate) fn remove_unused_local_bindings(body: &mut FunctionBody) -> bool {
    remove_unused_bindings_matching(body, |_| true)
}

pub(crate) fn remove_unused_temp_bindings(body: &mut FunctionBody) -> bool {
    remove_unused_bindings_matching(body, |local| {
        matches!(local.name, nia_function_ir::LocalName::Temporary(_))
    })
}

pub(crate) fn remove_unused_bindings_matching(
    body: &mut FunctionBody,
    is_candidate: impl Fn(&nia_function_ir::FunctionLocal) -> bool + Copy,
) -> bool {
    let mut changed = false;
    while remove_unused_bindings_matching_once(body, is_candidate) {
        changed = true;
    }
    changed
}

pub(crate) fn remove_unused_bindings_matching_once(
    body: &mut FunctionBody,
    is_candidate: impl Fn(&nia_function_ir::FunctionLocal) -> bool,
) -> bool {
    let referenced_locals = collect_referenced_locals(body);
    let removable_locals = body
        .locals
        .iter()
        .filter(|local| matches!(local.kind, FunctionLocalKind::MutableBinding))
        .filter(|local| is_candidate(local))
        .filter(|local| !referenced_locals.contains(&local.id))
        .map(|local| local.id)
        .collect::<HashSet<_>>();
    if removable_locals.is_empty() {
        return false;
    }

    let mut changed = remove_unused_local_binding_ops(&mut body.blocks, &removable_locals);
    let before = body.locals.len();
    body.locals
        .retain(|local| !removable_locals.contains(&local.id));
    changed |= body.locals.len() != before;
    changed
}

pub(crate) fn remove_unused_local_binding_ops(
    blocks: &mut [FunctionBlock],
    removable_locals: &HashSet<LocalId>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        let mut replacement_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op {
                FunctionOp::Binding(binding) if removable_locals.contains(&binding.local_id) => {
                    changed = true;
                    if let Some(value) = binding.value
                        && !is_pure_discardable_expr(&value)
                    {
                        replacement_ops.push(FunctionOp::Expr(value));
                    }
                }
                FunctionOp::Defer(mut body) => {
                    changed |= remove_unused_local_binding_ops(&mut body.blocks, removable_locals);
                    replacement_ops.push(FunctionOp::Defer(body));
                }
                other => replacement_ops.push(other),
            }
        }
        block.ops = replacement_ops;
    }
    changed
}

pub(crate) fn remove_overwritten_local_stores(blocks: &mut [FunctionBlock]) -> bool {
    let mut changed = false;
    for block in blocks {
        changed |= remove_overwritten_local_stores_in_block(block);
    }
    changed
}

pub(crate) fn remove_overwritten_local_stores_in_block(block: &mut FunctionBlock) -> bool {
    let mut changed = false;
    let mut replacement_ops = Vec::with_capacity(block.ops.len());
    let mut pending_stores = HashMap::<LocalId, usize>::new();
    for op in block.ops.drain(..) {
        let read_locals = collect_read_locals_in_current_op(&op);
        for local_id in read_locals {
            pending_stores.remove(&local_id);
        }

        match op {
            FunctionOp::StoreLocal {
                local_id,
                value,
                span,
            } => {
                if let Some(previous_index) = pending_stores.insert(local_id, replacement_ops.len())
                {
                    changed = true;
                    preserve_store_value_if_needed(&mut replacement_ops[previous_index]);
                }
                replacement_ops.push(Some(FunctionOp::StoreLocal {
                    local_id,
                    value,
                    span,
                }));
            }
            FunctionOp::Defer(mut body) => {
                changed |= remove_overwritten_local_stores(&mut body.blocks);
                pending_stores.clear();
                replacement_ops.push(Some(FunctionOp::Defer(body)));
            }
            other => replacement_ops.push(Some(other)),
        }
    }
    block.ops = replacement_ops.into_iter().flatten().collect();
    changed
}

pub(crate) fn preserve_store_value_if_needed(op: &mut Option<FunctionOp>) {
    let Some(FunctionOp::StoreLocal { value, .. }) = op.take() else {
        return;
    };
    if !is_pure_discardable_expr(&value) {
        *op = Some(FunctionOp::Expr(value));
    }
}

pub(crate) fn remove_never_read_local_stores(body: &mut FunctionBody) -> bool {
    let read_locals = collect_read_locals(body);
    remove_never_read_local_stores_in_blocks(&mut body.blocks, &read_locals)
}

pub(crate) fn remove_never_read_local_stores_in_blocks(
    blocks: &mut [FunctionBlock],
    read_locals: &HashSet<LocalId>,
) -> bool {
    let mut changed = false;
    for block in blocks {
        let mut replacement_ops = Vec::with_capacity(block.ops.len());
        for op in block.ops.drain(..) {
            match op {
                FunctionOp::StoreLocal {
                    local_id, value, ..
                } if !read_locals.contains(&local_id) => {
                    changed = true;
                    if !is_pure_discardable_expr(&value) {
                        replacement_ops.push(FunctionOp::Expr(value));
                    }
                }
                FunctionOp::Defer(mut body) => {
                    changed |=
                        remove_never_read_local_stores_in_blocks(&mut body.blocks, read_locals);
                    replacement_ops.push(FunctionOp::Defer(body));
                }
                other => replacement_ops.push(other),
            }
        }
        block.ops = replacement_ops;
    }
    changed
}
