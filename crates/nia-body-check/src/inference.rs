// SPDX-License-Identifier: GPL-3.0-or-later
//! Function-local constraint solving for types that source syntax omits.
//!
//! Inference terms deliberately live outside `nia_ty::TyKind`. They may be
//! incomplete while one function body is inspected, but only fully resolved
//! `InternedTyId`s are handed to the ordinary body checker and published in
//! semantic facts or Body IR. This keeps query, ABI, layout, and persistence
//! boundaries free of transient inference identities.

use crate::BodyChecker;
use crate::literals::{float_literal_suffix_ty, integer_literal_suffix_ty};
use nia_ast::{BinaryOp, Block, Expr, ExprKind, Pattern, PatternKind, Stmt, StmtKind, UnaryOp};
use nia_ids::{InternedTyId, LocalId};
use nia_node_id::VersionedNodeKey;
use nia_ty::{PrimitiveTy, TyKind};
use std::collections::HashMap;

type InferId = usize;

#[derive(Clone, Debug)]
pub(crate) struct InferredClosureSignature {
    pub(crate) params: Vec<InferredType>,
    pub(crate) return_type: InferredType,
}

/// A function-local type that may still contain unresolved leaves.
///
/// Partial types are consumed only while checking the owning function. They
/// let generic-call inference use a known component such as the error side of
/// `Error!_` without publishing transient inference variables to semantic IR.
#[derive(Clone, Debug)]
pub(crate) enum InferredType {
    Unknown,
    Known(InternedTyId),
    Tuple(Vec<InferredType>),
    Pointer {
        is_readonly: bool,
        elem: Box<InferredType>,
    },
    Optional(Box<InferredType>),
    ErrorUnion {
        error: Box<InferredType>,
        value: Box<InferredType>,
    },
    Callable {
        params: Vec<InferredType>,
        return_type: Box<InferredType>,
    },
}

impl BodyChecker<'_> {
    pub(crate) fn inferred_closure_signature(
        &self,
        expr: &Expr,
    ) -> Option<&InferredClosureSignature> {
        let closure = match &expr.kind {
            ExprKind::Closure { .. } => expr,
            ExprKind::Unary {
                op: UnaryOp::Ref | UnaryOp::RefReadOnly,
                expr,
            } if matches!(expr.kind, ExprKind::Closure { .. }) => expr,
            _ => return None,
        };
        self.inferred_closures.get(&closure.node_key)
    }

    fn inference_enum_variant_type(&mut self, expr: &Expr) -> Option<InternedTyId> {
        let def_id = self.variant_enum(expr)?;
        Some(self.interner.intern(TyKind::Nominal {
            def_id,
            args: Vec::new(),
            const_args: Vec::new(),
        }))
    }
}

#[derive(Clone, Debug)]
enum InferShape {
    Known(InternedTyId),
    Tuple(Vec<InferId>),
    Pointer {
        is_readonly: bool,
        elem: InferId,
    },
    // These wrappers remain structural while constraints are collected. A
    // closure arm may determine only one component (for example `!value`),
    // with the missing component supplied later by another arm or context.
    Optional {
        elem: InferId,
    },
    ErrorUnion {
        error: InferId,
        value: InferId,
    },
    Callable {
        params: Vec<InferId>,
        return_type: InferId,
    },
}

#[derive(Clone, Debug)]
struct InferNode {
    parent: InferId,
    rank: u8,
    shape: Option<InferShape>,
}

#[derive(Clone, Debug)]
struct ClosureTerms {
    key: VersionedNodeKey,
    params: Vec<InferId>,
    return_type: InferId,
}

#[derive(Clone, Copy, Debug)]
struct OperatorObligation {
    op: BinaryOp,
    lhs: InferId,
    rhs: InferId,
    output: InferId,
}

#[derive(Default)]
struct FunctionInference {
    nodes: Vec<InferNode>,
    concrete_terms: HashMap<InternedTyId, InferId>,
    locals: HashMap<LocalId, InferId>,
    exprs: HashMap<VersionedNodeKey, InferId>,
    closures: Vec<ClosureTerms>,
    operators: Vec<OperatorObligation>,
    return_stack: Vec<InferId>,
}

impl BodyChecker<'_> {
    pub(super) fn infer_function_closures(&mut self, body: &Block) {
        let mut inference = FunctionInference::default();
        for (local, ty) in self.local_types.clone() {
            let term = inference.known(ty);
            inference.locals.insert(local, term);
        }
        inference.collect_block_locals(self, body);
        let function_return = inference.known(self.current_return);
        inference.return_stack.push(function_return);
        inference.constrain_block(self, body, Some(function_return));
        inference.solve_operators(self);

        for closure in inference.closures.clone() {
            let params = closure
                .params
                .into_iter()
                .map(|param| inference.inferred_type(self, param))
                .collect();
            let return_type = inference.inferred_type(self, closure.return_type);
            self.inferred_closures.insert(
                closure.key,
                InferredClosureSignature {
                    params,
                    return_type,
                },
            );
        }
    }
}

impl FunctionInference {
    fn fresh(&mut self) -> InferId {
        let id = self.nodes.len();
        self.nodes.push(InferNode {
            parent: id,
            rank: 0,
            shape: None,
        });
        id
    }

    fn with_shape(&mut self, shape: InferShape) -> InferId {
        let id = self.fresh();
        self.nodes[id].shape = Some(shape);
        id
    }

    fn known(&mut self, ty: InternedTyId) -> InferId {
        if let Some(id) = self.concrete_terms.get(&ty).copied() {
            return id;
        }
        let id = self.with_shape(InferShape::Known(ty));
        self.concrete_terms.insert(ty, id);
        id
    }

    fn find(&mut self, id: InferId) -> InferId {
        let parent = self.nodes[id].parent;
        if parent == id {
            return id;
        }
        let root = self.find(parent);
        self.nodes[id].parent = root;
        root
    }

    fn expanded_shape(&mut self, checker: &BodyChecker<'_>, id: InferId) -> Option<InferShape> {
        let root = self.find(id);
        let shape = self.nodes[root].shape.clone()?;
        let InferShape::Known(ty) = shape else {
            return Some(shape);
        };
        match checker.interner.get(ty).cloned() {
            Some(TyKind::Tuple(elems)) => Some(InferShape::Tuple(
                elems.into_iter().map(|elem| self.known(elem)).collect(),
            )),
            Some(TyKind::Pointer { is_readonly, elem }) => Some(InferShape::Pointer {
                is_readonly,
                elem: self.known(elem),
            }),
            Some(TyKind::Optional { elem }) => Some(InferShape::Optional {
                elem: self.known(elem),
            }),
            Some(TyKind::ErrorUnion { error, value }) => Some(InferShape::ErrorUnion {
                error: self.known(error),
                value: self.known(value),
            }),
            Some(TyKind::FunctionPointer {
                params,
                return_type,
                is_variadic: false,
            })
            | Some(TyKind::Callable {
                params,
                return_type,
                ..
            })
            | Some(TyKind::CallablePointee {
                params,
                return_type,
            })
            | Some(TyKind::ClosureState {
                params,
                return_type,
                ..
            }) => Some(InferShape::Callable {
                params: params.into_iter().map(|param| self.known(param)).collect(),
                return_type: self.known(return_type),
            }),
            _ => Some(InferShape::Known(ty)),
        }
    }

    fn unify(&mut self, checker: &BodyChecker<'_>, left: InferId, right: InferId) -> bool {
        let left = self.find(left);
        let right = self.find(right);
        if left == right {
            return true;
        }
        let left_shape = self.expanded_shape(checker, left);
        let right_shape = self.expanded_shape(checker, right);
        if !self.shapes_compatible(checker, left_shape.as_ref(), right_shape.as_ref()) {
            return false;
        }

        // Union by rank bounds lookup depth. Structural children are unified
        // after the roots merge so tuples, pointers, and callables share the
        // same equivalence relation as their outer terms.
        let (root, child) = if self.nodes[left].rank < self.nodes[right].rank {
            (right, left)
        } else {
            (left, right)
        };
        self.nodes[child].parent = root;
        if self.nodes[left].rank == self.nodes[right].rank {
            self.nodes[root].rank = self.nodes[root].rank.saturating_add(1);
        }
        self.nodes[root].shape = left_shape.clone().or(right_shape.clone());

        match (left_shape, right_shape) {
            (Some(InferShape::Tuple(left)), Some(InferShape::Tuple(right))) => {
                for (left, right) in left.into_iter().zip(right) {
                    self.unify(checker, left, right);
                }
            }
            (
                Some(InferShape::Pointer { elem: left, .. }),
                Some(InferShape::Pointer { elem: right, .. }),
            ) => {
                self.unify(checker, left, right);
            }
            (
                Some(InferShape::Optional { elem: left }),
                Some(InferShape::Optional { elem: right }),
            ) => {
                self.unify(checker, left, right);
            }
            (
                Some(InferShape::ErrorUnion {
                    error: left_error,
                    value: left_value,
                }),
                Some(InferShape::ErrorUnion {
                    error: right_error,
                    value: right_value,
                }),
            ) => {
                self.unify(checker, left_error, right_error);
                self.unify(checker, left_value, right_value);
            }
            (
                Some(InferShape::Callable {
                    params: left_params,
                    return_type: left_return,
                }),
                Some(InferShape::Callable {
                    params: right_params,
                    return_type: right_return,
                }),
            ) => {
                for (left, right) in left_params.into_iter().zip(right_params) {
                    self.unify(checker, left, right);
                }
                self.unify(checker, left_return, right_return);
            }
            _ => {}
        }
        true
    }

    fn shapes_compatible(
        &self,
        checker: &BodyChecker<'_>,
        left: Option<&InferShape>,
        right: Option<&InferShape>,
    ) -> bool {
        match (left, right) {
            (None, _) | (_, None) => true,
            (Some(InferShape::Known(left)), Some(InferShape::Known(right))) => {
                left == right || checker.is_error_ty(*left) || checker.is_error_ty(*right)
            }
            (Some(InferShape::Tuple(left)), Some(InferShape::Tuple(right))) => {
                left.len() == right.len()
            }
            (
                Some(InferShape::Pointer {
                    is_readonly: left, ..
                }),
                Some(InferShape::Pointer {
                    is_readonly: right, ..
                }),
            ) => left == right,
            (Some(InferShape::Optional { .. }), Some(InferShape::Optional { .. })) => true,
            (Some(InferShape::ErrorUnion { .. }), Some(InferShape::ErrorUnion { .. })) => true,
            (
                Some(InferShape::Callable { params: left, .. }),
                Some(InferShape::Callable { params: right, .. }),
            ) => left.len() == right.len(),
            _ => false,
        }
    }

    fn resolve(&mut self, checker: &BodyChecker<'_>, id: InferId) -> Option<InternedTyId> {
        let shape = self.expanded_shape(checker, id)?;
        match shape {
            InferShape::Known(ty) => Some(ty),
            InferShape::Tuple(elems) => {
                let elems = elems
                    .into_iter()
                    .map(|elem| self.resolve(checker, elem))
                    .collect::<Option<Vec<_>>>()?;
                Some(checker.interner.intern(TyKind::Tuple(elems)))
            }
            InferShape::Pointer { is_readonly, elem } => {
                let elem = self.resolve(checker, elem)?;
                Some(
                    checker
                        .interner
                        .intern(TyKind::Pointer { is_readonly, elem }),
                )
            }
            InferShape::Optional { elem } => {
                let elem = self.resolve(checker, elem)?;
                Some(checker.interner.intern(TyKind::Optional { elem }))
            }
            InferShape::ErrorUnion { error, value } => {
                let error = self.resolve(checker, error)?;
                let value = self.resolve(checker, value)?;
                Some(checker.interner.intern(TyKind::ErrorUnion { error, value }))
            }
            InferShape::Callable {
                params: _,
                return_type: _,
            } => None,
        }
    }

    fn inferred_type(&mut self, checker: &BodyChecker<'_>, id: InferId) -> InferredType {
        let Some(shape) = self.expanded_shape(checker, id) else {
            return InferredType::Unknown;
        };
        match shape {
            InferShape::Known(ty) => InferredType::Known(ty),
            InferShape::Tuple(elems) => InferredType::Tuple(
                elems
                    .into_iter()
                    .map(|elem| self.inferred_type(checker, elem))
                    .collect(),
            ),
            InferShape::Pointer { is_readonly, elem } => InferredType::Pointer {
                is_readonly,
                elem: Box::new(self.inferred_type(checker, elem)),
            },
            InferShape::Optional { elem } => {
                InferredType::Optional(Box::new(self.inferred_type(checker, elem)))
            }
            InferShape::ErrorUnion { error, value } => InferredType::ErrorUnion {
                error: Box::new(self.inferred_type(checker, error)),
                value: Box::new(self.inferred_type(checker, value)),
            },
            InferShape::Callable {
                params,
                return_type,
            } => InferredType::Callable {
                params: params
                    .into_iter()
                    .map(|param| self.inferred_type(checker, param))
                    .collect(),
                return_type: Box::new(self.inferred_type(checker, return_type)),
            },
        }
    }

    fn local_term(&mut self, local: LocalId) -> InferId {
        if let Some(term) = self.locals.get(&local).copied() {
            return term;
        }
        let term = self.fresh();
        self.locals.insert(local, term);
        term
    }

    fn expr_term(&mut self, expr: &Expr) -> InferId {
        if let Some(term) = self.exprs.get(&expr.node_key).copied() {
            return term;
        }
        let term = self.fresh();
        self.exprs.insert(expr.node_key.clone(), term);
        term
    }

    fn collect_block_locals(&mut self, checker: &BodyChecker<'_>, block: &Block) {
        for stmt in &block.stmts {
            self.collect_stmt_locals(checker, stmt);
        }
        if let Some(tail) = block.tail.as_deref() {
            self.collect_expr_locals(checker, tail);
        }
    }

    fn collect_stmt_locals(&mut self, checker: &BodyChecker<'_>, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(binding) => {
                self.collect_pattern_locals(checker, &binding.pattern);
                if let Some(value) = &binding.value {
                    self.collect_expr_locals(checker, value);
                }
            }
            StmtKind::Expr(expr) | StmtKind::Defer(expr) => {
                self.collect_expr_locals(checker, expr);
            }
            StmtKind::Return(value) => {
                if let Some(value) = value {
                    self.collect_expr_locals(checker, value);
                }
            }
            StmtKind::ForIn(for_stmt) => {
                self.collect_pattern_locals(checker, &for_stmt.pattern);
                self.collect_expr_locals(checker, &for_stmt.iter);
                self.collect_block_locals(checker, &for_stmt.body);
            }
            StmtKind::While(while_stmt) => {
                self.collect_expr_locals(checker, &while_stmt.cond);
                self.collect_block_locals(checker, &while_stmt.body);
            }
            StmtKind::Loop(loop_stmt) => self.collect_block_locals(checker, &loop_stmt.body),
            StmtKind::Static(_) | StmtKind::Using(_) | StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn collect_pattern_locals(&mut self, checker: &BodyChecker<'_>, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Bind { node_key, .. } => {
                if let Some(local) = checker.local_def(node_key) {
                    self.local_term(local);
                }
            }
            PatternKind::Pointer(inner)
            | PatternKind::MutPointer(inner)
            | PatternKind::OptionalSome(inner)
            | PatternKind::ErrorOk(inner)
            | PatternKind::ErrorErr(inner) => self.collect_pattern_locals(checker, inner),
            PatternKind::Tuple(patterns) => {
                for pattern in patterns {
                    self.collect_pattern_locals(checker, pattern);
                }
            }
            PatternKind::Nominal { fields, .. } => match fields {
                nia_ast::NominalPatternFields::Tuple(fields) => {
                    for field in fields {
                        self.collect_pattern_locals(checker, field);
                    }
                }
                nia_ast::NominalPatternFields::Named { fields, .. } => {
                    for field in fields {
                        self.collect_pattern_locals(checker, &field.pattern);
                    }
                }
            },
            PatternKind::Wildcard
            | PatternKind::OptionalNull
            | PatternKind::Expr(_)
            | PatternKind::Range { .. } => {}
        }
    }

    fn collect_expr_locals(&mut self, checker: &BodyChecker<'_>, expr: &Expr) {
        if let ExprKind::Closure {
            captures,
            params,
            body,
            ..
        } = &expr.kind
        {
            for capture in captures {
                self.collect_expr_locals(checker, &capture.value);
                if let Some(local) = checker.local_def(&capture.node_key) {
                    self.local_term(local);
                }
            }
            for param in params {
                if let Some(local) = checker.local_def(&param.node_key) {
                    self.local_term(local);
                }
            }
            self.collect_expr_locals(checker, body);
            return;
        }
        nia_ast_walk::walk_expr(
            &mut LocalCollector {
                inference: self,
                checker,
            },
            expr,
        );
    }

    fn constrain_block(
        &mut self,
        checker: &mut BodyChecker<'_>,
        block: &Block,
        expected: Option<InferId>,
    ) -> InferId {
        for stmt in &block.stmts {
            self.constrain_stmt(checker, stmt);
        }
        let result = if let Some(tail) = block.tail.as_deref() {
            self.constrain_expr(checker, tail, expected)
        } else {
            self.known(checker.unit())
        };
        if let Some(expected) = expected {
            self.unify(checker, result, expected);
        }
        result
    }

    fn constrain_stmt(&mut self, checker: &mut BodyChecker<'_>, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Binding(binding) => {
                let explicit = binding.ty.as_ref().map(|ty| checker.ty_for_type(ty));
                let pattern = self.pattern_term(checker, &binding.pattern, explicit);
                if let Some(value) = &binding.value {
                    let value = self.constrain_expr(checker, value, Some(pattern));
                    self.unify(checker, pattern, value);
                }
            }
            StmtKind::Expr(expr) | StmtKind::Defer(expr) => {
                self.constrain_expr(checker, expr, None);
            }
            StmtKind::Return(value) => {
                if let (Some(expected), Some(value)) = (self.return_stack.last().copied(), value) {
                    let actual = self.constrain_expr(checker, value, Some(expected));
                    self.unify(checker, expected, actual);
                }
            }
            StmtKind::ForIn(for_stmt) => {
                self.constrain_expr(checker, &for_stmt.iter, None);
                self.constrain_block(checker, &for_stmt.body, None);
            }
            StmtKind::While(while_stmt) => {
                let bool_term = self.known(checker.bool());
                let cond = self.constrain_expr(checker, &while_stmt.cond, Some(bool_term));
                self.unify(checker, bool_term, cond);
                self.constrain_block(checker, &while_stmt.body, None);
            }
            StmtKind::Loop(loop_stmt) => {
                self.constrain_block(checker, &loop_stmt.body, None);
            }
            StmtKind::Static(_) | StmtKind::Using(_) | StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn pattern_term(
        &mut self,
        checker: &mut BodyChecker<'_>,
        pattern: &Pattern,
        explicit: Option<InternedTyId>,
    ) -> InferId {
        let term = match explicit {
            Some(ty) => self.known(ty),
            None => self.fresh(),
        };
        if let PatternKind::Bind { node_key, .. } = &pattern.kind
            && let Some(local) = checker.local_def(node_key)
        {
            let local = self.local_term(local);
            self.unify(checker, term, local);
        }
        term
    }

    fn constrain_expr(
        &mut self,
        checker: &mut BodyChecker<'_>,
        expr: &Expr,
        expected: Option<InferId>,
    ) -> InferId {
        let term = self.expr_term(expr);
        let actual = match &expr.kind {
            ExprKind::Integer(_) => self.known(
                integer_literal_suffix_ty(expr)
                    .map_or_else(|| checker.i32(), |ty| checker.interner.primitive(ty)),
            ),
            ExprKind::Float(_) => self.known(
                float_literal_suffix_ty(expr)
                    .map_or_else(|| checker.f64(), |ty| checker.interner.primitive(ty)),
            ),
            ExprKind::Bool(_) => self.known(checker.bool()),
            ExprKind::Char(_) => self.known(checker.interner.primitive(PrimitiveTy::Char)),
            ExprKind::ByteChar(_) => self.known(checker.interner.primitive(PrimitiveTy::U8)),
            ExprKind::Tuple(elems) => {
                let elems = elems
                    .iter()
                    .map(|elem| self.constrain_expr(checker, elem, None))
                    .collect();
                self.with_shape(InferShape::Tuple(elems))
            }
            ExprKind::Ident(_) | ExprKind::SelfValue => {
                if let Some(ty) = checker.inference_enum_variant_type(expr) {
                    self.known(ty)
                } else {
                    match checker.local_use(expr) {
                        Some(nia_local_resolve::LocalUse::Local(local)) => self.local_term(local),
                        _ => match checker.type_lowering.ty_for_key(&expr.node_key) {
                            Some(ty) => self.known(ty),
                            None => self.fresh(),
                        },
                    }
                }
            }
            ExprKind::Closure {
                captures,
                params,
                body,
            } => {
                for capture in captures {
                    let value = self.constrain_expr(checker, &capture.value, None);
                    if let Some(local) = checker.local_def(&capture.node_key) {
                        let local = self.local_term(local);
                        self.unify(checker, local, value);
                    }
                }
                let param_terms = params
                    .iter()
                    .map(|param| {
                        let term = match param.ty.as_ref() {
                            Some(ty) => {
                                let ty = checker.ty_for_type(ty);
                                self.known(ty)
                            }
                            None => self.fresh(),
                        };
                        if let Some(local) = checker.local_def(&param.node_key) {
                            let local = self.local_term(local);
                            self.unify(checker, local, term);
                        }
                        term
                    })
                    .collect::<Vec<_>>();
                let return_term = self.fresh();
                let callable = self.with_shape(InferShape::Callable {
                    params: param_terms.clone(),
                    return_type: return_term,
                });
                if let Some(expected) = expected {
                    self.unify(checker, callable, expected);
                }
                self.return_stack.push(return_term);
                let body_term = self.constrain_expr(checker, body, Some(return_term));
                self.return_stack.pop();
                self.unify(checker, return_term, body_term);
                self.closures.push(ClosureTerms {
                    key: expr.node_key.clone(),
                    params: param_terms,
                    return_type: return_term,
                });
                callable
            }
            ExprKind::Unary { op, expr: inner } => match op {
                UnaryOp::Ref | UnaryOp::RefReadOnly => {
                    if matches!(inner.kind, ExprKind::Closure { .. }) {
                        self.constrain_expr(checker, inner, expected)
                    } else {
                        let elem = self.constrain_expr(checker, inner, None);
                        self.with_shape(InferShape::Pointer {
                            is_readonly: matches!(op, UnaryOp::RefReadOnly),
                            elem,
                        })
                    }
                }
                UnaryOp::Deref => {
                    let elem = expected.unwrap_or_else(|| self.fresh());
                    let pointer = self.with_shape(InferShape::Pointer {
                        is_readonly: true,
                        elem,
                    });
                    let inner = self.constrain_expr(checker, inner, Some(pointer));
                    self.unify(checker, pointer, inner);
                    elem
                }
                UnaryOp::Not => {
                    let bool_term = self.known(checker.bool());
                    let inner = self.constrain_expr(checker, inner, Some(bool_term));
                    self.unify(checker, bool_term, inner);
                    bool_term
                }
                UnaryOp::Neg | UnaryOp::BitNot => self.constrain_expr(checker, inner, expected),
            },
            ExprKind::OptionalSome { expr: inner } => {
                let elem = self.fresh();
                let optional = self.with_shape(InferShape::Optional { elem });
                if let Some(expected) = expected {
                    self.unify(checker, optional, expected);
                }
                let inner = self.constrain_expr(checker, inner, Some(elem));
                self.unify(checker, elem, inner);
                optional
            }
            ExprKind::ErrorOk { expr: inner } => {
                let error = self.fresh();
                let value = self.fresh();
                let union = self.with_shape(InferShape::ErrorUnion { error, value });
                if let Some(expected) = expected {
                    self.unify(checker, union, expected);
                }
                let inner = self.constrain_expr(checker, inner, Some(value));
                self.unify(checker, value, inner);
                union
            }
            ExprKind::ErrorErr { expr: inner } => {
                let error = self.fresh();
                let value = self.fresh();
                let union = self.with_shape(InferShape::ErrorUnion { error, value });
                if let Some(expected) = expected {
                    self.unify(checker, union, expected);
                }
                let inner = self.constrain_expr(checker, inner, Some(error));
                self.unify(checker, error, inner);
                union
            }
            ExprKind::Call { callee, args } => {
                // Value resolution already distinguishes enum constructors
                // from callable values. Their result type is known before
                // body checking, while payloads may still contain constraints.
                if let Some(ty) = checker.inference_enum_variant_type(callee) {
                    for arg in args {
                        self.constrain_expr(checker, arg, None);
                    }
                    self.known(ty)
                } else {
                    let params = (0..args.len()).map(|_| self.fresh()).collect::<Vec<_>>();
                    let result = expected.unwrap_or_else(|| self.fresh());
                    let callable = self.with_shape(InferShape::Callable {
                        params: params.clone(),
                        return_type: result,
                    });
                    let callee = self.constrain_expr(checker, callee, Some(callable));
                    self.unify(checker, callable, callee);
                    for (arg, param) in args.iter().zip(params) {
                        let actual = self.constrain_expr(checker, arg, Some(param));
                        self.unify(checker, param, actual);
                    }
                    result
                }
            }
            ExprKind::Binary { lhs, op, rhs } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    let bool_term = self.known(checker.bool());
                    let lhs = self.constrain_expr(checker, lhs, Some(bool_term));
                    let rhs = self.constrain_expr(checker, rhs, Some(bool_term));
                    self.unify(checker, bool_term, lhs);
                    self.unify(checker, bool_term, rhs);
                    bool_term
                } else {
                    let lhs = self.constrain_expr(checker, lhs, None);
                    let rhs = self.constrain_expr(checker, rhs, None);
                    let output = if matches!(
                        op,
                        BinaryOp::Lt
                            | BinaryOp::Le
                            | BinaryOp::Gt
                            | BinaryOp::Ge
                            | BinaryOp::Eq
                            | BinaryOp::Ne
                    ) {
                        self.known(checker.bool())
                    } else {
                        expected.unwrap_or_else(|| self.fresh())
                    };
                    self.operators.push(OperatorObligation {
                        op: *op,
                        lhs,
                        rhs,
                        output,
                    });
                    output
                }
            }
            ExprKind::Assign { lhs, rhs, .. } => {
                let lhs = self.constrain_expr(checker, lhs, None);
                let rhs = self.constrain_expr(checker, rhs, Some(lhs));
                self.unify(checker, lhs, rhs);
                self.known(checker.unit())
            }
            ExprKind::Block(block) => self.constrain_block(checker, block, expected),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let bool_term = self.known(checker.bool());
                let cond = self.constrain_expr(checker, cond, Some(bool_term));
                self.unify(checker, bool_term, cond);
                let result = expected.unwrap_or_else(|| self.fresh());
                let then_term = self.constrain_block(checker, then_branch, Some(result));
                self.unify(checker, result, then_term);
                if let Some(else_branch) = else_branch {
                    let else_term = self.constrain_expr(checker, else_branch, Some(result));
                    self.unify(checker, result, else_term);
                }
                result
            }
            ExprKind::Cast { ty, expr: inner } => {
                self.constrain_expr(checker, inner, None);
                self.known(checker.ty_for_type(ty))
            }
            _ => {
                if let Some(ty) = checker.inference_enum_variant_type(expr) {
                    self.known(ty)
                } else {
                    match checker.type_lowering.ty_for_key(&expr.node_key) {
                        Some(ty) => self.known(ty),
                        None => expected.unwrap_or_else(|| self.fresh()),
                    }
                }
            }
        };
        self.unify(checker, term, actual);
        if let Some(expected) = expected {
            self.unify(checker, term, expected);
        }
        term
    }

    fn solve_operators(&mut self, checker: &mut BodyChecker<'_>) {
        // Operator traits are resolved only after ordinary equality
        // constraints settle. This reuses the authoritative trait solver and
        // avoids duplicating overload selection in the inference layer.
        let obligations = self.operators.clone();
        for obligation in obligations {
            let Some(lhs) = self.resolve(checker, obligation.lhs) else {
                continue;
            };
            let Some(rhs) = self.resolve(checker, obligation.rhs) else {
                continue;
            };
            let Some(trait_id) = nia_sema_ir::BuiltinOperatorOp::Binary(obligation.op).trait_id()
            else {
                continue;
            };
            let output = if matches!(
                obligation.op,
                BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::Eq
                    | BinaryOp::Ne
            ) {
                checker.bool()
            } else if checker.current_context_proves_trait_obligation(
                lhs,
                nia_ty::TraitId::Builtin(trait_id),
                vec![rhs],
            ) {
                let projection = checker.interner.intern(TyKind::Projection {
                    self_ty: lhs,
                    trait_id: nia_ty::TraitId::Builtin(trait_id),
                    trait_args: vec![rhs],
                    trait_const_args: Vec::new(),
                    name: nia_symbol::known::OUTPUT,
                });
                checker.normalize_projection(projection)
            } else {
                continue;
            };
            let output = self.known(output);
            self.unify(checker, obligation.output, output);
        }
    }
}

struct LocalCollector<'a, 'b, 'c> {
    inference: &'a mut FunctionInference,
    checker: &'b BodyChecker<'c>,
}

impl<'ast> nia_ast_walk::Visitor<'ast> for LocalCollector<'_, '_, '_> {
    fn visit_expr(&mut self, expr: &'ast Expr) {
        self.inference.collect_expr_locals(self.checker, expr);
    }
}
