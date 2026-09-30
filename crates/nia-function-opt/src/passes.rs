use std::collections::{HashMap, HashSet};

use nia_function_ir::{
    FunctionArrayElements, FunctionBlock, FunctionBlockId, FunctionBody, FunctionCallee,
    FunctionDeferBody, FunctionExpr, FunctionExprKind, FunctionForHeader, FunctionInlineAsm,
    FunctionLocalKind, FunctionMemoryIntrinsicSource, FunctionOp, FunctionPlace, FunctionPlaceBase,
    FunctionPlaceElem, FunctionRange, FunctionSliceRange, FunctionTerminator,
};
use nia_ids::{InternedTyId, LocalId};

mod casts;
mod cfg;
mod cfg_passes;
mod const_prop;
mod copy_prop;
mod dce;
mod local_analysis;
mod purity;
mod traversal;

pub(crate) use casts::*;
pub(crate) use cfg::*;
pub(crate) use cfg_passes::*;
pub(crate) use const_prop::*;
pub(crate) use copy_prop::*;
pub(crate) use dce::*;
pub(crate) use local_analysis::*;
pub(crate) use purity::*;
pub(crate) use traversal::*;

pub(crate) fn propagate_local_function_pointers(body: &mut FunctionBody) -> bool {
    let unstable = collect_place_locals_in_body(body);
    propagate_local_function_pointers_in_blocks(&mut body.blocks, body.entry, &unstable)
}

fn propagate_local_function_pointers_in_blocks(
    blocks: &mut [FunctionBlock],
    entry: FunctionBlockId,
    unstable: &HashSet<LocalId>,
) -> bool {
    let cfg = FunctionCfg::new(blocks);
    let mut incoming = HashMap::<FunctionBlockId, HashMap<LocalId, FunctionExpr>>::new();
    let mut stack = vec![entry];
    let mut visited = HashSet::new();
    let mut changed = false;
    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(index) = cfg.block(id) else {
            continue;
        };
        let mut known = incoming.remove(&id).unwrap_or_default();
        for op in &mut blocks[index].ops {
            match op {
                FunctionOp::Binding(binding) => {
                    if let Some(value) = &mut binding.value {
                        changed |= rewrite_local_constants_in_expr(value, &known);
                    }
                    known.remove(&binding.local_id);
                    if !unstable.contains(&binding.local_id)
                        && let Some(value) = binding.value.as_ref()
                        && is_function_pointer_value(value)
                    {
                        known.insert(binding.local_id, value.clone());
                    }
                }
                FunctionOp::StoreLocal {
                    local_id, value, ..
                } => {
                    changed |= rewrite_local_constants_in_expr(value, &known);
                    known.remove(local_id);
                }
                FunctionOp::Expr(expr) => {
                    changed |= rewrite_local_constants_in_expr(expr, &known);
                }
                FunctionOp::MemoryIntrinsic(_) => known.clear(),
                FunctionOp::Defer(body) => {
                    changed |= propagate_local_function_pointers_in_blocks(
                        &mut body.blocks,
                        body.entry,
                        unstable,
                    );
                    known.clear();
                }
            }
        }
        changed |= rewrite_local_constants_in_terminator(&mut blocks[index].terminator, &known);
        for succ in cfg.referenced_blocks(&blocks[index].terminator) {
            let preds = cfg.predecessors(succ);
            if preds.len() == 1 && preds[0] == id {
                incoming.insert(succ, known.clone());
            }
            stack.push(succ);
        }
    }
    changed
}

fn is_function_pointer_value(expr: &FunctionExpr) -> bool {
    match &expr.kind {
        FunctionExprKind::Unary {
            op: nia_ast::UnaryOp::Ref | nia_ast::UnaryOp::RefReadOnly,
            expr,
        } => {
            matches!(
                expr.kind,
                FunctionExprKind::Function(_) | FunctionExprKind::FunctionInstance { .. }
            )
        }
        _ => false,
    }
}
