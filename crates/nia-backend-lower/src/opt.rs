// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::ModuleLowerer;
use nia_function_ir::{
    FunctionBlock, FunctionBlockId, FunctionBody, FunctionDeferBody, FunctionExprKind, FunctionOp,
    FunctionTerminator,
};
use nia_opt::OptimizationDepth;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn optimize_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        if matches!(
            self.optimization.simplify_cfg,
            OptimizationDepth::Cheap | OptimizationDepth::Full | OptimizationDepth::Aggressive
        ) {
            fold_constant_bool_branches(&mut body.blocks);
            merge_empty_jump_blocks(&mut body);
            remove_unreachable_blocks(&mut body);
            optimize_defer_bodies(&mut body.blocks);
        }
        body
    }
}

fn fold_constant_bool_branches(blocks: &mut [FunctionBlock]) {
    for block in blocks {
        if let FunctionTerminator::If {
            cond,
            then_target,
            else_target,
            span,
        } = &block.terminator
            && let FunctionExprKind::Bool(value) = cond.kind
        {
            block.terminator = FunctionTerminator::Branch {
                target: if value { *then_target } else { *else_target },
                span: *span,
            };
        }
    }
}

fn merge_empty_jump_blocks(body: &mut FunctionBody) {
    loop {
        let empty_jumps = empty_jump_targets(&body.blocks, Some(body.entry));
        if empty_jumps.is_empty() {
            break;
        }
        let mut changed = false;
        for block in &mut body.blocks {
            changed |= retarget_terminator(&mut block.terminator, &empty_jumps);
        }
        if !changed {
            break;
        }
    }
}

fn empty_jump_targets(
    blocks: &[FunctionBlock],
    protected_entry: Option<FunctionBlockId>,
) -> HashMap<FunctionBlockId, FunctionBlockId> {
    let blocks_by_id = blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<HashMap<_, _>>();
    blocks
        .iter()
        .filter(|block| Some(block.id) != protected_entry && block.ops.is_empty())
        .filter_map(|block| {
            let target = jump_target(&block.terminator)?;
            let target_block = blocks_by_id.get(&target)?;
            (target != block.id && target_block.scope == block.scope).then_some((block.id, target))
        })
        .collect()
}

fn jump_target(terminator: &FunctionTerminator) -> Option<FunctionBlockId> {
    match terminator {
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            Some(*target)
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::If { .. }
        | FunctionTerminator::Switch { .. }
        | FunctionTerminator::Loop { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => None,
    }
}

fn retarget_terminator(
    terminator: &mut FunctionTerminator,
    empty_jumps: &HashMap<FunctionBlockId, FunctionBlockId>,
) -> bool {
    match terminator {
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            retarget_block_id(target, empty_jumps)
        }
        FunctionTerminator::If {
            then_target,
            else_target,
            ..
        } => {
            let then_changed = retarget_block_id(then_target, empty_jumps);
            let else_changed = retarget_block_id(else_target, empty_jumps);
            then_changed || else_changed
        }
        FunctionTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } => {
            let mut changed = false;
            for arm in arms {
                changed |= retarget_block_id(&mut arm.target, empty_jumps);
            }
            if let Some(default) = default {
                changed |= retarget_block_id(default, empty_jumps);
            }
            changed |= retarget_block_id(fallback, empty_jumps);
            changed
        }
        FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } => {
            let body_changed = retarget_block_id(body, empty_jumps);
            let continue_changed = retarget_block_id(continue_target, empty_jumps);
            let break_changed = retarget_block_id(break_target, empty_jumps);
            body_changed || continue_changed || break_changed
        }
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => false,
    }
}

fn retarget_block_id(
    target: &mut FunctionBlockId,
    empty_jumps: &HashMap<FunctionBlockId, FunctionBlockId>,
) -> bool {
    let mut changed = false;
    let mut seen = HashSet::new();
    while let Some(next) = empty_jumps.get(target).copied() {
        if !seen.insert(*target) {
            break;
        }
        *target = next;
        changed = true;
    }
    changed
}

fn remove_unreachable_blocks(body: &mut FunctionBody) {
    let reachable = reachable_blocks(body);
    if reachable.len() == body.blocks.len() {
        return;
    }
    body.blocks.retain(|block| reachable.contains(&block.id));
}

fn reachable_blocks(body: &FunctionBody) -> HashSet<FunctionBlockId> {
    reachable_block_ids(&body.blocks, body.entry)
}

fn reachable_defer_blocks(body: &FunctionDeferBody) -> HashSet<FunctionBlockId> {
    reachable_block_ids(&body.blocks, body.entry)
}

fn reachable_block_ids(
    blocks: &[FunctionBlock],
    entry: FunctionBlockId,
) -> HashSet<FunctionBlockId> {
    let blocks_by_id = blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<HashMap<_, _>>();
    let mut reachable = HashSet::new();
    let mut stack = vec![entry];
    while let Some(id) = stack.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let Some(block) = blocks_by_id.get(&id) else {
            continue;
        };
        stack.extend(terminator_referenced_blocks(&block.terminator));
    }
    reachable
}

fn terminator_referenced_blocks(terminator: &FunctionTerminator) -> Vec<FunctionBlockId> {
    match terminator {
        FunctionTerminator::Error { .. }
        | FunctionTerminator::Return { .. }
        | FunctionTerminator::Tail { .. } => Vec::new(),
        FunctionTerminator::Branch { target, .. } | FunctionTerminator::Next { target, .. } => {
            vec![*target]
        }
        FunctionTerminator::If {
            then_target,
            else_target,
            ..
        } => vec![*then_target, *else_target],
        FunctionTerminator::Switch {
            arms,
            default,
            fallback,
            ..
        } => {
            let mut targets = arms.iter().map(|arm| arm.target).collect::<Vec<_>>();
            if let Some(default) = default {
                targets.push(*default);
            }
            targets.push(*fallback);
            targets
        }
        FunctionTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } => vec![*body, *continue_target, *break_target],
    }
}

fn optimize_defer_bodies(blocks: &mut [FunctionBlock]) {
    for block in blocks {
        for op in &mut block.ops {
            if let FunctionOp::Defer(body) = op {
                fold_constant_bool_branches(&mut body.blocks);
                merge_empty_defer_jump_blocks(body);
                remove_unreachable_defer_blocks(body);
                optimize_defer_bodies(&mut body.blocks);
            }
        }
    }
}

fn merge_empty_defer_jump_blocks(body: &mut FunctionDeferBody) {
    loop {
        let empty_jumps = empty_jump_targets(&body.blocks, Some(body.entry));
        if empty_jumps.is_empty() {
            break;
        }
        let mut changed = false;
        for block in &mut body.blocks {
            changed |= retarget_terminator(&mut block.terminator, &empty_jumps);
        }
        if !changed {
            break;
        }
    }
}

fn remove_unreachable_defer_blocks(body: &mut FunctionDeferBody) {
    let reachable = reachable_defer_blocks(body);
    if reachable.len() == body.blocks.len() {
        return;
    }
    body.blocks.retain(|block| reachable.contains(&block.id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_function_ir::{
        FunctionBlock, FunctionScope, FunctionScopeId, FunctionTerminator, validate_function_body,
    };
    use nia_span::Span;

    #[test]
    fn removes_blocks_unreachable_from_entry() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(1)]
        );
        validate_function_body(&body).expect("optimized function body should remain valid");
    }

    #[test]
    fn preserves_blocks_referenced_by_reachable_loop_terminators() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Loop {
                    header: nia_function_ir::FunctionForHeader::Infinite,
                    body: FunctionBlockId(1),
                    continue_target: FunctionBlockId(2),
                    break_target: FunctionBlockId(3),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Branch {
                    target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Branch {
                    target: FunctionBlockId(0),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(3),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
            FunctionBlock {
                id: FunctionBlockId(4),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![
                FunctionBlockId(0),
                FunctionBlockId(1),
                FunctionBlockId(2),
                FunctionBlockId(3),
            ]
        );
        validate_function_body(&body).expect("optimized loop body should remain valid");
    }

    #[test]
    fn merges_empty_jump_blocks_within_the_same_scope() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        merge_empty_jump_blocks(&mut body);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(2)]
        );
        assert_eq!(
            body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(2)]
        );
        validate_function_body(&body).expect("merged function body should remain valid");
    }

    #[test]
    fn folds_constant_bool_if_to_selected_branch() {
        let span = Span::default();
        let ty = test_ty();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::If {
                    cond: nia_function_ir::FunctionExpr {
                        span,
                        ty,
                        kind: FunctionExprKind::Bool(false),
                    },
                    then_target: FunctionBlockId(1),
                    else_target: FunctionBlockId(2),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
            FunctionBlock {
                id: FunctionBlockId(2),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        fold_constant_bool_branches(&mut body.blocks);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(2)]
        );
        assert_eq!(
            body.blocks[0].terminator.successors(),
            vec![FunctionBlockId(2)]
        );
        validate_function_body(&body).expect("folded function body should remain valid");
    }

    #[test]
    fn does_not_merge_entry_block_even_when_it_is_an_empty_jump() {
        let span = Span::default();
        let mut body = test_body(vec![
            FunctionBlock {
                id: FunctionBlockId(0),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Next {
                    target: FunctionBlockId(1),
                    span,
                },
            },
            FunctionBlock {
                id: FunctionBlockId(1),
                scope: FunctionScopeId(0),
                span,
                ops: Vec::new(),
                terminator: FunctionTerminator::Return { value: None, span },
            },
        ]);

        merge_empty_jump_blocks(&mut body);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(1)]
        );
        validate_function_body(&body).expect("entry-preserving merge should remain valid");
    }

    #[test]
    fn does_not_merge_empty_jump_blocks_across_scope_boundaries() {
        let span = Span::default();
        let mut body = test_body_with_scopes(
            vec![
                FunctionScope {
                    id: FunctionScopeId(0),
                    parent: None,
                    span,
                },
                FunctionScope {
                    id: FunctionScopeId(1),
                    parent: Some(FunctionScopeId(0)),
                    span,
                },
            ],
            vec![
                FunctionBlock {
                    id: FunctionBlockId(0),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Next {
                        target: FunctionBlockId(1),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(1),
                    scope: FunctionScopeId(1),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Next {
                        target: FunctionBlockId(2),
                        span,
                    },
                },
                FunctionBlock {
                    id: FunctionBlockId(2),
                    scope: FunctionScopeId(0),
                    span,
                    ops: Vec::new(),
                    terminator: FunctionTerminator::Return { value: None, span },
                },
            ],
        );

        merge_empty_jump_blocks(&mut body);
        remove_unreachable_blocks(&mut body);

        assert_eq!(
            body.blocks.iter().map(|block| block.id).collect::<Vec<_>>(),
            vec![FunctionBlockId(0), FunctionBlockId(1), FunctionBlockId(2)]
        );
        assert_eq!(
            body.edge_exited_scopes(FunctionBlockId(1), FunctionBlockId(2)),
            Some(vec![FunctionScopeId(1)])
        );
        validate_function_body(&body).expect("scope-preserving merge should remain valid");
    }

    fn test_body(blocks: Vec<FunctionBlock>) -> FunctionBody {
        test_body_with_scopes(
            vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks,
        )
    }

    fn test_body_with_scopes(
        scopes: Vec<FunctionScope>,
        blocks: Vec<FunctionBlock>,
    ) -> FunctionBody {
        FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes,
            blocks,
            entry: FunctionBlockId(0),
            ty: test_ty(),
        }
    }

    fn test_ty() -> nia_ids::InternedTyId {
        nia_ids::InternedTyId::new(
            nia_ids::ModuleId(0),
            nia_ids::TyInternerIndex::from_interner_index(0),
        )
    }
}
