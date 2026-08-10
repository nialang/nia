// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::{BTreeSet, HashMap, HashSet};

use nia_body_ir::{
    PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee, TypedExpr,
    TypedExprKind, TypedMemoryIntrinsicSource, TypedPattern, TypedPatternKind, TypedPlace,
    TypedStmtKind, TypedSwitchArmBody,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{ClosureId, GlobalDefId, LocalId};
use nia_span::Span;
use nia_ty::{TyKind, TypeStore};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClosureEscapeSummary {
    pub returned_parameters: BTreeSet<usize>,
    pub escaping_parameters: BTreeSet<usize>,
    pub returned_captured_address_parameters: BTreeSet<usize>,
    pub escaping_captured_address_parameters: BTreeSet<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosureCheck {
    pub summaries: HashMap<GlobalDefId, ClosureEscapeSummary>,
    pub diagnostics: Vec<ClosureCheckDiagnostic>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosureCheckDiagnostic {
    pub owner: GlobalDefId,
    pub diagnostic: Diagnostic,
}

#[derive(Debug, Clone, Copy)]
pub struct ClosureCheckFunction<'a> {
    pub def_id: GlobalDefId,
    pub body: &'a TypedBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum CallableKey {
    Function(GlobalDefId),
    Closure(ClosureId),
}

#[derive(Debug, Clone)]
struct CallableBody<'a> {
    captures: Vec<LocalId>,
    params: Vec<LocalId>,
    body: &'a TypedBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum InputSource {
    Capture(usize),
    Parameter(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Provenance {
    Input(InputSource),
    StackAddress {
        scope_depth: usize,
    },
    CapturedInputAddress(InputSource),
    CapturedStackAddress {
        scope_depth: usize,
    },
    CallableClosure {
        closure_id: ClosureId,
        stack_backed: bool,
    },
}

type Provenances = BTreeSet<Provenance>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ValueProvenance {
    value: Provenances,
    error: Provenances,
}

impl ValueProvenance {
    fn from_value(value: Provenances) -> Self {
        Self {
            value,
            error: Provenances::new(),
        }
    }

    fn all(&self) -> Provenances {
        union(self.value.clone(), self.error.clone())
    }

    fn extend(&mut self, other: Self) {
        self.value.extend(other.value);
        self.error.extend(other.error);
    }
}

type Environment = HashMap<LocalId, ValueProvenance>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CallableSummary {
    returned_inputs: BTreeSet<InputSource>,
    returned_error_inputs: BTreeSet<InputSource>,
    escaping_inputs: BTreeSet<InputSource>,
    returned_captured_addresses: BTreeSet<InputSource>,
    returned_error_captured_addresses: BTreeSet<InputSource>,
    escaping_captured_addresses: BTreeSet<InputSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EscapeKind {
    Return,
    Store,
    Call,
    Scope,
}

pub fn check_closure_safety(
    functions: &[ClosureCheckFunction<'_>],
    type_store: &TypeStore,
) -> ClosureCheck {
    let mut callables = HashMap::new();
    for function in functions {
        callables.insert(
            CallableKey::Function(function.def_id),
            CallableBody {
                captures: Vec::new(),
                params: function
                    .body
                    .locals
                    .iter()
                    .filter(|local| matches!(local.kind, nia_body_ir::TypedLocalKind::Param))
                    .map(|local| local.id)
                    .collect(),
                body: function.body,
            },
        );
        collect_body_closures(function.body, &mut callables);
    }

    let mut summaries = callables
        .keys()
        .copied()
        .map(|key| (key, CallableSummary::default()))
        .collect::<HashMap<_, _>>();
    loop {
        let mut changed = false;
        for (key, callable) in &callables {
            let summary = Analyzer::new(type_store, &summaries, None).summarize(callable);
            if summaries.get(key) != Some(&summary) {
                summaries.insert(*key, summary);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut diagnostics = Vec::new();
    let mut reported = HashSet::new();
    let mut callable_keys = callables.keys().copied().collect::<Vec<_>>();
    callable_keys.sort_unstable();
    for key in callable_keys {
        let callable = callables
            .get(&key)
            .expect("collected callable key must retain its body");
        Analyzer::new(
            type_store,
            &summaries,
            Some(DiagnosticSink {
                owner: match key {
                    CallableKey::Function(def_id) => def_id,
                    CallableKey::Closure(closure_id) => closure_id.owner,
                },
                diagnostics: &mut diagnostics,
                reported: &mut reported,
            }),
        )
        .summarize(callable);
    }

    let summaries = summaries
        .into_iter()
        .filter_map(|(key, summary)| match key {
            CallableKey::Function(def_id) => Some((
                def_id,
                ClosureEscapeSummary {
                    returned_parameters: summary
                        .returned_inputs
                        .into_iter()
                        .chain(summary.returned_error_inputs)
                        .filter_map(|source| match source {
                            InputSource::Parameter(index) => Some(index),
                            InputSource::Capture(_) => None,
                        })
                        .collect(),
                    escaping_parameters: summary
                        .escaping_inputs
                        .into_iter()
                        .filter_map(|source| match source {
                            InputSource::Parameter(index) => Some(index),
                            InputSource::Capture(_) => None,
                        })
                        .collect(),
                    returned_captured_address_parameters: parameter_sources(
                        summary
                            .returned_captured_addresses
                            .into_iter()
                            .chain(summary.returned_error_captured_addresses)
                            .collect(),
                    ),
                    escaping_captured_address_parameters: parameter_sources(
                        summary.escaping_captured_addresses,
                    ),
                },
            )),
            CallableKey::Closure(_) => None,
        })
        .collect();
    ClosureCheck {
        summaries,
        diagnostics,
    }
}

struct DiagnosticSink<'a> {
    owner: GlobalDefId,
    diagnostics: &'a mut Vec<ClosureCheckDiagnostic>,
    reported: &'a mut HashSet<(GlobalDefId, Span, EscapeKind)>,
}

struct Analyzer<'a> {
    type_store: &'a TypeStore,
    summaries: &'a HashMap<CallableKey, CallableSummary>,
    returned: Provenances,
    returned_errors: Provenances,
    escaped: Provenances,
    diagnostics: Option<DiagnosticSink<'a>>,
    scope_depth: usize,
    closure_scopes: HashMap<ClosureId, usize>,
}

impl<'a> Analyzer<'a> {
    fn new(
        type_store: &'a TypeStore,
        summaries: &'a HashMap<CallableKey, CallableSummary>,
        diagnostics: Option<DiagnosticSink<'a>>,
    ) -> Self {
        Self {
            type_store,
            summaries,
            returned: Provenances::new(),
            returned_errors: Provenances::new(),
            escaped: Provenances::new(),
            diagnostics,
            scope_depth: 0,
            closure_scopes: HashMap::new(),
        }
    }

    fn summarize(mut self, callable: &CallableBody<'_>) -> CallableSummary {
        let mut env = Environment::new();
        for (index, local_id) in callable.captures.iter().copied().enumerate() {
            env.insert(
                local_id,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::Capture(index),
                )])),
            );
        }
        for (index, local_id) in callable.params.iter().copied().enumerate() {
            env.insert(
                local_id,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::Parameter(index),
                )])),
            );
        }
        let tail = self.analyze_body_contents(callable.body, &mut env);
        self.record_return(&tail, callable.body.span);
        CallableSummary {
            returned_inputs: input_sources(&self.returned),
            returned_error_inputs: input_sources(&self.returned_errors),
            escaping_inputs: input_sources(&self.escaped),
            returned_captured_addresses: captured_input_sources(&self.returned),
            returned_error_captured_addresses: captured_input_sources(&self.returned_errors),
            escaping_captured_addresses: captured_input_sources(&self.escaped),
        }
    }

    fn analyze_body_contents(
        &mut self,
        body: &TypedBody,
        env: &mut Environment,
    ) -> ValueProvenance {
        for stmt in &body.stmts {
            match &stmt.kind {
                TypedStmtKind::Binding(binding) => {
                    let value = binding
                        .value
                        .as_ref()
                        .map(|value| self.analyze_expr(value, env))
                        .unwrap_or_default();
                    env.insert(binding.local_id, value);
                }
                TypedStmtKind::PatternBinding(binding) => {
                    let value = self.analyze_expr(&binding.value, env);
                    bind_pattern(&binding.pattern, &value, env);
                }
                TypedStmtKind::Expr(expr) | TypedStmtKind::Defer(expr) => {
                    self.analyze_expr(expr, env);
                }
                TypedStmtKind::Return(value) => {
                    let value = value
                        .as_ref()
                        .map(|value| self.analyze_expr(value, env))
                        .unwrap_or_default();
                    self.record_return(&value, stmt.span);
                }
                TypedStmtKind::ForIn(for_in) => {
                    let value = self.analyze_expr(&for_in.iter, env);
                    let mut loop_env = env.clone();
                    bind_pattern(&for_in.pattern, &value, &mut loop_env);
                    self.analyze_loop(&for_in.body, env, loop_env);
                }
                TypedStmtKind::While(while_stmt) => {
                    self.analyze_expr(&while_stmt.cond, env);
                    self.analyze_loop(&while_stmt.body, env, env.clone());
                }
                TypedStmtKind::Loop(loop_stmt) => {
                    self.analyze_loop(&loop_stmt.body, env, env.clone());
                }
                TypedStmtKind::Break | TypedStmtKind::Continue => {}
            }
        }
        body.tail
            .as_deref()
            .map(|tail| self.analyze_expr(tail, env))
            .unwrap_or_default()
    }

    fn analyze_loop(&mut self, body: &TypedBody, outer: &mut Environment, mut head: Environment) {
        let entry = outer.clone();
        loop {
            let mut next = head.clone();
            self.analyze_nested_body(body, &mut next);
            join_environment(&mut next, &entry);
            if next == head {
                *outer = next;
                break;
            }
            head = next;
        }
    }

    fn analyze_nested_body(&mut self, body: &TypedBody, env: &mut Environment) -> ValueProvenance {
        self.scope_depth = self.scope_depth.saturating_add(1);
        let depth = self.scope_depth;
        let value = self.analyze_body_contents(body, env);
        let locals = body
            .locals
            .iter()
            .map(|local| local.id)
            .collect::<HashSet<_>>();
        let mut crossing = value.all();
        for (local_id, origins) in env.iter() {
            if !locals.contains(local_id) {
                crossing.extend(origins.all());
            }
        }
        self.report_scope_exit(&crossing, body.span, depth);
        env.retain(|local_id, _| !locals.contains(local_id));
        self.scope_depth = self.scope_depth.saturating_sub(1);
        value
    }

    fn analyze_expr(&mut self, expr: &TypedExpr, env: &mut Environment) -> ValueProvenance {
        let origins = match &expr.kind {
            TypedExprKind::Error
            | TypedExprKind::Integer(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::String(_)
            | TypedExprKind::ByteString(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::ByteChar(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Null
            | TypedExprKind::Global(_)
            | TypedExprKind::ConstGeneric(_)
            | TypedExprKind::Function(_)
            | TypedExprKind::FunctionInstance { .. }
            | TypedExprKind::BuiltinValue(_)
            | TypedExprKind::Trap
            | TypedExprKind::ClosureFunctionPointer { .. } => ValueProvenance::default(),
            TypedExprKind::Local(local_id) => env.get(local_id).cloned().unwrap_or_default(),
            TypedExprKind::EnumVariant { fields, .. } | TypedExprKind::Tuple(fields) => {
                ValueProvenance::from_value(self.analyze_exprs(fields, env))
            }
            TypedExprKind::Closure {
                captures,
                closure_id,
                params: _,
                body: _,
            } => {
                self.closure_scopes
                    .entry(*closure_id)
                    .or_insert(self.scope_depth);
                ValueProvenance::from_value(capture_address_origins(
                    captures
                        .iter()
                        .map(|capture| self.analyze_expr(&capture.value, env).all())
                        .fold(Provenances::new(), union),
                ))
            }
            TypedExprKind::Range(range) => ValueProvenance::from_value(
                range
                    .start
                    .iter()
                    .chain(&range.end)
                    .map(|bound| self.analyze_expr(bound, env).all())
                    .fold(Provenances::new(), union),
            ),
            TypedExprKind::InlineAsm(asm) => {
                let values = asm
                    .inputs
                    .iter()
                    .map(|input| self.analyze_expr(&input.value, env).all())
                    .fold(Provenances::new(), union);
                self.record_escape(&values, expr.span, EscapeKind::Call);
                for output in &asm.outputs {
                    self.analyze_place(&output.place, env);
                }
                ValueProvenance::default()
            }
            TypedExprKind::MemoryIntrinsic(intrinsic) => {
                let dest = self.analyze_expr(&intrinsic.dest, env).all();
                let source = match &intrinsic.source {
                    TypedMemoryIntrinsicSource::Slice(source)
                    | TypedMemoryIntrinsicSource::Byte(source) => {
                        self.analyze_expr(source, env).all()
                    }
                };
                self.record_escape(&source, expr.span, EscapeKind::Store);
                ValueProvenance::from_value(union(dest, source))
            }
            TypedExprKind::Atomic(atomic) => self.analyze_atomic(atomic, expr.span, env),
            TypedExprKind::LoadUnaligned { ptr, .. }
            | TypedExprKind::Splat { value: ptr }
            | TypedExprKind::BitIntrinsic { value: ptr, .. }
            | TypedExprKind::CharFromU32 { value: ptr }
            | TypedExprKind::StaticArrayPointer { array: ptr, .. }
            | TypedExprKind::OptionalSome { expr: ptr }
            | TypedExprKind::Discard(ptr)
            | TypedExprKind::TraitObjectUpcast { expr: ptr, .. }
            | TypedExprKind::TraitObjectCoercion { expr: ptr, .. } => {
                ValueProvenance::from_value(self.analyze_expr(ptr, env).all())
            }
            TypedExprKind::ErrorOk { expr: inner } => {
                ValueProvenance::from_value(self.analyze_expr(inner, env).all())
            }
            TypedExprKind::ErrorErr { expr: inner } => ValueProvenance {
                value: Provenances::new(),
                error: self.analyze_expr(inner, env).all(),
            },
            TypedExprKind::Cast { expr: inner, .. } => {
                let value = self.analyze_expr(inner, env).all();
                if is_pointer_type(self.type_store, inner.ty)
                    && is_integer_type(self.type_store, expr.ty)
                {
                    ValueProvenance::default()
                } else {
                    ValueProvenance::from_value(value)
                }
            }
            TypedExprKind::Unary { op, expr: inner } => {
                let mut value = self.analyze_expr(inner, env).all();
                if matches!(op, nia_ast::UnaryOp::Ref | nia_ast::UnaryOp::RefReadOnly)
                    && address_uses_stack_storage(inner)
                {
                    value.insert(Provenance::StackAddress {
                        scope_depth: self.scope_depth,
                    });
                }
                ValueProvenance::from_value(value)
            }
            TypedExprKind::ExtractElement { vector, index } => ValueProvenance::from_value(union(
                self.analyze_expr(vector, env).all(),
                self.analyze_expr(index, env).all(),
            )),
            TypedExprKind::InsertElement {
                vector,
                index,
                value,
            } => ValueProvenance::from_value(
                self.analyze_exprs([vector.as_ref(), index.as_ref(), value.as_ref()], env),
            ),
            TypedExprKind::Bitmask { vector } => {
                ValueProvenance::from_value(self.analyze_expr(vector, env).all())
            }
            TypedExprKind::ArrayLiteral { elems } => match elems {
                TypedArrayElements::List(elems) => {
                    ValueProvenance::from_value(self.analyze_exprs(elems, env))
                }
                TypedArrayElements::Repeat { value, .. } => {
                    ValueProvenance::from_value(self.analyze_expr(value, env).all())
                }
            },
            TypedExprKind::StructLiteral { fields, .. } => ValueProvenance::from_value(
                fields
                    .iter()
                    .map(|field| self.analyze_expr(&field.value, env).all())
                    .fold(Provenances::new(), union),
            ),
            TypedExprKind::UnionLiteral { field, .. } => {
                ValueProvenance::from_value(self.analyze_expr(&field.value, env).all())
            }
            TypedExprKind::UnionStorageLiteral { relocations, .. } => ValueProvenance::from_value(
                relocations
                    .iter()
                    .map(|relocation| self.analyze_expr(&relocation.pointee, env).all())
                    .fold(Provenances::new(), union),
            ),
            TypedExprKind::Try { expr: inner, .. } => {
                let value = self.analyze_expr(inner, env);
                self.record_error_return(&value.error, expr.span);
                ValueProvenance::from_value(value.value)
            }
            TypedExprKind::Binary { lhs, rhs, .. } | TypedExprKind::Index { lhs, index: rhs } => {
                ValueProvenance::from_value(union(
                    self.analyze_expr(lhs, env).all(),
                    self.analyze_expr(rhs, env).all(),
                ))
            }
            TypedExprKind::Assign { place, rhs, .. } => {
                let value = self.analyze_expr(rhs, env);
                self.assign_place(place, &value, env, expr.span);
                value
            }
            TypedExprKind::CallableCoercion { state, closure_id } => {
                let mut value = self.analyze_expr(state, env).all();
                let stack_backed = value
                    .iter()
                    .any(|origin| matches!(origin, Provenance::StackAddress { .. }));
                value.insert(Provenance::CallableClosure {
                    closure_id: *closure_id,
                    stack_backed,
                });
                ValueProvenance::from_value(value)
            }
            TypedExprKind::Call { callee, args } => self.analyze_call(callee, args, expr, env),
            TypedExprKind::Field { lhs, .. } | TypedExprKind::TupleField { lhs, .. } => {
                ValueProvenance::from_value(self.analyze_expr(lhs, env).all())
            }
            TypedExprKind::Slice { lhs, range, .. } => {
                let mut value = self.analyze_expr(lhs, env).all();
                if let Some(start) = &range.start {
                    value.extend(self.analyze_expr(start, env).all());
                }
                if let Some(end) = &range.end {
                    value.extend(self.analyze_expr(end, env).all());
                }
                ValueProvenance::from_value(value)
            }
            TypedExprKind::Block(body) => self.analyze_nested_body(body, env),
            TypedExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.analyze_expr(cond, env);
                let base = env.clone();
                let mut then_env = base.clone();
                let then_value = self.analyze_nested_body(then_branch, &mut then_env);
                let mut else_env = base;
                let else_value = else_branch
                    .as_deref()
                    .map(|branch| self.analyze_expr(branch, &mut else_env))
                    .unwrap_or_default();
                join_environment(&mut then_env, &else_env);
                *env = then_env;
                let mut value = then_value;
                value.extend(else_value);
                value
            }
            TypedExprKind::IfPattern(pattern) => {
                let target = self.analyze_expr(&pattern.target, env);
                let base = env.clone();
                let mut then_env = base.clone();
                bind_pattern(&pattern.pattern, &target, &mut then_env);
                let then_value = self.analyze_nested_body(&pattern.then_branch, &mut then_env);
                let mut else_env = base;
                let else_value = pattern
                    .else_branch
                    .as_deref()
                    .map(|branch| self.analyze_expr(branch, &mut else_env))
                    .unwrap_or_default();
                join_environment(&mut then_env, &else_env);
                *env = then_env;
                let mut value = then_value;
                value.extend(else_value);
                value
            }
            TypedExprKind::Switch(switch) => {
                let target = self.analyze_expr(&switch.target, env);
                let base = env.clone();
                let mut merged = base.clone();
                let mut value = ValueProvenance::default();
                for arm in &switch.arms {
                    let mut arm_env = base.clone();
                    for pattern in &arm.patterns {
                        bind_pattern(pattern, &target, &mut arm_env);
                    }
                    let arm_value = match &arm.body {
                        TypedSwitchArmBody::Expr(expr) => self.analyze_expr(expr, &mut arm_env),
                        TypedSwitchArmBody::Stmt(stmt) => {
                            let body = TypedBody {
                                span: stmt.span,
                                locals: Vec::new(),
                                stmts: vec![stmt.as_ref().clone()],
                                tail: None,
                                ty: expr.ty,
                            };
                            self.analyze_nested_body(&body, &mut arm_env)
                        }
                        TypedSwitchArmBody::Block(body) => {
                            self.analyze_nested_body(body, &mut arm_env)
                        }
                    };
                    join_environment(&mut merged, &arm_env);
                    value.extend(arm_value);
                }
                *env = merged;
                value
            }
        };
        self.filter_for_type(origins, expr.ty)
    }

    fn analyze_exprs<'b>(
        &mut self,
        exprs: impl IntoIterator<Item = &'b TypedExpr>,
        env: &mut Environment,
    ) -> Provenances {
        exprs
            .into_iter()
            .map(|expr| self.analyze_expr(expr, env).all())
            .fold(Provenances::new(), union)
    }

    fn analyze_atomic(
        &mut self,
        atomic: &TypedAtomic,
        span: Span,
        env: &mut Environment,
    ) -> ValueProvenance {
        match atomic {
            TypedAtomic::Load { ptr, .. } => {
                ValueProvenance::from_value(self.analyze_expr(ptr, env).all())
            }
            TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
                let ptr = self.analyze_expr(ptr, env).all();
                let value = self.analyze_expr(value, env).all();
                self.record_escape(&value, span, EscapeKind::Store);
                ValueProvenance::from_value(union(ptr, value))
            }
            TypedAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                let ptr = self.analyze_expr(ptr, env).all();
                let expected = self.analyze_expr(expected, env).all();
                let desired = self.analyze_expr(desired, env).all();
                self.record_escape(&desired, span, EscapeKind::Store);
                ValueProvenance::from_value(union(union(ptr, expected), desired))
            }
            TypedAtomic::Fence { .. } => ValueProvenance::default(),
        }
    }

    fn analyze_call(
        &mut self,
        callee: &TypedCallee,
        args: &[TypedExpr],
        call: &TypedExpr,
        env: &mut Environment,
    ) -> ValueProvenance {
        let args = args
            .iter()
            .map(|arg| self.analyze_expr(arg, env).all())
            .collect::<Vec<_>>();
        match callee {
            TypedCallee::Function(def_id) | TypedCallee::FunctionInstance { def_id, .. } => self
                .apply_summary(
                    CallableKey::Function(*def_id),
                    &Provenances::new(),
                    &args,
                    call.span,
                    call.ty,
                ),
            TypedCallee::Method {
                def_id, receiver, ..
            } => {
                let receiver_origins = self.analyze_expr(receiver, env).all();
                let mut operands = vec![receiver_origins];
                operands.extend(args);
                self.apply_summary(
                    CallableKey::Function(*def_id),
                    &Provenances::new(),
                    &operands,
                    call.span,
                    call.ty,
                )
            }
            TypedCallee::TraitMethod {
                method_id,
                receiver,
                ..
            } => {
                let receiver = self.analyze_expr(receiver, env).all();
                let mut operands = vec![receiver];
                operands.extend(args);
                self.apply_summary(
                    CallableKey::Function(*method_id),
                    &Provenances::new(),
                    &operands,
                    call.span,
                    call.ty,
                )
            }
            TypedCallee::TraitAssociatedFunction { method_id, .. } => self.apply_summary(
                CallableKey::Function(*method_id),
                &Provenances::new(),
                &args,
                call.span,
                call.ty,
            ),
            TypedCallee::Closure(state) => {
                let state_origins = self.analyze_expr(state, env).all();
                let closure_id = match self.type_store.get(state.ty) {
                    Some(TyKind::ClosureState { closure_id, .. }) => Some(*closure_id),
                    _ => None,
                };
                match closure_id {
                    Some(closure_id) => self.apply_summary(
                        CallableKey::Closure(closure_id),
                        &state_origins,
                        &args,
                        call.span,
                        call.ty,
                    ),
                    None => self.apply_unknown_call(&args, call.span, call.ty),
                }
            }
            TypedCallee::Callable(callee) => {
                let callee = self.analyze_expr(callee, env).all();
                let closure_ids = callee
                    .iter()
                    .filter_map(|origin| match origin {
                        Provenance::CallableClosure { closure_id, .. } => Some(*closure_id),
                        Provenance::Input(_)
                        | Provenance::StackAddress { .. }
                        | Provenance::CapturedInputAddress(_)
                        | Provenance::CapturedStackAddress { .. } => None,
                    })
                    .collect::<BTreeSet<_>>();
                let mut result = ValueProvenance::default();
                for closure_id in &closure_ids {
                    let mut captures = callee.clone();
                    captures.retain(|origin| {
                        !matches!(
                            origin,
                            Provenance::CallableClosure {
                                closure_id: candidate,
                                ..
                            } if candidate == closure_id
                        )
                    });
                    result.extend(self.apply_summary(
                        CallableKey::Closure(*closure_id),
                        &captures,
                        &args,
                        call.span,
                        call.ty,
                    ));
                }
                if closure_ids.is_empty()
                    || callee
                        .iter()
                        .any(|origin| matches!(origin, Provenance::Input(_)))
                {
                    result.extend(self.apply_unknown_call(&args, call.span, call.ty));
                }
                result
            }
            TypedCallee::FunctionPointer(callee) => {
                self.analyze_expr(callee, env);
                self.apply_unknown_call(&args, call.span, call.ty)
            }
            TypedCallee::DynamicTraitMethod { receiver, .. } => {
                let receiver = self.analyze_expr(receiver, env).all();
                let mut operands = vec![receiver];
                operands.extend(args);
                self.apply_unknown_call(&operands, call.span, call.ty)
            }
            TypedCallee::BuiltinMethod { receiver, .. }
            | TypedCallee::BuiltinPlaceMethod(nia_body_ir::BuiltinPlaceMethod {
                receiver, ..
            }) => {
                let mut result = self.analyze_expr(receiver, env).all();
                for arg in args {
                    result.extend(arg);
                }
                ValueProvenance::from_value(result)
            }
            TypedCallee::BuiltinOperator(_) => {
                ValueProvenance::from_value(args.into_iter().fold(Provenances::new(), union))
            }
        }
    }

    fn apply_summary(
        &mut self,
        key: CallableKey,
        captures: &Provenances,
        args: &[Provenances],
        span: Span,
        return_ty: nia_ids::InternedTyId,
    ) -> ValueProvenance {
        let Some(summary) = self.summaries.get(&key) else {
            return self.apply_unknown_call(args, span, return_ty);
        };
        let mut result = Provenances::new();
        for source in &summary.returned_inputs {
            result.extend(input_origins(*source, captures, args));
        }
        let mut error = Provenances::new();
        for source in &summary.returned_error_inputs {
            error.extend(input_origins(*source, captures, args));
        }
        for source in &summary.escaping_inputs {
            self.record_escape(
                &input_origins(*source, captures, args),
                span,
                EscapeKind::Call,
            );
        }
        for source in &summary.returned_captured_addresses {
            result.extend(capture_address_origins(input_origins(
                *source, captures, args,
            )));
        }
        for source in &summary.returned_error_captured_addresses {
            error.extend(capture_address_origins(input_origins(
                *source, captures, args,
            )));
        }
        for source in &summary.escaping_captured_addresses {
            self.record_escape(
                &capture_address_origins(input_origins(*source, captures, args)),
                span,
                EscapeKind::Call,
            );
        }
        ValueProvenance {
            value: result,
            error,
        }
    }

    fn apply_unknown_call(
        &mut self,
        args: &[Provenances],
        span: Span,
        return_ty: nia_ids::InternedTyId,
    ) -> ValueProvenance {
        let result = args.iter().cloned().fold(Provenances::new(), union);
        self.record_escape(&result, span, EscapeKind::Call);
        match self.type_store.get(return_ty) {
            Some(TyKind::ErrorUnion { .. }) => ValueProvenance {
                value: result.clone(),
                error: result,
            },
            _ => ValueProvenance::from_value(result),
        }
    }

    fn analyze_place(&mut self, place: &TypedPlace, env: &mut Environment) -> Provenances {
        let mut value = match &place.base {
            PlaceBase::Local(local_id) => env.get(local_id).cloned().unwrap_or_default().all(),
            PlaceBase::Global(_) | PlaceBase::Error => Provenances::new(),
            PlaceBase::Deref(expr) => self.analyze_expr(expr, env).all(),
        };
        for elem in &place.elems {
            if let PlaceElem::Index(index) = elem {
                value.extend(self.analyze_expr(index, env).all());
            }
        }
        value
    }

    fn assign_place(
        &mut self,
        place: &TypedPlace,
        value: &ValueProvenance,
        env: &mut Environment,
        span: Span,
    ) {
        self.analyze_place(place, env);
        match &place.base {
            PlaceBase::Local(local_id) if place.elems.is_empty() => {
                env.insert(*local_id, value.clone());
            }
            PlaceBase::Local(local_id) => {
                env.entry(*local_id).or_default().value.extend(value.all());
            }
            PlaceBase::Global(_) | PlaceBase::Deref(_) => {
                self.record_escape(&value.all(), span, EscapeKind::Store);
            }
            PlaceBase::Error => {}
        }
    }

    fn record_return(&mut self, value: &ValueProvenance, span: Span) {
        self.returned.extend(&value.value);
        self.returned_errors.extend(&value.error);
        self.report_escaping_state(&value.all(), span, EscapeKind::Return);
    }

    fn record_error_return(&mut self, error: &Provenances, span: Span) {
        self.returned_errors.extend(error);
        self.report_escaping_state(error, span, EscapeKind::Return);
    }

    fn record_escape(&mut self, value: &Provenances, span: Span, kind: EscapeKind) {
        self.escaped.extend(value);
        self.report_escaping_state(value, span, kind);
    }

    fn report_escaping_state(&mut self, value: &Provenances, span: Span, kind: EscapeKind) {
        let Some(sink) = &mut self.diagnostics else {
            return;
        };
        let stack_closure = value.iter().any(|origin| {
            matches!(
                origin,
                Provenance::CallableClosure {
                    stack_backed: true,
                    ..
                }
            )
        });
        let captured_stack_address = value
            .iter()
            .any(|origin| matches!(origin, Provenance::CapturedStackAddress { .. }));
        if (stack_closure || captured_stack_address)
            && sink.reported.insert((sink.owner, span, kind))
        {
            let context = match kind {
                EscapeKind::Return => "returned",
                EscapeKind::Store => "stored outside its local frame",
                EscapeKind::Call => "passed to a call that may retain it",
                EscapeKind::Scope => "moved beyond its closure state's lexical scope",
            };
            let summary = if stack_closure {
                format!(
                    "stack-backed callable view cannot be {context}; use it only while its closure state is live"
                )
            } else {
                format!(
                    "closure state capturing a local address cannot be {context}; keep the state within the captured storage's lifetime"
                )
            };
            sink.diagnostics.push(ClosureCheckDiagnostic {
                owner: sink.owner,
                diagnostic: Diagnostic::user_error_at(codes::TYPE_CHECK, span, summary),
            });
        }
    }

    fn report_scope_exit(&mut self, value: &Provenances, span: Span, depth: usize) {
        let escaping = value
            .iter()
            .filter_map(|origin| match origin {
                Provenance::CallableClosure {
                    closure_id,
                    stack_backed: true,
                } if self
                    .closure_scopes
                    .get(closure_id)
                    .is_some_and(|closure_depth| *closure_depth >= depth) =>
                {
                    Some(*origin)
                }
                Provenance::CapturedStackAddress { scope_depth } if *scope_depth >= depth => {
                    Some(*origin)
                }
                Provenance::Input(_)
                | Provenance::StackAddress { .. }
                | Provenance::CapturedInputAddress(_)
                | Provenance::CapturedStackAddress { .. }
                | Provenance::CallableClosure { .. } => None,
            })
            .collect();
        self.report_escaping_state(&escaping, span, EscapeKind::Scope);
    }

    fn filter_for_type(
        &self,
        origins: ValueProvenance,
        ty: nia_ids::InternedTyId,
    ) -> ValueProvenance {
        match self.type_store.get(ty) {
            Some(TyKind::ErrorUnion { error, value }) => ValueProvenance {
                value: self.filter_origins_for_type(origins.value, *value),
                error: self.filter_origins_for_type(origins.error, *error),
            },
            _ => ValueProvenance::from_value(self.filter_origins_for_type(origins.all(), ty)),
        }
    }

    fn filter_origins_for_type(
        &self,
        origins: Provenances,
        ty: nia_ids::InternedTyId,
    ) -> Provenances {
        if self.type_may_carry_borrowed_state(ty, &mut HashSet::new()) {
            origins
        } else {
            Provenances::new()
        }
    }

    fn type_may_carry_borrowed_state(
        &self,
        ty: nia_ids::InternedTyId,
        seen: &mut HashSet<nia_ids::InternedTyId>,
    ) -> bool {
        if !seen.insert(ty) {
            return true;
        }
        match self.type_store.get(ty) {
            Some(
                TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::Slice { .. }
                | TyKind::Callable { .. }
                | TyKind::ClosureState { .. }
                | TyKind::Nominal { .. }
                | TyKind::TraitObject { .. }
                | TyKind::Projection { .. }
                | TyKind::GenericParam(_)
                | TyKind::SelfParam,
            ) => true,
            Some(TyKind::Tuple(elems)) => elems
                .iter()
                .any(|elem| self.type_may_carry_borrowed_state(*elem, seen)),
            Some(TyKind::Array { elem, .. }) | Some(TyKind::Optional { elem }) => {
                self.type_may_carry_borrowed_state(*elem, seen)
            }
            Some(TyKind::ErrorUnion { error, value }) => {
                self.type_may_carry_borrowed_state(*error, seen)
                    || self.type_may_carry_borrowed_state(*value, seen)
            }
            Some(TyKind::Range { bound, .. }) => {
                bound.is_some_and(|bound| self.type_may_carry_borrowed_state(bound, seen))
            }
            Some(
                TyKind::Error
                | TyKind::ConstOnly
                | TyKind::Opaque
                | TyKind::Primitive(_)
                | TyKind::Vector { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::CallablePointee { .. }
                | TyKind::SlicePointee { .. }
                | TyKind::BuiltinType(_)
                | TyKind::BuiltinTrait { .. }
                | TyKind::TraitObjectPointee { .. },
            )
            | None => false,
        }
    }
}

fn input_sources(origins: &Provenances) -> BTreeSet<InputSource> {
    origins
        .iter()
        .filter_map(|origin| match origin {
            Provenance::Input(source) => Some(*source),
            Provenance::StackAddress { .. }
            | Provenance::CapturedInputAddress(_)
            | Provenance::CapturedStackAddress { .. }
            | Provenance::CallableClosure { .. } => None,
        })
        .collect()
}

fn captured_input_sources(origins: &Provenances) -> BTreeSet<InputSource> {
    origins
        .iter()
        .filter_map(|origin| match origin {
            Provenance::CapturedInputAddress(source) => Some(*source),
            Provenance::Input(_)
            | Provenance::StackAddress { .. }
            | Provenance::CapturedStackAddress { .. }
            | Provenance::CallableClosure { .. } => None,
        })
        .collect()
}

fn parameter_sources(sources: BTreeSet<InputSource>) -> BTreeSet<usize> {
    sources
        .into_iter()
        .filter_map(|source| match source {
            InputSource::Parameter(index) => Some(index),
            InputSource::Capture(_) => None,
        })
        .collect()
}

fn capture_address_origins(origins: Provenances) -> Provenances {
    origins
        .into_iter()
        .map(|origin| match origin {
            Provenance::Input(source) => Provenance::CapturedInputAddress(source),
            Provenance::StackAddress { scope_depth } => {
                Provenance::CapturedStackAddress { scope_depth }
            }
            Provenance::CapturedInputAddress(_)
            | Provenance::CapturedStackAddress { .. }
            | Provenance::CallableClosure { .. } => origin,
        })
        .collect()
}

fn address_uses_stack_storage(expr: &TypedExpr) -> bool {
    match &expr.kind {
        TypedExprKind::Local(_) => true,
        TypedExprKind::Field { lhs, .. } | TypedExprKind::TupleField { lhs, .. } => {
            address_uses_stack_storage(lhs)
        }
        TypedExprKind::Index { lhs, .. } => address_uses_stack_storage(lhs),
        TypedExprKind::Unary {
            op: nia_ast::UnaryOp::Deref,
            ..
        } => false,
        TypedExprKind::Global(_)
        | TypedExprKind::Function(_)
        | TypedExprKind::FunctionInstance { .. }
        | TypedExprKind::ClosureFunctionPointer { .. } => false,
        _ => true,
    }
}

fn is_pointer_type(type_store: &TypeStore, ty: nia_ids::InternedTyId) -> bool {
    matches!(
        type_store.get(ty),
        Some(
            TyKind::Pointer { .. }
                | TyKind::VolatilePointer { .. }
                | TyKind::Slice { .. }
                | TyKind::Callable { .. }
                | TyKind::FunctionPointer { .. }
                | TyKind::TraitObject { .. }
        )
    )
}

fn is_integer_type(type_store: &TypeStore, ty: nia_ids::InternedTyId) -> bool {
    matches!(
        type_store.get(ty),
        Some(TyKind::Primitive(primitive)) if primitive.is_integer()
    )
}

fn input_origins(source: InputSource, captures: &Provenances, args: &[Provenances]) -> Provenances {
    match source {
        InputSource::Capture(_) => captures.clone(),
        InputSource::Parameter(index) => args.get(index).cloned().unwrap_or_default(),
    }
}

fn bind_pattern(pattern: &TypedPattern, value: &ValueProvenance, env: &mut Environment) {
    match &pattern.kind {
        TypedPatternKind::Bind { local_id, .. } => {
            env.insert(*local_id, value.clone());
        }
        TypedPatternKind::Pointer(inner)
        | TypedPatternKind::MutPointer(inner)
        | TypedPatternKind::OptionalSome(inner) => {
            bind_pattern(inner, &ValueProvenance::from_value(value.all()), env)
        }
        TypedPatternKind::ErrorOk(inner) => bind_pattern(
            inner,
            &ValueProvenance::from_value(value.value.clone()),
            env,
        ),
        TypedPatternKind::ErrorErr(inner) => bind_pattern(
            inner,
            &ValueProvenance::from_value(value.error.clone()),
            env,
        ),
        TypedPatternKind::Tuple(patterns) => {
            for pattern in patterns {
                bind_pattern(pattern, &ValueProvenance::from_value(value.all()), env);
            }
        }
        TypedPatternKind::EnumVariant { fields, .. } => {
            for pattern in fields {
                bind_pattern(pattern, &ValueProvenance::from_value(value.all()), env);
            }
        }
        TypedPatternKind::Wildcard
        | TypedPatternKind::OptionalNull
        | TypedPatternKind::Expr(_)
        | TypedPatternKind::CheckedInt { .. }
        | TypedPatternKind::Range { .. }
        | TypedPatternKind::CheckedIntRange { .. } => {}
    }
}

fn join_environment(target: &mut Environment, source: &Environment) {
    for (local_id, origins) in source {
        target.entry(*local_id).or_default().extend(origins.clone());
    }
}

fn union(mut lhs: Provenances, rhs: Provenances) -> Provenances {
    lhs.extend(rhs);
    lhs
}

fn collect_body_closures<'a>(
    body: &'a TypedBody,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    for stmt in &body.stmts {
        collect_stmt_closures(stmt, callables);
    }
    if let Some(tail) = &body.tail {
        collect_expr_closures(tail, callables);
    }
}

fn collect_stmt_closures<'a>(
    stmt: &'a nia_body_ir::TypedStmt,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    match &stmt.kind {
        TypedStmtKind::Binding(binding) => {
            if let Some(value) = &binding.value {
                collect_expr_closures(value, callables);
            }
        }
        TypedStmtKind::PatternBinding(binding) => collect_expr_closures(&binding.value, callables),
        TypedStmtKind::Expr(expr)
        | TypedStmtKind::Return(Some(expr))
        | TypedStmtKind::Defer(expr) => collect_expr_closures(expr, callables),
        TypedStmtKind::ForIn(for_in) => {
            collect_expr_closures(&for_in.iter, callables);
            collect_body_closures(&for_in.body, callables);
        }
        TypedStmtKind::While(while_stmt) => {
            collect_expr_closures(&while_stmt.cond, callables);
            collect_body_closures(&while_stmt.body, callables);
        }
        TypedStmtKind::Loop(loop_stmt) => collect_body_closures(&loop_stmt.body, callables),
        TypedStmtKind::Return(None) | TypedStmtKind::Break | TypedStmtKind::Continue => {}
    }
}

fn collect_expr_closures<'a>(
    expr: &'a TypedExpr,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    match &expr.kind {
        TypedExprKind::Closure {
            closure_id,
            captures,
            params,
            body,
        } => {
            callables.insert(
                CallableKey::Closure(*closure_id),
                CallableBody {
                    captures: captures.iter().map(|capture| capture.local_id).collect(),
                    params: params.clone(),
                    body,
                },
            );
            for capture in captures {
                collect_expr_closures(&capture.value, callables);
            }
            collect_body_closures(body, callables);
        }
        TypedExprKind::EnumVariant { fields, .. } | TypedExprKind::Tuple(fields) => {
            for field in fields {
                collect_expr_closures(field, callables);
            }
        }
        TypedExprKind::Range(range) => {
            for bound in range.start.iter().chain(&range.end) {
                collect_expr_closures(bound, callables);
            }
        }
        TypedExprKind::InlineAsm(asm) => {
            for input in &asm.inputs {
                collect_expr_closures(&input.value, callables);
            }
            for output in &asm.outputs {
                collect_place_closures(&output.place, callables);
            }
        }
        TypedExprKind::MemoryIntrinsic(intrinsic) => {
            collect_expr_closures(&intrinsic.dest, callables);
            match &intrinsic.source {
                TypedMemoryIntrinsicSource::Slice(source)
                | TypedMemoryIntrinsicSource::Byte(source) => {
                    collect_expr_closures(source, callables)
                }
            }
        }
        TypedExprKind::Atomic(atomic) => match atomic {
            TypedAtomic::Load { ptr, .. } => collect_expr_closures(ptr, callables),
            TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
                collect_expr_closures(ptr, callables);
                collect_expr_closures(value, callables);
            }
            TypedAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                collect_expr_closures(ptr, callables);
                collect_expr_closures(expected, callables);
                collect_expr_closures(desired, callables);
            }
            TypedAtomic::Fence { .. } => {}
        },
        TypedExprKind::LoadUnaligned { ptr, .. }
        | TypedExprKind::Splat { value: ptr }
        | TypedExprKind::BitIntrinsic { value: ptr, .. }
        | TypedExprKind::CharFromU32 { value: ptr }
        | TypedExprKind::StaticArrayPointer { array: ptr, .. }
        | TypedExprKind::OptionalSome { expr: ptr }
        | TypedExprKind::ErrorOk { expr: ptr }
        | TypedExprKind::ErrorErr { expr: ptr }
        | TypedExprKind::Try { expr: ptr, .. }
        | TypedExprKind::Discard(ptr)
        | TypedExprKind::Cast { expr: ptr, .. }
        | TypedExprKind::TraitObjectUpcast { expr: ptr, .. }
        | TypedExprKind::TraitObjectCoercion { expr: ptr, .. }
        | TypedExprKind::CallableCoercion { state: ptr, .. }
        | TypedExprKind::Unary { expr: ptr, .. }
        | TypedExprKind::Field { lhs: ptr, .. }
        | TypedExprKind::TupleField { lhs: ptr, .. } => collect_expr_closures(ptr, callables),
        TypedExprKind::ExtractElement { vector, index }
        | TypedExprKind::Binary {
            lhs: vector,
            rhs: index,
            ..
        }
        | TypedExprKind::Index { lhs: vector, index } => {
            collect_expr_closures(vector, callables);
            collect_expr_closures(index, callables);
        }
        TypedExprKind::InsertElement {
            vector,
            index,
            value,
        } => {
            collect_expr_closures(vector, callables);
            collect_expr_closures(index, callables);
            collect_expr_closures(value, callables);
        }
        TypedExprKind::Bitmask { vector } => collect_expr_closures(vector, callables),
        TypedExprKind::ArrayLiteral { elems } => match elems {
            TypedArrayElements::List(elems) => {
                for elem in elems {
                    collect_expr_closures(elem, callables);
                }
            }
            TypedArrayElements::Repeat { value, .. } => collect_expr_closures(value, callables),
        },
        TypedExprKind::StructLiteral { fields, .. } => {
            for field in fields {
                collect_expr_closures(&field.value, callables);
            }
        }
        TypedExprKind::UnionLiteral { field, .. } => collect_expr_closures(&field.value, callables),
        TypedExprKind::UnionStorageLiteral { relocations, .. } => {
            for relocation in relocations {
                collect_expr_closures(&relocation.pointee, callables);
            }
        }
        TypedExprKind::Assign { place, rhs, .. } => {
            collect_place_closures(place, callables);
            collect_expr_closures(rhs, callables);
        }
        TypedExprKind::Call { callee, args } => {
            collect_callee_closures(callee, callables);
            for arg in args {
                collect_expr_closures(arg, callables);
            }
        }
        TypedExprKind::Slice { lhs, range, .. } => {
            collect_expr_closures(lhs, callables);
            for bound in range.start.iter().chain(&range.end) {
                collect_expr_closures(bound, callables);
            }
        }
        TypedExprKind::Block(body) => collect_body_closures(body, callables),
        TypedExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_closures(cond, callables);
            collect_body_closures(then_branch, callables);
            if let Some(branch) = else_branch {
                collect_expr_closures(branch, callables);
            }
        }
        TypedExprKind::IfPattern(pattern) => {
            collect_expr_closures(&pattern.target, callables);
            collect_body_closures(&pattern.then_branch, callables);
            if let Some(branch) = &pattern.else_branch {
                collect_expr_closures(branch, callables);
            }
        }
        TypedExprKind::Switch(switch) => {
            collect_expr_closures(&switch.target, callables);
            for arm in &switch.arms {
                match &arm.body {
                    TypedSwitchArmBody::Expr(expr) => collect_expr_closures(expr, callables),
                    TypedSwitchArmBody::Stmt(stmt) => collect_stmt_closures(stmt, callables),
                    TypedSwitchArmBody::Block(body) => collect_body_closures(body, callables),
                }
            }
        }
        TypedExprKind::Error
        | TypedExprKind::Integer(_)
        | TypedExprKind::Float(_)
        | TypedExprKind::String(_)
        | TypedExprKind::ByteString(_)
        | TypedExprKind::Char(_)
        | TypedExprKind::ByteChar(_)
        | TypedExprKind::Bool(_)
        | TypedExprKind::Null
        | TypedExprKind::Local(_)
        | TypedExprKind::Global(_)
        | TypedExprKind::ConstGeneric(_)
        | TypedExprKind::Function(_)
        | TypedExprKind::FunctionInstance { .. }
        | TypedExprKind::BuiltinValue(_)
        | TypedExprKind::Trap
        | TypedExprKind::ClosureFunctionPointer { .. } => {}
    }
}

fn collect_place_closures<'a>(
    place: &'a TypedPlace,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    if let PlaceBase::Deref(expr) = &place.base {
        collect_expr_closures(expr, callables);
    }
    for elem in &place.elems {
        if let PlaceElem::Index(expr) = elem {
            collect_expr_closures(expr, callables);
        }
    }
}

fn collect_callee_closures<'a>(
    callee: &'a TypedCallee,
    callables: &mut HashMap<CallableKey, CallableBody<'a>>,
) {
    match callee {
        TypedCallee::Closure(expr)
        | TypedCallee::Callable(expr)
        | TypedCallee::FunctionPointer(expr) => collect_expr_closures(expr, callables),
        TypedCallee::Method { receiver, .. }
        | TypedCallee::TraitMethod { receiver, .. }
        | TypedCallee::DynamicTraitMethod { receiver, .. }
        | TypedCallee::BuiltinMethod { receiver, .. }
        | TypedCallee::BuiltinPlaceMethod(nia_body_ir::BuiltinPlaceMethod { receiver, .. }) => {
            collect_expr_closures(receiver, callables)
        }
        TypedCallee::Function(_)
        | TypedCallee::FunctionInstance { .. }
        | TypedCallee::TraitAssociatedFunction { .. }
        | TypedCallee::BuiltinOperator(_) => {}
    }
}
