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
    StackClosure(ClosureId),
}

type Provenances = BTreeSet<Provenance>;
type Environment = HashMap<LocalId, Provenances>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CallableSummary {
    returned_inputs: BTreeSet<InputSource>,
    escaping_inputs: BTreeSet<InputSource>,
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
                Provenances::from([Provenance::Input(InputSource::Capture(index))]),
            );
        }
        for (index, local_id) in callable.params.iter().copied().enumerate() {
            env.insert(
                local_id,
                Provenances::from([Provenance::Input(InputSource::Parameter(index))]),
            );
        }
        let tail = self.analyze_body_contents(callable.body, &mut env);
        self.record_return(&tail, callable.body.span);
        CallableSummary {
            returned_inputs: input_sources(&self.returned),
            escaping_inputs: input_sources(&self.escaped),
        }
    }

    fn analyze_body_contents(&mut self, body: &TypedBody, env: &mut Environment) -> Provenances {
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

    fn analyze_nested_body(&mut self, body: &TypedBody, env: &mut Environment) -> Provenances {
        self.scope_depth = self.scope_depth.saturating_add(1);
        let depth = self.scope_depth;
        let value = self.analyze_body_contents(body, env);
        let locals = body
            .locals
            .iter()
            .map(|local| local.id)
            .collect::<HashSet<_>>();
        let mut crossing = value.clone();
        for (local_id, origins) in env.iter() {
            if !locals.contains(local_id) {
                crossing.extend(origins);
            }
        }
        self.report_scope_exit(&crossing, body.span, depth);
        env.retain(|local_id, _| !locals.contains(local_id));
        self.scope_depth = self.scope_depth.saturating_sub(1);
        value
    }

    fn analyze_expr(&mut self, expr: &TypedExpr, env: &mut Environment) -> Provenances {
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
            | TypedExprKind::ClosureFunctionPointer { .. } => Provenances::new(),
            TypedExprKind::Local(local_id) => env.get(local_id).cloned().unwrap_or_default(),
            TypedExprKind::EnumVariant { fields, .. } | TypedExprKind::Tuple(fields) => {
                self.analyze_exprs(fields, env)
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
                captures
                    .iter()
                    .map(|capture| self.analyze_expr(&capture.value, env))
                    .fold(Provenances::new(), union)
            }
            TypedExprKind::Range(range) => range
                .start
                .iter()
                .chain(&range.end)
                .map(|bound| self.analyze_expr(bound, env))
                .fold(Provenances::new(), union),
            TypedExprKind::InlineAsm(asm) => {
                let values = asm
                    .inputs
                    .iter()
                    .map(|input| self.analyze_expr(&input.value, env))
                    .fold(Provenances::new(), union);
                self.record_escape(&values, expr.span, EscapeKind::Call);
                for output in &asm.outputs {
                    self.analyze_place(&output.place, env);
                }
                Provenances::new()
            }
            TypedExprKind::MemoryIntrinsic(intrinsic) => {
                let dest = self.analyze_expr(&intrinsic.dest, env);
                let source = match &intrinsic.source {
                    TypedMemoryIntrinsicSource::Slice(source)
                    | TypedMemoryIntrinsicSource::Byte(source) => self.analyze_expr(source, env),
                };
                self.record_escape(&source, expr.span, EscapeKind::Store);
                union(dest, source)
            }
            TypedExprKind::Atomic(atomic) => self.analyze_atomic(atomic, expr.span, env),
            TypedExprKind::LoadUnaligned { ptr, .. }
            | TypedExprKind::Splat { value: ptr }
            | TypedExprKind::BitIntrinsic { value: ptr, .. }
            | TypedExprKind::CharFromU32 { value: ptr }
            | TypedExprKind::StaticArrayPointer { array: ptr, .. }
            | TypedExprKind::OptionalSome { expr: ptr }
            | TypedExprKind::ErrorOk { expr: ptr }
            | TypedExprKind::ErrorErr { expr: ptr }
            | TypedExprKind::Discard(ptr)
            | TypedExprKind::Cast { expr: ptr, .. }
            | TypedExprKind::TraitObjectUpcast { expr: ptr, .. }
            | TypedExprKind::TraitObjectCoercion { expr: ptr, .. }
            | TypedExprKind::Unary { expr: ptr, .. } => self.analyze_expr(ptr, env),
            TypedExprKind::ExtractElement { vector, index } => union(
                self.analyze_expr(vector, env),
                self.analyze_expr(index, env),
            ),
            TypedExprKind::InsertElement {
                vector,
                index,
                value,
            } => self.analyze_exprs([vector.as_ref(), index.as_ref(), value.as_ref()], env),
            TypedExprKind::Bitmask { vector } => self.analyze_expr(vector, env),
            TypedExprKind::ArrayLiteral { elems } => match elems {
                TypedArrayElements::List(elems) => self.analyze_exprs(elems, env),
                TypedArrayElements::Repeat { value, .. } => self.analyze_expr(value, env),
            },
            TypedExprKind::StructLiteral { fields, .. } => fields
                .iter()
                .map(|field| self.analyze_expr(&field.value, env))
                .fold(Provenances::new(), union),
            TypedExprKind::UnionLiteral { field, .. } => self.analyze_expr(&field.value, env),
            TypedExprKind::UnionStorageLiteral { relocations, .. } => relocations
                .iter()
                .map(|relocation| self.analyze_expr(&relocation.pointee, env))
                .fold(Provenances::new(), union),
            TypedExprKind::Try { expr: inner, .. } => {
                let value = self.analyze_expr(inner, env);
                self.record_return(&value, expr.span);
                value
            }
            TypedExprKind::Binary { lhs, rhs, .. } | TypedExprKind::Index { lhs, index: rhs } => {
                union(self.analyze_expr(lhs, env), self.analyze_expr(rhs, env))
            }
            TypedExprKind::Assign { place, rhs, .. } => {
                let value = self.analyze_expr(rhs, env);
                self.assign_place(place, &value, env, expr.span);
                value
            }
            TypedExprKind::CallableCoercion { state, closure_id } => {
                let mut value = self.analyze_expr(state, env);
                value.insert(Provenance::StackClosure(*closure_id));
                value
            }
            TypedExprKind::Call { callee, args } => self.analyze_call(callee, args, expr, env),
            TypedExprKind::Field { lhs, .. } | TypedExprKind::TupleField { lhs, .. } => {
                self.analyze_expr(lhs, env)
            }
            TypedExprKind::Slice { lhs, range, .. } => {
                let mut value = self.analyze_expr(lhs, env);
                if let Some(start) = &range.start {
                    value.extend(self.analyze_expr(start, env));
                }
                if let Some(end) = &range.end {
                    value.extend(self.analyze_expr(end, env));
                }
                value
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
                union(then_value, else_value)
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
                union(then_value, else_value)
            }
            TypedExprKind::Switch(switch) => {
                let target = self.analyze_expr(&switch.target, env);
                let base = env.clone();
                let mut merged = base.clone();
                let mut value = Provenances::new();
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
            .map(|expr| self.analyze_expr(expr, env))
            .fold(Provenances::new(), union)
    }

    fn analyze_atomic(
        &mut self,
        atomic: &TypedAtomic,
        span: Span,
        env: &mut Environment,
    ) -> Provenances {
        match atomic {
            TypedAtomic::Load { ptr, .. } => self.analyze_expr(ptr, env),
            TypedAtomic::Store { ptr, value, .. } | TypedAtomic::Rmw { ptr, value, .. } => {
                let ptr = self.analyze_expr(ptr, env);
                let value = self.analyze_expr(value, env);
                self.record_escape(&value, span, EscapeKind::Store);
                union(ptr, value)
            }
            TypedAtomic::Cmpxchg {
                ptr,
                expected,
                desired,
                ..
            } => {
                let ptr = self.analyze_expr(ptr, env);
                let expected = self.analyze_expr(expected, env);
                let desired = self.analyze_expr(desired, env);
                self.record_escape(&desired, span, EscapeKind::Store);
                union(union(ptr, expected), desired)
            }
            TypedAtomic::Fence { .. } => Provenances::new(),
        }
    }

    fn analyze_call(
        &mut self,
        callee: &TypedCallee,
        args: &[TypedExpr],
        call: &TypedExpr,
        env: &mut Environment,
    ) -> Provenances {
        let args = args
            .iter()
            .map(|arg| self.analyze_expr(arg, env))
            .collect::<Vec<_>>();
        match callee {
            TypedCallee::Function(def_id) | TypedCallee::FunctionInstance { def_id, .. } => self
                .apply_summary(
                    CallableKey::Function(*def_id),
                    &Provenances::new(),
                    &args,
                    call.span,
                ),
            TypedCallee::Method {
                def_id, receiver, ..
            } => {
                let receiver = self.analyze_expr(receiver, env);
                let mut operands = vec![receiver];
                operands.extend(args);
                self.apply_summary(
                    CallableKey::Function(*def_id),
                    &Provenances::new(),
                    &operands,
                    call.span,
                )
            }
            TypedCallee::TraitMethod {
                method_id,
                receiver,
                ..
            } => {
                let receiver = self.analyze_expr(receiver, env);
                let mut operands = vec![receiver];
                operands.extend(args);
                self.apply_summary(
                    CallableKey::Function(*method_id),
                    &Provenances::new(),
                    &operands,
                    call.span,
                )
            }
            TypedCallee::TraitAssociatedFunction { method_id, .. } => self.apply_summary(
                CallableKey::Function(*method_id),
                &Provenances::new(),
                &args,
                call.span,
            ),
            TypedCallee::Closure(state) => {
                let state_origins = self.analyze_expr(state, env);
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
                    ),
                    None => self.apply_unknown_call(&args, call.span),
                }
            }
            TypedCallee::Callable(callee) => {
                let callee = self.analyze_expr(callee, env);
                let closure_ids = callee
                    .iter()
                    .filter_map(|origin| match origin {
                        Provenance::StackClosure(closure_id) => Some(*closure_id),
                        Provenance::Input(_) => None,
                    })
                    .collect::<BTreeSet<_>>();
                let mut result = Provenances::new();
                for closure_id in &closure_ids {
                    let mut captures = callee.clone();
                    captures.remove(&Provenance::StackClosure(*closure_id));
                    result.extend(self.apply_summary(
                        CallableKey::Closure(*closure_id),
                        &captures,
                        &args,
                        call.span,
                    ));
                }
                if closure_ids.is_empty()
                    || callee
                        .iter()
                        .any(|origin| matches!(origin, Provenance::Input(_)))
                {
                    result.extend(self.apply_unknown_call(&args, call.span));
                }
                result
            }
            TypedCallee::FunctionPointer(callee) => {
                self.analyze_expr(callee, env);
                self.apply_unknown_call(&args, call.span)
            }
            TypedCallee::DynamicTraitMethod { receiver, .. } => {
                let receiver = self.analyze_expr(receiver, env);
                let mut operands = vec![receiver];
                operands.extend(args);
                self.apply_unknown_call(&operands, call.span)
            }
            TypedCallee::BuiltinMethod { receiver, .. }
            | TypedCallee::BuiltinPlaceMethod(nia_body_ir::BuiltinPlaceMethod {
                receiver, ..
            }) => {
                let mut result = self.analyze_expr(receiver, env);
                for arg in args {
                    result.extend(arg);
                }
                result
            }
            TypedCallee::BuiltinOperator(_) => args.into_iter().fold(Provenances::new(), union),
        }
    }

    fn apply_summary(
        &mut self,
        key: CallableKey,
        captures: &Provenances,
        args: &[Provenances],
        span: Span,
    ) -> Provenances {
        let Some(summary) = self.summaries.get(&key) else {
            return self.apply_unknown_call(args, span);
        };
        let mut result = Provenances::new();
        for source in &summary.returned_inputs {
            result.extend(input_origins(*source, captures, args));
        }
        for source in &summary.escaping_inputs {
            self.record_escape(
                &input_origins(*source, captures, args),
                span,
                EscapeKind::Call,
            );
        }
        result
    }

    fn apply_unknown_call(&mut self, args: &[Provenances], span: Span) -> Provenances {
        let result = args.iter().cloned().fold(Provenances::new(), union);
        self.record_escape(&result, span, EscapeKind::Call);
        result
    }

    fn analyze_place(&mut self, place: &TypedPlace, env: &mut Environment) -> Provenances {
        let mut value = match &place.base {
            PlaceBase::Local(local_id) => env.get(local_id).cloned().unwrap_or_default(),
            PlaceBase::Global(_) | PlaceBase::Error => Provenances::new(),
            PlaceBase::Deref(expr) => self.analyze_expr(expr, env),
        };
        for elem in &place.elems {
            if let PlaceElem::Index(index) = elem {
                value.extend(self.analyze_expr(index, env));
            }
        }
        value
    }

    fn assign_place(
        &mut self,
        place: &TypedPlace,
        value: &Provenances,
        env: &mut Environment,
        span: Span,
    ) {
        self.analyze_place(place, env);
        match &place.base {
            PlaceBase::Local(local_id) if place.elems.is_empty() => {
                env.insert(*local_id, value.clone());
            }
            PlaceBase::Local(local_id) => {
                env.entry(*local_id).or_default().extend(value);
            }
            PlaceBase::Global(_) | PlaceBase::Deref(_) => {
                self.record_escape(value, span, EscapeKind::Store);
            }
            PlaceBase::Error => {}
        }
    }

    fn record_return(&mut self, value: &Provenances, span: Span) {
        self.returned.extend(value);
        self.report_stack_closures(value, span, EscapeKind::Return);
    }

    fn record_escape(&mut self, value: &Provenances, span: Span, kind: EscapeKind) {
        self.escaped.extend(value);
        self.report_stack_closures(value, span, kind);
    }

    fn report_stack_closures(&mut self, value: &Provenances, span: Span, kind: EscapeKind) {
        let Some(sink) = &mut self.diagnostics else {
            return;
        };
        if value
            .iter()
            .any(|origin| matches!(origin, Provenance::StackClosure(_)))
            && sink.reported.insert((sink.owner, span, kind))
        {
            let context = match kind {
                EscapeKind::Return => "returned",
                EscapeKind::Store => "stored outside its local frame",
                EscapeKind::Call => "passed to a call that may retain it",
                EscapeKind::Scope => "moved beyond its closure state's lexical scope",
            };
            sink.diagnostics.push(ClosureCheckDiagnostic {
                owner: sink.owner,
                diagnostic: Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "stack-backed callable view cannot be {context}; use it only while its closure state is live"
                    ),
                ),
            });
        }
    }

    fn report_scope_exit(&mut self, value: &Provenances, span: Span, depth: usize) {
        let escaping = value
            .iter()
            .filter_map(|origin| match origin {
                Provenance::StackClosure(closure_id)
                    if self
                        .closure_scopes
                        .get(closure_id)
                        .is_some_and(|closure_depth| *closure_depth >= depth) =>
                {
                    Some(*origin)
                }
                Provenance::Input(_) | Provenance::StackClosure(_) => None,
            })
            .collect();
        self.report_stack_closures(&escaping, span, EscapeKind::Scope);
    }

    fn filter_for_type(&self, origins: Provenances, ty: nia_ids::InternedTyId) -> Provenances {
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
            Provenance::StackClosure(_) => None,
        })
        .collect()
}

fn input_origins(source: InputSource, captures: &Provenances, args: &[Provenances]) -> Provenances {
    match source {
        InputSource::Capture(_) => captures.clone(),
        InputSource::Parameter(index) => args.get(index).cloned().unwrap_or_default(),
    }
}

fn bind_pattern(pattern: &TypedPattern, value: &Provenances, env: &mut Environment) {
    match &pattern.kind {
        TypedPatternKind::Bind { local_id, .. } => {
            env.insert(*local_id, value.clone());
        }
        TypedPatternKind::Pointer(inner)
        | TypedPatternKind::MutPointer(inner)
        | TypedPatternKind::OptionalSome(inner)
        | TypedPatternKind::ErrorOk(inner)
        | TypedPatternKind::ErrorErr(inner) => bind_pattern(inner, value, env),
        TypedPatternKind::Tuple(patterns) => {
            for pattern in patterns {
                bind_pattern(pattern, value, env);
            }
        }
        TypedPatternKind::EnumVariant { fields, .. } => {
            for pattern in fields {
                bind_pattern(pattern, value, env);
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
        target.entry(*local_id).or_default().extend(origins);
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
