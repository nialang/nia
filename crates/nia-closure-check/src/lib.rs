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
    TypedExprKind, TypedMatchArmBody, TypedMemoryIntrinsicSource, TypedNominalPatternConstructor,
    TypedPattern, TypedPatternKind, TypedPlace, TypedStmtKind,
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
enum InputRoot {
    Capture(usize),
    Parameter(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct InputSource {
    root: InputRoot,
    projections: Vec<AggregateProjection>,
    imprecise: bool,
}

impl InputSource {
    fn capture(index: usize) -> Self {
        Self {
            root: InputRoot::Capture(index),
            projections: Vec::new(),
            imprecise: false,
        }
    }

    fn parameter(index: usize) -> Self {
        Self {
            root: InputRoot::Parameter(index),
            projections: Vec::new(),
            imprecise: false,
        }
    }

    fn projected(&self, projection: AggregateProjection) -> Self {
        if self.imprecise {
            return self.clone();
        }
        let mut projected = self.clone();
        if projected.projections.len() == MAX_PROJECTION_DEPTH {
            projected.projections.clear();
            projected.imprecise = true;
        } else {
            projected.projections.push(projection);
        }
        projected
    }
}

// Recursive aggregate types make access paths theoretically unbounded. Keep
// common paths precise and widen deeper paths to one conservative top value so
// interprocedural summary iteration always reaches a finite fixed point.
const MAX_PROJECTION_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum AggregateProjection {
    Field(GlobalDefId),
    TupleField(usize),
    Element,
}

/// Monotone origin attached to a value or error channel during escape analysis.
///
/// The variants distinguish borrowed stack addresses from callable values and
/// preserve the input slot that introduced each origin. Keeping these facts
/// separate lets diagnostics reject only lexical escapes while still allowing
/// ordinary scalar values to flow through the same expressions.
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
    Aggregate {
        projection: AggregateProjection,
        origin: Box<Provenance>,
    },
    OpaqueAggregate {
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

/// Summary transfer facts for one callable over its captures and parameters.
///
/// Every field uses the bounded access-path domain above. Summary iteration
/// therefore converges even for recursive and mutually recursive call graphs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CallableSummary {
    returned_inputs: Provenances,
    returned_error_inputs: Provenances,
    escaping_inputs: Provenances,
    returned_captured_addresses: Provenances,
    returned_error_captured_addresses: Provenances,
    escaping_captured_addresses: Provenances,
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
/// `functions` contains the diagnostic roots for the program being checked.
/// Nested closures are discovered from those bodies before the summary fixed
/// point begins. Use [`check_closure_safety_with_support`] when calls from
/// those roots can target bodies outside the diagnostic set.
pub fn check_closure_safety(
    functions: &[ClosureCheckFunction<'_>],
    type_store: &TypeStore,
) -> ClosureCheck {
    check_closure_safety_with_support(functions, &[], type_store)
}

/// Checks closure safety for `functions`, using `support_functions` to resolve
/// interprocedural summaries without making those support bodies diagnostic
/// roots. Executable checking commonly supplies only reachable bodies, while a
/// generic helper (for example an option mapper) may be defined in an imported
/// module outside that executable subgraph. Treating that helper as unknown
/// would incorrectly report every stack-backed callback as escaping.
pub fn check_closure_safety_with_support(
    functions: &[ClosureCheckFunction<'_>],
    support_functions: &[ClosureCheckFunction<'_>],
    type_store: &TypeStore,
) -> ClosureCheck {
    let mut callables = HashMap::new();
    for function in support_functions.iter().chain(functions) {
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

    let diagnostic_owners = functions
        .iter()
        .map(|function| function.def_id)
        .collect::<HashSet<_>>();

    let mut summaries = callables
        .keys()
        .copied()
        .map(|key| (key, CallableSummary::default()))
        .collect::<HashMap<_, _>>();
    // Summaries form a finite domain over callable inputs and widened aggregate
    // paths. Replaying every body to stability handles recursive and mutually
    // recursive calls without depending on discovery or hash iteration order.
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
        let diagnostic_root = match key {
            CallableKey::Function(def_id) => diagnostic_owners.contains(&def_id),
            CallableKey::Closure(closure_id) => diagnostic_owners.contains(&closure_id.owner),
        };
        if !diagnostic_root {
            continue;
        }
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

    ClosureCheck {
        summaries: summaries
            .into_iter()
            .filter_map(|(key, summary)| match key {
                CallableKey::Function(def_id) if diagnostic_owners.contains(&def_id) => {
                    Some((def_id, summary))
                }
                _ => None,
            })
            .map(|(def_id, summary)| {
                (
                    def_id,
                    ClosureEscapeSummary {
                        returned_parameters: summary
                            .returned_inputs
                            .into_iter()
                            .chain(summary.returned_error_inputs)
                            .filter_map(|origin| input_source(&origin))
                            .filter_map(parameter_source)
                            .collect(),
                        escaping_parameters: summary
                            .escaping_inputs
                            .into_iter()
                            .filter_map(|origin| input_source(&origin))
                            .filter_map(parameter_source)
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
                )
            })
            .collect(),
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
    defer_scopes: Vec<Vec<TypedExpr>>,
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
            defer_scopes: Vec::new(),
        }
    }

    fn summarize(mut self, callable: &CallableBody<'_>) -> CallableSummary {
        // Captures and parameters enter as distinct input origins. The body
        // walk then propagates those origins through assignments, calls,
        // closures, and defers without needing a path-sensitive heap model.
        let mut env = Environment::new();
        for (index, local_id) in callable.captures.iter().copied().enumerate() {
            env.insert(
                local_id,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::capture(index),
                )])),
            );
        }
        for (index, local_id) in callable.params.iter().copied().enumerate() {
            env.insert(
                local_id,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::parameter(index),
                )])),
            );
        }
        let tail = self.analyze_body_contents(callable.body, &mut env);
        self.record_return(&tail, callable.body.span);
        CallableSummary {
            returned_inputs: input_provenances(&self.returned, false),
            returned_error_inputs: input_provenances(&self.returned_errors, false),
            escaping_inputs: input_provenances(&self.escaped, false),
            returned_captured_addresses: input_provenances(&self.returned, true),
            returned_error_captured_addresses: input_provenances(&self.returned_errors, true),
            escaping_captured_addresses: input_provenances(&self.escaped, true),
        }
    }

    fn analyze_body_contents(
        &mut self,
        body: &TypedBody,
        env: &mut Environment,
    ) -> ValueProvenance {
        self.defer_scopes.push(Vec::new());
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
                    self.analyze_pattern(&binding.pattern, env);
                    let value = self.analyze_expr(&binding.value, env);
                    bind_pattern(&binding.pattern, &value, env);
                }
                TypedStmtKind::Expr(expr) => {
                    self.analyze_expr(expr, env);
                }
                TypedStmtKind::Defer(expr) => self
                    .defer_scopes
                    .last_mut()
                    .expect("body analysis must retain its defer scope")
                    .push(expr.clone()),
                TypedStmtKind::Return(value) => {
                    let value = value
                        .as_ref()
                        .map(|value| self.analyze_expr(value, env))
                        .unwrap_or_default();
                    self.analyze_active_defers(env);
                    self.record_return(&value, stmt.span);
                }
                TypedStmtKind::ForIn(for_in) => {
                    let value = self.analyze_expr(&for_in.iter, env);
                    let mut loop_env = env.clone();
                    self.analyze_pattern(&for_in.pattern, &mut loop_env);
                    bind_pattern(&for_in.pattern, &value, &mut loop_env);
                    self.analyze_loop(&for_in.body, env, loop_env);
                }
                TypedStmtKind::While(while_stmt) => {
                    self.analyze_while_loop(&while_stmt.cond, &while_stmt.body, env);
                }
                TypedStmtKind::Loop(loop_stmt) => {
                    self.analyze_loop(&loop_stmt.body, env, env.clone());
                }
                TypedStmtKind::Break | TypedStmtKind::Continue => {}
            }
        }
        // Runtime lowering registers defers in source order and executes them
        // in reverse order after the scope's result has been evaluated. Analyze
        // the same delayed transfer against the exit environment so later
        // assignments are visible to deferred calls and stores.
        let tail = body
            .tail
            .as_deref()
            .map(|tail| self.analyze_expr(tail, env))
            .unwrap_or_default();
        let deferred = self
            .defer_scopes
            .pop()
            .expect("body analysis must pop its defer scope");
        for deferred in deferred.into_iter().rev() {
            self.analyze_expr(&deferred, env);
        }
        tail
    }

    fn analyze_active_defers(&mut self, env: &Environment) {
        // A return or error propagation evaluates its payload first, then
        // unwinds every registered lexical defer from inner to outer. Analyze
        // against a clone because the terminated path must not mutate the
        // fallthrough environment that this path-insensitive pass also joins.
        // Remove the scopes during replay so a `return` nested inside a defer
        // cannot recursively schedule the same unwind a second time.
        let active_scopes = std::mem::take(&mut self.defer_scopes);
        let deferred = active_scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .cloned()
            .collect::<Vec<_>>();
        let mut exit_env = env.clone();
        for deferred in deferred {
            self.analyze_expr(&deferred, &mut exit_env);
        }
        self.defer_scopes = active_scopes;
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

    fn analyze_while_loop(
        &mut self,
        condition: &TypedExpr,
        body: &TypedBody,
        outer: &mut Environment,
    ) {
        let entry = outer.clone();
        let mut head = entry.clone();
        // `head` is the environment before the condition. Both the initial
        // edge and every body backedge reach it, so the condition transfer must
        // participate in the fixed point rather than run only once.
        loop {
            let mut after_condition = head.clone();
            self.analyze_expr(condition, &mut after_condition);
            let mut next = after_condition.clone();
            self.analyze_nested_body(body, &mut next);
            join_environment(&mut next, &entry);
            if next == head {
                // The false edge exits immediately after evaluating the
                // condition, including its assignments and call effects.
                *outer = after_condition;
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
        // Expression transfer is deliberately conservative at effectful
        // boundaries: stores and calls record all incoming origins, while
        // pure constructors preserve child origins under their projections.
        // This prevents an optimizer or unknown callee from hiding a borrowed
        // address without conflating known sibling fields.
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
            | TypedExprKind::CallerLocation(_)
            | TypedExprKind::Trap
            | TypedExprKind::ClosureFunctionPointer { .. } => ValueProvenance::default(),
            TypedExprKind::FunctionCallable { function } => self.analyze_expr(function, env),
            TypedExprKind::Local(local_id) => env.get(local_id).cloned().unwrap_or_default(),
            TypedExprKind::EnumVariant { fields, .. } | TypedExprKind::Tuple(fields) => {
                ValueProvenance::from_value(
                    fields
                        .iter()
                        .enumerate()
                        .flat_map(|(index, field)| {
                            embed_projection(
                                AggregateProjection::TupleField(index),
                                self.analyze_expr(field, env).all(),
                            )
                        })
                        .collect(),
                )
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
                TypedArrayElements::List(elems) => ValueProvenance::from_value(
                    elems
                        .iter()
                        .flat_map(|elem| {
                            embed_projection(
                                AggregateProjection::Element,
                                self.analyze_expr(elem, env).all(),
                            )
                        })
                        .collect(),
                ),
                TypedArrayElements::Repeat { value, .. } => {
                    ValueProvenance::from_value(embed_projection(
                        AggregateProjection::Element,
                        self.analyze_expr(value, env).all(),
                    ))
                }
            },
            TypedExprKind::StructLiteral { fields, .. } => ValueProvenance::from_value(
                fields
                    .iter()
                    .flat_map(|field| {
                        let origins = self.analyze_expr(&field.value, env).all();
                        match field.field {
                            Some(field) => {
                                embed_projection(AggregateProjection::Field(field), origins)
                            }
                            None => origins,
                        }
                    })
                    .collect(),
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
                self.analyze_active_defers(env);
                self.record_error_return(&value.error, expr.span);
                ValueProvenance::from_value(value.value)
            }
            TypedExprKind::Binary { lhs, rhs, .. } => ValueProvenance::from_value(union(
                self.analyze_expr(lhs, env).all(),
                self.analyze_expr(rhs, env).all(),
            )),
            TypedExprKind::Index { lhs, index } => {
                let lhs = self.analyze_expr(lhs, env).all();
                self.analyze_expr(index, env);
                ValueProvenance::from_value(project_origins(lhs, AggregateProjection::Element))
            }
            TypedExprKind::Assign { place, rhs, .. } => {
                // Function lowering materializes the destination, including
                // dereference and index expressions, before evaluating the
                // right-hand side. Those expressions may mutate locals read by
                // the RHS, so preserve the same order in provenance transfer.
                self.analyze_place(place, env);
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
            TypedExprKind::Field { lhs, field, .. } => {
                ValueProvenance::from_value(project_origins(
                    self.analyze_expr(lhs, env).all(),
                    AggregateProjection::Field(*field),
                ))
            }
            TypedExprKind::TupleField { lhs, index } => {
                ValueProvenance::from_value(project_origins(
                    self.analyze_expr(lhs, env).all(),
                    AggregateProjection::TupleField(*index),
                ))
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
                self.analyze_pattern(&pattern.pattern, &mut then_env);
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
            TypedExprKind::IfPatternChain(chain) => {
                let base = env.clone();
                let mut then_env = base.clone();
                let mut value = ValueProvenance::default();
                for clause in &chain.clauses {
                    match clause {
                        nia_body_ir::TypedIfPatternClause::Pattern { target, pattern } => {
                            let target_value = self.analyze_expr(target, &mut then_env);
                            self.analyze_pattern(pattern, &mut then_env);
                            bind_pattern(pattern, &target_value, &mut then_env);
                        }
                        nia_body_ir::TypedIfPatternClause::Condition(condition) => {
                            value.extend(self.analyze_expr(condition, &mut then_env));
                        }
                    }
                }
                value.extend(self.analyze_nested_body(&chain.then_branch, &mut then_env));
                let mut else_env = base;
                if let Some(branch) = chain.else_branch.as_deref() {
                    value.extend(self.analyze_expr(branch, &mut else_env));
                }
                join_environment(&mut then_env, &else_env);
                *env = then_env;
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
                        self.analyze_pattern(pattern, &mut arm_env);
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

    fn analyze_pattern(&mut self, pattern: &TypedPattern, env: &mut Environment) {
        // Expression and range patterns are evaluated by function lowering.
        // Replay them here in declaration order so their calls, stores, and
        // environment mutations cannot disappear from escape summaries.
        match &pattern.kind {
            TypedPatternKind::Pointer(inner)
            | TypedPatternKind::MutPointer(inner)
            | TypedPatternKind::OptionalSome(inner)
            | TypedPatternKind::ErrorOk(inner)
            | TypedPatternKind::ErrorErr(inner) => self.analyze_pattern(inner, env),
            TypedPatternKind::Tuple(patterns)
            | TypedPatternKind::Nominal {
                fields: patterns, ..
            } => {
                for pattern in patterns {
                    self.analyze_pattern(pattern, env);
                }
            }
            TypedPatternKind::Expr(expr) => {
                self.analyze_expr(expr, env);
            }
            TypedPatternKind::Range { start, end, .. } => {
                self.analyze_expr(start, env);
                self.analyze_expr(end, env);
            }
            TypedPatternKind::Wildcard
            | TypedPatternKind::Bind { .. }
            | TypedPatternKind::OptionalNull
            | TypedPatternKind::CheckedInt { .. }
            | TypedPatternKind::CheckedIntRange { .. } => {}
        }
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
            TypedCallee::Tracked { callee, .. } => self.analyze_call(callee, args, call, env),
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
                implementation_method,
                receiver,
                ..
            } => {
                let receiver = self.analyze_expr(receiver, env).all();
                let args = self.analyze_call_args(args, env);
                let mut operands = vec![receiver];
                operands.extend(args);
                self.apply_summary(
                    CallableKey::Function(implementation_method.unwrap_or(*method_id)),
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
                    .flat_map(callable_closure_ids)
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
                if closure_ids.is_empty() || callee.iter().any(contains_input) {
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
        // A known callable maps each summarized input slot back to the
        // caller's provenance. Escaping inputs are reported at the call site;
        // returned inputs remain in the value/error channels for outer flows.
        let Some(summary) = self.summaries.get(&key) else {
            return self.apply_unknown_call(args, span, return_ty);
        };
        let mut result = substitute_summary(&summary.returned_inputs, key, captures, args, false);
        result.extend(substitute_summary(
            &summary.returned_captured_addresses,
            key,
            captures,
            args,
            true,
        ));
        let mut error =
            substitute_summary(&summary.returned_error_inputs, key, captures, args, false);
        error.extend(substitute_summary(
            &summary.returned_error_captured_addresses,
            key,
            captures,
            args,
            true,
        ));
        self.record_escape(
            &substitute_summary(&summary.escaping_inputs, key, captures, args, false),
            span,
            EscapeKind::Call,
        );
        self.record_escape(
            &substitute_summary(
                &summary.escaping_captured_addresses,
                key,
                captures,
                args,
                true,
            ),
            span,
            EscapeKind::Call,
        );
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
        // Without a summary, assume every argument may be retained and may be
        // returned. This is the sound fallback for indirect calls and prevents
        // missing discovery edges from weakening closure diagnostics.
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
            match elem {
                PlaceElem::Field(field) => {
                    value = project_origins(value, AggregateProjection::Field(*field));
                }
                PlaceElem::TupleField(index) => {
                    value = project_origins(value, AggregateProjection::TupleField(*index));
                }
                PlaceElem::Index(index) => {
                    self.analyze_expr(index, env);
                    value = project_origins(value, AggregateProjection::Element);
                }
                PlaceElem::Error => {}
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
        match &place.base {
            PlaceBase::Local(local_id) if place.elems.is_empty() => {
                env.insert(*local_id, value.clone());
            }
            PlaceBase::Local(local_id) => {
                let projections = place
                    .elems
                    .iter()
                    .filter_map(place_projection)
                    .collect::<Vec<_>>();
                let embedded = embed_projections(&projections, value.all());
                let current = &mut env.entry(*local_id).or_default().value;
                // All runtime indices share one `Element` bucket. Replacing it
                // would incorrectly forget origins stored in sibling elements.
                if !projections.contains(&AggregateProjection::Element) {
                    remove_projection(current, &projections);
                }
                current.extend(embedded);
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
        // `seen` is the active structural path, not a global visited set.
        // Reusing a scalar in sibling tuple/error fields is not recursion and
        // must not make an otherwise value-only aggregate retain provenance.
        let may_carry = match self.type_store.get(ty) {
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
        };
        seen.remove(&ty);
        may_carry
    }
}

fn input_provenances(origins: &Provenances, captured: bool) -> Provenances {
    origins
        .iter()
        .flat_map(|origin| input_provenance(origin, captured))
        .collect()
}

fn input_provenance(origin: &Provenance, captured: bool) -> Provenances {
    match origin {
        Provenance::Input(_) if !captured => Provenances::from([origin.clone()]),
        Provenance::CapturedInputAddress(_) if captured => Provenances::from([origin.clone()]),
        Provenance::Aggregate { projection, origin } => {
            embed_projection(*projection, input_provenance(origin, captured))
        }
        Provenance::ClosureCapture { origin, .. } => input_provenance(origin, captured),
        Provenance::OpaqueAggregate { origin } => input_provenance(origin, captured)
            .into_iter()
            .map(|origin| Provenance::OpaqueAggregate {
                origin: Box::new(origin),
            })
            .collect(),
        Provenance::Input(_)
        | Provenance::StackAddress { .. }
        | Provenance::CapturedInputAddress(_)
        | Provenance::CapturedStackAddress { .. }
        | Provenance::CallableClosure { .. } => Provenances::new(),
    }
}

fn parameter_sources(origins: Provenances) -> BTreeSet<usize> {
    origins
        .iter()
        .filter_map(|origin| input_source(origin).or_else(|| captured_input_source(origin)))
        .filter_map(parameter_source)
        .collect()
}

fn parameter_source(source: InputSource) -> Option<usize> {
    match source.root {
        InputRoot::Parameter(index) => Some(index),
        InputRoot::Capture(_) => None,
    }
}

fn capture_address_origins(origins: Provenances) -> Provenances {
    origins.into_iter().map(capture_address_origin).collect()
}

fn capture_address_origin(origin: Provenance) -> Provenance {
    match origin {
        Provenance::Input(source) => Provenance::CapturedInputAddress(source),
        Provenance::StackAddress { scope_depth } => {
            Provenance::CapturedStackAddress { scope_depth }
        }
        Provenance::Aggregate { projection, origin } => Provenance::Aggregate {
            projection,
            origin: Box::new(capture_address_origin(*origin)),
        },
        Provenance::OpaqueAggregate { origin } => Provenance::OpaqueAggregate {
            origin: Box::new(capture_address_origin(*origin)),
        },
        Provenance::CapturedInputAddress(_)
        | Provenance::CapturedStackAddress { .. }
        | Provenance::CallableClosure { .. }
        | Provenance::ClosureCapture { .. } => origin,
    }
}

fn embed_projection(projection: AggregateProjection, origins: Provenances) -> Provenances {
    origins
        .into_iter()
        .map(|origin| {
            if matches!(origin, Provenance::OpaqueAggregate { .. }) {
                return origin;
            }
            if aggregate_depth(&origin) >= MAX_PROJECTION_DEPTH {
                return Provenance::OpaqueAggregate {
                    origin: Box::new(origin),
                };
            }
            Provenance::Aggregate {
                projection,
                origin: Box::new(origin),
            }
        })
        .collect()
}

fn aggregate_depth(origin: &Provenance) -> usize {
    match origin {
        Provenance::Aggregate { origin, .. } => 1 + aggregate_depth(origin),
        _ => 0,
    }
}

fn embed_projections(projections: &[AggregateProjection], mut origins: Provenances) -> Provenances {
    for projection in projections.iter().rev() {
        origins = embed_projection(*projection, origins);
    }
    origins
}

fn project_origins(origins: Provenances, projection: AggregateProjection) -> Provenances {
    origins
        .into_iter()
        .filter_map(|origin| match origin {
            Provenance::Aggregate {
                projection: candidate,
                origin,
            } => (candidate == projection).then_some(*origin),
            Provenance::Input(source) => Some(Provenance::Input(source.projected(projection))),
            Provenance::CapturedInputAddress(source) => Some(Provenance::CapturedInputAddress(
                source.projected(projection),
            )),
            origin @ Provenance::OpaqueAggregate { .. } => Some(origin),
            // Origins without aggregate structure predate or cross an opaque
            // boundary. Retaining them is the sound fallback; known sibling
            // fields above are still discarded precisely.
            origin => Some(origin),
        })
        .collect()
}

fn place_projection(elem: &PlaceElem) -> Option<AggregateProjection> {
    match elem {
        PlaceElem::Field(field) => Some(AggregateProjection::Field(*field)),
        PlaceElem::TupleField(index) => Some(AggregateProjection::TupleField(*index)),
        PlaceElem::Index(_) => Some(AggregateProjection::Element),
        PlaceElem::Error => None,
    }
}

fn remove_projection(origins: &mut Provenances, projections: &[AggregateProjection]) {
    let Some((projection, rest)) = projections.split_first() else {
        origins.clear();
        return;
    };
    let old = std::mem::take(origins);
    for origin in old {
        match origin {
            Provenance::Aggregate {
                projection: candidate,
                origin,
            } if candidate == *projection => {
                let mut nested = Provenances::from([*origin]);
                remove_projection(&mut nested, rest);
                origins.extend(embed_projection(candidate, nested));
            }
            origin => {
                origins.insert(origin);
            }
        }
    }
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
    source: &InputSource,
    captures: &Provenances,
    args: &[Provenances],
) -> Provenances {
    let mut origins = match source.root {
        InputRoot::Capture(index) => {
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
                // An opaque flow can erase closure-state slot wrappers. Retain
                // every capture origin when the requested slot cannot be
                // selected instead of silently dropping a possible escape.
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
        InputRoot::Parameter(index) => args.get(index).cloned().unwrap_or_default(),
    };
    if source.imprecise {
        return origins;
    }
    for projection in &source.projections {
        origins = project_origins(origins, *projection);
    }
    origins
}

fn substitute_summary(
    template: &Provenances,
    key: CallableKey,
    captures: &Provenances,
    args: &[Provenances],
    capture_address: bool,
) -> Provenances {
    template
        .iter()
        .flat_map(|origin| match origin {
            Provenance::Input(source) | Provenance::CapturedInputAddress(source) => {
                let origins = input_origins(key, source, captures, args);
                if capture_address {
                    capture_address_origins(origins)
                } else {
                    origins
                }
            }
            Provenance::Aggregate { projection, origin } => embed_projection(
                *projection,
                substitute_summary(
                    &Provenances::from([origin.as_ref().clone()]),
                    key,
                    captures,
                    args,
                    capture_address,
                ),
            ),
            Provenance::OpaqueAggregate { origin } => substitute_summary(
                &Provenances::from([origin.as_ref().clone()]),
                key,
                captures,
                args,
                capture_address,
            )
            .into_iter()
            .map(|origin| Provenance::OpaqueAggregate {
                origin: Box::new(origin),
            })
            .collect(),
            _ => Provenances::new(),
        })
        .collect()
}

fn input_source(origin: &Provenance) -> Option<InputSource> {
    match origin {
        Provenance::Input(source) => Some(source.clone()),
        Provenance::ClosureCapture { origin, .. }
        | Provenance::Aggregate { origin, .. }
        | Provenance::OpaqueAggregate { origin } => input_source(origin),
        Provenance::StackAddress { .. }
        | Provenance::CapturedInputAddress(_)
        | Provenance::CapturedStackAddress { .. }
        | Provenance::CallableClosure { .. } => None,
    }
}

fn captured_input_source(origin: &Provenance) -> Option<InputSource> {
    match origin {
        Provenance::CapturedInputAddress(source) => Some(source.clone()),
        Provenance::ClosureCapture { origin, .. }
        | Provenance::Aggregate { origin, .. }
        | Provenance::OpaqueAggregate { origin } => captured_input_source(origin),
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
        Provenance::ClosureCapture { origin, .. }
        | Provenance::Aggregate { origin, .. }
        | Provenance::OpaqueAggregate { origin } => contains_stack_backed_callable(origin),
        _ => false,
    }
}

fn contains_captured_stack_address(origin: &Provenance) -> bool {
    match origin {
        Provenance::CapturedStackAddress { .. } => true,
        Provenance::ClosureCapture { origin, .. }
        | Provenance::Aggregate { origin, .. }
        | Provenance::OpaqueAggregate { origin } => contains_captured_stack_address(origin),
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
        Provenance::ClosureCapture { origin, .. }
        | Provenance::Aggregate { origin, .. }
        | Provenance::OpaqueAggregate { origin } => {
            provenance_expires_at(origin, closure_scopes, depth)
        }
        _ => false,
    }
}

fn callable_closure_ids(origin: &Provenance) -> BTreeSet<ClosureId> {
    match origin {
        Provenance::CallableClosure { closure_id, .. } => BTreeSet::from([*closure_id]),
        Provenance::ClosureCapture { origin, .. }
        | Provenance::Aggregate { origin, .. }
        | Provenance::OpaqueAggregate { origin } => callable_closure_ids(origin),
        _ => BTreeSet::new(),
    }
}

fn contains_input(origin: &Provenance) -> bool {
    match origin {
        Provenance::Input(_) => true,
        Provenance::ClosureCapture { origin, .. }
        | Provenance::Aggregate { origin, .. }
        | Provenance::OpaqueAggregate { origin } => contains_input(origin),
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
            for (index, pattern) in patterns.iter().enumerate() {
                bind_pattern(
                    pattern,
                    &ValueProvenance::from_value(project_origins(
                        value.all(),
                        AggregateProjection::TupleField(index),
                    )),
                    env,
                );
            }
        }
        TypedPatternKind::Nominal {
            constructor,
            fields,
        } => {
            for (index, pattern) in fields.iter().enumerate() {
                let projection = match constructor {
                    TypedNominalPatternConstructor::Struct { field_defs } => field_defs
                        .get(index)
                        .copied()
                        .map(AggregateProjection::Field),
                    TypedNominalPatternConstructor::EnumVariant { .. } => {
                        Some(AggregateProjection::TupleField(index))
                    }
                };
                let origins = projection
                    .map(|projection| project_origins(value.all(), projection))
                    .unwrap_or_else(|| value.all());
                bind_pattern(pattern, &ValueProvenance::from_value(origins), env);
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

    #[test]
    fn deep_input_projection_widens_to_a_stable_conservative_source() {
        let projection = AggregateProjection::TupleField(0);
        let mut source = InputSource::parameter(0);
        for _ in 0..=MAX_PROJECTION_DEPTH {
            source = source.projected(projection);
        }

        assert!(source.imprecise);
        assert!(source.projections.is_empty());
        assert_eq!(source.projected(projection), source);

        let closure_id = closure_id();
        let stack_backed = Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        };
        assert_eq!(
            input_origins(
                CallableKey::Function(closure_id.owner),
                &source,
                &Provenances::new(),
                &[Provenances::from([stack_backed.clone()])],
            ),
            Provenances::from([stack_backed]),
        );
    }

    #[test]
    fn deep_output_embedding_widens_and_stabilizes() {
        let projection = AggregateProjection::TupleField(0);
        let mut origins = Provenances::from([Provenance::Input(InputSource::parameter(0))]);
        for _ in 0..=MAX_PROJECTION_DEPTH {
            origins = embed_projection(projection, origins);
        }

        assert!(matches!(
            origins.iter().next(),
            Some(Provenance::OpaqueAggregate { .. })
        ));
        assert_eq!(embed_projection(projection, origins.clone()), origins);
    }

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
    fn discovers_closures_nested_in_pattern_operands() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let unit_ty = append.intern(TyKind::Tuple(Vec::new()));
        let ty = append.intern(TyKind::Callable {
            is_readonly: true,
            params: Vec::new(),
            return_type: unit_ty,
        });
        let closure = |ordinal| TypedExpr {
            span: Span::default(),
            ty,
            kind: TypedExprKind::Closure {
                closure_id: ClosureId { owner, ordinal },
                captures: Vec::new(),
                params: Vec::new(),
                body: TypedBody {
                    span: Span::default(),
                    locals: Vec::new(),
                    stmts: Vec::new(),
                    tail: None,
                    ty,
                },
            },
        };
        let body = TypedBody {
            span: Span::default(),
            locals: Vec::new(),
            stmts: vec![nia_body_ir::TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::PatternBinding(Box::new(nia_body_ir::TypedPatternBinding {
                    pattern: TypedPattern {
                        ty,
                        span: Span::default(),
                        kind: TypedPatternKind::Tuple(vec![
                            TypedPattern {
                                ty,
                                span: Span::default(),
                                kind: TypedPatternKind::Expr(Box::new(closure(0))),
                            },
                            TypedPattern {
                                ty,
                                span: Span::default(),
                                kind: TypedPatternKind::Range {
                                    start: Box::new(closure(1)),
                                    end: Box::new(closure(2)),
                                    inclusive: true,
                                },
                            },
                        ]),
                    },
                    value: TypedExpr {
                        span: Span::default(),
                        ty,
                        kind: TypedExprKind::Tuple(Vec::new()),
                    },
                })),
            }],
            tail: None,
            ty,
        };
        let mut callables = HashMap::new();

        collect_body_closures(&body, &mut callables);

        assert!((0..3).all(|ordinal| {
            callables.contains_key(&CallableKey::Closure(ClosureId { owner, ordinal }))
        }));
    }

    #[test]
    fn pattern_operand_effects_contribute_to_escape_analysis() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let closure_id = ClosureId { owner, ordinal: 0 };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let unit_ty = append.intern(TyKind::Tuple(Vec::new()));
        let ty = append.intern(TyKind::Callable {
            is_readonly: true,
            params: Vec::new(),
            return_type: unit_ty,
        });
        let selected = LocalId(0);
        let pattern = TypedPattern {
            span: Span::default(),
            ty,
            kind: TypedPatternKind::Expr(Box::new(TypedExpr {
                span: Span::default(),
                ty,
                kind: TypedExprKind::Assign {
                    place: TypedPlace {
                        span: Span::default(),
                        ty,
                        base: PlaceBase::Global(GlobalDefId {
                            module_id,
                            def_id: DefId(2),
                        }),
                        elems: Vec::new(),
                    },
                    op: nia_ast::AssignOp::Assign,
                    rhs: Box::new(TypedExpr {
                        span: Span::default(),
                        ty,
                        kind: TypedExprKind::Local(selected),
                    }),
                },
            })),
        };
        let mut env = Environment::from([(
            selected,
            ValueProvenance::from_value(Provenances::from([Provenance::CallableClosure {
                closure_id,
                stack_backed: true,
            }])),
        )]);
        let summaries = HashMap::new();
        let mut analyzer = Analyzer::new(&types, &summaries, None);

        analyzer.analyze_pattern(&pattern, &mut env);

        assert!(analyzer.escaped.contains(&Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        }));
    }

    #[test]
    fn known_closure_capture_lookup_selects_only_the_requested_slot() {
        let closure_id = closure_id();
        let selected = Provenance::CapturedInputAddress(InputSource::parameter(0));
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
                &InputSource::capture(0),
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
            Provenance::CapturedInputAddress(InputSource::parameter(0)),
            Provenance::CapturedInputAddress(InputSource::parameter(1)),
        ]);

        assert_eq!(
            input_origins(
                CallableKey::Closure(closure_id),
                &InputSource::capture(0),
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
                    InputSource::parameter(0),
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

    #[test]
    fn while_backedges_reapply_condition_provenance_transfers() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let closure_id = ClosureId { owner, ordinal: 0 };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let unit_ty = append.intern(TyKind::Tuple(Vec::new()));
        let bool_ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::Bool));
        let callable_ty = append.intern(TyKind::Callable {
            is_readonly: true,
            params: Vec::new(),
            return_type: unit_ty,
        });
        let pending = LocalId(0);
        let selected = LocalId(1);
        let stack_backed = LocalId(2);
        let input = ValueProvenance::from_value(Provenances::from([Provenance::Input(
            InputSource::parameter(0),
        )]));
        let mut env = Environment::from([
            (pending, input.clone()),
            (selected, input),
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
        let assign = |target, source| TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Assign {
                place: TypedPlace {
                    span: Span::default(),
                    ty: callable_ty,
                    base: PlaceBase::Local(target),
                    elems: Vec::new(),
                },
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(local(source)),
            },
        };
        let condition = TypedExpr {
            span: Span::default(),
            ty: bool_ty,
            kind: TypedExprKind::Block(TypedBody {
                span: Span::default(),
                locals: Vec::new(),
                stmts: vec![nia_body_ir::TypedStmt {
                    span: Span::default(),
                    kind: TypedStmtKind::Expr(assign(selected, pending)),
                }],
                tail: Some(Box::new(TypedExpr {
                    span: Span::default(),
                    ty: bool_ty,
                    kind: TypedExprKind::Bool(true),
                })),
                ty: bool_ty,
            }),
        };
        let body = TypedBody {
            span: Span::default(),
            locals: Vec::new(),
            stmts: vec![nia_body_ir::TypedStmt {
                span: Span::default(),
                kind: TypedStmtKind::Expr(assign(pending, stack_backed)),
            }],
            tail: None,
            ty: unit_ty,
        };
        let summaries = HashMap::new();
        let mut analyzer = Analyzer::new(&types, &summaries, None);

        analyzer.analyze_while_loop(&condition, &body, &mut env);

        assert!(
            env.get(&selected)
                .expect("selected local must remain in the loop environment")
                .value
                .contains(&Provenance::CallableClosure {
                    closure_id,
                    stack_backed: true,
                })
        );
    }

    #[test]
    fn repeated_value_types_do_not_look_like_recursive_borrowed_state() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let closure_id = ClosureId { owner, ordinal: 0 };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let i32_ty = append.intern(TyKind::Primitive(nia_ty::PrimitiveTy::I32));
        let pair_ty = append.intern(TyKind::Tuple(vec![i32_ty, i32_ty]));
        let summaries = HashMap::new();
        let analyzer = Analyzer::new(&types, &summaries, None);
        let origins = Provenances::from([Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        }]);

        assert!(
            analyzer
                .filter_origins_for_type(origins, pair_ty)
                .is_empty()
        );
    }

    #[test]
    fn defers_observe_exit_state_and_execute_in_lifo_order() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let closure_id = ClosureId { owner, ordinal: 0 };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let unit_ty = append.intern(TyKind::Tuple(Vec::new()));
        let callable_ty = append.intern(TyKind::Callable {
            is_readonly: true,
            params: Vec::new(),
            return_type: unit_ty,
        });
        let selected = LocalId(0);
        let stack_backed = LocalId(1);
        let local = |local_id| TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Local(local_id),
        };
        let assign = |base, rhs| TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Assign {
                place: TypedPlace {
                    span: Span::default(),
                    ty: callable_ty,
                    base,
                    elems: Vec::new(),
                },
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(rhs),
            },
        };
        let body = TypedBody {
            span: Span::default(),
            locals: Vec::new(),
            stmts: vec![
                nia_body_ir::TypedStmt {
                    span: Span::default(),
                    kind: TypedStmtKind::Defer(assign(
                        PlaceBase::Global(GlobalDefId {
                            module_id,
                            def_id: DefId(2),
                        }),
                        local(selected),
                    )),
                },
                nia_body_ir::TypedStmt {
                    span: Span::default(),
                    kind: TypedStmtKind::Defer(assign(
                        PlaceBase::Local(selected),
                        local(stack_backed),
                    )),
                },
            ],
            tail: None,
            ty: unit_ty,
        };
        let mut env = Environment::from([
            (
                selected,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::parameter(0),
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
        let summaries = HashMap::new();
        let mut analyzer = Analyzer::new(&types, &summaries, None);

        analyzer.analyze_body_contents(&body, &mut env);

        assert!(analyzer.escaped.contains(&Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        }));
    }

    #[test]
    fn active_return_defers_use_an_isolated_exit_environment() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let closure_id = ClosureId { owner, ordinal: 0 };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let unit_ty = append.intern(TyKind::Tuple(Vec::new()));
        let callable_ty = append.intern(TyKind::Callable {
            is_readonly: true,
            params: Vec::new(),
            return_type: unit_ty,
        });
        let selected = LocalId(0);
        let safe = LocalId(1);
        let local = |local_id| TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Local(local_id),
        };
        let assign = |base, rhs| TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Assign {
                place: TypedPlace {
                    span: Span::default(),
                    ty: callable_ty,
                    base,
                    elems: Vec::new(),
                },
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(rhs),
            },
        };
        let stack_backed =
            ValueProvenance::from_value(Provenances::from([Provenance::CallableClosure {
                closure_id,
                stack_backed: true,
            }]));
        let env = Environment::from([
            (selected, stack_backed.clone()),
            (
                safe,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::parameter(0),
                )])),
            ),
        ]);
        let summaries = HashMap::new();
        let mut analyzer = Analyzer::new(&types, &summaries, None);
        analyzer.defer_scopes.push(vec![
            assign(PlaceBase::Local(selected), local(safe)),
            assign(
                PlaceBase::Global(GlobalDefId {
                    module_id,
                    def_id: DefId(2),
                }),
                local(selected),
            ),
        ]);

        analyzer.analyze_active_defers(&env);

        assert!(analyzer.escaped.contains(&Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        }));
        assert_eq!(env.get(&selected), Some(&stack_backed));
    }

    #[test]
    fn assignment_places_are_evaluated_before_the_rhs() {
        let mut module_ids = ModuleIdAllocator::new();
        let module_id = module_ids.allocate();
        let owner = GlobalDefId {
            module_id,
            def_id: DefId(1),
        };
        let closure_id = ClosureId { owner, ordinal: 0 };
        let types = TypeStore::new();
        let append = types.append_for_module(module_id);
        let unit_ty = append.intern(TyKind::Tuple(Vec::new()));
        let callable_ty = append.intern(TyKind::Callable {
            is_readonly: true,
            params: Vec::new(),
            return_type: unit_ty,
        });
        let selected = LocalId(0);
        let stack_backed = LocalId(1);
        let local = |local_id| TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Local(local_id),
        };
        let place_effect = TypedExpr {
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
        let assignment = TypedExpr {
            span: Span::default(),
            ty: callable_ty,
            kind: TypedExprKind::Assign {
                place: TypedPlace {
                    span: Span::default(),
                    ty: callable_ty,
                    base: PlaceBase::Deref(Box::new(place_effect)),
                    elems: Vec::new(),
                },
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(local(selected)),
            },
        };
        let mut env = Environment::from([
            (
                selected,
                ValueProvenance::from_value(Provenances::from([Provenance::Input(
                    InputSource::parameter(0),
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
        let summaries = HashMap::new();
        let mut analyzer = Analyzer::new(&types, &summaries, None);

        analyzer.analyze_expr(&assignment, &mut env);

        assert!(analyzer.escaped.contains(&Provenance::CallableClosure {
            closure_id,
            stack_backed: true,
        }));
    }
}
