use crate::lower::{ComptimeLowerContext, lower_type_id, unresolved_error};
use crate::*;
use nia_ids::{
    BuiltinComptime, BuiltinTraitMethod, GlobalConstExprId, GlobalDefId, InternedTyId,
    LayoutBuiltin, LocalId, ValueBuiltin,
};
use nia_sema_ir::{AssociatedComptimeProjection, BuiltinAssociatedValue, SemanticValueUse};
use nia_span::Span;
use nia_symbol::SymbolId;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedComptimeModule {
    enums: Vec<ResolvedComptimeEnum>,
    global_initializers: HashMap<GlobalDefId, ResolvedComptimeExpr>,
    deferred_global_initializers: HashMap<GlobalDefId, ResolvedComptimeExpr>,
    local_initializers: HashMap<LocalId, ResolvedComptimeLocalInitializer>,
    functions: HashMap<GlobalDefId, ResolvedComptimeFunction>,
    const_exprs: HashMap<GlobalConstExprId, ResolvedComptimeExpr>,
}

impl ResolvedComptimeModule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enums(&self) -> &[ResolvedComptimeEnum] {
        &self.enums
    }

    pub fn global_initializers(&self) -> &HashMap<GlobalDefId, ResolvedComptimeExpr> {
        &self.global_initializers
    }

    pub fn deferred_global_initializers(&self) -> &HashMap<GlobalDefId, ResolvedComptimeExpr> {
        &self.deferred_global_initializers
    }

    pub fn local_initializers(&self) -> &HashMap<LocalId, ResolvedComptimeLocalInitializer> {
        &self.local_initializers
    }

    pub fn functions(&self) -> &HashMap<GlobalDefId, ResolvedComptimeFunction> {
        &self.functions
    }

    pub fn const_exprs(&self) -> &HashMap<GlobalConstExprId, ResolvedComptimeExpr> {
        &self.const_exprs
    }

    pub fn push_enum(&mut self, item: ResolvedComptimeEnum) {
        self.enums.push(item);
    }

    pub fn insert_global_initializer(
        &mut self,
        id: GlobalDefId,
        value: ResolvedComptimeExpr,
    ) -> Option<ResolvedComptimeExpr> {
        self.global_initializers.insert(id, value)
    }

    pub fn insert_deferred_global_initializer(
        &mut self,
        id: GlobalDefId,
        value: ResolvedComptimeExpr,
    ) -> Option<ResolvedComptimeExpr> {
        self.deferred_global_initializers.insert(id, value)
    }

    pub fn insert_local_initializer(
        &mut self,
        id: LocalId,
        value: ResolvedComptimeLocalInitializer,
    ) -> Option<ResolvedComptimeLocalInitializer> {
        self.local_initializers.insert(id, value)
    }

    pub fn insert_function(
        &mut self,
        id: GlobalDefId,
        function: ResolvedComptimeFunction,
    ) -> Option<ResolvedComptimeFunction> {
        self.functions.insert(id, function)
    }

    pub fn insert_const_expr(
        &mut self,
        id: GlobalConstExprId,
        expr: ResolvedComptimeExpr,
    ) -> Option<ResolvedComptimeExpr> {
        self.const_exprs.insert(id, expr)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeLocalInitializer {
    explicit_type: Option<InternedTyId>,
    value: ResolvedComptimeExpr,
}

impl ResolvedComptimeLocalInitializer {
    pub fn new(explicit_type: Option<InternedTyId>, value: ResolvedComptimeExpr) -> Self {
        Self {
            explicit_type,
            value,
        }
    }

    pub fn explicit_type(&self) -> Option<InternedTyId> {
        self.explicit_type
    }

    pub fn value(&self) -> &ResolvedComptimeExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeEnum {
    def_id: GlobalDefId,
    span: Span,
    variants: Vec<ResolvedComptimeEnumVariant>,
}

impl ResolvedComptimeEnum {
    pub fn new(
        def_id: GlobalDefId,
        span: Span,
        variants: Vec<ResolvedComptimeEnumVariant>,
    ) -> Self {
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

    pub fn variants(&self) -> &[ResolvedComptimeEnumVariant] {
        &self.variants
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeEnumVariant {
    def_id: GlobalDefId,
    span: Span,
    value: Option<ResolvedComptimeExpr>,
}

impl ResolvedComptimeEnumVariant {
    pub fn new(def_id: GlobalDefId, span: Span, value: Option<ResolvedComptimeExpr>) -> Self {
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

    pub fn value(&self) -> Option<&ResolvedComptimeExpr> {
        self.value.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeExpr {
    span: Span,
    kind: ResolvedComptimeExprKind,
}

impl ResolvedComptimeExpr {
    pub(crate) fn new(expr: EarlyComptimeExpr) -> Result<Self, ComptimeLowerError> {
        resolve_expr(expr)
    }

    pub fn from_parts(span: Span, kind: ResolvedComptimeExprKind) -> Self {
        Self { span, kind }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &ResolvedComptimeExprKind {
        &self.kind
    }

    pub fn name(span: Span, resolution: ComptimeNameResolution) -> Self {
        Self {
            span,
            kind: ResolvedComptimeExprKind::Name(resolution),
        }
    }

    pub fn field(span: Span, lhs: ResolvedComptimeExpr, name: SymbolId) -> Self {
        Self {
            span,
            kind: ResolvedComptimeExprKind::Field {
                lhs: Box::new(lhs),
                name,
            },
        }
    }

    pub fn index(span: Span, lhs: ResolvedComptimeExpr, index: ResolvedComptimeExpr) -> Self {
        Self {
            span,
            kind: ResolvedComptimeExprKind::Index {
                lhs: Box::new(lhs),
                index: Box::new(index),
            },
        }
    }

    pub fn call(
        span: Span,
        callee: ResolvedComptimeExpr,
        type_args: Vec<ResolvedComptimeTypeArg>,
        args: Vec<ResolvedComptimeExpr>,
    ) -> Self {
        Self {
            span,
            kind: ResolvedComptimeExprKind::Call {
                callee: Box::new(callee),
                type_args,
                args,
            },
        }
    }

    pub fn name_resolution(&self) -> Option<ComptimeNameResolution> {
        match &self.kind {
            ResolvedComptimeExprKind::Name(resolution) => Some(resolution.clone()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeFunction {
    span: Span,
    params: Vec<ResolvedComptimeParam>,
    body: ResolvedComptimeBlock,
}

impl ResolvedComptimeFunction {
    pub(crate) fn new(function: EarlyComptimeFunction) -> Result<Self, ComptimeLowerError> {
        resolve_function(function)
    }

    pub(crate) fn from_parts(
        span: Span,
        params: Vec<ResolvedComptimeParam>,
        body: ResolvedComptimeBlock,
    ) -> Self {
        Self { span, params, body }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn params(&self) -> &[ResolvedComptimeParam] {
        &self.params
    }

    pub fn body(&self) -> &ResolvedComptimeBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeParam {
    span: Span,
    name: SymbolId,
    local_id: LocalId,
    ty: Option<InternedTyId>,
}

impl ResolvedComptimeParam {
    pub fn new(span: Span, name: SymbolId, local_id: LocalId, ty: Option<InternedTyId>) -> Self {
        Self {
            span,
            name,
            local_id,
            ty,
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeBlock {
    span: Span,
    stmts: Vec<ResolvedComptimeStmt>,
    tail: Option<Box<ResolvedComptimeExpr>>,
}

impl ResolvedComptimeBlock {
    pub fn new(
        span: Span,
        stmts: Vec<ResolvedComptimeStmt>,
        tail: Option<Box<ResolvedComptimeExpr>>,
    ) -> Self {
        Self { span, stmts, tail }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn stmts(&self) -> &[ResolvedComptimeStmt] {
        &self.stmts
    }

    pub fn tail(&self) -> Option<&ResolvedComptimeExpr> {
        self.tail.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.stmts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeStmt {
    span: Span,
    kind: ResolvedComptimeStmtKind,
}

impl ResolvedComptimeStmt {
    pub fn new(span: Span, kind: ResolvedComptimeStmtKind) -> Self {
        Self { span, kind }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &ResolvedComptimeStmtKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeStmtKind {
    Binding(ResolvedComptimeBinding),
    Expr(ResolvedComptimeExpr),
    Return(Option<ResolvedComptimeExpr>),
    Break,
    Continue,
    If {
        cond: ResolvedComptimeExpr,
        then_branch: ResolvedComptimeBlock,
        else_branch: Option<ResolvedComptimeBlock>,
    },
    ForIn(ResolvedComptimeForIn),
    While {
        cond: ResolvedComptimeExpr,
        body: ResolvedComptimeBlock,
    },
    Loop {
        body: ResolvedComptimeBlock,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeBinding {
    span: Span,
    name: SymbolId,
    local_id: LocalId,
    explicit_type: Option<InternedTyId>,
    is_mutable: bool,
    value: ResolvedComptimeExpr,
}

impl ResolvedComptimeBinding {
    pub fn new(
        span: Span,
        name: SymbolId,
        local_id: LocalId,
        explicit_type: Option<InternedTyId>,
        is_mutable: bool,
        value: ResolvedComptimeExpr,
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

    pub fn value(&self) -> &ResolvedComptimeExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeAssign {
    lhs: ResolvedComptimeAssignTarget,
    op: ComptimeAssignOp,
    rhs: ResolvedComptimeExpr,
}

impl ResolvedComptimeAssign {
    pub fn new(
        lhs: ResolvedComptimeAssignTarget,
        op: ComptimeAssignOp,
        rhs: ResolvedComptimeExpr,
    ) -> Self {
        Self { lhs, op, rhs }
    }

    pub fn lhs(&self) -> &ResolvedComptimeAssignTarget {
        &self.lhs
    }

    pub fn op(&self) -> ComptimeAssignOp {
        self.op
    }

    pub fn rhs(&self) -> &ResolvedComptimeExpr {
        &self.rhs
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeAssignTarget {
    kind: ResolvedComptimeAssignTargetKind,
}

impl ResolvedComptimeAssignTarget {
    pub fn local(
        span: Span,
        name: SymbolId,
        local_id: LocalId,
        path: Vec<ResolvedComptimeAssignPathElem>,
    ) -> Self {
        Self {
            kind: ResolvedComptimeAssignTargetKind::Local {
                span,
                name,
                local_id,
                path,
            },
        }
    }

    pub fn kind(&self) -> &ResolvedComptimeAssignTargetKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeAssignTargetKind {
    Local {
        span: Span,
        name: SymbolId,
        local_id: LocalId,
        path: Vec<ResolvedComptimeAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeAssignPathElem {
    kind: ResolvedComptimeAssignPathElemKind,
}

impl ResolvedComptimeAssignPathElem {
    pub fn field(span: Span, name: SymbolId) -> Self {
        Self {
            kind: ResolvedComptimeAssignPathElemKind::Field { span, name },
        }
    }

    pub fn index(span: Span, index: ResolvedComptimeExpr) -> Self {
        Self {
            kind: ResolvedComptimeAssignPathElemKind::Index { span, index },
        }
    }

    pub fn kind(&self) -> &ResolvedComptimeAssignPathElemKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeAssignPathElemKind {
    Field {
        span: Span,
        name: SymbolId,
    },
    Index {
        span: Span,
        index: ResolvedComptimeExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeForIn {
    pattern: ResolvedComptimePattern,
    iter: ResolvedComptimeExpr,
    body: ResolvedComptimeBlock,
}

impl ResolvedComptimeForIn {
    pub fn new(
        pattern: ResolvedComptimePattern,
        iter: ResolvedComptimeExpr,
        body: ResolvedComptimeBlock,
    ) -> Self {
        Self {
            pattern,
            iter,
            body,
        }
    }

    pub fn iter(&self) -> &ResolvedComptimeExpr {
        &self.iter
    }

    pub fn pattern(&self) -> &ResolvedComptimePattern {
        &self.pattern
    }

    pub fn body(&self) -> &ResolvedComptimeBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSwitch {
    span: Span,
    target: ResolvedComptimeExpr,
    arms: Vec<ResolvedComptimeSwitchArm>,
}

impl ResolvedComptimeSwitch {
    pub fn new(
        span: Span,
        target: ResolvedComptimeExpr,
        arms: Vec<ResolvedComptimeSwitchArm>,
    ) -> Self {
        Self { span, target, arms }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn target(&self) -> &ResolvedComptimeExpr {
        &self.target
    }

    pub fn arms(&self) -> &[ResolvedComptimeSwitchArm] {
        &self.arms
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSwitchArm {
    span: Span,
    patterns: Vec<ResolvedComptimePattern>,
    body: ResolvedComptimeSwitchArmBody,
}

impl ResolvedComptimeSwitchArm {
    pub fn new(
        span: Span,
        patterns: Vec<ResolvedComptimePattern>,
        body: ResolvedComptimeSwitchArmBody,
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

    pub fn patterns(&self) -> &[ResolvedComptimePattern] {
        &self.patterns
    }

    pub fn body(&self) -> &ResolvedComptimeSwitchArmBody {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimePattern {
    kind: ResolvedComptimePatternKind,
}

impl ResolvedComptimePattern {
    pub fn wildcard(span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::Wildcard { span },
        }
    }

    pub fn bind(name: SymbolId, local_id: LocalId, span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::Bind {
                name,
                local_id,
                span,
            },
        }
    }

    pub fn optional_some(pattern: ResolvedComptimePattern, span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::OptionalSome {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn pointer(pattern: ResolvedComptimePattern, span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::Pointer {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn mut_pointer(pattern: ResolvedComptimePattern, span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::MutPointer {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn optional_null(span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::OptionalNull { span },
        }
    }

    pub fn error_ok(pattern: ResolvedComptimePattern, span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::ErrorOk {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn error_err(pattern: ResolvedComptimePattern, span: Span) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::ErrorErr {
                pattern: Box::new(pattern),
                span,
            },
        }
    }

    pub fn expr(expr: ResolvedComptimeExpr) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::Expr(expr),
        }
    }

    pub fn range(
        start: ResolvedComptimeExpr,
        end: ResolvedComptimeExpr,
        inclusive: bool,
        span: Span,
    ) -> Self {
        Self {
            kind: ResolvedComptimePatternKind::Range {
                start,
                end,
                inclusive,
                span,
            },
        }
    }

    pub fn kind(&self) -> &ResolvedComptimePatternKind {
        &self.kind
    }
}

impl Default for ResolvedComptimePattern {
    fn default() -> Self {
        Self {
            kind: ResolvedComptimePatternKind::Wildcard {
                span: Span::new(0, 0),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimePatternKind {
    Wildcard {
        span: Span,
    },
    Bind {
        name: SymbolId,
        local_id: LocalId,
        span: Span,
    },
    Pointer {
        pattern: Box<ResolvedComptimePattern>,
        span: Span,
    },
    MutPointer {
        pattern: Box<ResolvedComptimePattern>,
        span: Span,
    },
    OptionalSome {
        pattern: Box<ResolvedComptimePattern>,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        pattern: Box<ResolvedComptimePattern>,
        span: Span,
    },
    ErrorErr {
        pattern: Box<ResolvedComptimePattern>,
        span: Span,
    },
    Expr(ResolvedComptimeExpr),
    Range {
        start: ResolvedComptimeExpr,
        end: ResolvedComptimeExpr,
        inclusive: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSwitchArmBody {
    kind: ResolvedComptimeSwitchArmBodyKind,
}

impl ResolvedComptimeSwitchArmBody {
    pub fn expr(expr: ResolvedComptimeExpr) -> Self {
        Self {
            kind: ResolvedComptimeSwitchArmBodyKind::Expr(expr),
        }
    }

    pub fn stmt(stmt: ResolvedComptimeStmt) -> Self {
        Self {
            kind: ResolvedComptimeSwitchArmBodyKind::Stmt(Box::new(stmt)),
        }
    }

    pub fn block(block: ResolvedComptimeBlock) -> Self {
        Self {
            kind: ResolvedComptimeSwitchArmBodyKind::Block(block),
        }
    }

    pub fn kind(&self) -> &ResolvedComptimeSwitchArmBodyKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeSwitchArmBodyKind {
    Expr(ResolvedComptimeExpr),
    Stmt(Box<ResolvedComptimeStmt>),
    Block(ResolvedComptimeBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeExprKind {
    Integer(String),
    Char(String),
    ByteChar(String),
    Float(String),
    String(ComptimeStringLiteral),
    ByteString(ComptimeStringLiteral),
    Bool(bool),
    Null,
    Name(ComptimeNameResolution),
    Field {
        lhs: Box<ResolvedComptimeExpr>,
        name: SymbolId,
    },
    BuiltinMethod {
        method: BuiltinTraitMethod,
        lhs: Box<ResolvedComptimeExpr>,
    },
    Index {
        lhs: Box<ResolvedComptimeExpr>,
        index: Box<ResolvedComptimeExpr>,
    },
    Slice {
        lhs: Box<ResolvedComptimeExpr>,
        range: ResolvedComptimeSliceRange,
    },
    ArrayLiteral {
        ty: Option<InternedTyId>,
        elems: ResolvedComptimeArrayElements,
    },
    StructLiteral {
        ty: Option<InternedTyId>,
        fields: Vec<ResolvedComptimeFieldInit>,
    },
    CompileError {
        message: Box<ResolvedComptimeExpr>,
    },
    BuiltinComptime(BuiltinComptime),
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: ResolvedComptimeTypeArg,
    },
    FieldOffsetBuiltin {
        type_arg: ResolvedComptimeTypeArg,
        field: SymbolId,
    },
    Embed {
        path: ComptimeStringLiteral,
    },
    Call {
        callee: Box<ResolvedComptimeExpr>,
        type_args: Vec<ResolvedComptimeTypeArg>,
        args: Vec<ResolvedComptimeExpr>,
    },
    Unary {
        op: ComptimeUnaryOp,
        expr: Box<ResolvedComptimeExpr>,
    },
    OptionalSome {
        expr: Box<ResolvedComptimeExpr>,
    },
    ErrorOk {
        expr: Box<ResolvedComptimeExpr>,
    },
    ErrorErr {
        expr: Box<ResolvedComptimeExpr>,
    },
    Try {
        expr: Box<ResolvedComptimeExpr>,
    },
    Binary {
        lhs: Box<ResolvedComptimeExpr>,
        op: ComptimeBinaryOp,
        rhs: Box<ResolvedComptimeExpr>,
    },
    Assign(Box<ResolvedComptimeAssign>),
    Range(ResolvedComptimeRange),
    If {
        cond: Box<ResolvedComptimeExpr>,
        then_branch: ResolvedComptimeBlock,
        else_branch: Option<Box<ResolvedComptimeExpr>>,
    },
    Switch(Box<ResolvedComptimeSwitch>),
    Cast {
        expr: Box<ResolvedComptimeExpr>,
        ty: InternedTyId,
    },
    Block(ResolvedComptimeBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeRange {
    start: Option<Box<ResolvedComptimeExpr>>,
    end: Option<Box<ResolvedComptimeExpr>>,
    inclusive: bool,
}

impl ResolvedComptimeRange {
    pub fn new(
        start: Option<Box<ResolvedComptimeExpr>>,
        end: Option<Box<ResolvedComptimeExpr>>,
        inclusive: bool,
    ) -> Self {
        Self {
            start,
            end,
            inclusive,
        }
    }

    pub fn start(&self) -> Option<&ResolvedComptimeExpr> {
        self.start.as_deref()
    }

    pub fn end(&self) -> Option<&ResolvedComptimeExpr> {
        self.end.as_deref()
    }

    pub fn is_inclusive(&self) -> bool {
        self.inclusive
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSliceRange {
    start: Option<Box<ResolvedComptimeExpr>>,
    end: Option<Box<ResolvedComptimeExpr>>,
    inclusive: bool,
}

impl ResolvedComptimeSliceRange {
    pub fn new(
        start: Option<Box<ResolvedComptimeExpr>>,
        end: Option<Box<ResolvedComptimeExpr>>,
        inclusive: bool,
    ) -> Self {
        Self {
            start,
            end,
            inclusive,
        }
    }

    pub fn start(&self) -> Option<&ResolvedComptimeExpr> {
        self.start.as_deref()
    }

    pub fn end(&self) -> Option<&ResolvedComptimeExpr> {
        self.end.as_deref()
    }

    pub fn is_inclusive(&self) -> bool {
        self.inclusive
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeArrayElements {
    kind: ResolvedComptimeArrayElementsKind,
}

impl ResolvedComptimeArrayElements {
    pub fn list(elems: Vec<ResolvedComptimeExpr>) -> Self {
        Self {
            kind: ResolvedComptimeArrayElementsKind::List(elems),
        }
    }

    pub fn repeat(value: ResolvedComptimeExpr, count: ResolvedComptimeExpr) -> Self {
        Self {
            kind: ResolvedComptimeArrayElementsKind::Repeat {
                value: Box::new(value),
                count: Box::new(count),
            },
        }
    }

    pub fn kind(&self) -> &ResolvedComptimeArrayElementsKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeArrayElementsKind {
    List(Vec<ResolvedComptimeExpr>),
    Repeat {
        value: Box<ResolvedComptimeExpr>,
        count: Box<ResolvedComptimeExpr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeFieldInit {
    span: Span,
    name: SymbolId,
    value: ResolvedComptimeExpr,
}

impl ResolvedComptimeFieldInit {
    pub fn new(span: Span, name: SymbolId, value: ResolvedComptimeExpr) -> Self {
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

    pub fn value(&self) -> &ResolvedComptimeExpr {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeTypeArg {
    span: Span,
    ty_span: Span,
    ty: InternedTyId,
}

impl ResolvedComptimeTypeArg {
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
pub struct EarlyComptimeFunction {
    pub span: Span,
    pub params: Vec<EarlyComptimeParam>,
    pub body: EarlyComptimeBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeParam {
    pub span: Span,
    pub name: SymbolId,
    pub local_id: Option<LocalId>,
    pub ty: Option<InternedTyId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeBlock {
    pub span: Span,
    pub stmts: Vec<EarlyComptimeStmt>,
    pub tail: Option<Box<EarlyComptimeExpr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeStmt {
    pub span: Span,
    pub kind: EarlyComptimeStmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeStmtKind {
    Binding(EarlyComptimeBinding),
    Expr(EarlyComptimeExpr),
    Return(Option<EarlyComptimeExpr>),
    Break,
    Continue,
    If {
        cond: EarlyComptimeExpr,
        then_branch: EarlyComptimeBlock,
        else_branch: Option<EarlyComptimeBlock>,
    },
    ForIn(Box<EarlyComptimeForIn>),
    While {
        cond: EarlyComptimeExpr,
        body: EarlyComptimeBlock,
    },
    Loop {
        body: EarlyComptimeBlock,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeBinding {
    pub span: Span,
    pub name: SymbolId,
    pub local_id: Option<LocalId>,
    pub explicit_type: Option<InternedTyId>,
    pub is_mutable: bool,
    pub value: EarlyComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeAssign {
    pub lhs: EarlyComptimeAssignTarget,
    pub op: ComptimeAssignOp,
    pub rhs: EarlyComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeAssignTarget {
    Local {
        span: Span,
        name: SymbolId,
        local_id: Option<LocalId>,
        path: Vec<EarlyComptimeAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeAssignPathElem {
    Field {
        span: Span,
        name: SymbolId,
    },
    Index {
        span: Span,
        index: EarlyComptimeExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeForIn {
    pub pattern: EarlyComptimePattern,
    pub iter: EarlyComptimeExpr,
    pub body: EarlyComptimeBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeSwitch {
    pub span: Span,
    pub target: EarlyComptimeExpr,
    pub arms: Vec<EarlyComptimeSwitchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeSwitchArm {
    pub span: Span,
    pub patterns: Vec<EarlyComptimePattern>,
    pub body: EarlyComptimeSwitchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimePattern {
    Wildcard {
        span: Span,
    },
    Bind {
        name: SymbolId,
        local_id: Option<LocalId>,
        span: Span,
    },
    Pointer {
        pattern: Box<EarlyComptimePattern>,
        span: Span,
    },
    MutPointer {
        pattern: Box<EarlyComptimePattern>,
        span: Span,
    },
    OptionalSome {
        pattern: Box<EarlyComptimePattern>,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        pattern: Box<EarlyComptimePattern>,
        span: Span,
    },
    ErrorErr {
        pattern: Box<EarlyComptimePattern>,
        span: Span,
    },
    Expr(EarlyComptimeExpr),
    Range {
        start: EarlyComptimeExpr,
        end: EarlyComptimeExpr,
        inclusive: bool,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeSwitchArmBody {
    Expr(EarlyComptimeExpr),
    Stmt(Box<EarlyComptimeStmt>),
    Block(EarlyComptimeBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeExpr {
    pub span: Span,
    pub kind: EarlyComptimeExprKind,
}

impl EarlyComptimeExpr {
    pub fn span(&self) -> Span {
        self.span
    }

    pub fn kind(&self) -> &EarlyComptimeExprKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EarlyComptimeName {
    Unresolved(SymbolId),
    Resolved {
        display: SymbolId,
        resolution: ComptimeNameResolution,
    },
}

impl EarlyComptimeName {
    pub fn unresolved(display: SymbolId) -> Self {
        Self::Unresolved(display)
    }

    pub fn resolved(display: SymbolId, resolution: ComptimeNameResolution) -> Self {
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

    pub fn resolution(&self) -> Option<ComptimeNameResolution> {
        match self {
            Self::Unresolved(_) => None,
            Self::Resolved { resolution, .. } => Some(resolution.clone()),
        }
    }

    pub(crate) fn into_resolution(
        self,
        span: Span,
    ) -> Result<ComptimeNameResolution, ComptimeLowerError> {
        match self {
            Self::Resolved { resolution, .. } => Ok(resolution),
            Self::Unresolved(_) => Err(unresolved_error(span, "comptime name")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeExprKind {
    Integer(String),
    Char(String),
    ByteChar(String),
    Float(String),
    String(ComptimeStringLiteral),
    ByteString(ComptimeStringLiteral),
    Bool(bool),
    Null,
    Ident(EarlyComptimeName),
    Qualified(EarlyComptimeName),
    Field {
        lhs: Box<EarlyComptimeExpr>,
        name: SymbolId,
    },
    BuiltinMethod {
        method: BuiltinTraitMethod,
        lhs: Box<EarlyComptimeExpr>,
    },
    Index {
        lhs: Box<EarlyComptimeExpr>,
        index: Box<EarlyComptimeExpr>,
    },
    Slice {
        lhs: Box<EarlyComptimeExpr>,
        range: EarlyComptimeSliceRange,
    },
    ArrayLiteral {
        ty: Option<InternedTyId>,
        elems: EarlyComptimeArrayElements,
    },
    StructLiteral {
        ty: Option<InternedTyId>,
        fields: Vec<EarlyComptimeFieldInit>,
    },
    CompileError {
        message: Box<EarlyComptimeExpr>,
    },
    BuiltinComptime(BuiltinComptime),
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: EarlyComptimeTypeArg,
    },
    FieldOffsetBuiltin {
        type_arg: EarlyComptimeTypeArg,
        field: SymbolId,
    },
    Embed {
        path: ComptimeStringLiteral,
    },
    Call {
        callee: Box<EarlyComptimeExpr>,
        type_args: Vec<EarlyComptimeTypeArg>,
        args: Vec<EarlyComptimeExpr>,
    },
    Unary {
        op: ComptimeUnaryOp,
        expr: Box<EarlyComptimeExpr>,
    },
    OptionalSome {
        expr: Box<EarlyComptimeExpr>,
    },
    ErrorOk {
        expr: Box<EarlyComptimeExpr>,
    },
    ErrorErr {
        expr: Box<EarlyComptimeExpr>,
    },
    Try {
        expr: Box<EarlyComptimeExpr>,
    },
    Binary {
        lhs: Box<EarlyComptimeExpr>,
        op: ComptimeBinaryOp,
        rhs: Box<EarlyComptimeExpr>,
    },
    Assign(Box<EarlyComptimeAssign>),
    Range(EarlyComptimeRange),
    If {
        cond: Box<EarlyComptimeExpr>,
        then_branch: EarlyComptimeBlock,
        else_branch: Option<Box<EarlyComptimeExpr>>,
    },
    Switch(Box<EarlyComptimeSwitch>),
    Cast {
        expr: Box<EarlyComptimeExpr>,
        ty: Option<InternedTyId>,
    },
    Block(EarlyComptimeBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeStringLiteral {
    pub parts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeUnaryOp {
    Neg,
    Not,
    BitNot,
    RefReadOnly,
    Ref,
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeBinaryOp {
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
pub enum ComptimeAssignOp {
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
pub struct EarlyComptimeRange {
    pub start: Option<Box<EarlyComptimeExpr>>,
    pub end: Option<Box<EarlyComptimeExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeSliceRange {
    pub start: Option<Box<EarlyComptimeExpr>>,
    pub end: Option<Box<EarlyComptimeExpr>>,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeArrayElements {
    List(Vec<EarlyComptimeExpr>),
    Repeat {
        value: Box<EarlyComptimeExpr>,
        count: Box<EarlyComptimeExpr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComptimeNameResolution {
    Local(LocalId),
    Global(GlobalDefId),
    GenericParam(SymbolId),
    BuiltinAssociatedValue(BuiltinAssociatedValue),
    AssociatedComptimeProjection(AssociatedComptimeProjection),
}

impl From<SemanticValueUse> for ComptimeNameResolution {
    fn from(value: SemanticValueUse) -> Self {
        match value {
            SemanticValueUse::Local(local_id) => Self::Local(local_id),
            SemanticValueUse::Global(global_id) => Self::Global(global_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeFieldInit {
    pub span: Span,
    pub name: SymbolId,
    pub value: EarlyComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeTypeArg {
    pub span: Span,
    pub ty_span: Span,
    pub ty: Option<InternedTyId>,
}

impl EarlyComptimeTypeArg {
    pub(crate) fn from_type_ref(
        ty: &nia_ast::TypeRef,
        context: &dyn ComptimeLowerContext,
    ) -> Result<Self, ComptimeLowerError> {
        Ok(Self {
            span: ty.span,
            ty_span: ty.span,
            ty: lower_type_id(context, &ty.node_key, ty.span)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeLowerError {
    pub span: Span,
    pub message: String,
}
