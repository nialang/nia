use super::*;

pub(crate) fn propagate_local_copies(body: &mut FunctionBody) -> bool {
    let local_tys = body
        .locals
        .iter()
        .map(|local| (local.id, local.ty))
        .collect::<HashMap<_, _>>();
    let unstable_locals = collect_place_locals_in_body(body);
    propagate_local_copies_in_blocks(&mut body.blocks, body.entry, &local_tys, &unstable_locals)
}

pub(crate) fn propagate_local_copies_in_defer_body(
    body: &mut FunctionDeferBody,
    local_tys: &HashMap<LocalId, nia_ids::InternedTyId>,
    unstable_locals: &HashSet<LocalId>,
) -> bool {
    propagate_local_copies_in_blocks(&mut body.blocks, body.entry, local_tys, unstable_locals)
}

pub(crate) fn propagate_local_copies_in_blocks(
    blocks: &mut [FunctionBlock],
    entry: FunctionBlockId,
    local_tys: &HashMap<LocalId, nia_ids::InternedTyId>,
    unstable_locals: &HashSet<LocalId>,
) -> bool {
    let cfg = FunctionCfg::new(blocks);
    let mut changed = false;
    let mut input_copies = HashMap::<FunctionBlockId, HashMap<LocalId, LocalId>>::new();
    let mut stack = vec![entry];
    let mut visited = HashSet::new();
    while let Some(block_id) = stack.pop() {
        if !visited.insert(block_id) {
            continue;
        }
        let Some(index) = cfg.block(block_id) else {
            continue;
        };
        let mut copies = input_copies.remove(&block_id).unwrap_or_default();
        changed |= propagate_local_copies_in_block(
            &mut blocks[index],
            local_tys,
            unstable_locals,
            &mut copies,
        );
        for successor in cfg.referenced_blocks(&blocks[index].terminator) {
            let preds = cfg.predecessors(successor);
            if preds.len() == 1 && preds[0] == block_id {
                input_copies.insert(successor, copies.clone());
            }
            stack.push(successor);
        }
    }
    changed
}

pub(crate) fn propagate_local_copies_in_block(
    block: &mut FunctionBlock,
    local_tys: &HashMap<LocalId, nia_ids::InternedTyId>,
    unstable_locals: &HashSet<LocalId>,
    copies: &mut HashMap<LocalId, LocalId>,
) -> bool {
    let mut changed = false;
    for op in &mut block.ops {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &mut binding.value {
                    changed |= rewrite_local_copies_in_expr(value, copies);
                }
                invalidate_local_copy(copies, binding.local_id);
                if let Some(value) = &binding.value
                    && let Some(source) = copy_source_from_expr(value, copies)
                    && can_copy_local(binding.local_id, source, local_tys, unstable_locals)
                {
                    copies.insert(binding.local_id, source);
                }
            }
            FunctionOp::StoreLocal {
                local_id, value, ..
            } => {
                changed |= rewrite_local_copies_in_expr(value, copies);
                invalidate_local_copy(copies, *local_id);
                if let Some(source) = copy_source_from_expr(value, copies)
                    && can_copy_local(*local_id, source, local_tys, unstable_locals)
                {
                    copies.insert(*local_id, source);
                }
            }
            FunctionOp::Expr(_) | FunctionOp::Defer(_) => {
                if let FunctionOp::Defer(body) = op {
                    changed |=
                        propagate_local_copies_in_defer_body(body, local_tys, unstable_locals);
                }
                copies.clear();
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                changed |= rewrite_local_copies_in_expr(&mut memory.dest, copies);
                changed |= match &mut memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => {
                        rewrite_local_copies_in_expr(source, copies)
                    }
                };
                copies.clear();
            }
        }
    }
    changed |= rewrite_local_copies_in_terminator(&mut block.terminator, copies);
    changed
}

pub(crate) fn copy_source_from_expr(
    expr: &FunctionExpr,
    copies: &HashMap<LocalId, LocalId>,
) -> Option<LocalId> {
    let FunctionExprKind::Local(local_id) = expr.kind else {
        return None;
    };
    Some(resolve_copy_source(local_id, copies))
}

pub(crate) fn can_copy_local(
    dest: LocalId,
    source: LocalId,
    local_tys: &HashMap<LocalId, nia_ids::InternedTyId>,
    unstable_locals: &HashSet<LocalId>,
) -> bool {
    dest != source
        && !unstable_locals.contains(&dest)
        && !unstable_locals.contains(&source)
        && local_tys.get(&dest).is_some()
        && local_tys.get(&dest) == local_tys.get(&source)
}

pub(crate) fn invalidate_local_copy(copies: &mut HashMap<LocalId, LocalId>, local_id: LocalId) {
    copies.remove(&local_id);
    copies.retain(|dest, source| *dest != local_id && *source != local_id);
}

pub(crate) fn rewrite_local_copies_in_terminator(
    terminator: &mut FunctionTerminator,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    rewrite_terminator_exprs(terminator, &mut |expr| {
        rewrite_local_copies_in_expr(expr, copies)
    })
}

pub(crate) fn rewrite_local_copies_in_expr(
    expr: &mut FunctionExpr,
    copies: &HashMap<LocalId, LocalId>,
) -> bool {
    match &mut expr.kind {
        FunctionExprKind::Local(local_id) => {
            let source = resolve_copy_source(*local_id, copies);
            if source == *local_id {
                false
            } else {
                *local_id = source;
                true
            }
        }
        _ => rewrite_expr_children(expr, ExprTraversal::values_only(), &mut |expr| {
            rewrite_local_copies_in_expr(expr, copies)
        }),
    }
}

pub(crate) fn resolve_copy_source(
    local_id: LocalId,
    copies: &HashMap<LocalId, LocalId>,
) -> LocalId {
    let mut current = local_id;
    let mut seen = HashSet::new();
    while let Some(next) = copies.get(&current).copied() {
        if !seen.insert(current) {
            break;
        }
        current = next;
    }
    current
}
