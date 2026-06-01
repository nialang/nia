// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{HashMap, HashSet};

use crate::ModuleLowerer;
use nia_function_ir::{
    FunctionBlock, FunctionBlockId, FunctionBody, FunctionDeferBody, FunctionOp, FunctionTerminator,
};
use nia_opt::OptimizationDepth;

impl<'a> ModuleLowerer<'a> {
    pub(crate) fn optimize_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        if matches!(
            self.optimization.simplify_cfg,
            OptimizationDepth::Cheap | OptimizationDepth::Full | OptimizationDepth::Aggressive
        ) {
            remove_unreachable_blocks(&mut body);
            optimize_defer_bodies(&mut body.blocks);
        }
        body
    }
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
                remove_unreachable_defer_blocks(body);
                optimize_defer_bodies(&mut body.blocks);
            }
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

    fn test_body(blocks: Vec<FunctionBlock>) -> FunctionBody {
        FunctionBody {
            span: Span::default(),
            locals: Vec::new(),
            scopes: vec![FunctionScope {
                id: FunctionScopeId(0),
                parent: None,
                span: Span::default(),
            }],
            blocks,
            entry: FunctionBlockId(0),
            ty: nia_ids::InternedTyId::new(
                nia_ids::ModuleId(0),
                nia_ids::TyInternerIndex::from_interner_index(0),
            ),
        }
    }
}
