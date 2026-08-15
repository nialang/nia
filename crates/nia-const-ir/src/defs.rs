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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enums(&self) -> &[ResolvedConstEnum] {
        &self.enums
    }

    pub fn global_initializers(&self) -> &HashMap<GlobalDefId, ResolvedConstExpr> {
        &self.global_initializers
    }

    pub fn deferred_global_initializers(&self) -> &HashMap<GlobalDefId, ResolvedConstExpr> {
        &self.deferred_global_initializers
    }

    pub fn local_initializers(&self) -> &HashMap<LocalId, ResolvedConstLocalInitializer> {
        &self.local_initializers
    }

    pub fn functions(&self) -> &HashMap<GlobalDefId, ResolvedConstFunction> {
        &self.functions
    }

    pub fn const_exprs(&self) -> &HashMap<GlobalConstExprId, ResolvedConstExpr> {
        &self.const_exprs
    }

    pub fn push_enum(&mut self, item: ResolvedConstEnum) {
        self.enums.push(item);
    }

    pub fn insert_global_initializer(
        &mut self,
        id: GlobalDefId,
        value: ResolvedConstExpr,
    ) -> Option<ResolvedConstExpr> {
        self.global_initializers.insert(id, value)
    }

    pub fn insert_deferred_global_initializer(
        &mut self,
        id: GlobalDefId,
        value: ResolvedConstExpr,
    ) -> Option<ResolvedConstExpr> {
        self.deferred_global_initializers.insert(id, value)
    }

    pub fn insert_local_initializer(
        &mut self,
        id: LocalId,
        value: ResolvedConstLocalInitializer,
    ) -> Option<ResolvedConstLocalInitializer> {
        self.local_initializers.insert(id, value)
    }

    pub fn insert_function(
        &mut self,
        id: GlobalDefId,
        function: ResolvedConstFunction,
    ) -> Option<ResolvedConstFunction> {
        self.functions.insert(id, function)
    }

    pub fn insert_const_expr(
        &mut self,
        id: GlobalConstExprId,
        expr: ResolvedConstExpr,
    ) -> Option<ResolvedConstExpr> {
        self.const_exprs.insert(id, expr)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstLocalInitializer {
    explicit_type: Option<InternedTyId>,
    value: ResolvedConstExpr,
}

impl ResolvedConstLocalInitializer {
    pub fn new(explicit_type: Option<InternedTyId>, value: ResolvedConstExpr) -> Self {
        Self {
            explicit_type,
            value,
        }
    }

    pub fn explicit_type(&self) -> Option<InternedTyId> {
        self.explicit_type
    }

    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstEnum {
    def_id: GlobalDefId,
    span: Span,
    variants: Vec<ResolvedConstEnumVariant>,
}

impl ResolvedConstEnum {
    pub fn new(def_id: GlobalDefId, span: Span, variants: Vec<ResolvedConstEnumVariant>) -> Self {
        Self {
            def_id,
            span,
            variants,
        }
    }

    pub fn def_id(&self) -> GlobalDefId {
        self.def_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn variants(&self) -> &[ResolvedConstEnumVariant] {
        &self.variants
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstEnumVariant {
    def_id: GlobalDefId,
    span: Span,
    value: Option<ResolvedConstExpr>,
}

impl ResolvedConstEnumVariant {
    pub fn new(def_id: GlobalDefId, span: Span, value: Option<ResolvedConstExpr>) -> Self {
        Self {
            def_id,
            span,
            value,
        }
    }

    pub fn def_id(&self) -> GlobalDefId {
        self.def_id
    }

    pub fn span(&self) -> Span {
        self.span
    }

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

    pub fn from_parts(span: Span, kind: ResolvedConstExprKind) -> Self {
        Self { span, kind }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &ResolvedConstExprKind {
        &self.kind
    }

    pub fn name(span: Span, resolution: ConstNameResolution) -> Self {
        Self {
            span,
            kind: ResolvedConstExprKind::Name(resolution),
        }
    }

    pub fn field(span: Span, lhs: ResolvedConstExpr, name: SymbolId) -> Self {
        Self {
            span,
            kind: ResolvedConstExprKind::Field {
                lhs: Box::new(lhs),
                name,
            },
        }
    }

    pub fn index(span: Span, lhs: ResolvedConstExpr, index: ResolvedConstExpr) -> Self {
        Self {
            span,
            kind: ResolvedConstExprKind::Index {
                lhs: Box::new(lhs),
                index: Box::new(index),
            },
        }
    }

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

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn params(&self) -> &[ResolvedConstParam] {
        &self.params
    }

    pub fn body(&self) -> &ResolvedConstBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstParam {
    span: Span,
    name: SymbolId,
    local_id: LocalId,
    ty: Option<InternedTyId>,
    receiver: Option<nia_ids::ReceiverKind>,
}

impl ResolvedConstParam {
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

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn name(&self) -> SymbolId {
        self.name
    }

    pub fn local_id(&self) -> LocalId {
        self.local_id
    }

    pub fn ty(&self) -> Option<InternedTyId> {
        self.ty
    }

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
    pub fn new(
        span: Span,
        stmts: Vec<ResolvedConstStmt>,
        tail: Option<Box<ResolvedConstExpr>>,
    ) -> Self {
        Self { span, stmts, tail }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn stmts(&self) -> &[ResolvedConstStmt] {
        &self.stmts
    }

    pub fn tail(&self) -> Option<&ResolvedConstExpr> {
        self.tail.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.stmts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstStmt {
    span: Span,
    kind: ResolvedConstStmtKind,
}

impl ResolvedConstStmt {
    pub fn new(span: Span, kind: ResolvedConstStmtKind) -> Self {
        Self { span, kind }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &ResolvedConstStmtKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedConstStmtKind {
    Binding(ResolvedConstBinding),
    PatternBinding(ResolvedConstPatternBinding),
    Expr(ResolvedConstExpr),
    Return(Option<ResolvedConstExpr>),
    Break,
    Continue,
    If {
        cond: ResolvedConstExpr,
        then_branch: ResolvedConstBlock,
        else_branch: Option<ResolvedConstBlock>,
    },
    ForIn(ResolvedConstForIn),
    While {
        cond: ResolvedConstExpr,
        body: ResolvedConstBlock,
    },
    Loop {
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

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn pattern(&self) -> &ResolvedConstPattern {
        &self.pattern
    }

    pub fn explicit_type(&self) -> Option<InternedTyId> {
        self.explicit_type
    }

    pub fn is_mutable(&self) -> bool {
        self.is_mutable
    }

    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstBinding {
    span: Span,
    name: SymbolId,
    local_id: LocalId,
    explicit_type: Option<InternedTyId>,
    is_mutable: bool,
    value: ResolvedConstExpr,
}

impl ResolvedConstBinding {
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

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn name(&self) -> SymbolId {
        self.name
    }

    pub fn local_id(&self) -> LocalId {
        self.local_id
    }

    pub fn explicit_type(&self) -> Option<InternedTyId> {
        self.explicit_type
    }

    pub fn is_mutable(&self) -> bool {
        self.is_mutable
    }

    pub fn value(&self) -> &ResolvedConstExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstAssign {
    lhs: ResolvedConstAssignTarget,
    op: ConstAssignOp,
    rhs: ResolvedConstExpr,
}

impl ResolvedConstAssign {
    pub fn new(lhs: ResolvedConstAssignTarget, op: ConstAssignOp, rhs: ResolvedConstExpr) -> Self {
        Self { lhs, op, rhs }
    }

    pub fn lhs(&self) -> &ResolvedConstAssignTarget {
        &self.lhs
    }

    pub fn op(&self) -> ConstAssignOp {
        self.op
    }

    pub fn rhs(&self) -> &ResolvedConstExpr {
        &self.rhs
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstAssignTarget {
    kind: ResolvedConstAssignTargetKind,
}

impl ResolvedConstAssignTarget {
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

    pub fn kind(&self) -> &ResolvedConstAssignTargetKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedConstAssignTargetKind {
    Local {
        span: Span,
        name: SymbolId,
        local_id: LocalId,
        path: Vec<ResolvedConstAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstAssignPathElem {
    kind: ResolvedConstAssignPathElemKind,
}

impl ResolvedConstAssignPathElem {
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

    pub fn kind(&self) -> &ResolvedConstAssignPathElemKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedConstAssignPathElemKind {
    Field {
        span: Span,
        name: SymbolId,
    },
    Index {
        span: Span,
        index: ResolvedConstExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstForIn {
    pattern: ResolvedConstPattern,
    iter: ResolvedConstExpr,
    body: ResolvedConstBlock,
}

impl ResolvedConstForIn {
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

    pub fn iter(&self) -> &ResolvedConstExpr {
        &self.iter
    }

    pub fn pattern(&self) -> &ResolvedConstPattern {
        &self.pattern
    }

    pub fn body(&self) -> &ResolvedConstBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstSwitch {
    span: Span,
    target: ResolvedConstExpr,
    arms: Vec<ResolvedConstSwitchArm>,
}

impl ResolvedConstSwitch {
    pub fn new(span: Span, target: ResolvedConstExpr, arms: Vec<ResolvedConstSwitchArm>) -> Self {
        Self { span, target, arms }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn target(&self) -> &ResolvedConstExpr {
        &self.target
    }

    pub fn arms(&self) -> &[ResolvedConstSwitchArm] {
        &self.arms
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstSwitchArm {
    span: Span,
    patterns: Vec<ResolvedConstPattern>,
    body: ResolvedConstSwitchArmBody,
}

impl ResolvedConstSwitchArm {
    pub fn new(
        span: Span,
        patterns: Vec<ResolvedConstPattern>,
        body: ResolvedConstSwitchArmBody,
    ) -> Self {
        Self {
            span,
            patterns,
            body,
        }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn patterns(&self) -> &[ResolvedConstPattern] {
        &self.patterns
    }

    pub fn body(&self) -> &ResolvedConstSwitchArmBody {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstPattern {
    kind: ResolvedConstPatternKind,
}

impl ResolvedConstPattern {
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
        span: Span,
    ) -> Self {
        Self {
            kind: ResolvedConstPatternKind::Struct {
                def_id,
                fields,
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

    pub fn kind(&self) -> &ResolvedConstPatternKind {
        &self.kind
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
pub enum ResolvedConstPatternKind {
    Wildcard {
        span: Span,
    },
    Bind {
        name: SymbolId,
        local_id: LocalId,
        span: Span,
    },
    Pointer {
        pattern: Box<ResolvedConstPattern>,
        span: Span,
    },
    MutPointer {
        pattern: Box<ResolvedConstPattern>,
        span: Span,
    },
    OptionalSome {
        pattern: Box<ResolvedConstPattern>,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        pattern: Box<ResolvedConstPattern>,
        span: Span,
    },
    ErrorErr {
        pattern: Box<ResolvedConstPattern>,
        span: Span,
    },
    Tuple {
        patterns: Vec<ResolvedConstPattern>,
        span: Span,
    },
    EnumVariant {
        variant: ResolvedConstExpr,
        fields: ConstEnumPatternFields<ResolvedConstPattern>,
        span: Span,
    },
    Struct {
        def_id: GlobalDefId,
        fields: Vec<ConstNamedPatternField<ResolvedConstPattern>>,
        span: Span,
    },
    Expr(ResolvedConstExpr),
    Range {
        start: ResolvedConstExpr,
        end: ResolvedConstExpr,
        inclusive: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstSwitchArmBody {
    kind: ResolvedConstSwitchArmBodyKind,
}

impl ResolvedConstSwitchArmBody {
    pub fn expr(expr: ResolvedConstExpr) -> Self {
        Self {
            kind: ResolvedConstSwitchArmBodyKind::Expr(expr),
        }
    }

    pub fn stmt(stmt: ResolvedConstStmt) -> Self {
        Self {
            kind: ResolvedConstSwitchArmBodyKind::Stmt(Box::new(stmt)),
        }
    }

    pub fn block(block: ResolvedConstBlock) -> Self {
        Self {
            kind: ResolvedConstSwitchArmBodyKind::Block(block),
        }
    }

    pub fn kind(&self) -> &ResolvedConstSwitchArmBodyKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedConstSwitchArmBodyKind {
    Expr(ResolvedConstExpr),
    Stmt(Box<ResolvedConstStmt>),
    Block(ResolvedConstBlock),
}

#[derive(Debug, Clone, PartialEq)]
/// Expression forms accepted after the early-to-resolved validation boundary.
///
/// Optional literal type ids represent contextual inference, not unresolved
/// semantic facts. Required types such as casts and builtin type arguments use
/// non-optional resolved ids in their owning nodes.
pub enum ResolvedConstExprKind {
    Integer(String),
    Char(String),
    ByteChar(String),
    Float(String),
    String(ConstStringLiteral),
    ByteString(ConstStringLiteral),
    Bool(bool),
    Null,
    Name(ConstNameResolution),
    Field {
        lhs: Box<ResolvedConstExpr>,
        name: SymbolId,
    },
    Method {
        receiver: Box<ResolvedConstExpr>,
        name: SymbolId,
    },
    AssociatedFunction {
        target: ResolvedConstAssociatedTarget,
        name: SymbolId,
    },
    Index {
        lhs: Box<ResolvedConstExpr>,
        index: Box<ResolvedConstExpr>,
    },
    Slice {
        lhs: Box<ResolvedConstExpr>,
        range: ResolvedConstSliceRange,
    },
    Tuple(Vec<ResolvedConstExpr>),
    TupleField {
        lhs: Box<ResolvedConstExpr>,
        index: usize,
    },
    ArrayLiteral {
        elems: ResolvedConstArrayElements,
    },
    StructLiteral {
        /// Nominal construction is encoded in the IR itself: a struct value
        /// can never rely on an expected type to acquire its identity.
        ty: InternedTyId,
        fields: Vec<ResolvedConstFieldInit>,
    },
    EnumStructLiteral {
        variant: Box<ResolvedConstExpr>,
        fields: Vec<ResolvedConstFieldInit>,
    },
    CompileError {
        message: Box<ResolvedConstExpr>,
    },
    Trap,
    BuiltinConstValue(BuiltinConstValue),
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: ResolvedConstTypeArg,
    },
    FieldOffsetBuiltin {
        type_arg: ResolvedConstTypeArg,
        field: SymbolId,
    },
    Embed {
        path: ConstStringLiteral,
    },
    Call {
        callee: Box<ResolvedConstExpr>,
        generic_args: Vec<ResolvedConstGenericArg>,
        args: Vec<ResolvedConstExpr>,
    },
    Unary {
        op: ConstUnaryOp,
        expr: Box<ResolvedConstExpr>,
    },
    OptionalSome {
        expr: Box<ResolvedConstExpr>,
    },
    ErrorOk {
        expr: Box<ResolvedConstExpr>,
    },
    ErrorErr {
        expr: Box<ResolvedConstExpr>,
    },
    Try {
        expr: Box<ResolvedConstExpr>,
    },
    Binary {
        lhs: Box<ResolvedConstExpr>,
        op: ConstBinaryOp,
        rhs: Box<ResolvedConstExpr>,
    },
    Assign(Box<ResolvedConstAssign>),
    Range(ResolvedConstRange),
    If {
        cond: Box<ResolvedConstExpr>,
        then_branch: ResolvedConstBlock,
        else_branch: Option<Box<ResolvedConstExpr>>,
    },
    Switch(Box<ResolvedConstSwitch>),
    Cast {
        expr: Box<ResolvedConstExpr>,
        ty: InternedTyId,
    },
    Block(ResolvedConstBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstRange {
    start: Option<Box<ResolvedConstExpr>>,
    end: Option<Box<ResolvedConstExpr>>,
    inclusive: bool,
}

impl ResolvedConstRange {
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
pub struct ResolvedConstSliceRange {
    start: Option<Box<ResolvedConstExpr>>,
    end: Option<Box<ResolvedConstExpr>>,
    inclusive: bool,
}

impl ResolvedConstSliceRange {
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
pub struct ResolvedConstArrayElements {
    kind: ResolvedConstArrayElementsKind,
}

impl ResolvedConstArrayElements {
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
pub enum ResolvedConstArrayElementsKind {
    List(Vec<ResolvedConstExpr>),
    Repeat {
        value: Box<ResolvedConstExpr>,
        count: Box<ResolvedConstExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConstFieldInit {
    span: Span,
    name: SymbolId,
    value: ResolvedConstExpr,
}

impl ResolvedConstFieldInit {
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
pub struct EarlyConstSwitch {
    pub span: Span,
    pub target: EarlyConstExpr,
    pub arms: Vec<EarlyConstSwitchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyConstSwitchArm {
    pub span: Span,
    pub patterns: Vec<EarlyConstPattern>,
    pub body: EarlyConstSwitchArmBody,
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
    Named(Vec<ConstNamedPatternField<P>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstNamedPatternField<P> {
    pub name: SymbolId,
    pub pattern: P,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyConstSwitchArmBody {
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
    Switch(Box<EarlyConstSwitch>),
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
