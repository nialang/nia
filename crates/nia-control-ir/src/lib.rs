// SPDX-License-Identifier: GPL-3.0-or-later
use nia_body_ir::{
    TypedBinding, TypedBody, TypedExpr, TypedForHeader, TypedForInit, TypedLocal, TypedStmtKind,
};
use nia_ids::InternedTyId;
use nia_span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlBlockId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct ControlBody {
    pub span: Span,
    pub locals: Vec<TypedLocal>,
    pub blocks: Vec<ControlBlock>,
    pub entry: ControlBlockId,
    pub ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlBlock {
    pub id: ControlBlockId,
    pub span: Span,
    pub ops: Vec<ControlOp>,
    pub terminator: ControlTerminator,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlOp {
    Binding(TypedBinding),
    Expr(TypedExpr),
    Defer(TypedExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlTerminator {
    Branch {
        target: ControlBlockId,
        span: Span,
    },
    Next {
        target: ControlBlockId,
        span: Span,
    },
    Loop {
        header: TypedForHeader,
        body: ControlBlockId,
        continue_target: ControlBlockId,
        break_target: ControlBlockId,
        span: Span,
    },
    Return {
        value: Option<TypedExpr>,
        span: Span,
    },
    Tail {
        value: Option<TypedExpr>,
        span: Span,
    },
}

impl ControlTerminator {
    pub fn successors(&self) -> Vec<ControlBlockId> {
        match self {
            ControlTerminator::Branch { target, .. } | ControlTerminator::Next { target, .. } => {
                vec![*target]
            }
            ControlTerminator::Loop {
                body, break_target, ..
            } => vec![*body, *break_target],
            ControlTerminator::Return { .. } | ControlTerminator::Tail { .. } => Vec::new(),
        }
    }
}

pub fn lower_control_body(body: &TypedBody) -> ControlBody {
    ControlLowerer::new().lower_body(body)
}

struct ControlLowerer {
    next_block: u32,
    loop_targets: Vec<LoopTargetIds>,
}

#[derive(Debug, Clone, Copy)]
struct LoopTargetIds {
    break_target: ControlBlockId,
    continue_target: ControlBlockId,
}

#[derive(Debug, Clone, Copy)]
enum Fallthrough {
    Tail,
    Branch(ControlBlockId),
}

impl ControlLowerer {
    fn new() -> Self {
        Self {
            next_block: 0,
            loop_targets: Vec::new(),
        }
    }

    fn lower_body(&mut self, body: &TypedBody) -> ControlBody {
        let entry = self.alloc_block();
        let mut blocks = Vec::new();
        self.lower_body_into(body, entry, &mut blocks, Fallthrough::Tail);
        ControlBody {
            span: body.span,
            locals: body.locals.clone(),
            blocks,
            entry,
            ty: body.ty,
        }
    }

    fn lower_body_into(
        &mut self,
        body: &TypedBody,
        entry: ControlBlockId,
        blocks: &mut Vec<ControlBlock>,
        fallthrough: Fallthrough,
    ) {
        let mut current = entry;
        let mut ops = Vec::new();
        for stmt in &body.stmts {
            match &stmt.kind {
                TypedStmtKind::Binding(binding) => ops.push(ControlOp::Binding(binding.clone())),
                TypedStmtKind::Expr(expr) => ops.push(ControlOp::Expr(expr.clone())),
                TypedStmtKind::Defer(expr) => ops.push(ControlOp::Defer(expr.clone())),
                TypedStmtKind::Return(value) => {
                    self.finish_block(
                        blocks,
                        current,
                        stmt.span,
                        ops,
                        ControlTerminator::Return {
                            value: value.clone(),
                            span: stmt.span,
                        },
                    );
                    return;
                }
                TypedStmtKind::Break => {
                    let target = self
                        .loop_targets
                        .last()
                        .map(|targets| targets.break_target)
                        .unwrap_or(ControlBlockId(u32::MAX));
                    self.finish_block(
                        blocks,
                        current,
                        stmt.span,
                        ops,
                        ControlTerminator::Branch {
                            target,
                            span: stmt.span,
                        },
                    );
                    return;
                }
                TypedStmtKind::Continue => {
                    let target = self
                        .loop_targets
                        .last()
                        .map(|targets| targets.continue_target)
                        .unwrap_or(ControlBlockId(u32::MAX));
                    self.finish_block(
                        blocks,
                        current,
                        stmt.span,
                        ops,
                        ControlTerminator::Branch {
                            target,
                            span: stmt.span,
                        },
                    );
                    return;
                }
                TypedStmtKind::For(for_stmt) => {
                    self.lower_for_stmt(stmt.span, for_stmt, &mut current, &mut ops, blocks);
                }
            }
        }
        self.finish_fallthrough_block(blocks, current, body, ops, fallthrough);
    }

    fn lower_for_stmt(
        &mut self,
        span: Span,
        for_stmt: &nia_body_ir::TypedFor,
        current: &mut ControlBlockId,
        ops: &mut Vec<ControlOp>,
        blocks: &mut Vec<ControlBlock>,
    ) {
        self.push_for_init_ops(&for_stmt.header, ops);
        let loop_header = if ops.is_empty() {
            *current
        } else {
            let loop_header = self.alloc_block();
            blocks.push(ControlBlock {
                id: *current,
                span,
                ops: std::mem::take(ops),
                terminator: ControlTerminator::Next {
                    target: loop_header,
                    span,
                },
            });
            loop_header
        };
        let body_entry = self.alloc_block();
        let continue_target = self.alloc_block();
        let break_target = self.alloc_block();
        blocks.push(ControlBlock {
            id: loop_header,
            span,
            ops: Vec::new(),
            terminator: ControlTerminator::Loop {
                header: self.lower_loop_header(&for_stmt.header),
                body: body_entry,
                continue_target,
                break_target,
                span,
            },
        });

        self.loop_targets.push(LoopTargetIds {
            break_target,
            continue_target,
        });
        self.lower_body_into(
            &for_stmt.body,
            body_entry,
            blocks,
            Fallthrough::Branch(continue_target),
        );
        self.loop_targets.pop();

        blocks.push(ControlBlock {
            id: continue_target,
            span,
            ops: self.for_step_ops(&for_stmt.header),
            terminator: ControlTerminator::Branch {
                target: loop_header,
                span,
            },
        });
        *current = break_target;
    }

    fn finish_block(
        &mut self,
        blocks: &mut Vec<ControlBlock>,
        current: ControlBlockId,
        span: Span,
        ops: Vec<ControlOp>,
        terminator: ControlTerminator,
    ) {
        if ops.is_empty() {
            blocks.push(ControlBlock {
                id: current,
                span,
                ops,
                terminator,
            });
        } else {
            let term_block = self.alloc_block();
            blocks.push(ControlBlock {
                id: current,
                span,
                ops,
                terminator: ControlTerminator::Next {
                    target: term_block,
                    span,
                },
            });
            blocks.push(ControlBlock {
                id: term_block,
                span,
                ops: Vec::new(),
                terminator,
            });
        }
    }

    fn finish_fallthrough_block(
        &mut self,
        blocks: &mut Vec<ControlBlock>,
        current: ControlBlockId,
        body: &TypedBody,
        mut ops: Vec<ControlOp>,
        fallthrough: Fallthrough,
    ) {
        let span = body
            .tail
            .as_ref()
            .map(|tail| tail.span)
            .unwrap_or(body.span);
        let terminator = match fallthrough {
            Fallthrough::Tail => ControlTerminator::Tail {
                value: body.tail.as_ref().map(|tail| (**tail).clone()),
                span,
            },
            Fallthrough::Branch(target) => {
                if let Some(tail) = &body.tail {
                    ops.push(ControlOp::Expr((**tail).clone()));
                }
                ControlTerminator::Branch { target, span }
            }
        };
        self.finish_block(blocks, current, span, ops, terminator);
    }

    fn push_for_init_ops(&self, header: &TypedForHeader, ops: &mut Vec<ControlOp>) {
        if let TypedForHeader::CStyle {
            init: Some(init), ..
        } = header
        {
            match &**init {
                TypedForInit::Binding(binding) => ops.push(ControlOp::Binding(binding.clone())),
                TypedForInit::Expr(expr) => ops.push(ControlOp::Expr(expr.clone())),
            }
        }
    }

    fn for_step_ops(&self, header: &TypedForHeader) -> Vec<ControlOp> {
        match header {
            TypedForHeader::CStyle {
                step: Some(step), ..
            } => vec![ControlOp::Expr((**step).clone())],
            _ => Vec::new(),
        }
    }

    fn lower_loop_header(&self, header: &TypedForHeader) -> TypedForHeader {
        match header {
            TypedForHeader::Infinite => TypedForHeader::Infinite,
            TypedForHeader::Condition(cond) => TypedForHeader::Condition(cond.clone()),
            TypedForHeader::CStyle { cond, .. } => TypedForHeader::CStyle {
                init: None,
                cond: cond.as_ref().map(|cond| Box::new((**cond).clone())),
                step: None,
            },
        }
    }

    fn alloc_block(&mut self) -> ControlBlockId {
        let id = ControlBlockId(self.next_block);
        self.next_block += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_body_ir::{TypedExprKind, TypedLocalKind, TypedStmt};
    use nia_ids::{LocalId, ModuleId, TyInternerIndex};

    #[test]
    fn lowers_body_to_entry_block_with_tail() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: vec![TypedLocal {
                id: LocalId(0),
                name: "x".to_string(),
                kind: TypedLocalKind::Binding,
                ty,
                span,
            }],
            stmts: Vec::new(),
            tail: Some(Box::new(TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Integer("1".to_string()),
            })),
            ty,
        };

        let control = lower_control_body(&body);

        assert_eq!(control.entry, ControlBlockId(0));
        assert_eq!(control.blocks.len(), 1);
        assert!(control.blocks[0].ops.is_empty());
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Tail { value: Some(_), .. }
        ));
    }

    #[test]
    fn non_terminal_ops_branch_to_tail_block() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::Expr(expr.clone()),
            }],
            tail: Some(Box::new(expr)),
            ty,
        };

        let control = lower_control_body(&body);

        assert_eq!(control.blocks.len(), 2);
        assert_eq!(control.blocks[0].ops.len(), 1);
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Next {
                target: ControlBlockId(1),
                ..
            }
        ));
        assert_eq!(
            control.blocks[0].terminator.successors(),
            vec![ControlBlockId(1)]
        );
        assert!(matches!(
            control.blocks[1].terminator,
            ControlTerminator::Tail { value: Some(_), .. }
        ));
    }

    #[test]
    fn return_terminates_block_before_later_statements() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![
                TypedStmt {
                    span,
                    kind: TypedStmtKind::Return(Some(expr.clone())),
                },
                TypedStmt {
                    span,
                    kind: TypedStmtKind::Expr(expr),
                },
            ],
            tail: None,
            ty,
        };

        let control = lower_control_body(&body);

        assert_eq!(control.blocks.len(), 1);
        assert!(control.blocks[0].ops.is_empty());
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Return { value: Some(_), .. }
        ));
    }

    #[test]
    fn resolves_break_to_loop_exit_branch() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Break,
                        }],
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let control = lower_control_body(&body);
        let ControlTerminator::Loop {
            body: loop_body,
            break_target,
            ..
        } = control.blocks[0].terminator
        else {
            panic!("expected loop terminator");
        };
        let loop_body = control
            .blocks
            .iter()
            .find(|block| block.id == loop_body)
            .expect("loop body block");

        assert_eq!(loop_body.terminator.successors(), vec![break_target]);
        assert!(matches!(
            loop_body.terminator,
            ControlTerminator::Branch { .. }
        ));
    }

    #[test]
    fn resolves_continue_to_loop_continue_branch() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::Infinite,
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: vec![TypedStmt {
                            span,
                            kind: TypedStmtKind::Continue,
                        }],
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let control = lower_control_body(&body);
        let ControlTerminator::Loop {
            body: loop_body,
            continue_target,
            ..
        } = control.blocks[0].terminator
        else {
            panic!("expected loop terminator");
        };
        let loop_body = control
            .blocks
            .iter()
            .find(|block| block.id == loop_body)
            .expect("loop body block");

        assert_eq!(loop_body.terminator.successors(), vec![continue_target]);
        assert!(matches!(
            loop_body.terminator,
            ControlTerminator::Branch { .. }
        ));
    }

    #[test]
    fn lowers_c_style_for_init_step_and_edges() {
        let span = Span::default();
        let ty = InternedTyId::new(ModuleId(0), TyInternerIndex::from_interner_index(0));
        let expr = TypedExpr {
            span,
            ty,
            kind: TypedExprKind::Integer("1".to_string()),
        };
        let body = TypedBody {
            span,
            locals: Vec::new(),
            stmts: vec![TypedStmt {
                span,
                kind: TypedStmtKind::For(Box::new(nia_body_ir::TypedFor {
                    header: TypedForHeader::CStyle {
                        init: Some(Box::new(TypedForInit::Expr(expr.clone()))),
                        cond: Some(Box::new(expr.clone())),
                        step: Some(Box::new(expr)),
                    },
                    body: TypedBody {
                        span,
                        locals: Vec::new(),
                        stmts: Vec::new(),
                        tail: None,
                        ty,
                    },
                })),
            }],
            tail: None,
            ty,
        };

        let control = lower_control_body(&body);

        assert!(matches!(control.blocks[0].ops[0], ControlOp::Expr(_)));
        assert!(matches!(
            control.blocks[0].terminator,
            ControlTerminator::Next { .. }
        ));
        let loop_block = &control.blocks[1];
        let ControlTerminator::Loop {
            body,
            continue_target,
            break_target,
            ..
        } = loop_block.terminator
        else {
            panic!("expected loop terminator");
        };
        assert_eq!(loop_block.terminator.successors(), vec![body, break_target]);
        let continue_block = control
            .blocks
            .iter()
            .find(|block| block.id == continue_target)
            .expect("continue block");
        assert!(matches!(continue_block.ops[0], ControlOp::Expr(_)));
        assert_eq!(continue_block.terminator.successors(), vec![loop_block.id]);
    }
}
