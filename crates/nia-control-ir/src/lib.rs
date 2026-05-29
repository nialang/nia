// SPDX-License-Identifier: GPL-3.0-or-later
use nia_body_ir::{
    TypedBinding, TypedBody, TypedExpr, TypedForHeader, TypedForInit, TypedLocal, TypedStmt,
    TypedStmtKind,
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
    For {
        header: TypedForHeader,
        body: Box<ControlBody>,
        break_target: ControlBlockId,
        continue_target: ControlBlockId,
        span: Span,
    },
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
        let mut ops = Vec::new();
        let current = entry;
        for stmt in &body.stmts {
            if let Some(term) = self.lower_stmt(stmt, &mut ops) {
                if ops.is_empty() {
                    blocks.push(ControlBlock {
                        id: current,
                        span: stmt.span,
                        ops,
                        terminator: term,
                    });
                } else {
                    let term_block = self.alloc_block();
                    blocks.push(ControlBlock {
                        id: current,
                        span: body.span,
                        ops,
                        terminator: ControlTerminator::Next {
                            target: term_block,
                            span: stmt.span,
                        },
                    });
                    blocks.push(ControlBlock {
                        id: term_block,
                        span: stmt.span,
                        ops: Vec::new(),
                        terminator: term,
                    });
                }
                return ControlBody {
                    span: body.span,
                    locals: body.locals.clone(),
                    blocks,
                    entry,
                    ty: body.ty,
                };
            }
        }

        let tail = ControlTerminator::Tail {
            value: body.tail.as_ref().map(|tail| (**tail).clone()),
            span: body
                .tail
                .as_ref()
                .map(|tail| tail.span)
                .unwrap_or(body.span),
        };
        if ops.is_empty() {
            blocks.push(ControlBlock {
                id: current,
                span: body.span,
                ops,
                terminator: tail,
            });
        } else {
            let tail_block = self.alloc_block();
            blocks.push(ControlBlock {
                id: current,
                span: body.span,
                ops,
                terminator: ControlTerminator::Next {
                    target: tail_block,
                    span: body
                        .tail
                        .as_ref()
                        .map(|tail| tail.span)
                        .unwrap_or(body.span),
                },
            });
            blocks.push(ControlBlock {
                id: tail_block,
                span: body
                    .tail
                    .as_ref()
                    .map(|tail| tail.span)
                    .unwrap_or(body.span),
                ops: Vec::new(),
                terminator: tail,
            });
        }
        ControlBody {
            span: body.span,
            locals: body.locals.clone(),
            blocks,
            entry,
            ty: body.ty,
        }
    }

    fn lower_stmt(
        &mut self,
        stmt: &TypedStmt,
        ops: &mut Vec<ControlOp>,
    ) -> Option<ControlTerminator> {
        match &stmt.kind {
            TypedStmtKind::Binding(binding) => {
                ops.push(ControlOp::Binding(binding.clone()));
                None
            }
            TypedStmtKind::Expr(expr) => {
                ops.push(ControlOp::Expr(expr.clone()));
                None
            }
            TypedStmtKind::Defer(expr) => {
                ops.push(ControlOp::Defer(expr.clone()));
                None
            }
            TypedStmtKind::Return(value) => Some(ControlTerminator::Return {
                value: value.clone(),
                span: stmt.span,
            }),
            TypedStmtKind::Break => {
                let target = self
                    .loop_targets
                    .last()
                    .map(|targets| targets.break_target)
                    .unwrap_or(ControlBlockId(u32::MAX));
                Some(ControlTerminator::Branch {
                    target,
                    span: stmt.span,
                })
            }
            TypedStmtKind::Continue => {
                let target = self
                    .loop_targets
                    .last()
                    .map(|targets| targets.continue_target)
                    .unwrap_or(ControlBlockId(u32::MAX));
                Some(ControlTerminator::Branch {
                    target,
                    span: stmt.span,
                })
            }
            TypedStmtKind::For(for_stmt) => {
                let break_target = self.alloc_block();
                let continue_target = self.alloc_block();
                self.loop_targets.push(LoopTargetIds {
                    break_target,
                    continue_target,
                });
                let body = self.lower_body(&for_stmt.body);
                self.loop_targets.pop();
                ops.push(ControlOp::For {
                    header: self.lower_for_header(&for_stmt.header),
                    body: Box::new(body),
                    break_target,
                    continue_target,
                    span: stmt.span,
                });
                None
            }
        }
    }

    fn lower_for_header(&self, header: &TypedForHeader) -> TypedForHeader {
        match header {
            TypedForHeader::Infinite => TypedForHeader::Infinite,
            TypedForHeader::Condition(cond) => TypedForHeader::Condition(cond.clone()),
            TypedForHeader::CStyle { init, cond, step } => TypedForHeader::CStyle {
                init: init.as_ref().map(|init| {
                    Box::new(match &**init {
                        TypedForInit::Binding(binding) => TypedForInit::Binding(binding.clone()),
                        TypedForInit::Expr(expr) => TypedForInit::Expr(expr.clone()),
                    })
                }),
                cond: cond.as_ref().map(|cond| Box::new((**cond).clone())),
                step: step.as_ref().map(|step| Box::new((**step).clone())),
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
    use nia_body_ir::{TypedExprKind, TypedLocalKind};
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
        let ControlOp::For {
            break_target, body, ..
        } = &control.blocks[0].ops[0]
        else {
            panic!("expected for op");
        };

        assert_eq!(body.blocks[0].terminator.successors(), vec![*break_target]);
        assert!(matches!(
            body.blocks[0].terminator,
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
        let ControlOp::For {
            continue_target,
            body,
            ..
        } = &control.blocks[0].ops[0]
        else {
            panic!("expected for op");
        };

        assert_eq!(
            body.blocks[0].terminator.successors(),
            vec![*continue_target]
        );
        assert!(matches!(
            body.blocks[0].terminator,
            ControlTerminator::Branch { .. }
        ));
    }
}
