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

    /// Creates an indexed projection.
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

    /// Creates a binding pattern with a resolved local id.
    pub fn bind(name: SymbolId, local_id: LocalId, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Bind {
                name,
                local_id,
                span,
            },
        }
    }

    /// Creates an optional-some pattern.
    pub fn optional_some(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::OptionalSome {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    /// Creates an immutable pointer pattern.
    pub fn pointer(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Pointer {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    /// Creates a mutable pointer pattern.
    pub fn mut_pointer(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::MutPointer {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    /// Creates an optional-null pattern.
    pub fn optional_null(span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::OptionalNull { span },
        }
    }

    /// Creates an error-union success pattern.
    pub fn error_ok(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::ErrorOk {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    /// Creates an error-union error pattern.
    pub fn error_err(pattern: ResolvedConstPattern, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::ErrorErr {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    /// Creates a tuple pattern.
    pub fn tuple(patterns: Vec<ResolvedConstPattern>, span: Span) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Tuple { patterns, span },
        }
    }

    /// Creates an enum-variant pattern with tuple or named fields.
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

    /// Creates a nominal struct pattern and optional rest marker.
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

    /// Creates an expression-equality pattern.
    pub fn expr(expr: ResolvedConstExpr) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Expr(expr),
        }
    }

    /// Creates an integer range pattern.
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

    /// Creates a statement arm body.
    pub fn stmt(stmt: ResolvedConstStmt) -> Self {
        Self {
            kind: ResolvedConstMatchArmBodyKind::Stmt(Box::new(stmt)),
        }
    }

    /// Creates a block arm body.
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
    /// Nominal struct construction.
    /// Nominal struct construction.
    StructLiteral {
        /// Nominal construction is encoded in the IR itself: a struct value
        /// can never rely on an expected type to acquire its identity.
        ty: InternedTyId,
        /// Field initializers.
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

    /// Returns the lower bound expression.
    pub fn start(&self) -> Option<&ResolvedConstExpr> {
        self.start.as_deref()
    }

    /// Returns the upper bound expression.
    pub fn end(&self) -> Option<&ResolvedConstExpr> {
        self.end.as_deref()
    }

    /// Returns whether the upper bound is inclusive.
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

    /// Returns the lower bound expression.
    pub fn start(&self) -> Option<&ResolvedConstExpr> {
        self.start.as_deref()
    }

    /// Returns the upper bound expression.
    pub fn end(&self) -> Option<&ResolvedConstExpr> {
        self.end.as_deref()
    }

    /// Returns whether the upper bound is inclusive.
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

    /// Creates a repeated element/count form.
    pub fn repeat(value: ResolvedConstExpr, count: ResolvedConstExpr) -> Self {
        Self {
            kind: ResolvedConstArrayElementsKind::Repeat {
                value: Box::new(value),
                count: Box::new(count),
            },
        }
    }

    /// Returns the list or repeat payload.
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

    /// Returns the initializer source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the field identity.
    pub fn name(&self) -> SymbolId {
        self.name
    }

    /// Returns the field identity by reference.
    pub fn name_symbol(&self) -> &SymbolId {
        &self.name
    }

    /// Returns the initializer expression.
    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
/// A resolved type argument whose identity is guaranteed to be interned.
pub struct ResolvedConstTypeArg {
    /// Full source span of the type occurrence.
    span: Span,
    /// Span of the type syntax used in diagnostics.
    ty_span: Span,
    /// Interned runtime type identity.
    ty: InternedTyId,
}

#[derive(Debug, Clone, PartialEq)]
/// A resolved generic argument, retaining type versus const kind.
pub enum ResolvedConstGenericArg {
    /// Generic argument explicitly left for call-site inference with `_`.
    Infer(Span),
    /// Resolved type argument.
    Type(ResolvedConstTypeArg),
    /// Resolved const expression argument.
    Const(ResolvedConstExpr),
}

impl ResolvedConstGenericArg {
    /// Returns the source span of either generic argument form.
    pub fn span(&self) -> Span {
        match self {
            Self::Infer(span) => *span,
            Self::Type(arg) => arg.span(),
            Self::Const(expr) => expr.span(),
        }
    }
}

impl ResolvedConstTypeArg {
    /// Creates a resolved type argument.
    pub fn new(span: Span, ty_span: Span, ty: InternedTyId) -> Self {
        Self { span, ty_span, ty }
    }

    /// Returns the full source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the type syntax span.
    pub fn ty_span(&self) -> Span {
        self.ty_span
    }

    /// Returns the interned type identity.
    pub fn ty(&self) -> InternedTyId {
        self.ty
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Syntax-oriented const function IR that may still lack semantic identities.
pub struct EarlyConstFunction {
    /// Function declaration span.
    pub span: Span,
    /// Parameters in source/call order.
    pub params: Vec<EarlyConstParam>,
    /// Function body.
    pub body: EarlyConstBlock,
}

#[derive(Debug, Clone, PartialEq)]
/// An early function parameter. The outer type option records whether syntax
/// supplied a type; the inner type id may remain unresolved until validation.
pub struct EarlyConstParam {
    /// Parameter declaration span.
    pub span: Span,
    /// Source-level parameter name.
    pub name: SymbolId,
    /// Optional local identity from local resolution.
    pub local_id: Option<LocalId>,
    /// Optional syntax type argument, possibly unresolved.
    pub ty: Option<EarlyConstTypeArg>,
    /// Receiver passing mode, when this parameter is a receiver.
    pub receiver: Option<nia_ids::ReceiverKind>,
}

#[derive(Debug, Clone, PartialEq)]
/// A lexical early const block before identity validation.
pub struct EarlyConstBlock {
    /// Block source span.
    pub span: Span,
    /// Statements in evaluation order.
    pub stmts: Vec<EarlyConstStmt>,
    /// Optional value-producing tail expression.
    pub tail: Option<Box<EarlyConstExpr>>,
}

#[derive(Debug, Clone, PartialEq)]
/// One early statement with source-order payload.
pub struct EarlyConstStmt {
    /// Statement source span.
    pub span: Span,
    /// Statement payload.
    pub kind: EarlyConstStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
/// Early statement forms retained until resolution.
pub enum EarlyConstStmtKind {
    /// Named local binding.
    Binding(EarlyConstBinding),
    /// Destructuring local binding.
    PatternBinding(Box<EarlyConstPatternBinding>),
    /// Expression statement.
    Expr(EarlyConstExpr),
    /// Return from the current const function.
    Return(Option<EarlyConstExpr>),
    /// Exit the nearest loop.
    Break,
    /// Continue the nearest loop.
    Continue,
    /// Conditional statement.
    If {
        /// Condition expression.
        cond: EarlyConstExpr,
        /// True branch.
        then_branch: EarlyConstBlock,
        /// Optional false branch.
        else_branch: Option<EarlyConstBlock>,
    },
    /// Iterator loop.
    ForIn(Box<EarlyConstForIn>),
    /// Condition-controlled loop.
    While {
        /// Loop condition.
        cond: EarlyConstExpr,
        /// Loop body.
        body: EarlyConstBlock,
    },
    /// Unconditional loop.
    Loop {
        /// Loop body.
        body: EarlyConstBlock,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// The early form of a destructuring local binding in a const function.
pub struct EarlyConstPatternBinding {
    /// Binding source span.
    pub span: Span,
    /// Early destructuring pattern.
    pub pattern: EarlyConstPattern,
    /// Optional explicit type syntax.
    pub explicit_type: Option<EarlyConstTypeArg>,
    /// Whether every leaf is mutable.
    pub is_mutable: bool,
    /// Value expression matched by the pattern.
    pub value: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
/// An early local binding whose explicit annotation, when present in syntax,
/// remains distinguishable from a binding that relies on inference.
pub struct EarlyConstBinding {
    /// Binding source span.
    pub span: Span,
    /// Source-level binding name.
    pub name: SymbolId,
    /// Optional local identity.
    pub local_id: Option<LocalId>,
    /// Optional explicit type syntax.
    pub explicit_type: Option<EarlyConstTypeArg>,
    /// Whether the local is mutable.
    pub is_mutable: bool,
    /// Initializer expression.
    pub value: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
/// Early assignment target and operation.
pub struct EarlyConstAssign {
    /// Assignment target.
    pub lhs: EarlyConstAssignTarget,
    /// Assignment operator.
    pub op: ConstAssignOp,
    /// Right-hand expression.
    pub rhs: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
/// Early assignment root and optional projection path.
pub enum EarlyConstAssignTarget {
    /// Local root plus root-to-leaf projection path.
    Local {
        /// Target source span.
        span: Span,
        /// Source-level local name.
        name: SymbolId,
        /// Optional local identity.
        local_id: Option<LocalId>,
        /// Projections ordered root to leaf.
        path: Vec<EarlyConstAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Early assignment projection.
pub enum EarlyConstAssignPathElem {
    /// Named field projection.
    Field {
        /// Projection source span.
        span: Span,
        /// Field identity.
        name: SymbolId,
    },
    /// Indexed projection.
    Index {
        /// Projection source span.
        span: Span,
        /// Index expression.
        index: EarlyConstExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Early iterator loop payload.
pub struct EarlyConstForIn {
    /// Pattern bound for each yielded value.
    pub pattern: EarlyConstPattern,
    /// Iterable expression.
    pub iter: EarlyConstExpr,
    /// Loop body.
    pub body: EarlyConstBlock,
}

#[derive(Debug, Clone, PartialEq)]
/// Early match expression payload.
pub struct EarlyConstMatch {
    /// Match source span.
    pub span: Span,
    /// Target expression.
    pub target: EarlyConstExpr,
    /// Arms in source order.
    pub arms: Vec<EarlyConstMatchArm>,
}

#[derive(Debug, Clone, PartialEq)]
/// Early match arm payload.
pub struct EarlyConstMatchArm {
    /// Arm source span.
    pub span: Span,
    /// Alternative patterns.
    pub patterns: Vec<EarlyConstPattern>,
    /// Arm body.
    pub body: EarlyConstMatchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
/// Early pattern constructors before local/type resolution.
pub enum EarlyConstPattern {
    /// Matches every value.
    Wildcard {
        /// Pattern source span.
        span: Span,
    },
    /// Binds a value to an optional local identity.
    Bind {
        /// Source-level binding name.
        name: SymbolId,
        /// Optional semantic local identity.
        local_id: Option<LocalId>,
        /// Pattern source span.
        span: Span,
    },
    /// Immutable pointer pattern.
    Pointer {
        /// Nested pointee pattern.
        pattern: Box<EarlyConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Mutable pointer pattern.
    MutPointer {
        /// Nested pointee pattern.
        pattern: Box<EarlyConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Optional payload pattern.
    OptionalSome {
        /// Nested payload pattern.
        pattern: Box<EarlyConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Optional-null pattern.
    OptionalNull {
        /// Pattern source span.
        span: Span,
    },
    /// Error-union success pattern.
    ErrorOk {
        /// Nested success pattern.
        pattern: Box<EarlyConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Error-union error pattern.
    ErrorErr {
        /// Nested error pattern.
        pattern: Box<EarlyConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Tuple pattern.
    Tuple {
        /// Nested field patterns.
        patterns: Vec<EarlyConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Enum variant pattern.
    EnumVariant {
        /// Variant expression.
        variant: EarlyConstExpr,
        /// Tuple or named payload fields.
        fields: ConstEnumPatternFields<EarlyConstPattern>,
        /// Pattern source span.
        span: Span,
    },
    /// Nominal struct pattern.
    Struct {
        /// Struct definition identity.
        def_id: GlobalDefId,
        /// Named fields.
        fields: Vec<ConstNamedPatternField<EarlyConstPattern>>,
        /// Optional rest-pattern span.
        rest: Option<Span>,
        /// Pattern source span.
        span: Span,
    },
    /// Expression-equality pattern.
    Expr(EarlyConstExpr),
    /// Integer range pattern.
    Range {
        /// Lower bound.
        start: EarlyConstExpr,
        /// Upper bound.
        end: EarlyConstExpr,
        /// Whether the upper bound is inclusive.
        inclusive: bool,
        /// Pattern source span.
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Tuple or named enum payload patterns.
pub enum ConstEnumPatternFields<P> {
    /// Positional payload patterns.
    Tuple(Vec<P>),
    /// Named payload patterns and optional rest marker.
    Named {
        /// Named fields.
        fields: Vec<ConstNamedPatternField<P>>,
        /// Optional rest-pattern span.
        rest: Option<Span>,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Named pattern field shared by early and resolved IR.
pub struct ConstNamedPatternField<P> {
    /// Field identity.
    pub name: SymbolId,
    /// Nested field pattern.
    pub pattern: P,
    /// Field pattern span.
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
/// Early match arm body forms.
pub enum EarlyConstMatchArmBody {
    /// Value-producing expression.
    Expr(EarlyConstExpr),
    /// Statement body.
    Stmt(Box<EarlyConstStmt>),
    /// Lexical block body.
    Block(EarlyConstBlock),
}

#[derive(Debug, Clone, PartialEq)]
/// A const expression produced by syntax lowering before identity validation.
pub struct EarlyConstExpr {
    /// Expression source span.
    pub span: Span,
    /// Early expression payload.
    pub kind: EarlyConstExprKind,
}

impl EarlyConstExpr {
    /// Returns the expression source span.
    pub fn span(&self) -> Span {
        self.span
    }

    /// Returns the early expression payload.
    pub fn kind(&self) -> &EarlyConstExprKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Preserves a display symbol even when semantic name resolution has not run.
pub enum EarlyConstName {
    /// Name whose semantic resolution has not run.
    Unresolved(SymbolId),
    /// Name carrying an authoritative semantic resolution.
    Resolved {
        /// Source-level display name.
        display: SymbolId,
        /// Semantic identity.
        resolution: ConstNameResolution,
    },
}

impl EarlyConstName {
    /// Creates an unresolved name.
    pub fn unresolved(display: SymbolId) -> Self {
        Self::Unresolved(display)
    }

    /// Creates a resolved name with its display spelling.
    pub fn resolved(display: SymbolId, resolution: ConstNameResolution) -> Self {
        Self::Resolved {
            display,
            resolution,
        }
    }

    /// Returns the display symbol.
    pub fn display(&self) -> SymbolId {
        match self {
            Self::Unresolved(display) | Self::Resolved { display, .. } => *display,
        }
    }

    /// Returns semantic resolution when available.
    pub fn resolution(&self) -> Option<ConstNameResolution> {
        match self {
            Self::Unresolved(_) => None,
            Self::Resolved { resolution, .. } => Some(resolution.clone()),
        }
    }

    /// Converts to resolved identity, rejecting unresolved names.
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
    /// Integer literal text.
    Integer(String),
    /// Character literal text.
    Char(String),
    /// Byte-character literal text.
    ByteChar(String),
    /// Floating-point literal text.
    Float(String),
    /// Segmented string literal.
    String(ConstStringLiteral),
    /// Segmented byte-string literal.
    ByteString(ConstStringLiteral),
    /// Boolean literal.
    Bool(bool),
    /// Null literal.
    Null,
    /// Unqualified early name.
    Ident(EarlyConstName),
    /// Qualified early name.
    Qualified(EarlyConstName),
    /// Named field projection.
    Field {
        /// Aggregate expression.
        lhs: Box<EarlyConstExpr>,
        /// Field identity.
        name: SymbolId,
    },
    /// Method reference.
    Method {
        /// Receiver expression.
        receiver: Box<EarlyConstExpr>,
        /// Method name.
        name: SymbolId,
    },
    /// Associated function reference.
    AssociatedFunction {
        /// Associated target.
        target: EarlyConstAssociatedTarget,
        /// Function name.
        name: SymbolId,
    },
    /// Indexed projection.
    Index {
        /// Indexed aggregate.
        lhs: Box<EarlyConstExpr>,
        /// Index expression.
        index: Box<EarlyConstExpr>,
    },
    /// Slice projection.
    Slice {
        /// Sliced aggregate.
        lhs: Box<EarlyConstExpr>,
        /// Slice bounds.
        range: EarlyConstSliceRange,
    },
    /// Tuple literal.
    Tuple(Vec<EarlyConstExpr>),
    /// Tuple-field projection.
    TupleField {
        /// Tuple expression.
        lhs: Box<EarlyConstExpr>,
        /// Zero-based field index.
        index: usize,
    },
    /// Array list or repeat literal.
    ArrayLiteral {
        /// Array payload.
        elems: EarlyConstArrayElements,
    },
    /// Nominal struct construction.
    StructLiteral {
        /// Nominal type argument, possibly unresolved.
        /// The source syntax names every constructed aggregate. Resolution
        /// may still fail inside this type argument, but it is never absent.
        ty: EarlyConstTypeArg,
        /// Field initializers.
        fields: Vec<EarlyConstFieldInit>,
    },
    /// Positional nominal construction lowered from `Type(value, ...)`.
    /// Keep the early generic arguments until the semantic resolution pass;
    /// type-vs-const interpretation is still supplied by semantic facts.
    TupleStructLiteral {
        /// Nominal tuple-struct identity.
        def_id: GlobalDefId,
        /// Early generic arguments.
        generic_args: Vec<EarlyConstGenericArg>,
        /// Field initializers.
        fields: Vec<EarlyConstFieldInit>,
    },
    /// Enum variant aggregate literal.
    EnumStructLiteral {
        /// Variant expression.
        variant: Box<EarlyConstExpr>,
        /// Field initializers.
        fields: Vec<EarlyConstFieldInit>,
    },
    /// Compile-time error expression.
    CompileError {
        /// Error message expression.
        message: Box<EarlyConstExpr>,
    },
    /// Explicit compile-time trap.
    Trap,
    /// Builtin const value.
    BuiltinConstValue(BuiltinConstValue),
    /// Builtin value query.
    BuiltinValue(ValueBuiltin),
    /// Layout builtin.
    LayoutBuiltin {
        /// Layout operation.
        builtin: LayoutBuiltin,
        /// Early type argument.
        type_arg: EarlyConstTypeArg,
    },
    /// Field-offset builtin.
    FieldOffsetBuiltin {
        /// Early type argument.
        type_arg: EarlyConstTypeArg,
        /// Field identity.
        field: SymbolId,
    },
    /// Embedded resource.
    Embed {
        /// Path literal.
        path: ConstStringLiteral,
    },
    /// Const function call.
    Call {
        /// Callee expression.
        callee: Box<EarlyConstExpr>,
        /// Early generic arguments.
        generic_args: Vec<EarlyConstGenericArg>,
        /// Argument expressions.
        args: Vec<EarlyConstExpr>,
    },
    /// Unary operation.
    Unary {
        /// Operator.
        op: ConstUnaryOp,
        /// Operand.
        expr: Box<EarlyConstExpr>,
    },
    /// Optional success constructor.
    OptionalSome {
        /// Payload expression.
        expr: Box<EarlyConstExpr>,
    },
    /// Error-union success constructor.
    ErrorOk {
        /// Payload expression.
        expr: Box<EarlyConstExpr>,
    },
    /// Error-union error constructor.
    ErrorErr {
        /// Payload expression.
        expr: Box<EarlyConstExpr>,
    },
    /// Error propagation expression.
    Try {
        /// Fallible expression.
        expr: Box<EarlyConstExpr>,
    },
    /// Binary operation.
    Binary {
        /// Left operand.
        lhs: Box<EarlyConstExpr>,
        /// Operator.
        op: ConstBinaryOp,
        /// Right operand.
        rhs: Box<EarlyConstExpr>,
    },
    /// Assignment expression.
    Assign(Box<EarlyConstAssign>),
    /// Integer range expression.
    Range(EarlyConstRange),
    /// Conditional expression.
    If {
        /// Condition.
        cond: Box<EarlyConstExpr>,
        /// True branch.
        then_branch: EarlyConstBlock,
        /// Optional false branch.
        else_branch: Option<Box<EarlyConstExpr>>,
    },
    /// Match expression.
    Match(Box<EarlyConstMatch>),
    /// Cast with an optional unresolved target type.
    Cast {
        /// Operand expression.
        expr: Box<EarlyConstExpr>,
        /// Optional target type identity.
        ty: Option<InternedTyId>,
    },
    /// Lexical block expression.
    Block(EarlyConstBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Segmented string literal preserving source parts before escape decoding.
pub struct ConstStringLiteral {
    /// Literal segments in source order.
    pub parts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Unary operations available in const expressions.
pub enum ConstUnaryOp {
    /// Arithmetic negation.
    Neg,
    /// Boolean/logical negation.
    Not,
    /// Integer bitwise complement.
    BitNot,
    /// Read-only reference creation.
    RefReadOnly,
    /// Mutable reference creation.
    Ref,
    /// Pointer dereference.
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Binary operations available in const expressions.
pub enum ConstBinaryOp {
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Remainder.
    Rem,
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Left shift.
    Shl,
    /// Right shift.
    Shr,
    /// Less-than comparison.
    Lt,
    /// Less-than-or-equal comparison.
    Le,
    /// Greater-than comparison.
    Gt,
    /// Greater-than-or-equal comparison.
    Ge,
    /// Equality comparison.
    Eq,
    /// Inequality comparison.
    Ne,
    /// Bitwise and.
    BitAnd,
    /// Bitwise xor.
    BitXor,
    /// Bitwise or.
    BitOr,
    /// Logical and.
    And,
    /// Logical or.
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Assignment operators accepted by const locals.
pub enum ConstAssignOp {
    /// Replacement assignment.
    Assign,
    /// Add-and-assign.
    Add,
    /// Subtract-and-assign.
    Sub,
    /// Shift-left-and-assign.
    Shl,
    /// Shift-right-and-assign.
    Shr,
    /// Multiply-and-assign.
    Mul,
    /// Divide-and-assign.
    Div,
    /// Remainder-and-assign.
    Rem,
    /// Bitwise-and-and-assign.
    BitAnd,
    /// Bitwise-xor-and-assign.
    BitXor,
    /// Bitwise-or-and-assign.
    BitOr,
}

#[derive(Debug, Clone, PartialEq)]
/// Optional-bound range in early IR.
pub struct EarlyConstRange {
    /// Optional lower bound.
    pub start: Option<Box<EarlyConstExpr>>,
    /// Optional upper bound.
    pub end: Option<Box<EarlyConstExpr>>,
    /// Whether the upper bound is inclusive.
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
/// Optional-bound slice range in early IR.
pub struct EarlyConstSliceRange {
    /// Optional lower bound.
    pub start: Option<Box<EarlyConstExpr>>,
    /// Optional upper bound.
    pub end: Option<Box<EarlyConstExpr>>,
    /// Whether the upper bound is inclusive.
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
/// Early array literal list or repeat payload.
pub enum EarlyConstArrayElements {
    /// Explicit element list.
    List(Vec<EarlyConstExpr>),
    /// Repeated value and count.
    Repeat {
        /// Repeated value expression.
        value: Box<EarlyConstExpr>,
        /// Repeat count expression.
        count: Box<EarlyConstExpr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Semantic identity carried by a resolved const name.
pub enum ConstNameResolution {
    /// Local binding identity.
    Local(LocalId),
    /// Global definition identity.
    Global(GlobalDefId),
    /// Generic parameter identity by symbol.
    GenericParam(SymbolId),
    /// Builtin associated value identity.
    BuiltinAssociatedValue(BuiltinAssociatedValue),
    /// Associated-constant projection identity.
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
/// Named field initializer in early IR.
pub struct EarlyConstFieldInit {
    /// Initializer source span.
    pub span: Span,
    /// Field identity.
    pub name: SymbolId,
    /// Field value expression.
    pub value: EarlyConstExpr,
}

#[derive(Debug, Clone, PartialEq)]
/// A type occurrence carried through early lowering.
///
/// `ty == None` means semantic type identity is not available yet. This differs
/// from an untyped aggregate literal, whose optional type lives on the literal
/// expression itself and intentionally survives into resolved IR for inference.
pub struct EarlyConstTypeArg {
    /// Full type occurrence span.
    pub span: Span,
    /// Type syntax span.
    pub ty_span: Span,
    /// Optional interned type identity.
    pub ty: Option<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq)]
/// Early generic argument retaining type versus const form.
pub enum EarlyConstGenericArg {
    /// Generic argument explicitly left for call-site inference with `_`.
    Infer(Span),
    /// Type argument, possibly unresolved.
    Type(EarlyConstTypeArg),
    /// Const expression argument.
    Const(EarlyConstExpr),
}

#[derive(Debug, Clone, PartialEq)]
/// Early associated-function target.
pub enum EarlyConstAssociatedTarget {
    /// Type target.
    Type(EarlyConstTypeArg),
    /// Nominal definition and type arguments.
    Nominal {
        /// Definition identity.
        def_id: GlobalDefId,
        /// Type arguments.
        args: Vec<EarlyConstTypeArg>,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Resolved associated-function target.
pub enum ResolvedConstAssociatedTarget {
    /// Resolved type target.
    Type(ResolvedConstTypeArg),
    /// Nominal definition and resolved type arguments.
    Nominal {
        /// Definition identity.
        def_id: GlobalDefId,
        /// Resolved type arguments.
        args: Vec<ResolvedConstTypeArg>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A structural failure while lowering syntax or validating early IR.
pub struct ConstLowerError {
    /// Source span of the lowering/validation failure.
    pub span: Span,
    /// Stable diagnostic message.
    pub message: String,
}
