use super::*;

pub(crate) fn propagate_local_constants(body: &mut FunctionBody) -> bool {
    let unstable_locals = collect_place_locals_in_body(body);
    propagate_local_constants_in_blocks(&mut body.blocks, body.entry, &unstable_locals)
}

pub(crate) fn propagate_local_constants_in_defer_body(
    body: &mut FunctionDeferBody,
    unstable_locals: &HashSet<LocalId>,
) -> bool {
    propagate_local_constants_in_blocks(&mut body.blocks, body.entry, unstable_locals)
}

pub(crate) fn propagate_local_constants_in_blocks(
    blocks: &mut [FunctionBlock],
    entry: FunctionBlockId,
    unstable_locals: &HashSet<LocalId>,
) -> bool {
    let cfg = FunctionCfg::new(blocks);
    let mut changed = false;
    let mut input_constants = HashMap::<FunctionBlockId, HashMap<LocalId, FunctionExpr>>::new();
    let mut stack = vec![entry];
    let mut visited = HashSet::new();
    while let Some(block_id) = stack.pop() {
        if !visited.insert(block_id) {
            continue;
        }
        let Some(index) = cfg.block(block_id) else {
            continue;
        };
        let mut constants = input_constants.remove(&block_id).unwrap_or_default();
        changed |=
            propagate_local_constants_in_block(&mut blocks[index], unstable_locals, &mut constants);
        for successor in cfg.referenced_blocks(&blocks[index].terminator) {
            let preds = cfg.predecessors(successor);
            if preds.len() == 1 && preds[0] == block_id {
                input_constants.insert(successor, constants.clone());
            }
            stack.push(successor);
        }
    }
    changed
}

pub(crate) fn propagate_local_constants_in_block(
    block: &mut FunctionBlock,
    unstable_locals: &HashSet<LocalId>,
    constants: &mut HashMap<LocalId, FunctionExpr>,
) -> bool {
    let mut changed = false;
    for op in &mut block.ops {
        match op {
            FunctionOp::Binding(binding) => {
                if let Some(value) = &mut binding.value {
                    changed |= rewrite_local_constants_in_expr(value, constants);
                }
                constants.remove(&binding.local_id);
                if !unstable_locals.contains(&binding.local_id)
                    && let Some(value) = &binding.value
                    && let Some(value) = local_constant_value(value)
                {
                    constants.insert(binding.local_id, value);
                }
            }
            FunctionOp::StoreLocal {
                local_id, value, ..
            } => {
                changed |= rewrite_local_constants_in_expr(value, constants);
                constants.remove(local_id);
            }
            FunctionOp::Expr(expr) => {
                changed |= rewrite_local_constants_in_expr(expr, constants);
            }
            FunctionOp::MemoryIntrinsic(memory) => {
                changed |= rewrite_local_constants_in_expr(&mut memory.dest, constants);
                changed |= match &mut memory.source {
                    FunctionMemoryIntrinsicSource::Slice(source)
                    | FunctionMemoryIntrinsicSource::Byte(source) => {
                        rewrite_local_constants_in_expr(source, constants)
                    }
                };
                constants.clear();
            }
            FunctionOp::Defer(_) => {
                if let FunctionOp::Defer(body) = op {
                    changed |= propagate_local_constants_in_defer_body(body, unstable_locals);
                }
                constants.clear();
            }
        }
    }
    changed |= rewrite_local_constants_in_terminator(&mut block.terminator, constants);
    changed
}

pub(crate) fn local_constant_value(expr: &FunctionExpr) -> Option<FunctionExpr> {
    match &expr.kind {
        FunctionExprKind::Integer(_)
        | FunctionExprKind::Bool(_)
        | FunctionExprKind::Char(_)
        | FunctionExprKind::ByteChar(_)
        | FunctionExprKind::EnumVariantTag(_) => Some(expr.clone()),
        FunctionExprKind::EnumVariant { fields, .. }
            if fields
                .iter()
                .all(|field| local_constant_value(field).is_some()) =>
        {
            Some(expr.clone())
        }
        _ => None,
    }
}

pub(crate) fn rewrite_local_constants_in_terminator(
    terminator: &mut FunctionTerminator,
    constants: &HashMap<LocalId, FunctionExpr>,
) -> bool {
    rewrite_terminator_exprs(terminator, &mut |expr| {
        rewrite_local_constants_in_expr(expr, constants)
    })
}

pub(crate) fn rewrite_local_constants_in_expr(
    expr: &mut FunctionExpr,
    constants: &HashMap<LocalId, FunctionExpr>,
) -> bool {
    match &mut expr.kind {
        FunctionExprKind::Local(local_id) => {
            let Some(value) = constants.get(local_id) else {
                return false;
            };
            let mut value = value.clone();
            value.span = expr.span;
            *expr = value;
            true
        }
        _ => rewrite_expr_children(
            expr,
            ExprTraversal::values_without_static_array_pointer(),
            &mut |expr| rewrite_local_constants_in_expr(expr, constants),
        ),
    }
}

pub(crate) fn simplify_constant_logical_exprs(body: &mut FunctionBody) -> bool {
    simplify_constant_logical_exprs_in_blocks(&mut body.blocks)
}

pub(crate) fn simplify_constant_logical_exprs_in_blocks(blocks: &mut [FunctionBlock]) -> bool {
    rewrite_blocks_exprs(blocks, ExprTraversal::all(), &mut |expr| {
        simplify_constant_logical_expr(expr)
    })
}

pub(crate) fn simplify_constant_logical_expr(expr: &mut FunctionExpr) -> bool {
    let mut changed = false;
    let mut replacement = None;
    match &mut expr.kind {
        FunctionExprKind::Binary { lhs, op, rhs }
            if matches!(op, nia_ast::BinaryOp::And | nia_ast::BinaryOp::Or) =>
        {
            changed |= simplify_constant_logical_expr(lhs);
            match (bool_literal_value(lhs), *op) {
                (Some(false), nia_ast::BinaryOp::And) => {
                    replacement = Some(FunctionExpr {
                        span: expr.span,
                        ty: expr.ty,
                        kind: FunctionExprKind::Bool(false),
                    });
                }
                (Some(true), nia_ast::BinaryOp::Or) => {
                    replacement = Some(FunctionExpr {
                        span: expr.span,
                        ty: expr.ty,
                        kind: FunctionExprKind::Bool(true),
                    });
                }
                (Some(true), nia_ast::BinaryOp::And) | (Some(false), nia_ast::BinaryOp::Or) => {
                    changed |= simplify_constant_logical_expr(rhs);
                    replacement = Some((**rhs).clone());
                }
                (None, nia_ast::BinaryOp::And) | (None, nia_ast::BinaryOp::Or) => {
                    changed |= simplify_constant_logical_expr(rhs);
                    match (bool_literal_value(rhs), *op) {
                        (Some(true), nia_ast::BinaryOp::And)
                        | (Some(false), nia_ast::BinaryOp::Or) => {
                            replacement = Some((**lhs).clone());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        _ => {
            changed |= rewrite_expr_children(expr, ExprTraversal::all(), &mut |expr| {
                simplify_constant_logical_expr(expr)
            });
        }
    }

    if let Some(replacement) = replacement {
        *expr = replacement;
        true
    } else {
        changed
    }
}

pub(crate) fn bool_literal_value(expr: &FunctionExpr) -> Option<bool> {
    match expr.kind {
        FunctionExprKind::Bool(value) => Some(value),
        _ => None,
    }
}
