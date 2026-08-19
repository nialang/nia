//! Const IR data shared by lowering, checking, evaluation, and static analysis.
//!
//! Early IR mirrors const-capable syntax and can retain unresolved semantic
//! identities. Resolved IR is the downstream contract: required names, locals,
//! and types have concrete ids, while only contextually inferred annotations
//! remain optional.

use crate::resolve::unresolved_error;
use crate::*;
use nia_ids::{
    BuiltinConstValue, GlobalConstExprId, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId,
    ValueBuiltin,
};
use nia_sema_ir::{AssociatedConstProjection, BuiltinAssociatedValue, SemanticValueUse};
use nia_span::Span;
use nia_symbol::SymbolId;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Default)]
/// Per-module resolved const products indexed by their semantic owners.
///
/// Initializers are kept separate by execution/storage role so query clients do
/// not need to reinterpret a single heterogeneous expression table.
pub struct ResolvedConstModule {
    enums: Vec<ResolvedConstEnum>,
    global_initializers: HashMap<GlobalDefId, ResolvedConstExpr>,
    deferred_global_initializers: HashMap<GlobalDefId, ResolvedConstExpr>,
    local_initializers: HashMap<LocalId, ResolvedConstLocalInitializer>,
    functions: HashMap<GlobalDefId, ResolvedConstFunction>,
    const_exprs: HashMap<GlobalConstExprId, ResolvedConstExpr>,
}

impl ResolvedConstModule {
    /// Creates an empty resolved module product.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns enums in active source order.
    pub fn enums(&self) -> &[ResolvedConstEnum] {
        &self.enums
    }

    /// Returns eager global const and static initializers by owner identity.
    pub fn global_initializers(&self) -> &HashMap<GlobalDefId, ResolvedConstExpr> {
        &self.global_initializers
    }

    /// Returns global initializers whose execution is deferred to a later phase.
    pub fn deferred_global_initializers(&self) -> &HashMap<GlobalDefId, ResolvedConstExpr> {
        &self.deferred_global_initializers
    }

    /// Returns function-local const initializers by local identity.
    pub fn local_initializers(&self) -> &HashMap<LocalId, ResolvedConstLocalInitializer> {
        &self.local_initializers
    }

    /// Returns const function bodies by global definition identity.
    pub fn functions(&self) -> &HashMap<GlobalDefId, ResolvedConstFunction> {
        &self.functions
    }

    /// Returns standalone const expressions by query identity.
    pub fn const_exprs(&self) -> &HashMap<GlobalConstExprId, ResolvedConstExpr> {
        &self.const_exprs
    }

    /// Appends a resolved enum in active source order.
    pub fn push_enum(&mut self, item: ResolvedConstEnum) {
        self.enums.push(item);
    }

    /// Installs an eager global initializer, returning any previous entry.
    pub fn insert_global_initializer(
        &mut self,
        id: GlobalDefId,
        value: ResolvedConstExpr,
    ) -> Option<ResolvedConstExpr> {
        self.global_initializers.insert(id, value)
    }

    /// Installs a deferred global initializer, returning any previous entry.
    pub fn insert_deferred_global_initializer(
        &mut self,
        id: GlobalDefId,
        value: ResolvedConstExpr,
    ) -> Option<ResolvedConstExpr> {
        self.deferred_global_initializers.insert(id, value)
    }

    /// Installs a local initializer, returning any previous entry.
    pub fn insert_local_initializer(
        &mut self,
        id: LocalId,
        value: ResolvedConstLocalInitializer,
    ) -> Option<ResolvedConstLocalInitializer> {
        self.local_initializers.insert(id, value)
    }

    /// Installs a const function body, returning any previous entry.
    pub fn insert_function(
        &mut self,
        id: GlobalDefId,
        function: ResolvedConstFunction,
    ) -> Option<ResolvedConstFunction> {
        self.functions.insert(id, function)
    }

    /// Installs a standalone const expression, returning any previous entry.
    pub fn insert_const_expr(
        &mut self,
        id: GlobalConstExprId,
        expr: ResolvedConstExpr,
    ) -> Option<ResolvedConstExpr> {
        self.const_exprs.insert(id, expr)
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved local initializer and its optional explicit type annotation.
pub struct ResolvedConstLocalInitializer {
    explicit_type: Option<InternedTyId>,
    value: ResolvedConstExpr,
}

impl ResolvedConstLocalInitializer {
    /// Creates a local initializer product.
    pub fn new(explicit_type: Option<InternedTyId>, value: ResolvedConstExpr) -> Self {
        Self {
            explicit_type,
            value,
        }
    }

    /// Returns the explicit runtime type annotation, when present.
    pub fn explicit_type(&self) -> Option<InternedTyId> {
        self.explicit_type
    }

    /// Returns the initializer expression.
    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Enum definition and the const expressions supplying its discriminants.
pub struct ResolvedConstEnum {
    def_id: GlobalDefId,
    span: Span,
    variants: Vec<ResolvedConstEnumVariant>,
}

impl ResolvedConstEnum {
    /// Creates a resolved enum product.
    pub fn new(def_id: GlobalDefId, span: Span, variants: Vec<ResolvedConstEnumVariant>) -> Self {
        Self {
            def_id,
            span,
            variants,
        }
    }

    /// Returns the enum's stable definition identity.
    pub fn def_id(&self) -> GlobalDefId {
        self.def_id
    }

    /// Returns the enum declaration span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns variants in declaration order.
    pub fn variants(&self) -> &[ResolvedConstEnumVariant] {
        &self.variants
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Enum variant and its optional explicit discriminant expression.
pub struct ResolvedConstEnumVariant {
    def_id: GlobalDefId,
    span: Span,
    value: Option<ResolvedConstExpr>,
}

impl ResolvedConstEnumVariant {
    /// Creates a resolved enum variant product.
    pub fn new(def_id: GlobalDefId, span: Span, value: Option<ResolvedConstExpr>) -> Self {
        Self {
            def_id,
            span,
            value,
        }
    }

    /// Returns the variant's stable definition identity.
    pub fn def_id(&self) -> GlobalDefId {
        self.def_id
    }

    /// Returns the variant declaration span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the explicit discriminant expression, when present.
    pub fn value(&self) -> Option<&ResolvedConstExpr> {
        self.value.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
/// An identity-complete const expression ready for checking and evaluation.
pub struct ResolvedConstExpr {
    span: Span,
    kind: ResolvedConstExprKind,
}

impl ResolvedConstExpr {
    pub(crate) fn new(expr: EarlyConstExpr) -> Result<Self, ConstLowerError> {
        resolve_expr(expr)
    }

    /// Creates an expression from an already validated payload.
    pub fn from_parts(span: Span, kind: ResolvedConstExprKind) -> Self {
        Self { span, kind }
    }

    /// Returns the complete source span of the expression.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the expression payload.
    pub fn kind(&self) -> &ResolvedConstExprKind {
        &self.kind
    }

    /// Creates a resolved name expression.
    pub fn name(span: Span, resolution: ConstNameResolution) -> Self {
        Self {
            span,
            kind: ResolvedConstExprKind::Name(resolution),
        }
    }

    /// Creates a named-field projection.
    pub fn field(span: Span, lhs: ResolvedConstExpr, name: SymbolId) -> Self {
        Self {
            span,
            kind: ResolvedConstExprKind::Field {
                lhs: Box::new(lhs),
                name,
            },
        }
    }

    /// Creates an indexed projection.
    pub fn index(span: Span, lhs: ResolvedConstExpr, index: ResolvedConstExpr) -> Self {
        Self {
            span,
            kind: ResolvedConstExprKind::Index {
                lhs: Box::new(lhs),
                index: Box::new(index),
            },
        }
    }

    /// Creates a resolved call expression.
    pub fn call(
        span: Span,
        callee: ResolvedConstExpr,
        generic_args: Vec<ResolvedConstGenericArg>,
        args: Vec<ResolvedConstExpr>,
    ) -> Self {
        Self {
            span,
            kind: ResolvedConstExprKind::Call {
                callee: Box::new(callee),
                generic_args,
                args,
            },
        }
    }

    /// Returns the semantic identity when this is a name expression.
    pub fn name_resolution(&self) -> Option<ConstNameResolution> {
        match &self.kind {
            ResolvedConstExprKind::Name(resolution) => Some(resolution.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// A resolved const function with semantic local ids on parameters and every
/// binding pattern in its body.
pub struct ResolvedConstFunction {
    span: Span,
    params: Vec<ResolvedConstParam>,
    body: ResolvedConstBlock,
}

impl ResolvedConstFunction {
    pub(crate) fn new(function: EarlyConstFunction) -> Result<Self, ConstLowerError> {
        resolve_function(function)
    }

    pub(crate) fn from_parts(
        span: Span,
        params: Vec<ResolvedConstParam>,
        body: ResolvedConstBlock,
    ) -> Self {
        Self { span, params, body }
    }

    /// Returns the function declaration span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns parameters in call ABI order.
    pub fn params(&self) -> &[ResolvedConstParam] {
        &self.params
    }

    /// Returns the resolved function body.
    pub fn body(&self) -> &ResolvedConstBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved const-function parameter and its execution-frame local identity.
pub struct ResolvedConstParam {
    span: Span,
    name: SymbolId,
    local_id: LocalId,
    ty: Option<InternedTyId>,
    receiver: Option<nia_ids::ReceiverKind>,
}

impl ResolvedConstParam {
    /// Creates a resolved function parameter.
    pub fn new(
        span: Span,
        name: SymbolId,
        local_id: LocalId,
        ty: Option<InternedTyId>,
        receiver: Option<nia_ids::ReceiverKind>,
    ) -> Self {
        Self {
            span,
            name,
            local_id,
            ty,
            receiver,
        }
    }

    /// Returns the parameter declaration span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the source-level parameter name.
    pub fn name(&self) -> SymbolId {
        self.name
    }

    /// Returns the local identity bound in the callee frame.
    pub fn local_id(&self) -> LocalId {
        self.local_id
    }

    /// Returns the explicit parameter type, when present.
    pub fn ty(&self) -> Option<InternedTyId> {
        self.ty
    }

    /// Returns receiver mutability and passing mode, when this is a receiver.
    pub fn receiver(&self) -> Option<nia_ids::ReceiverKind> {
        self.receiver
    }
}

#[derive(Debug, Clone, PartialEq)]
/// A lexical const block. Statement order and the optional tail expression are
/// preserved because const evaluation follows source evaluation order.
pub struct ResolvedConstBlock {
    span: Span,
    stmts: Vec<ResolvedConstStmt>,
    tail: Option<Box<ResolvedConstExpr>>,
}

impl ResolvedConstBlock {
    /// Creates a lexical block preserving statement order and its optional tail.
    pub fn new(
        span: Span,
        stmts: Vec<ResolvedConstStmt>,
        tail: Option<Box<ResolvedConstExpr>>,
    ) -> Self {
        Self { span, stmts, tail }
    }

    /// Returns the block source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns statements in evaluation order.
    pub fn stmts(&self) -> &[ResolvedConstStmt] {
        &self.stmts
    }

    /// Returns the optional value-producing tail expression.
    pub fn tail(&self) -> Option<&ResolvedConstExpr> {
        self.tail.as_deref()
    }

    /// Returns whether the block has no statements.
    ///
    /// A block with only a tail expression is considered empty by this helper.
    pub fn is_empty(&self) -> bool {
        self.stmts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
/// One resolved statement in a const-function block.
pub struct ResolvedConstStmt {
    span: Span,
    kind: ResolvedConstStmtKind,
}

impl ResolvedConstStmt {
    /// Creates a statement from its source span and payload.
    pub fn new(span: Span, kind: ResolvedConstStmtKind) -> Self {
        Self { span, kind }
    }

    /// Returns the statement source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the statement payload.
    pub fn kind(&self) -> &ResolvedConstStmtKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Executable statement forms supported by const functions.
pub enum ResolvedConstStmtKind {
    /// Declares and initializes one named local.
    Binding(ResolvedConstBinding),
    /// Declares locals through a destructuring pattern.
    PatternBinding(ResolvedConstPatternBinding),
    /// Evaluates an expression for its value or side effects.
    Expr(ResolvedConstExpr),
    /// Returns an optional value from the current const function.
    Return(Option<ResolvedConstExpr>),
    /// Exits the nearest loop.
    Break,
    /// Continues the nearest loop.
    Continue,
    /// Conditional statement.
    If {
        /// Condition evaluated before selecting a branch.
        cond: ResolvedConstExpr,
        /// Branch executed when the condition is true.
        then_branch: ResolvedConstBlock,
        /// Optional branch executed when the condition is false.
        else_branch: Option<ResolvedConstBlock>,
    },
    /// Iterator-based loop.
    ForIn(ResolvedConstForIn),
    /// Condition-controlled loop.
    While {
        /// Condition evaluated before every iteration.
        cond: ResolvedConstExpr,
        /// Loop body.
        body: ResolvedConstBlock,
    },
    /// Unconditional loop.
    Loop {
        /// Loop body.
        body: ResolvedConstBlock,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// A destructuring local binding in a const function.
///
/// The annotation constrains the initializer and therefore the whole pattern. `is_mutable`
/// applies to every binding leaf, matching the runtime interpretation of `let mut PATTERN`.
pub struct ResolvedConstPatternBinding {
    span: Span,
    pattern: ResolvedConstPattern,
    explicit_type: Option<InternedTyId>,
    is_mutable: bool,
    value: ResolvedConstExpr,
}

impl ResolvedConstPatternBinding {
    /// Creates a destructuring binding with its resolved pattern and value.
    pub fn new(
        span: Span,
        pattern: ResolvedConstPattern,
        explicit_type: Option<InternedTyId>,
        is_mutable: bool,
        value: ResolvedConstExpr,
    ) -> Self {
        Self {
            span,
            pattern,
            explicit_type,
            is_mutable,
            value,
        }
    }

    /// Returns the binding source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the resolved destructuring pattern.
    pub fn pattern(&self) -> &ResolvedConstPattern {
        &self.pattern
    }

    /// Returns the optional explicit type constraint.
    pub fn explicit_type(&self) -> Option<InternedTyId> {
        self.explicit_type
    }

    /// Returns whether every leaf binding is mutable.
    pub fn is_mutable(&self) -> bool {
        self.is_mutable
    }

    /// Returns the expression supplying the pattern value.
    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
/// A named local binding with a resolved local identity.
pub struct ResolvedConstBinding {
    span: Span,
    name: SymbolId,
    local_id: LocalId,
    explicit_type: Option<InternedTyId>,
    is_mutable: bool,
    value: ResolvedConstExpr,
}

impl ResolvedConstBinding {
    /// Creates a resolved named local binding.
    pub fn new(
        span: Span,
        name: SymbolId,
        local_id: LocalId,
        explicit_type: Option<InternedTyId>,
        is_mutable: bool,
        value: ResolvedConstExpr,
    ) -> Self {
        Self {
            span,
            name,
            local_id,
            explicit_type,
            is_mutable,
            value,
        }
    }

    /// Returns the binding source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the source-level binding name.
    pub fn name(&self) -> SymbolId {
        self.name
    }

    /// Returns the semantic local identity used by evaluation.
    pub fn local_id(&self) -> LocalId {
        self.local_id
    }

    /// Returns the optional explicit type constraint.
    pub fn explicit_type(&self) -> Option<InternedTyId> {
        self.explicit_type
    }

    /// Returns whether assignments to the local are permitted.
    pub fn is_mutable(&self) -> bool {
        self.is_mutable
    }

    /// Returns the initializer expression.
    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Compound assignment with a resolved target and right-hand expression.
pub struct ResolvedConstAssign {
    lhs: ResolvedConstAssignTarget,
    op: ConstAssignOp,
    rhs: ResolvedConstExpr,
}

impl ResolvedConstAssign {
    /// Creates an assignment payload.
    pub fn new(lhs: ResolvedConstAssignTarget, op: ConstAssignOp, rhs: ResolvedConstExpr) -> Self {
        Self { lhs, op, rhs }
    }

    /// Returns the target being updated.
    pub fn lhs(&self) -> &ResolvedConstAssignTarget {
        &self.lhs
    }

    /// Returns the assignment operator.
    pub fn op(&self) -> ConstAssignOp {
        self.op
    }

    /// Returns the value expression evaluated for the assignment.
    pub fn rhs(&self) -> &ResolvedConstExpr {
        &self.rhs
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved writable target. Its projection path is ordered root to leaf.
pub struct ResolvedConstAssignTarget {
    kind: ResolvedConstAssignTargetKind,
}

impl ResolvedConstAssignTarget {
    /// Creates a local target with an optional field/index projection path.
    pub fn local(
        span: Span,
        name: SymbolId,
        local_id: LocalId,
        path: Vec<ResolvedConstAssignPathElem>,
    ) -> Self {
        Self {
            kind: ResolvedConstAssignTargetKind::Local {
                span,
                name,
                local_id,
                path,
            },
        }
    }

    /// Returns the target identity and projection payload.
    pub fn kind(&self) -> &ResolvedConstAssignTargetKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Kinds of resolved assignment roots.
pub enum ResolvedConstAssignTargetKind {
    /// A local binding and projections into its aggregate value.
    Local {
        /// Source span of the target root.
        span: Span,
        /// Source-level local name.
        name: SymbolId,
        /// Semantic local identity.
        local_id: LocalId,
        /// Projections from root to the assigned leaf.
        path: Vec<ResolvedConstAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// One root-to-leaf projection in an assignment target.
pub struct ResolvedConstAssignPathElem {
    kind: ResolvedConstAssignPathElemKind,
}

impl ResolvedConstAssignPathElem {
    /// Creates a named-field projection.
    pub fn field(span: Span, name: SymbolId) -> Self {
        Self {
            kind: ResolvedConstAssignPathElemKind::Field { span, name },
        }
    }

    pub fn index(span: Span, index: ResolvedConstExpr) -> Self {
        Self {
            kind: ResolvedConstAssignPathElemKind::Index { span, index },
        }
    }

    /// Returns the projection payload.
    pub fn kind(&self) -> &ResolvedConstAssignPathElemKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Assignment projection forms.
pub enum ResolvedConstAssignPathElemKind {
    /// Named field selection.
    Field {
        /// Span of the field projection.
        span: Span,
        /// Field identity.
        name: SymbolId,
    },
    /// Sequence index selection.
    Index {
        /// Span of the index projection.
        span: Span,
        /// Index expression evaluated before writeback.
        index: ResolvedConstExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved iterator loop with pattern, iterable expression, and body.
pub struct ResolvedConstForIn {
    pattern: ResolvedConstPattern,
    iter: ResolvedConstExpr,
    body: ResolvedConstBlock,
}

impl ResolvedConstForIn {
    /// Creates a resolved iterator loop.
    pub fn new(
        pattern: ResolvedConstPattern,
        iter: ResolvedConstExpr,
        body: ResolvedConstBlock,
    ) -> Self {
        Self {
            pattern,
            iter,
            body,
        }
    }

    /// Returns the iterable expression.
    pub fn iter(&self) -> &ResolvedConstExpr {
        &self.iter
    }

    /// Returns the pattern bound on each iteration.
    pub fn pattern(&self) -> &ResolvedConstPattern {
        &self.pattern
    }

    /// Returns the loop body.
    pub fn body(&self) -> &ResolvedConstBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved match expression and its ordered arms.
pub struct ResolvedConstMatch {
    span: Span,
    target: ResolvedConstExpr,
    arms: Vec<ResolvedConstMatchArm>,
}

impl ResolvedConstMatch {
    /// Creates a resolved match expression.
    pub fn new(span: Span, target: ResolvedConstExpr, arms: Vec<ResolvedConstMatchArm>) -> Self {
        Self { span, target, arms }
    }

    /// Returns the match source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the expression being matched.
    pub fn target(&self) -> &ResolvedConstExpr {
        &self.target
    }

    /// Returns arms in source order; first matching arm wins.
    pub fn arms(&self) -> &[ResolvedConstMatchArm] {
        &self.arms
    }
}

#[derive(Debug, Clone, PartialEq)]
/// One resolved match arm with one or more alternative patterns.
pub struct ResolvedConstMatchArm {
    span: Span,
    patterns: Vec<ResolvedConstPattern>,
    body: ResolvedConstMatchArmBody,
}

impl ResolvedConstMatchArm {
    /// Creates a match arm.
    pub fn new(
        span: Span,
        patterns: Vec<ResolvedConstPattern>,
        body: ResolvedConstMatchArmBody,
    ) -> Self {
        Self {
            span,
            patterns,
            body,
        }
    }

    /// Returns the arm source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns alternative patterns in source order.
    pub fn patterns(&self) -> &[ResolvedConstPattern] {
        &self.patterns
    }

    /// Returns the arm body.
    pub fn body(&self) -> &ResolvedConstMatchArmBody {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved pattern whose constructors carry authoritative semantic ids.
pub struct ResolvedConstPattern {
    kind: ResolvedConstPatternKind,
}

impl ResolvedConstPattern {
    /// Creates a wildcard pattern.
    pub fn wildcard(span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Wildcard { span },
        }
    }

    pub fn bind(name: SymbolId, local_id: LocalId, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Bind {
                name,
                local_id,
                span,
            },
        }
    }

    pub fn optional_some(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::OptionalSome {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn pointer(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Pointer {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn mut_pointer(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::MutPointer {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn optional_null(span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::OptionalNull { span },
        }
    }

    pub fn error_ok(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::ErrorOk {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn error_err(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::ErrorErr {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn tuple(patterns: Vec<ResolvedConstPattern>, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Tuple { patterns, span },
        }
    }

    pub fn enum_variant(
        variant: ResolvedConstExpr,
        fields: ConstEnumPatternFields<ResolvedConstPattern>,
        span: Span,
    ) -> Self {
        Self {
            kind: ResolvedConstPatternKind::EnumVariant {
                variant,
                fields,
                span,
            },
        }
    }

    pub fn struct_pattern(
        def_id: GlobalDefId,
        fields: Vec<ConstNamedPatternField<ResolvedConstPattern>>,
        rest: Option<Span>,
        span: Span,
    ) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Struct {
                def_id,
                fields,
                rest,
                span,
            },
        }
    }

    pub fn expr(expr: ResolvedConstExpr) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Expr(expr),
        }
    }

    pub fn range(
        start: ResolvedConstExpr,
        end: ResolvedConstExpr,
        inclusive: bool,
        span: Span,
    ) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Range {
                start,
                end,
                inclusive,
                span,
            },
        }
    }

    /// Returns the pattern constructor payload.
    pub fn kind(&self) -> &ResolvedConstPatternKind {
        &self.kind
    }

    /// Returns the source span carried by this pattern constructor.
    pub fn span(&self) -> Span {
        match &self.kind {
            ResolvedConstPatternKind::Wildcard { span }
            | ResolvedConstPatternKind::Bind { span, .. }
            | ResolvedConstPatternKind::Pointer { span, .. }
            | ResolvedConstPatternKind::MutPointer { span, .. }
            | ResolvedConstPatternKind::OptionalSome { span, .. }
            | ResolvedConstPatternKind::OptionalNull { span }
            | ResolvedConstPatternKind::ErrorOk { span, .. }
            | ResolvedConstPatternKind::ErrorErr { span, .. }
            | ResolvedConstPatternKind::Tuple { span, .. }
            | ResolvedConstPatternKind::EnumVariant { span, .. }
            | ResolvedConstPatternKind::Struct { span, .. }
            | ResolvedConstPatternKind::Range { span, .. } => *span,
            ResolvedConstPatternKind::Expr(expr) => expr.span(),
        }
    }
}

impl Default for ResolvedConstPattern {
    fn default() -> Self {
        Self {
            kind: ResolvedConstPatternKind::Wildcard {
                span: Span::new(0, 0),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved pattern constructors used by const matching.
pub enum ResolvedConstPatternKind {
    /// Matches every value.
    Wildcard {
        /// Pattern source span.
        span: Span,
    },
    /// Binds a resolved local to the matched value.
    Bind {
        /// Source-level binding name.
        name: SymbolId,
        /// Semantic local identity.
        local_id: LocalId,
        /// Pattern source span.
        span: Span,
    },
    /// Dereferences an immutable pointer before matching.
    Pointer {
        /// Nested pointee pattern.
        pattern: Box<ResolvedConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Dereferences a mutable pointer before matching.
    MutPointer {
        /// Nested pointee pattern.
        pattern: Box<ResolvedConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Matches an optional payload.
    OptionalSome {
        /// Nested payload pattern.
        pattern: Box<ResolvedConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Matches an absent optional value.
    OptionalNull {
        /// Pattern source span.
        span: Span,
    },
    /// Matches an error-union success payload.
    ErrorOk {
        /// Nested success pattern.
        pattern: Box<ResolvedConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Matches an error-union error payload.
    ErrorErr {
        /// Nested error pattern.
        pattern: Box<ResolvedConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Matches tuple fields positionally.
    Tuple {
        /// Nested field patterns.
        patterns: Vec<ResolvedConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Matches a resolved enum variant and its payload fields.
    EnumVariant {
        /// Variant identity expression.
        variant: ResolvedConstExpr,
        /// Tuple or named payload patterns.
        fields: ConstEnumPatternFields<ResolvedConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Matches fields of a nominal struct.
    Struct {
        /// Struct definition identity.
        def_id: GlobalDefId,
        /// Named field patterns.
        fields: Vec<ConstNamedPatternField<ResolvedConstPattern>>,
        /// Optional rest-pattern span.
        rest: Option<Span>,
        /// Pattern source span.
        span: Span,
    },
    /// Matches by evaluating an equality expression.
    Expr(ResolvedConstExpr),
    /// Matches an integer interval.
    Range {
        /// Lower bound.
        start: ResolvedConstExpr,
        /// Upper bound.
        end: ResolvedConstExpr,
        /// Whether the upper bound is inclusive.
        inclusive: bool,
        /// Pattern source span.
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Body of one resolved match arm.
pub struct ResolvedConstMatchArmBody {
    kind: ResolvedConstMatchArmBodyKind,
}

impl ResolvedConstMatchArmBody {
    /// Creates an expression arm body.
    pub fn expr(expr: ResolvedConstExpr) -> Self {
        Self {
            kind: ResolvedConstMatchArmBodyKind::Expr(expr),
        }
    }

    pub fn stmt(stmt: ResolvedConstStmt) -> Self {
        Self {
            kind: ResolvedConstMatchArmBodyKind::Stmt(Box::new(stmt)),
        }
    }

    pub fn block(block: ResolvedConstBlock) -> Self {
        Self {
            kind: ResolvedConstMatchArmBodyKind::Block(block),
        }
    }

    /// Returns the arm body payload.
    pub fn kind(&self) -> &ResolvedConstMatchArmBodyKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved match arm body forms.
pub enum ResolvedConstMatchArmBodyKind {
    /// Value-producing expression.
    Expr(ResolvedConstExpr),
    /// Statement whose control flow determines arm completion.
    Stmt(Box<ResolvedConstStmt>),
    /// Lexical block body.
    Block(ResolvedConstBlock),
}

#[derive(Debug, Clone, PartialEq)]
/// Expression forms accepted after the early-to-resolved validation boundary.
///
/// Optional literal type ids represent contextual inference, not unresolved
/// semantic facts. Required types such as casts and builtin type arguments use
/// non-optional resolved ids in their owning nodes.
pub enum ResolvedConstExprKind {
    /// Integer literal text, retaining source spelling until typing.
    Integer(String),
    /// Unicode scalar literal text.
    Char(String),
    /// Byte character literal text.
    ByteChar(String),
    /// Floating-point literal text.
    Float(String),
    /// Segmented string literal.
    String(ConstStringLiteral),
    /// Segmented byte-string literal.
    ByteString(ConstStringLiteral),
    /// Boolean literal.
    Bool(bool),
    /// Null optional literal.
    Null,
    /// Name with authoritative semantic resolution.
    Name(ConstNameResolution),
    /// Named field projection.
    Field {
        /// Aggregate expression.
        lhs: Box<ResolvedConstExpr>,
        /// Field identity.
        name: SymbolId,
    },
    /// Method reference on a receiver expression.
    Method {
        /// Receiver expression.
        receiver: Box<ResolvedConstExpr>,
        /// Method name.
        name: SymbolId,
    },
    /// Associated function reference with resolved target.
    AssociatedFunction {
        /// Type or nominal target.
        target: ResolvedConstAssociatedTarget,
        /// Function name.
        name: SymbolId,
    },
    /// Indexed projection.
    Index {
        /// Indexed aggregate.
        lhs: Box<ResolvedConstExpr>,
        /// Index expression.
        index: Box<ResolvedConstExpr>,
    },
    /// Slice projection with optional bounds.
    Slice {
        /// Sliced aggregate.
        lhs: Box<ResolvedConstExpr>,
        /// Slice bounds.
        range: ResolvedConstSliceRange,
    },
    /// Tuple literal.
    Tuple(Vec<ResolvedConstExpr>),
    /// Positional tuple-field projection.
    TupleField {
        /// Tuple expression.
        lhs: Box<ResolvedConstExpr>,
        /// Zero-based field index.
        index: usize,
    },
    /// Array literal or repeat form.
    ArrayLiteral {
        /// Element payload.
        elems: ResolvedConstArrayElements,
    },
    StructLiteral {
        /// Nominal construction is encoded in the IR itself: a struct value
        /// can never rely on an expected type to acquire its identity.
        ty: InternedTyId,
        fields: Vec<ResolvedConstFieldInit>,
    },
    /// Positional nominal construction lowered from `Type(value, ...)`.
    /// Generic arguments remain attached to the constructor so const checking
    /// can instantiate field types before validating the positional values.
    TupleStructLiteral {
        /// Nominal tuple-struct identity.
        def_id: GlobalDefId,
        /// Type and const arguments retained for instantiation.
        generic_args: Vec<ResolvedConstGenericArg>,
        /// Positional field initializers.
        fields: Vec<ResolvedConstFieldInit>,
    },
    /// Enum variant with named payload fields.
    EnumStructLiteral {
        /// Variant expression.
        variant: Box<ResolvedConstExpr>,
        /// Payload field initializers.
        fields: Vec<ResolvedConstFieldInit>,
    },
    /// Emits a compile-time diagnostic using an evaluated message.
    CompileError {
        /// Message expression.
        message: Box<ResolvedConstExpr>,
    },
    /// Explicit compile-time trap.
    Trap,
    /// Builtin compile-time value.
    BuiltinConstValue(BuiltinConstValue),
    /// Builtin runtime value query.
    BuiltinValue(ValueBuiltin),
    /// Layout builtin over a resolved type argument.
    LayoutBuiltin {
        /// Layout operation.
        builtin: LayoutBuiltin,
        /// Required resolved type argument.
        type_arg: ResolvedConstTypeArg,
    },
    /// Field-offset builtin over a resolved type argument.
    FieldOffsetBuiltin {
        /// Required resolved type argument.
        type_arg: ResolvedConstTypeArg,
        /// Field identity.
        field: SymbolId,
    },
    /// Embedded resource path.
    Embed {
        /// Segmented path literal.
        path: ConstStringLiteral,
    },
    /// Const function call.
    Call {
        /// Callee expression.
        callee: Box<ResolvedConstExpr>,
        /// Generic arguments retained for inference/instantiation.
        generic_args: Vec<ResolvedConstGenericArg>,
        /// Argument expressions in call order.
        args: Vec<ResolvedConstExpr>,
    },
    /// Unary operation.
    Unary {
        /// Operator.
        op: ConstUnaryOp,
        /// Operand.
        expr: Box<ResolvedConstExpr>,
    },
    /// Optional success constructor.
    OptionalSome {
        /// Payload expression.
        expr: Box<ResolvedConstExpr>,
    },
    /// Error-union success constructor.
    ErrorOk {
        /// Payload expression.
        expr: Box<ResolvedConstExpr>,
    },
    /// Error-union error constructor.
    ErrorErr {
        /// Payload expression.
        expr: Box<ResolvedConstExpr>,
    },
    /// Error propagation expression.
    Try {
        /// Fallible expression.
        expr: Box<ResolvedConstExpr>,
    },
    /// Binary operation.
    Binary {
        /// Left operand.
        lhs: Box<ResolvedConstExpr>,
        /// Operator.
        op: ConstBinaryOp,
        /// Right operand.
        rhs: Box<ResolvedConstExpr>,
    },
    /// Assignment expression.
    Assign(Box<ResolvedConstAssign>),
    /// Integer range expression.
    Range(ResolvedConstRange),
    /// Conditional expression.
    If {
        /// Condition.
        cond: Box<ResolvedConstExpr>,
        /// True branch.
        then_branch: ResolvedConstBlock,
        /// Optional false branch.
        else_branch: Option<Box<ResolvedConstExpr>>,
    },
    /// Match expression.
    Match(Box<ResolvedConstMatch>),
    /// Explicit cast to a resolved runtime type.
    Cast {
        /// Operand expression.
        expr: Box<ResolvedConstExpr>,
        /// Target type identity.
        ty: InternedTyId,
    },
    /// Lexical block expression.
    Block(ResolvedConstBlock),
}

#[derive(Debug, Clone, PartialEq)]
/// Optional-bound integer range expression.
pub struct ResolvedConstRange {
    start: Option<Box<ResolvedConstExpr>>,
    end: Option<Box<ResolvedConstExpr>>,
    inclusive: bool,
}

impl ResolvedConstRange {
    /// Creates a range with optional lower/upper expressions.
    pub fn new(
        start: Option<Box<ResolvedConstExpr>>,
        end: Option<Box<ResolvedConstExpr>>,
        inclusive: bool,
    ) -> Self {
        Self {
            start,
            end,
            inclusive,
        }
    }

    pub fn start(&self) -> Option<&ResolvedConstExpr> {
        self.start.as_deref()
    }

    pub fn end(&self) -> Option<&ResolvedConstExpr> {
        self.end.as_deref()
    }

    pub fn is_inclusive(&self) -> bool {
        self.inclusive
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Optional-bound slice range expression.
pub struct ResolvedConstSliceRange {
    start: Option<Box<ResolvedConstExpr>>,
    end: Option<Box<ResolvedConstExpr>>,
    inclusive: bool,
}

impl ResolvedConstSliceRange {
    /// Creates a slice range with optional lower/upper expressions.
    pub fn new(
        start: Option<Box<ResolvedConstExpr>>,
        end: Option<Box<ResolvedConstExpr>>,
        inclusive: bool,
    ) -> Self {
        Self {
            start,
            end,
            inclusive,
        }
    }

    pub fn start(&self) -> Option<&ResolvedConstExpr> {
        self.start.as_deref()
    }

    pub fn end(&self) -> Option<&ResolvedConstExpr> {
        self.end.as_deref()
    }

    pub fn is_inclusive(&self) -> bool {
        self.inclusive
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Array literal payload preserving list versus repeat syntax.
pub struct ResolvedConstArrayElements {
    kind: ResolvedConstArrayElementsKind,
}

impl ResolvedConstArrayElements {
    /// Creates an explicit element list.
    pub fn list(elems: Vec<ResolvedConstExpr>) -> Self {
        Self {
            kind: ResolvedConstArrayElementsKind::List(elems),
        }
    }

    pub fn repeat(value: ResolvedConstExpr, count: ResolvedConstExpr) -> Self {
        Self {
            kind: ResolvedConstArrayElementsKind::Repeat {
                value: Box::new(value),
                count: Box::new(count),
            },
        }
    }

    pub fn kind(&self) -> &ResolvedConstArrayElementsKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Array literal payload variants.
pub enum ResolvedConstArrayElementsKind {
    /// Explicit elements in source order.
    List(Vec<ResolvedConstExpr>),
    /// One value evaluated with a repeat count.
    Repeat {
        /// Repeated value expression.
        value: Box<ResolvedConstExpr>,
        /// Count expression.
        count: Box<ResolvedConstExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Named field initializer in an aggregate literal.
pub struct ResolvedConstFieldInit {
    span: Span,
    name: SymbolId,
    value: ResolvedConstExpr,
}

impl ResolvedConstFieldInit {
    /// Creates a field initializer.
    pub fn new(span: Span, name: SymbolId, value: ResolvedConstExpr) -> Self {
        Self { span, name, value }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn name(&self) -> SymbolId {
        self.name
    }

    pub fn name_symbol(&self) -> &SymbolId {
        &self.name
    }

    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstTypeArg {
    span: Span,
    ty_span: Span,
    ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedConstGenericArg {
    Type(ResolvedConstTypeArg),
    Const(ResolvedConstExpr),
}

impl ResolvedConstGenericArg {
    pub fn span(&self) -> Span {
        match self {
            Self::Type(arg) => arg.span(),
            Self::Const(expr) => expr.span(),
        }
    }
}

impl ResolvedConstTypeArg {
    pub fn new(span: Span, ty_span: Span, ty: InternedTyId) -> Self {
        Self { span, ty_span, ty }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn ty_span(&self) -> Span {
        self.ty_span
    }

    pub fn ty(&self) -> InternedTyId {
        self.ty
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Syntax-oriented const function IR that may still lack semantic identities.
pub struct EarlyConstFunction {
    pub span: Span,
    pub params: Vec<EarlyConstParam>,
    pub body: EarlyConstBlock,
}

#[derive(Debug, Clone, PartialEq)]
/// An early function parameter. The outer type option records whether syntax
/// supplied a type; the inner type id may remain unresolved until validation.
pub struct EarlyConstParam {
    pub span: Span,
    pub name: SymbolId,
    pub local_id: Option<LocalId>,
    pub ty: Option<EarlyConstTypeArg>,
    pub receiver: Option<nia_ids::ReceiverKind>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstBlock {
    pub span: Span,
    pub stmts: Vec<EarlyConstStmt>,
    pub tail: Option<Box<EarlyConstExpr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstStmt {
    pub span: Span,
    pub kind: EarlyConstStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstStmtKind {
    Binding(EarlyConstBinding),
    PatternBinding(Box<EarlyConstPatternBinding>),
    Expr(EarlyConstExpr),
    Return(Option<EarlyConstExpr>),
    Break,
    Continue,
    If {
        cond: EarlyConstExpr,
        then_branch: EarlyConstBlock,
        else_branch: Option<EarlyConstBlock>,
    },
    ForIn(Box<EarlyConstForIn>),
    While {
        cond: EarlyConstExpr,
        body: EarlyConstBlock,
    },
    Loop {
        body: EarlyConstBlock,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// The early form of a destructuring local binding in a const function.
pub struct EarlyConstPatternBinding {
    pub span: Span,
    pub pattern: EarlyConstPattern,
    pub explicit_type: Option<EarlyConstTypeArg>,
    pub is_mutable: bool,
    pub value: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
/// An early local binding whose explicit annotation, when present in syntax,
/// remains distinguishable from a binding that relies on inference.
pub struct EarlyConstBinding {
    pub span: Span,
    pub name: SymbolId,
    pub local_id: Option<LocalId>,
    pub explicit_type: Option<EarlyConstTypeArg>,
    pub is_mutable: bool,
    pub value: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstAssign {
    pub lhs: EarlyConstAssignTarget,
    pub op: ConstAssignOp,
    pub rhs: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstAssignTarget {
    Local {
        span: Span,
        name: SymbolId,
        local_id: Option<LocalId>,
        path: Vec<EarlyConstAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstAssignPathElem {
    Field { span: Span, name: SymbolId },
    Index { span: Span, index: EarlyConstExpr },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstForIn {
    pub pattern: EarlyConstPattern,
    pub iter: EarlyConstExpr,
    pub body: EarlyConstBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstMatch {
    pub span: Span,
    pub target: EarlyConstExpr,
    pub arms: Vec<EarlyConstMatchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstMatchArm {
    pub span: Span,
    pub patterns: Vec<EarlyConstPattern>,
    pub body: EarlyConstMatchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstPattern {
    Wildcard {
        span: Span,
    },
    Bind {
        name: SymbolId,
        local_id: Option<LocalId>,
        span: Span,
    },
    Pointer {
        pattern: Box<EarlyConstPattern>,
        span: Span,
    },
    MutPointer {
        pattern: Box<EarlyConstPattern>,
        span: Span,
    },
    OptionalSome {
        pattern: Box<EarlyConstPattern>,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        pattern: Box<EarlyConstPattern>,
        span: Span,
    },
    ErrorErr {
        pattern: Box<EarlyConstPattern>,
        span: Span,
    },
    Tuple {
        patterns: Vec<EarlyConstPattern>,
        span: Span,
    },
    EnumVariant {
        variant: EarlyConstExpr,
        fields: ConstEnumPatternFields<EarlyConstPattern>,
        span: Span,
    },
    Struct {
        def_id: GlobalDefId,
        fields: Vec<ConstNamedPatternField<EarlyConstPattern>>,
        rest: Option<Span>,
        span: Span,
    },
    Expr(EarlyConstExpr),
    Range {
        start: EarlyConstExpr,
        end: EarlyConstExpr,
        inclusive: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstEnumPatternFields<P> {
    Tuple(Vec<P>),
    Named {
        fields: Vec<ConstNamedPatternField<P>>,
        rest: Option<Span>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstNamedPatternField<P> {
    pub name: SymbolId,
    pub pattern: P,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstMatchArmBody {
    Expr(EarlyConstExpr),
    Stmt(Box<EarlyConstStmt>),
    Block(EarlyConstBlock),
}

#[derive(Debug, Clone, PartialEq)]
/// A const expression produced by syntax lowering before identity validation.
pub struct EarlyConstExpr {
    pub span: Span,
    pub kind: EarlyConstExprKind,
}

impl EarlyConstExpr {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &EarlyConstExprKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Preserves a display symbol even when semantic name resolution has not run.
pub enum EarlyConstName {
    Unresolved(SymbolId),
    Resolved {
        display: SymbolId,
        resolution: ConstNameResolution,
    },
}

impl EarlyConstName {
    pub fn unresolved(display: SymbolId) -> Self {
        Self::Unresolved(display)
    }

    pub fn resolved(display: SymbolId, resolution: ConstNameResolution) -> Self {
        Self::Resolved {
            display,
            resolution,
        }
    }

    pub fn display(&self) -> SymbolId {
        match self {
            Self::Unresolved(display) | Self::Resolved { display, .. } => *display,
        }
    }

    pub fn resolution(&self) -> Option<ConstNameResolution> {
        match self {
            Self::Unresolved(_) => None,
            Self::Resolved { resolution, .. } => Some(resolution.clone()),
        }
    }

    pub(crate) fn into_resolution(
        self,
        span: Span,
    ) -> Result<ConstNameResolution, ConstLowerError> {
        match self {
            Self::Resolved { resolution, .. } => Ok(resolution),
            Self::Unresolved(_) => Err(unresolved_error(span, "const name")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Const-capable syntax forms with optional semantic ids where early lowering
/// is allowed to proceed before name and type analysis completes.
///
/// Aggregate literal types use `Option<EarlyConstTypeArg>` so `None` means the
/// source omitted a type, while a present type argument may independently carry
/// a not-yet-resolved type id.
pub enum EarlyConstExprKind {
    Integer(String),
    Char(String),
    ByteChar(String),
    Float(String),
    String(ConstStringLiteral),
    ByteString(ConstStringLiteral),
    Bool(bool),
    Null,
    Ident(EarlyConstName),
    Qualified(EarlyConstName),
    Field {
        lhs: Box<EarlyConstExpr>,
        name: SymbolId,
    },
    Method {
        receiver: Box<EarlyConstExpr>,
        name: SymbolId,
    },
    AssociatedFunction {
        target: EarlyConstAssociatedTarget,
        name: SymbolId,
    },
    Index {
        lhs: Box<EarlyConstExpr>,
        index: Box<EarlyConstExpr>,
    },
    Slice {
        lhs: Box<EarlyConstExpr>,
        range: EarlyConstSliceRange,
    },
    Tuple(Vec<EarlyConstExpr>),
    TupleField {
        lhs: Box<EarlyConstExpr>,
        index: usize,
    },
    ArrayLiteral {
        elems: EarlyConstArrayElements,
    },
    StructLiteral {
        /// The source syntax names every constructed aggregate. Resolution
        /// may still fail inside this type argument, but it is never absent.
        ty: EarlyConstTypeArg,
        fields: Vec<EarlyConstFieldInit>,
    },
    /// Positional nominal construction lowered from `Type(value, ...)`.
    /// Keep the early generic arguments until the semantic resolution pass;
    /// type-vs-const interpretation is still supplied by semantic facts.
    TupleStructLiteral {
        def_id: GlobalDefId,
        generic_args: Vec<EarlyConstGenericArg>,
        fields: Vec<EarlyConstFieldInit>,
    },
    EnumStructLiteral {
        variant: Box<EarlyConstExpr>,
        fields: Vec<EarlyConstFieldInit>,
    },
    CompileError {
        message: Box<EarlyConstExpr>,
    },
    Trap,
    BuiltinConstValue(BuiltinConstValue),
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: EarlyConstTypeArg,
    },
    FieldOffsetBuiltin {
        type_arg: EarlyConstTypeArg,
        field: SymbolId,
    },
    Embed {
        path: ConstStringLiteral,
    },
    Call {
        callee: Box<EarlyConstExpr>,
        generic_args: Vec<EarlyConstGenericArg>,
        args: Vec<EarlyConstExpr>,
    },
    Unary {
        op: ConstUnaryOp,
        expr: Box<EarlyConstExpr>,
    },
    OptionalSome {
        expr: Box<EarlyConstExpr>,
    },
    ErrorOk {
        expr: Box<EarlyConstExpr>,
    },
    ErrorErr {
        expr: Box<EarlyConstExpr>,
    },
    Try {
        expr: Box<EarlyConstExpr>,
    },
    Binary {
        lhs: Box<EarlyConstExpr>,
        op: ConstBinaryOp,
        rhs: Box<EarlyConstExpr>,
    },
    Assign(Box<EarlyConstAssign>),
    Range(EarlyConstRange),
    If {
        cond: Box<EarlyConstExpr>,
        then_branch: EarlyConstBlock,
        else_branch: Option<Box<EarlyConstExpr>>,
    },
    Match(Box<EarlyConstMatch>),
    Cast {
        expr: Box<EarlyConstExpr>,
        ty: Option<InternedTyId>,
    },
    Block(EarlyConstBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstStringLiteral {
    pub parts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstUnaryOp {
    Neg,
    Not,
    BitNot,
    RefReadOnly,
    Ref,
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstBinaryOp {
    Mul,
    Div,
    Rem,
    Add,
    Sub,
    Shl,
    Shr,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitAnd,
    BitXor,
    BitOr,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstAssignOp {
    Assign,
    Add,
    Sub,
    Shl,
    Shr,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitXor,
    BitOr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstRange {
    pub start: Option<Box<EarlyConstExpr>>,
    pub end: Option<Box<EarlyConstExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstSliceRange {
    pub start: Option<Box<EarlyConstExpr>>,
    pub end: Option<Box<EarlyConstExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstArrayElements {
    List(Vec<EarlyConstExpr>),
    Repeat {
        value: Box<EarlyConstExpr>,
        count: Box<EarlyConstExpr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstNameResolution {
    Local(LocalId),
    Global(GlobalDefId),
    GenericParam(SymbolId),
    BuiltinAssociatedValue(BuiltinAssociatedValue),
    AssociatedConstProjection(AssociatedConstProjection),
}

impl From<SemanticValueUse> for ConstNameResolution {
    fn from(value: SemanticValueUse) -> Self {
        match value {
            SemanticValueUse::Local(local_id) => Self::Local(local_id),
            SemanticValueUse::Global(global_id) => Self::Global(global_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstFieldInit {
    pub span: Span,
    pub name: SymbolId,
    pub value: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
/// A type occurrence carried through early lowering.
///
/// `ty == None` means semantic type identity is not available yet. This differs
/// from an untyped aggregate literal, whose optional type lives on the literal
/// expression itself and intentionally survives into resolved IR for inference.
pub struct EarlyConstTypeArg {
    pub span: Span,
    pub ty_span: Span,
    pub ty: Option<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstGenericArg {
    Type(EarlyConstTypeArg),
    Const(EarlyConstExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstAssociatedTarget {
    Type(EarlyConstTypeArg),
    Nominal {
        def_id: GlobalDefId,
        args: Vec<EarlyConstTypeArg>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedConstAssociatedTarget {
    Type(ResolvedConstTypeArg),
    Nominal {
        def_id: GlobalDefId,
        args: Vec<ResolvedConstTypeArg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A structural failure while lowering syntax or validating early IR.
pub struct ConstLowerError {
    pub span: Span,
    pub message: String,
}
