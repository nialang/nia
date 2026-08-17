// SPDX-License-Identifier: GPL-3.0-or-later
//! Interprocedural escape analysis for closures and non-owning callable views.
//!
//! The analysis computes a finite, monotone summary for every source function
//! and nested closure, then replays the bodies with the stable summaries to
//! diagnose stack-backed callable views and closure states that capture local
//! addresses. It deliberately does not model general pointer lifetimes.

use std::collections::{BTreeSet, HashMap, HashSet};

use nia_body_ir::{
    PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBody, TypedCallee, TypedExpr,
    TypedExprKind, TypedMatchArmBody, TypedMemoryIntrinsicSource, TypedPattern, TypedPatternKind,
    TypedPlace, TypedStmtKind,
};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{ClosureId, GlobalDefId, LocalId};
use nia_span::Span;
use nia_ty::{TyKind, TypeStore};

mod discovery;

use discovery::collect_body_closures;

/// Parameter-level escape facts published for a source function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClosureEscapeSummary {
    /// Zero-based parameters whose values may be returned on either channel.
    pub returned_parameters: BTreeSet<usize>,
    /// Zero-based parameters that may be retained by a store or call.
    pub escaping_parameters: BTreeSet<usize>,
    /// Parameters whose addresses may become part of returned closure state.
    pub returned_captured_address_parameters: BTreeSet<usize>,
    /// Parameters whose addresses may enter closure state retained elsewhere.
    pub escaping_captured_address_parameters: BTreeSet<usize>,
}

/// Closure escape summaries and diagnostics for one checked program graph.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureCheck {
    /// Escape summaries keyed by source function identity.
    pub summaries: HashMap<GlobalDefId, ClosureEscapeSummary>,
    /// Invalid lexical escapes found after summary convergence.
    pub diagnostics: Vec<ClosureCheckDiagnostic>,
}

/// A closure-safety diagnostic paired with the function that owns its span.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureCheckDiagnostic {
    /// Function used to map the diagnostic span back to its source module.
    pub owner: GlobalDefId,
    /// The user-facing closure-safety diagnostic.
    pub diagnostic: Diagnostic,
}

/// A typed source function supplied to closure escape analysis.
#[derive(Debug, Clone, Copy)]
pub struct ClosureCheckFunction<'a> {
    /// Stable identity of the source function.
    pub def_id: GlobalDefId,
    /// Checked typed body owned by `def_id`.
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    ClosureCapture {
        closure_id: ClosureId,
        index: usize,
        origin: Box<Provenance>,
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

/// Computes closure escape summaries and reports invalid lexical escapes.
///
/// `functions` must contain the complete source-function set for the program
/// being checked. Nested closures are discovered from those bodies before the
/// summary fixed point begins.
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
    // Summaries form a finite powerset lattice over callable inputs. Replaying
    // every body until no set grows handles recursive and mutually recursive
    // calls without depending on callable discovery or hash iteration order.
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
        // A loop may feed values assigned in one iteration into the next. Join
        // each body result with the immutable entry environment until the set
        // of provenances stabilizes; all transfers only add set members.
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
                ValueProvenance::from_value(
                    captures
                        .iter()
                        .enumerate()
                        .flat_map(|(index, capture)| {
                            closure_capture_origins(
                                *closure_id,
                                index,
                                self.analyze_expr(&capture.value, env).all(),
                            )
                        })
                        .collect(),
                )
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
            TypedExprKind::Match(matched) => {
                let target = self.analyze_expr(&matched.target, env);
                let base = env.clone();
                let mut merged = base.clone();
                let mut value = ValueProvenance::default();
                for arm in &matched.arms {
                    let mut arm_env = base.clone();
                    for pattern in &arm.patterns {
                        bind_pattern(pattern, &target, &mut arm_env);
                    }
                    let arm_value = match &arm.body {
                        TypedMatchArmBody::Expr(expr) => self.analyze_expr(expr, &mut arm_env),
                        TypedMatchArmBody::Stmt(stmt) => {
                            let body = TypedBody {
                                span: stmt.span,
                                locals: Vec::new(),
                                stmts: vec![stmt.as_ref().clone()],
                                tail: None,
                                ty: expr.ty,
                            };
                            self.analyze_nested_body(&body, &mut arm_env)
                        }
                        TypedMatchArmBody::Block(body) => {
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
        // Function lowering evaluates an expression-backed callee or receiver
        // before explicit arguments. Preserve that order here because either
        // side may contain assignments that change later provenance reads.
        match callee {
            TypedCallee::Function(def_id) | TypedCallee::FunctionInstance { def_id, .. } => {
                let args = self.analyze_call_args(args, env);
                self.apply_summary(
                    CallableKey::Function(*def_id),
                    &Provenances::new(),
                    &args,
                    call.span,
                    call.ty,
                )
            }
            TypedCallee::Method {
                def_id, receiver, ..
            } => {
                let receiver_origins = self.analyze_expr(receiver, env).all();
                let args = self.analyze_call_args(args, env);
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
                let args = self.analyze_call_args(args, env);
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
            TypedCallee::TraitAssociatedFunction { method_id, .. } => {
                let args = self.analyze_call_args(args, env);
                self.apply_summary(
                    CallableKey::Function(*method_id),
                    &Provenances::new(),
                    &args,
                    call.span,
                    call.ty,
                )
            }
            TypedCallee::Closure(state) => {
                let state_origins = self.analyze_expr(state, env).all();
                let args = self.analyze_call_args(args, env);
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
                let args = self.analyze_call_args(args, env);
                let closure_ids = callee
                    .iter()
                    .filter_map(|origin| match origin {
                        Provenance::CallableClosure { closure_id, .. } => Some(*closure_id),
                        Provenance::Input(_)
                        | Provenance::StackAddress { .. }
                        | Provenance::CapturedInputAddress(_)
                        | Provenance::CapturedStackAddress { .. }
                        | Provenance::ClosureCapture { .. } => None,
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
                let args = self.analyze_call_args(args, env);
                self.apply_unknown_call(&args, call.span, call.ty)
            }
            TypedCallee::DynamicTraitMethod { receiver, .. } => {
                let receiver = self.analyze_expr(receiver, env).all();
                let args = self.analyze_call_args(args, env);
                let mut operands = vec![receiver];
                operands.extend(args);
                self.apply_unknown_call(&operands, call.span, call.ty)
            }
            TypedCallee::BuiltinMethod { receiver, .. }
            | TypedCallee::BuiltinPlaceMethod(nia_body_ir::BuiltinPlaceMethod {
                receiver, ..
            }) => {
                let mut result = self.analyze_expr(receiver, env).all();
                let args = self.analyze_call_args(args, env);
                for arg in args {
                    result.extend(arg);
                }
                ValueProvenance::from_value(result)
            }
            TypedCallee::BuiltinOperator(_) => {
                let args = self.analyze_call_args(args, env);
                ValueProvenance::from_value(args.into_iter().fold(Provenances::new(), union))
            }
        }
    }

    fn analyze_call_args(&mut self, args: &[TypedExpr], env: &mut Environment) -> Vec<Provenances> {
        args.iter()
            .map(|arg| self.analyze_expr(arg, env).all())
            .collect()
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
            result.extend(input_origins(key, *source, captures, args));
        }
        let mut error = Provenances::new();
        for source in &summary.returned_error_inputs {
            error.extend(input_origins(key, *source, captures, args));
        }
        for source in &summary.escaping_inputs {
            self.record_escape(
                &input_origins(key, *source, captures, args),
                span,
                EscapeKind::Call,
            );
        }
        for source in &summary.returned_captured_addresses {
            result.extend(capture_address_origins(input_origins(
                key, *source, captures, args,
            )));
        }
        for source in &summary.returned_error_captured_addresses {
            error.extend(capture_address_origins(input_origins(
                key, *source, captures, args,
            )));
        }
        for source in &summary.escaping_captured_addresses {
            self.record_escape(
                &capture_address_origins(input_origins(key, *source, captures, args)),
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
        self.returned.extend(value.value.iter().cloned());
        self.returned_errors.extend(value.error.iter().cloned());
        self.report_escaping_state(&value.all(), span, EscapeKind::Return);
    }

    fn record_error_return(&mut self, error: &Provenances, span: Span) {
        self.returned_errors.extend(error.iter().cloned());
        self.report_escaping_state(error, span, EscapeKind::Return);
    }

    fn record_escape(&mut self, value: &Provenances, span: Span, kind: EscapeKind) {
        self.escaped.extend(value.iter().cloned());
        self.report_escaping_state(value, span, kind);
    }

    fn report_escaping_state(&mut self, value: &Provenances, span: Span, kind: EscapeKind) {
        let Some(sink) = &mut self.diagnostics else {
            return;
        };
        let stack_closure = value.iter().any(contains_stack_backed_callable);
        let captured_stack_address = value.iter().any(contains_captured_stack_address);
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
            .filter(|origin| provenance_expires_at(origin, &self.closure_scopes, depth))
            .cloned()
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
            Provenance::ClosureCapture { origin, .. } => input_source(origin),
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
            Provenance::ClosureCapture { origin, .. } => captured_input_source(origin),
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
            | Provenance::CallableClosure { .. }
            | Provenance::ClosureCapture { .. } => origin,
        })
        .collect()
}

fn closure_capture_origins(
    closure_id: ClosureId,
    index: usize,
    origins: Provenances,
) -> Provenances {
    // Capture slots are part of closure identity. Keeping the slot on each
    // origin lets a summary for `Capture(0)` select only that capture instead
    // of conservatively treating every field in the state as interchangeable.
    capture_address_origins(origins)
        .into_iter()
        .map(|origin| Provenance::ClosureCapture {
            closure_id,
            index,
            origin: Box::new(origin),
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

fn input_origins(
    key: CallableKey,
    source: InputSource,
    captures: &Provenances,
    args: &[Provenances],
) -> Provenances {
    match source {
        InputSource::Capture(index) => {
            let CallableKey::Closure(closure_id) = key else {
                return captures.clone();
            };
            let has_slots = captures.iter().any(|origin| {
                matches!(
                    origin,
                    Provenance::ClosureCapture {
                        closure_id: candidate,
                        ..
                    } if *candidate == closure_id
                )
            });
            if !has_slots {
                // Summaries crossing a function boundary currently publish
                // flattened parameter facts. Retain the conservative fallback
                // until those products carry closure-state field provenance.
                return captures.clone();
            }
            captures
                .iter()
                .filter_map(|origin| match origin {
                    Provenance::ClosureCapture {
                        closure_id: candidate,
                        index: candidate_index,
                        origin,
                    } if *candidate == closure_id && *candidate_index == index => {
                        Some(origin.as_ref().clone())
                    }
                    _ => None,
                })
                .collect()
        }
        InputSource::Parameter(index) => args.get(index).cloned().unwrap_or_default(),
    }
}

fn input_source(origin: &Provenance) -> Option<InputSource> {
    match origin {
        Provenance::Input(source) => Some(*source),
        Provenance::ClosureCapture { origin, .. } => input_source(origin),
        Provenance::StackAddress { .. }
        | Provenance::CapturedInputAddress(_)
        | Provenance::CapturedStackAddress { .. }
        | Provenance::CallableClosure { .. } => None,
    }
}

fn captured_input_source(origin: &Provenance) -> Option<InputSource> {
    match origin {
        Provenance::CapturedInputAddress(source) => Some(*source),
        Provenance::ClosureCapture { origin, .. } => captured_input_source(origin),
        Provenance::Input(_)
        | Provenance::StackAddress { .. }
        | Provenance::CapturedStackAddress { .. }
        | Provenance::CallableClosure { .. } => None,
    }
}

fn contains_stack_backed_callable(origin: &Provenance) -> bool {
    match origin {
        Provenance::CallableClosure {
            stack_backed: true, ..
        } => true,
        Provenance::ClosureCapture { origin, .. } => contains_stack_backed_callable(origin),
        _ => false,
    }
}

fn contains_captured_stack_address(origin: &Provenance) -> bool {
    match origin {
        Provenance::CapturedStackAddress { .. } => true,
        Provenance::ClosureCapture { origin, .. } => contains_captured_stack_address(origin),
        _ => false,
    }
}

fn provenance_expires_at(
    origin: &Provenance,
    closure_scopes: &HashMap<ClosureId, usize>,
    depth: usize,
) -> bool {
    match origin {
        Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        } => closure_scopes
            .get(closure_id)
            .is_some_and(|closure_depth| *closure_depth >= depth),
        Provenance::CapturedStackAddress { scope_depth } => *scope_depth >= depth,
        Provenance::ClosureCapture { origin, .. } => {
            provenance_expires_at(origin, closure_scopes, depth)
        }
        _ => false,
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
        TypedPatternKind::Nominal { fields, .. } => {
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

#[cfg(test)]
mod tests {
    use nia_ids::{DefId, ModuleIdAllocator};
    use nia_ty::TyKind;

    use super::*;

    fn closure_id() -> ClosureId {
        let module_id = ModuleIdAllocator::new().allocate();
        ClosureId {
            owner: GlobalDefId {
                module_id,
                def_id: DefId(1),
            },
            ordinal: 0,
        }
    }

    #[test]
    fn known_closure_capture_lookup_selects_only_the_requested_slot() {
        let closure_id = closure_id();
        let selected = Provenance::CapturedInputAddress(InputSource::Parameter(0));
        let ignored = Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        };
        let captures = Provenances::from([
            Provenance::ClosureCapture {
                closure_id,
                index: 0,
                origin: Box::new(selected.clone()),
            },
            Provenance::ClosureCapture {
                closure_id,
                index: 1,
                origin: Box::new(ignored),
            },
        ]);

        assert_eq!(
            input_origins(
                CallableKey::Closure(closure_id),
                InputSource::Capture(0),
                &captures,
                &[],
            ),
            Provenances::from([selected]),
        );
    }

    #[test]
    fn flattened_closure_capture_lookup_remains_conservative() {
        let closure_id = closure_id();
        let captures = Provenances::from([
            Provenance::CapturedInputAddress(InputSource::Parameter(0)),
            Provenance::CapturedInputAddress(InputSource::Parameter(1)),
        ]);

        assert_eq!(
            input_origins(
                CallableKey::Closure(closure_id),
                InputSource::Capture(0),
                &captures,
                &[],
            ),
            captures,
        );
    }

    #[test]
    fn expression_callee_mutations_precede_argument_provenance_reads() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let closure_id = ClosureId { owner, ordinal: 0 };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let callable_ty = append.intern(TyKind::Callable {
            is_readonly: true,
            params: Vec::new(),
            return_type: append.intern(TyKind::Tuple(Vec::new())),
        });
        let unit_ty = append.intern(TyKind::Tuple(Vec::new()));
        let selected = LocalId(0);
        let stack_backed = LocalId(1);
        let mut env = Environment::from([
            (
                selected,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::Parameter(0),
                )])),
            ),
            (
                stack_backed,
                ValueProvenance::from_value(Provenances::from([Provenance::CallableClosure {
                    closure_id,
                    stack_backed: true,
                }])),
            ),
        ]);
        let local = |local_id| TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Local(local_id),
        };
        let callee = TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Assign {
                place: TypedPlace {
                    span: Span::default(),
                    ty: callable_ty,
                    base: PlaceBase::Local(selected),
                    elems: Vec::new(),
                },
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(local(stack_backed)),
            },
        };
        let call = TypedExpr {
            span: Span::default(),
            ty: unit_ty,
            kind: TypedExprKind::Error,
        };
        let summaries = HashMap::new();
        let mut analyzer = Analyzer::new(&types, &summaries, None);

        analyzer.analyze_call(
            &TypedCallee::FunctionPointer(Box::new(callee)),
            &[local(selected)],
            &call,
            &mut env,
        );

        assert!(analyzer.escaped.contains(&Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        }));
    }
}
