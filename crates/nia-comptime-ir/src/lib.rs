// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashMap;

use nia_ids::{
    BuiltinTraitMethod, GlobalConstExprId, GlobalDefId, InternedTyId, LayoutBuiltin, LocalId,
    ValueBuiltin,
};
use nia_node_id::NodeKey;
use nia_sema_ir::{BuiltinAssociatedValue, SemanticUseTable, SemanticValueUse};
use nia_span::Span;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedComptimeModule {
    enums: Vec<ResolvedComptimeEnum>,
    global_initializers: HashMap<GlobalDefId, ResolvedComptimeExpr>,
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
    fn new(expr: EarlyComptimeExpr) -> Result<Self, ComptimeLowerError> {
        resolve_expr(expr)
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

    pub fn field(span: Span, lhs: ResolvedComptimeExpr, name: String) -> Self {
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
        match self.kind {
            ResolvedComptimeExprKind::Name(resolution) => Some(resolution),
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
    fn new(function: EarlyComptimeFunction) -> Result<Self, ComptimeLowerError> {
        resolve_function(function)
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
    name: String,
    local_id: LocalId,
    ty: Option<InternedTyId>,
}

impl ResolvedComptimeParam {
    pub fn new(span: Span, name: String, local_id: LocalId, ty: Option<InternedTyId>) -> Self {
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

    pub fn name(&self) -> &str {
        &self.name
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
    name: String,
    local_id: LocalId,
    explicit_type: Option<InternedTyId>,
    is_mutable: bool,
    value: ResolvedComptimeExpr,
}

impl ResolvedComptimeBinding {
    pub fn new(
        span: Span,
        name: String,
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

    pub fn name(&self) -> &str {
        &self.name
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
        name: String,
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
        name: String,
        local_id: LocalId,
        path: Vec<ResolvedComptimeAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeAssignPathElem {
    kind: ResolvedComptimeAssignPathElemKind,
}

impl ResolvedComptimeAssignPathElem {
    pub fn field(span: Span, name: String) -> Self {
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
        name: String,
    },
    Index {
        span: Span,
        index: ResolvedComptimeExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeForIn {
    binding: ResolvedComptimeForBinding,
    iter: ResolvedComptimeExpr,
    body: ResolvedComptimeBlock,
}

impl ResolvedComptimeForIn {
    pub fn new(
        binding: ResolvedComptimeForBinding,
        iter: ResolvedComptimeExpr,
        body: ResolvedComptimeBlock,
    ) -> Self {
        Self {
            binding,
            iter,
            body,
        }
    }

    pub fn binding(&self) -> &ResolvedComptimeForBinding {
        &self.binding
    }

    pub fn iter(&self) -> &ResolvedComptimeExpr {
        &self.iter
    }

    pub fn body(&self) -> &ResolvedComptimeBlock {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeForBinding {
    span: Span,
    name: Option<String>,
    local_id: Option<LocalId>,
    pattern_kind: nia_ast::ForPatternKind,
}

impl ResolvedComptimeForBinding {
    pub fn new(
        span: Span,
        name: Option<String>,
        local_id: Option<LocalId>,
        pattern_kind: nia_ast::ForPatternKind,
    ) -> Self {
        Self {
            span,
            name,
            local_id,
            pattern_kind,
        }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn local_id(&self) -> Option<LocalId> {
        self.local_id
    }

    pub fn pattern_kind(&self) -> nia_ast::ForPatternKind {
        self.pattern_kind
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
    patterns: Vec<ResolvedComptimeSwitchPattern>,
    body: ResolvedComptimeSwitchArmBody,
}

impl ResolvedComptimeSwitchArm {
    pub fn new(
        span: Span,
        patterns: Vec<ResolvedComptimeSwitchPattern>,
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

    pub fn patterns(&self) -> &[ResolvedComptimeSwitchPattern] {
        &self.patterns
    }

    pub fn body(&self) -> &ResolvedComptimeSwitchArmBody {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedComptimeSwitchPattern {
    kind: ResolvedComptimeSwitchPatternKind,
}

impl ResolvedComptimeSwitchPattern {
    pub fn default() -> Self {
        Self {
            kind: ResolvedComptimeSwitchPatternKind::Default,
        }
    }

    pub fn optional_some(name: String, local_id: LocalId, span: Span) -> Self {
        Self {
            kind: ResolvedComptimeSwitchPatternKind::OptionalSome {
                name,
                local_id,
                span,
            },
        }
    }

    pub fn optional_null(span: Span) -> Self {
        Self {
            kind: ResolvedComptimeSwitchPatternKind::OptionalNull { span },
        }
    }

    pub fn error_ok(name: String, local_id: LocalId, span: Span) -> Self {
        Self {
            kind: ResolvedComptimeSwitchPatternKind::ErrorOk {
                name,
                local_id,
                span,
            },
        }
    }

    pub fn error_err(name: String, local_id: LocalId, span: Span) -> Self {
        Self {
            kind: ResolvedComptimeSwitchPatternKind::ErrorErr {
                name,
                local_id,
                span,
            },
        }
    }

    pub fn expr(expr: ResolvedComptimeExpr) -> Self {
        Self {
            kind: ResolvedComptimeSwitchPatternKind::Expr(expr),
        }
    }

    pub fn range(
        start: ResolvedComptimeExpr,
        end: ResolvedComptimeExpr,
        inclusive: bool,
        span: Span,
    ) -> Self {
        Self {
            kind: ResolvedComptimeSwitchPatternKind::Range {
                start,
                end,
                inclusive,
                span,
            },
        }
    }

    pub fn kind(&self) -> &ResolvedComptimeSwitchPatternKind {
        &self.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedComptimeSwitchPatternKind {
    Default,
    OptionalSome {
        name: String,
        local_id: LocalId,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        name: String,
        local_id: LocalId,
        span: Span,
    },
    ErrorErr {
        name: String,
        local_id: LocalId,
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
            kind: ResolvedComptimeSwitchArmBodyKind::Stmt(stmt),
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
    Stmt(ResolvedComptimeStmt),
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
    CString(ComptimeStringLiteral),
    Bool(bool),
    Null,
    Name(ComptimeNameResolution),
    Field {
        lhs: Box<ResolvedComptimeExpr>,
        name: String,
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
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: ResolvedComptimeTypeArg,
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
    name: String,
    value: ResolvedComptimeExpr,
}

impl ResolvedComptimeFieldInit {
    pub fn new(span: Span, name: String, value: ResolvedComptimeExpr) -> Self {
        Self { span, name, value }
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn name(&self) -> &str {
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
    pub name: String,
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
    ForIn(EarlyComptimeForIn),
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
    pub name: String,
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
        name: String,
        local_id: Option<LocalId>,
        path: Vec<EarlyComptimeAssignPathElem>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeAssignPathElem {
    Field {
        span: Span,
        name: String,
    },
    Index {
        span: Span,
        index: EarlyComptimeExpr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeForIn {
    pub binding: EarlyComptimeForBinding,
    pub iter: EarlyComptimeExpr,
    pub body: EarlyComptimeBlock,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeForBinding {
    pub span: Span,
    pub name: Option<String>,
    pub local_id: Option<LocalId>,
    pub pattern_kind: nia_ast::ForPatternKind,
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
    pub patterns: Vec<EarlyComptimeSwitchPattern>,
    pub body: EarlyComptimeSwitchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EarlyComptimeSwitchPattern {
    Default,
    OptionalSome {
        name: String,
        local_id: Option<LocalId>,
        span: Span,
    },
    OptionalNull {
        span: Span,
    },
    ErrorOk {
        name: String,
        local_id: Option<LocalId>,
        span: Span,
    },
    ErrorErr {
        name: String,
        local_id: Option<LocalId>,
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
    Stmt(EarlyComptimeStmt),
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
    Unresolved(String),
    Resolved {
        display: String,
        resolution: ComptimeNameResolution,
    },
}

impl EarlyComptimeName {
    pub fn unresolved(display: String) -> Self {
        Self::Unresolved(display)
    }

    pub fn resolved(display: String, resolution: ComptimeNameResolution) -> Self {
        Self::Resolved {
            display,
            resolution,
        }
    }

    pub fn display(&self) -> &str {
        match self {
            Self::Unresolved(display) | Self::Resolved { display, .. } => display,
        }
    }

    pub fn resolution(&self) -> Option<ComptimeNameResolution> {
        match self {
            Self::Unresolved(_) => None,
            Self::Resolved { resolution, .. } => Some(*resolution),
        }
    }

    fn into_resolution(self, span: Span) -> Result<ComptimeNameResolution, ComptimeLowerError> {
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
    CString(ComptimeStringLiteral),
    Bool(bool),
    Null,
    Ident(EarlyComptimeName),
    Qualified(EarlyComptimeName),
    Field {
        lhs: Box<EarlyComptimeExpr>,
        name: String,
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
    BuiltinValue(ValueBuiltin),
    LayoutBuiltin {
        builtin: LayoutBuiltin,
        type_arg: EarlyComptimeTypeArg,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComptimeNameResolution {
    Local(LocalId),
    Global(GlobalDefId),
    BuiltinAssociatedValue(BuiltinAssociatedValue),
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
    pub name: String,
    pub value: EarlyComptimeExpr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlyComptimeTypeArg {
    pub span: Span,
    pub ty_span: Span,
    pub ty: Option<InternedTyId>,
}

impl EarlyComptimeTypeArg {
    fn from_type_ref(
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

pub fn lower_expr_early(expr: &nia_ast::Expr) -> Result<EarlyComptimeExpr, ComptimeLowerError> {
    lower_expr_internal(expr, &EarlyComptimeLowerInputs::default())
}

pub fn lower_expr_early_with_context(
    expr: &nia_ast::Expr,
    context: &EarlyComptimeLowerInputs<'_>,
) -> Result<EarlyComptimeExpr, ComptimeLowerError> {
    lower_expr_internal(expr, context)
}

#[derive(Clone, Copy, Default)]
pub struct EarlyComptimeLowerInputs<'a> {
    pub semantic_uses: Option<&'a SemanticUseTable>,
}

impl<'a> EarlyComptimeLowerInputs<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_semantic_uses(mut self, semantic_uses: &'a SemanticUseTable) -> Self {
        self.semantic_uses = Some(semantic_uses);
        self
    }
}

#[derive(Clone, Copy)]
pub struct ResolvedComptimeLowerInputs<'a> {
    pub semantic_uses: &'a SemanticUseTable,
}

impl<'a> ResolvedComptimeLowerInputs<'a> {
    pub fn new(semantic_uses: &'a SemanticUseTable) -> Self {
        Self { semantic_uses }
    }
}

trait ComptimeLowerContext {
    fn resolve_name(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError>;

    fn lower_local_use(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError>;

    fn lower_local_id(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError>;

    fn lower_type_id(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<InternedTyId>, ComptimeLowerError>;
}

impl ComptimeLowerContext for EarlyComptimeLowerInputs<'_> {
    fn resolve_name(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
        Ok(self.semantic_uses.and_then(|semantic_uses| {
            semantic_uses
                .node_builtin_associated_value(key)
                .map(ComptimeNameResolution::BuiltinAssociatedValue)
                .or_else(|| {
                    semantic_uses
                        .node_value_use(key)
                        .map(ComptimeNameResolution::from)
                })
        }))
    }

    fn lower_local_use(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_value_use(key))
            .and_then(|value_use| match value_use {
                SemanticValueUse::Local(local_id) => Some(local_id),
                SemanticValueUse::Global(_) => None,
            }))
    }

    fn lower_local_id(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_local_def(key)))
    }

    fn lower_type_id(
        &self,
        key: &NodeKey,
        _span: Span,
    ) -> Result<Option<InternedTyId>, ComptimeLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_type_use(key)))
    }
}

impl ComptimeLowerContext for ResolvedComptimeLowerInputs<'_> {
    fn resolve_name(
        &self,
        key: &NodeKey,
        span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
        if let Some(value) = self.semantic_uses.node_builtin_associated_value(key) {
            return Ok(Some(ComptimeNameResolution::BuiltinAssociatedValue(value)));
        }
        self.semantic_uses
            .node_value_use(key)
            .map(ComptimeNameResolution::from)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime name"))
    }

    fn lower_local_use(
        &self,
        key: &NodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError> {
        match self.semantic_uses.node_value_use(key) {
            Some(SemanticValueUse::Local(local_id)) => Ok(Some(local_id)),
            Some(SemanticValueUse::Global(_)) | None => {
                Err(unresolved_error(span, "comptime assignment target"))
            }
        }
    }

    fn lower_local_id(
        &self,
        key: &NodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError> {
        self.semantic_uses
            .node_local_def(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime local binding"))
    }

    fn lower_type_id(
        &self,
        key: &NodeKey,
        span: Span,
    ) -> Result<Option<InternedTyId>, ComptimeLowerError> {
        self.semantic_uses
            .node_type_use(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime type"))
    }
}

fn lower_expr_internal(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeExpr, ComptimeLowerError> {
    let kind = match &expr.kind {
        nia_ast::ExprKind::Integer(text) => EarlyComptimeExprKind::Integer(text.clone()),
        nia_ast::ExprKind::Char(text) => EarlyComptimeExprKind::Char(text.clone()),
        nia_ast::ExprKind::ByteChar(text) => EarlyComptimeExprKind::ByteChar(text.clone()),
        nia_ast::ExprKind::Float(text) => EarlyComptimeExprKind::Float(text.clone()),
        nia_ast::ExprKind::String(literal) => {
            EarlyComptimeExprKind::String(lower_string_literal(literal))
        }
        nia_ast::ExprKind::ByteString(literal) => {
            EarlyComptimeExprKind::ByteString(lower_string_literal(literal))
        }
        nia_ast::ExprKind::CString(literal) => {
            EarlyComptimeExprKind::CString(lower_string_literal(literal))
        }
        nia_ast::ExprKind::Bool(value) => EarlyComptimeExprKind::Bool(*value),
        nia_ast::ExprKind::Null => EarlyComptimeExprKind::Null,
        nia_ast::ExprKind::Ident(name) => EarlyComptimeExprKind::Ident(lower_comptime_name(
            name,
            &expr.node_key,
            expr.span,
            context,
        )?),
        nia_ast::ExprKind::Qualified { name, .. } => EarlyComptimeExprKind::Qualified(
            lower_comptime_name(name, &expr.node_key, expr.span, context)?,
        ),
        nia_ast::ExprKind::Field { lhs, name } => EarlyComptimeExprKind::Field {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
            name: name.clone(),
        },
        nia_ast::ExprKind::BracketSuffix { callee, args } => {
            let [arg] = args.as_slice() else {
                return Err(ComptimeLowerError {
                    span: expr.span,
                    message: "comptime bracket suffix requires exactly one index argument"
                        .to_string(),
                });
            };
            let Some(index) = &arg.expr else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message: "comptime bracket suffix requires an expression index".to_string(),
                });
            };
            EarlyComptimeExprKind::Index {
                lhs: Box::new(lower_expr_internal(callee, context)?),
                index: Box::new(lower_expr_internal(index, context)?),
            }
        }
        nia_ast::ExprKind::Index { lhs, index } => match index {
            nia_ast::IndexArg::Expr(index) => EarlyComptimeExprKind::Index {
                lhs: Box::new(lower_expr_internal(lhs, context)?),
                index: Box::new(lower_expr_internal(index, context)?),
            },
            nia_ast::IndexArg::Range(range) => EarlyComptimeExprKind::Slice {
                lhs: Box::new(lower_expr_internal(lhs, context)?),
                range: lower_slice_range_with_context(range, context)?,
            },
        },
        nia_ast::ExprKind::ArrayLiteral { elems } => EarlyComptimeExprKind::ArrayLiteral {
            ty: None,
            elems: lower_array_elements_with_context(elems, context)?,
        },
        nia_ast::ExprKind::TypedArrayLiteral { ty, elems } => EarlyComptimeExprKind::ArrayLiteral {
            ty: lower_type_id(context, &ty.node_key, ty.span)?,
            elems: lower_array_elements_with_context(elems, context)?,
        },
        nia_ast::ExprKind::StructLiteral { fields } => EarlyComptimeExprKind::StructLiteral {
            ty: None,
            fields: fields
                .iter()
                .map(|field| lower_field_init_with_context(field, context))
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::TypedStructLiteral { ty, fields } => {
            EarlyComptimeExprKind::StructLiteral {
                ty: lower_type_id(context, &ty.node_key, ty.span)?,
                fields: fields
                    .iter()
                    .map(|field| lower_field_init_with_context(field, context))
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        nia_ast::ExprKind::Builtin { name, type_arg } => {
            if let Some(type_arg) = type_arg {
                let Some(builtin) = LayoutBuiltin::from_name(name) else {
                    return Err(ComptimeLowerError {
                        span: expr.span,
                        message: format!("unsupported builtin in comptime expression: @{name}"),
                    });
                };
                EarlyComptimeExprKind::LayoutBuiltin {
                    builtin,
                    type_arg: EarlyComptimeTypeArg::from_type_ref(type_arg, context)?,
                }
            } else {
                let Some(builtin) = ValueBuiltin::from_name(name) else {
                    return Err(ComptimeLowerError {
                        span: expr.span,
                        message: format!(
                            "unsupported builtin value in comptime expression: @{name}"
                        ),
                    });
                };
                EarlyComptimeExprKind::BuiltinValue(builtin)
            }
        }
        nia_ast::ExprKind::Call { callee, args } => lower_call_with_context(callee, args, context)?,
        nia_ast::ExprKind::Unary { op, expr } => EarlyComptimeExprKind::Unary {
            op: lower_unary_op(*op),
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::OptionalSome { expr } => EarlyComptimeExprKind::OptionalSome {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::ErrorOk { expr } => EarlyComptimeExprKind::ErrorOk {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::ErrorErr { expr } => EarlyComptimeExprKind::ErrorErr {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::Try { expr } => EarlyComptimeExprKind::Try {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::Binary { lhs, op, rhs } => EarlyComptimeExprKind::Binary {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
            op: lower_binary_op(*op),
            rhs: Box::new(lower_expr_internal(rhs, context)?),
        },
        nia_ast::ExprKind::Assign { lhs, op, rhs } => {
            EarlyComptimeExprKind::Assign(Box::new(EarlyComptimeAssign {
                lhs: lower_assign_target_with_context(lhs, context)?,
                op: lower_assign_op(*op),
                rhs: lower_expr_internal(rhs, context)?,
            }))
        }
        nia_ast::ExprKind::Range(range) => {
            EarlyComptimeExprKind::Range(lower_comptime_range_with_context(range, context)?)
        }
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => EarlyComptimeExprKind::If {
            cond: Box::new(lower_expr_internal(cond, context)?),
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_expr_internal(else_branch, context))
                .transpose()?
                .map(Box::new),
        },
        nia_ast::ExprKind::ComptimeIf(comptime_if) => {
            lower_comptime_if_with_context(comptime_if, context)?
        }
        nia_ast::ExprKind::Switch(switch) => EarlyComptimeExprKind::Switch(Box::new(
            lower_switch_with_context(expr.span, switch, context)?,
        )),
        nia_ast::ExprKind::Cast { expr, ty } => EarlyComptimeExprKind::Cast {
            expr: Box::new(lower_expr_internal(expr, context)?),
            ty: lower_type_id(context, &ty.node_key, ty.span)?,
        },
        nia_ast::ExprKind::Block(block) => {
            EarlyComptimeExprKind::Block(lower_block_with_context(block, context)?)
        }
        _ => {
            return Err(ComptimeLowerError {
                span: expr.span,
                message: "unsupported comptime expression".to_string(),
            });
        }
    };
    Ok(EarlyComptimeExpr {
        span: expr.span,
        kind,
    })
}

pub fn lower_expr_resolved_with_context(
    expr: &nia_ast::Expr,
    context: &ResolvedComptimeLowerInputs<'_>,
) -> Result<ResolvedComptimeExpr, ComptimeLowerError> {
    let expr = lower_expr_internal(expr, context)?;
    ResolvedComptimeExpr::new(expr)
}

fn lower_string_literal(literal: &nia_ast::StringLiteral) -> ComptimeStringLiteral {
    ComptimeStringLiteral {
        parts: literal.parts.clone(),
    }
}

fn lower_unary_op(op: nia_ast::UnaryOp) -> ComptimeUnaryOp {
    match op {
        nia_ast::UnaryOp::Neg => ComptimeUnaryOp::Neg,
        nia_ast::UnaryOp::Not => ComptimeUnaryOp::Not,
        nia_ast::UnaryOp::BitNot => ComptimeUnaryOp::BitNot,
        nia_ast::UnaryOp::RefReadOnly => ComptimeUnaryOp::RefReadOnly,
        nia_ast::UnaryOp::Ref => ComptimeUnaryOp::Ref,
        nia_ast::UnaryOp::Deref => ComptimeUnaryOp::Deref,
    }
}

fn lower_binary_op(op: nia_ast::BinaryOp) -> ComptimeBinaryOp {
    match op {
        nia_ast::BinaryOp::Mul => ComptimeBinaryOp::Mul,
        nia_ast::BinaryOp::Div => ComptimeBinaryOp::Div,
        nia_ast::BinaryOp::Rem => ComptimeBinaryOp::Rem,
        nia_ast::BinaryOp::Add => ComptimeBinaryOp::Add,
        nia_ast::BinaryOp::Sub => ComptimeBinaryOp::Sub,
        nia_ast::BinaryOp::Shl => ComptimeBinaryOp::Shl,
        nia_ast::BinaryOp::Shr => ComptimeBinaryOp::Shr,
        nia_ast::BinaryOp::Lt => ComptimeBinaryOp::Lt,
        nia_ast::BinaryOp::Le => ComptimeBinaryOp::Le,
        nia_ast::BinaryOp::Gt => ComptimeBinaryOp::Gt,
        nia_ast::BinaryOp::Ge => ComptimeBinaryOp::Ge,
        nia_ast::BinaryOp::Eq => ComptimeBinaryOp::Eq,
        nia_ast::BinaryOp::Ne => ComptimeBinaryOp::Ne,
        nia_ast::BinaryOp::BitAnd => ComptimeBinaryOp::BitAnd,
        nia_ast::BinaryOp::BitXor => ComptimeBinaryOp::BitXor,
        nia_ast::BinaryOp::BitOr => ComptimeBinaryOp::BitOr,
        nia_ast::BinaryOp::And => ComptimeBinaryOp::And,
        nia_ast::BinaryOp::Or => ComptimeBinaryOp::Or,
    }
}

fn lower_assign_op(op: nia_ast::AssignOp) -> ComptimeAssignOp {
    match op {
        nia_ast::AssignOp::Assign => ComptimeAssignOp::Assign,
        nia_ast::AssignOp::Add => ComptimeAssignOp::Add,
        nia_ast::AssignOp::Sub => ComptimeAssignOp::Sub,
        nia_ast::AssignOp::Shl => ComptimeAssignOp::Shl,
        nia_ast::AssignOp::Shr => ComptimeAssignOp::Shr,
        nia_ast::AssignOp::Mul => ComptimeAssignOp::Mul,
        nia_ast::AssignOp::Div => ComptimeAssignOp::Div,
        nia_ast::AssignOp::Rem => ComptimeAssignOp::Rem,
        nia_ast::AssignOp::BitAnd => ComptimeAssignOp::BitAnd,
        nia_ast::AssignOp::BitXor => ComptimeAssignOp::BitXor,
        nia_ast::AssignOp::BitOr => ComptimeAssignOp::BitOr,
    }
}

fn lower_call_with_context(
    callee: &nia_ast::Expr,
    args: &[nia_ast::Expr],
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeExprKind, ComptimeLowerError> {
    if let nia_ast::ExprKind::Builtin { name, type_arg } = &callee.kind {
        if name == "error" {
            if type_arg.is_some() {
                return Err(ComptimeLowerError {
                    span: callee.span,
                    message: "builtin `@error` does not take a type argument".to_string(),
                });
            }
            if args.len() != 1 {
                return Err(ComptimeLowerError {
                    span: callee.span,
                    message: "builtin `@error` requires exactly one message argument".to_string(),
                });
            }
            return Ok(EarlyComptimeExprKind::CompileError {
                message: Box::new(lower_expr_internal(&args[0], context)?),
            });
        }
        if type_arg.is_none() && ValueBuiltin::from_name(name).is_none() {
            return Err(ComptimeLowerError {
                span: callee.span,
                message: format!("unsupported builtin call in comptime expression: @{name}"),
            });
        }
    }
    if args.is_empty()
        && let nia_ast::ExprKind::Field { lhs, name } = &callee.kind
        && let Some(method) = comptime_builtin_method_name(name)
    {
        return Ok(EarlyComptimeExprKind::BuiltinMethod {
            method,
            lhs: Box::new(lower_expr_internal(lhs, context)?),
        });
    }
    let (callee, type_args) = match &callee.kind {
        nia_ast::ExprKind::BracketSuffix {
            callee: generic_callee,
            args: bracket_args,
        } if bracket_args.iter().all(|arg| arg.ty.is_some()) => (
            generic_callee.as_ref(),
            lower_type_args_with_context(bracket_args, context)?,
        ),
        _ => (callee, Vec::new()),
    };
    Ok(EarlyComptimeExprKind::Call {
        callee: Box::new(lower_expr_internal(callee, context)?),
        type_args,
        args: args
            .iter()
            .map(|arg| lower_expr_internal(arg, context))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn comptime_builtin_method_name(name: &str) -> Option<BuiltinTraitMethod> {
    match name {
        "len" => Some(BuiltinTraitMethod::Len),
        "start" => Some(BuiltinTraitMethod::Start),
        "end" => Some(BuiltinTraitMethod::End),
        _ => None,
    }
}

fn lower_comptime_range_with_context(
    range: &nia_ast::SliceRange,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeRange, ComptimeLowerError> {
    Ok(EarlyComptimeRange {
        start: range
            .start
            .as_deref()
            .map(|start| lower_expr_internal(start, context))
            .transpose()?
            .map(Box::new),
        end: range
            .end
            .as_deref()
            .map(|end| lower_expr_internal(end, context))
            .transpose()?
            .map(Box::new),
        inclusive: range.inclusive,
    })
}

fn lower_slice_range_with_context(
    range: &nia_ast::SliceRange,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeSliceRange, ComptimeLowerError> {
    let range = lower_comptime_range_with_context(range, context)?;
    Ok(EarlyComptimeSliceRange {
        start: range.start,
        end: range.end,
        inclusive: range.inclusive,
    })
}

fn lower_comptime_if_with_context(
    comptime_if: &nia_ast::ComptimeIfExpr,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeExprKind, ComptimeLowerError> {
    Ok(EarlyComptimeExprKind::If {
        cond: Box::new(lower_expr_internal(&comptime_if.cond, context)?),
        then_branch: lower_block_with_context(&comptime_if.then_branch, context)?,
        else_branch: comptime_if
            .else_branch
            .as_deref()
            .map(|else_branch| lower_expr_internal(else_branch, context))
            .transpose()?
            .map(Box::new),
    })
}

fn lower_type_args_with_context(
    args: &[nia_ast::BracketArg],
    context: &dyn ComptimeLowerContext,
) -> Result<Vec<EarlyComptimeTypeArg>, ComptimeLowerError> {
    args.iter()
        .map(|arg| {
            let Some(ty) = &arg.ty else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message: "comptime generic function arguments must be types".to_string(),
                });
            };
            Ok(EarlyComptimeTypeArg {
                span: arg.span,
                ty_span: ty.span,
                ty: lower_type_id(context, &ty.node_key, ty.span)?,
            })
        })
        .collect()
}

fn lower_assign_target_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeAssignTarget, ComptimeLowerError> {
    let mut path = Vec::new();
    let (span, name, local_id) = lower_assign_target_base_with_context(expr, context, &mut path)?;
    Ok(EarlyComptimeAssignTarget::Local {
        span,
        name,
        local_id,
        path,
    })
}

fn lower_assign_target_base_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
    path: &mut Vec<EarlyComptimeAssignPathElem>,
) -> Result<(Span, String, Option<LocalId>), ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::Ident(name) => Ok((
            expr.span,
            name.clone(),
            lower_local_use(context, &expr.node_key, expr.span)?,
        )),
        nia_ast::ExprKind::Field { lhs, name } => {
            let base = lower_assign_target_base_with_context(lhs, context, path)?;
            path.push(EarlyComptimeAssignPathElem::Field {
                span: expr.span,
                name: name.clone(),
            });
            Ok(base)
        }
        nia_ast::ExprKind::Index { lhs, index } => {
            let base = lower_assign_target_base_with_context(lhs, context, path)?;
            let nia_ast::IndexArg::Expr(index) = index else {
                return Err(ComptimeLowerError {
                    span: expr.span,
                    message: "comptime assignment target does not support slicing".to_string(),
                });
            };
            path.push(EarlyComptimeAssignPathElem::Index {
                span: expr.span,
                index: lower_expr_internal(index, context)?,
            });
            Ok(base)
        }
        nia_ast::ExprKind::BracketSuffix { callee, args } => {
            let base = lower_assign_target_base_with_context(callee, context, path)?;
            let [arg] = args.as_slice() else {
                return Err(ComptimeLowerError {
                    span: expr.span,
                    message: "comptime assignment target bracket suffix requires exactly one index argument".to_string(),
                });
            };
            let Some(index) = &arg.expr else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message:
                        "comptime assignment target bracket suffix requires an expression index"
                            .to_string(),
                });
            };
            path.push(EarlyComptimeAssignPathElem::Index {
                span: expr.span,
                index: lower_expr_internal(index, context)?,
            });
            Ok(base)
        }
        _ => Err(ComptimeLowerError {
            span: expr.span,
            message: "unsupported comptime assignment target".to_string(),
        }),
    }
}

fn lower_array_elements_with_context(
    elems: &nia_ast::ArrayElements,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeArrayElements, ComptimeLowerError> {
    match elems {
        nia_ast::ArrayElements::List(elems) => Ok(EarlyComptimeArrayElements::List(
            elems
                .iter()
                .map(|elem| lower_expr_internal(elem, context))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        nia_ast::ArrayElements::Repeat { value, count } => Ok(EarlyComptimeArrayElements::Repeat {
            value: Box::new(lower_expr_internal(value, context)?),
            count: Box::new(lower_expr_internal(count, context)?),
        }),
    }
}

fn resolve_name(
    context: &dyn ComptimeLowerContext,
    key: &NodeKey,
    span: Span,
) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
    context.resolve_name(key, span)
}

fn lower_comptime_name(
    name: &str,
    key: &NodeKey,
    span: Span,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeName, ComptimeLowerError> {
    match resolve_name(context, key, span)? {
        Some(resolution) => Ok(EarlyComptimeName::resolved(name.to_string(), resolution)),
        None => Ok(EarlyComptimeName::unresolved(name.to_string())),
    }
}

fn lower_local_id(
    context: &dyn ComptimeLowerContext,
    key: &NodeKey,
    span: Span,
) -> Result<Option<LocalId>, ComptimeLowerError> {
    context.lower_local_id(key, span)
}

fn lower_local_use(
    context: &dyn ComptimeLowerContext,
    key: &NodeKey,
    span: Span,
) -> Result<Option<LocalId>, ComptimeLowerError> {
    context.lower_local_use(key, span)
}

fn lower_type_id(
    context: &dyn ComptimeLowerContext,
    key: &NodeKey,
    span: Span,
) -> Result<Option<InternedTyId>, ComptimeLowerError> {
    context.lower_type_id(key, span)
}

pub fn resolve_function(
    function: EarlyComptimeFunction,
) -> Result<ResolvedComptimeFunction, ComptimeLowerError> {
    let params = function
        .params
        .into_iter()
        .map(resolve_comptime_param)
        .collect::<Result<Vec<_>, _>>()?;
    let body = resolve_comptime_block(function.body)?;
    Ok(ResolvedComptimeFunction {
        span: function.span,
        params,
        body,
    })
}

fn resolve_comptime_param(
    param: EarlyComptimeParam,
) -> Result<ResolvedComptimeParam, ComptimeLowerError> {
    let local_id = param
        .local_id
        .ok_or_else(|| unresolved_error(param.span, "comptime function parameter local"))?;
    Ok(ResolvedComptimeParam::new(
        param.span, param.name, local_id, param.ty,
    ))
}

fn resolve_comptime_block(
    block: EarlyComptimeBlock,
) -> Result<ResolvedComptimeBlock, ComptimeLowerError> {
    let stmts = block
        .stmts
        .into_iter()
        .map(resolve_comptime_stmt)
        .collect::<Result<Vec<_>, _>>()?;
    let tail = block
        .tail
        .map(|tail| resolve_expr(*tail).map(Box::new))
        .transpose()?;
    Ok(ResolvedComptimeBlock::new(block.span, stmts, tail))
}

fn resolve_comptime_stmt(
    stmt: EarlyComptimeStmt,
) -> Result<ResolvedComptimeStmt, ComptimeLowerError> {
    let kind = match stmt.kind {
        EarlyComptimeStmtKind::Binding(binding) => {
            ResolvedComptimeStmtKind::Binding(resolve_comptime_binding(binding)?)
        }
        EarlyComptimeStmtKind::Expr(expr) => ResolvedComptimeStmtKind::Expr(resolve_expr(expr)?),
        EarlyComptimeStmtKind::Return(expr) => {
            ResolvedComptimeStmtKind::Return(expr.map(resolve_expr).transpose()?)
        }
        EarlyComptimeStmtKind::Break => ResolvedComptimeStmtKind::Break,
        EarlyComptimeStmtKind::Continue => ResolvedComptimeStmtKind::Continue,
        EarlyComptimeStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => ResolvedComptimeStmtKind::If {
            cond: resolve_expr(cond)?,
            then_branch: resolve_comptime_block(then_branch)?,
            else_branch: else_branch.map(resolve_comptime_block).transpose()?,
        },
        EarlyComptimeStmtKind::ForIn(for_in) => {
            ResolvedComptimeStmtKind::ForIn(resolve_comptime_for_in(for_in)?)
        }
        EarlyComptimeStmtKind::While { cond, body } => ResolvedComptimeStmtKind::While {
            cond: resolve_expr(cond)?,
            body: resolve_comptime_block(body)?,
        },
        EarlyComptimeStmtKind::Loop { body } => ResolvedComptimeStmtKind::Loop {
            body: resolve_comptime_block(body)?,
        },
    };
    Ok(ResolvedComptimeStmt::new(stmt.span, kind))
}

fn resolve_comptime_binding(
    binding: EarlyComptimeBinding,
) -> Result<ResolvedComptimeBinding, ComptimeLowerError> {
    let local_id = binding
        .local_id
        .ok_or_else(|| unresolved_error(binding.span, "comptime local binding"))?;
    Ok(ResolvedComptimeBinding::new(
        binding.span,
        binding.name,
        local_id,
        binding.explicit_type,
        binding.is_mutable,
        resolve_expr(binding.value)?,
    ))
}

fn resolve_comptime_for_in(
    for_in: EarlyComptimeForIn,
) -> Result<ResolvedComptimeForIn, ComptimeLowerError> {
    if for_in.binding.name.is_some() && for_in.binding.local_id.is_none() {
        return Err(unresolved_error(
            for_in.binding.span,
            "comptime for binding",
        ));
    }
    Ok(ResolvedComptimeForIn::new(
        ResolvedComptimeForBinding::new(
            for_in.binding.span,
            for_in.binding.name,
            for_in.binding.local_id,
            for_in.binding.pattern_kind,
        ),
        resolve_expr(for_in.iter)?,
        resolve_comptime_block(for_in.body)?,
    ))
}

pub fn resolve_expr(expr: EarlyComptimeExpr) -> Result<ResolvedComptimeExpr, ComptimeLowerError> {
    let span = expr.span;
    let kind = match expr.kind {
        EarlyComptimeExprKind::Integer(value) => ResolvedComptimeExprKind::Integer(value),
        EarlyComptimeExprKind::Char(value) => ResolvedComptimeExprKind::Char(value),
        EarlyComptimeExprKind::ByteChar(value) => ResolvedComptimeExprKind::ByteChar(value),
        EarlyComptimeExprKind::Float(value) => ResolvedComptimeExprKind::Float(value),
        EarlyComptimeExprKind::String(value) => ResolvedComptimeExprKind::String(value),
        EarlyComptimeExprKind::ByteString(value) => ResolvedComptimeExprKind::ByteString(value),
        EarlyComptimeExprKind::CString(value) => ResolvedComptimeExprKind::CString(value),
        EarlyComptimeExprKind::Bool(value) => ResolvedComptimeExprKind::Bool(value),
        EarlyComptimeExprKind::Null => ResolvedComptimeExprKind::Null,
        EarlyComptimeExprKind::Ident(name) | EarlyComptimeExprKind::Qualified(name) => {
            ResolvedComptimeExprKind::Name(name.into_resolution(span)?)
        }
        EarlyComptimeExprKind::Field { lhs, name } => ResolvedComptimeExprKind::Field {
            lhs: Box::new(resolve_expr(*lhs)?),
            name,
        },
        EarlyComptimeExprKind::BuiltinMethod { method, lhs } => {
            ResolvedComptimeExprKind::BuiltinMethod {
                method,
                lhs: Box::new(resolve_expr(*lhs)?),
            }
        }
        EarlyComptimeExprKind::Index { lhs, index } => ResolvedComptimeExprKind::Index {
            lhs: Box::new(resolve_expr(*lhs)?),
            index: Box::new(resolve_expr(*index)?),
        },
        EarlyComptimeExprKind::Slice { lhs, range } => ResolvedComptimeExprKind::Slice {
            lhs: Box::new(resolve_expr(*lhs)?),
            range: resolve_comptime_slice_range(range)?,
        },
        EarlyComptimeExprKind::ArrayLiteral { ty, elems } => {
            ResolvedComptimeExprKind::ArrayLiteral {
                ty,
                elems: resolve_comptime_array_elements(elems)?,
            }
        }
        EarlyComptimeExprKind::StructLiteral { ty, fields } => {
            ResolvedComptimeExprKind::StructLiteral {
                ty,
                fields: fields
                    .into_iter()
                    .map(resolve_comptime_field_init)
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        EarlyComptimeExprKind::CompileError { message } => ResolvedComptimeExprKind::CompileError {
            message: Box::new(resolve_expr(*message)?),
        },
        EarlyComptimeExprKind::BuiltinValue(builtin) => {
            ResolvedComptimeExprKind::BuiltinValue(builtin)
        }
        EarlyComptimeExprKind::LayoutBuiltin { builtin, type_arg } => {
            ResolvedComptimeExprKind::LayoutBuiltin {
                builtin,
                type_arg: resolve_type_arg(type_arg)?,
            }
        }
        EarlyComptimeExprKind::Call {
            callee,
            type_args,
            args,
        } => ResolvedComptimeExprKind::Call {
            callee: Box::new(resolve_expr(*callee)?),
            type_args: type_args
                .into_iter()
                .map(resolve_type_arg)
                .collect::<Result<Vec<_>, _>>()?,
            args: args
                .into_iter()
                .map(resolve_expr)
                .collect::<Result<Vec<_>, _>>()?,
        },
        EarlyComptimeExprKind::Unary { op, expr } => ResolvedComptimeExprKind::Unary {
            op,
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyComptimeExprKind::OptionalSome { expr } => ResolvedComptimeExprKind::OptionalSome {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyComptimeExprKind::ErrorOk { expr } => ResolvedComptimeExprKind::ErrorOk {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyComptimeExprKind::ErrorErr { expr } => ResolvedComptimeExprKind::ErrorErr {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyComptimeExprKind::Try { expr } => ResolvedComptimeExprKind::Try {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyComptimeExprKind::Binary { lhs, op, rhs } => ResolvedComptimeExprKind::Binary {
            lhs: Box::new(resolve_expr(*lhs)?),
            op,
            rhs: Box::new(resolve_expr(*rhs)?),
        },
        EarlyComptimeExprKind::Assign(assign) => {
            ResolvedComptimeExprKind::Assign(Box::new(resolve_comptime_assign(*assign)?))
        }
        EarlyComptimeExprKind::Range(range) => {
            ResolvedComptimeExprKind::Range(resolve_comptime_range(range)?)
        }
        EarlyComptimeExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => ResolvedComptimeExprKind::If {
            cond: Box::new(resolve_expr(*cond)?),
            then_branch: resolve_comptime_block(then_branch)?,
            else_branch: else_branch
                .map(|else_branch| resolve_expr(*else_branch).map(Box::new))
                .transpose()?,
        },
        EarlyComptimeExprKind::Switch(switch) => {
            ResolvedComptimeExprKind::Switch(Box::new(resolve_comptime_switch(*switch)?))
        }
        EarlyComptimeExprKind::Cast { expr, ty } => ResolvedComptimeExprKind::Cast {
            expr: Box::new(resolve_expr(*expr)?),
            ty: ty.ok_or_else(|| unresolved_error(span, "comptime cast type"))?,
        },
        EarlyComptimeExprKind::Block(block) => {
            ResolvedComptimeExprKind::Block(resolve_comptime_block(block)?)
        }
    };
    Ok(ResolvedComptimeExpr { span, kind })
}

fn resolve_comptime_assign(
    assign: EarlyComptimeAssign,
) -> Result<ResolvedComptimeAssign, ComptimeLowerError> {
    Ok(ResolvedComptimeAssign::new(
        resolve_comptime_assign_target(assign.lhs)?,
        assign.op,
        resolve_expr(assign.rhs)?,
    ))
}

fn resolve_comptime_assign_target(
    target: EarlyComptimeAssignTarget,
) -> Result<ResolvedComptimeAssignTarget, ComptimeLowerError> {
    match target {
        EarlyComptimeAssignTarget::Local {
            span,
            name,
            local_id,
            path,
        } => {
            let local_id =
                local_id.ok_or_else(|| unresolved_error(span, "comptime assignment target"))?;
            Ok(ResolvedComptimeAssignTarget::local(
                span,
                name,
                local_id,
                path.into_iter()
                    .map(resolve_comptime_assign_path_elem)
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
    }
}

fn resolve_comptime_assign_path_elem(
    elem: EarlyComptimeAssignPathElem,
) -> Result<ResolvedComptimeAssignPathElem, ComptimeLowerError> {
    match elem {
        EarlyComptimeAssignPathElem::Field { span, name } => {
            Ok(ResolvedComptimeAssignPathElem::field(span, name))
        }
        EarlyComptimeAssignPathElem::Index { span, index } => Ok(
            ResolvedComptimeAssignPathElem::index(span, resolve_expr(index)?),
        ),
    }
}

fn resolve_comptime_switch(
    switch: EarlyComptimeSwitch,
) -> Result<ResolvedComptimeSwitch, ComptimeLowerError> {
    Ok(ResolvedComptimeSwitch::new(
        switch.span,
        resolve_expr(switch.target)?,
        switch
            .arms
            .into_iter()
            .map(resolve_comptime_switch_arm)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

fn resolve_comptime_switch_arm(
    arm: EarlyComptimeSwitchArm,
) -> Result<ResolvedComptimeSwitchArm, ComptimeLowerError> {
    Ok(ResolvedComptimeSwitchArm::new(
        arm.span,
        arm.patterns
            .into_iter()
            .map(resolve_comptime_switch_pattern)
            .collect::<Result<Vec<_>, _>>()?,
        resolve_comptime_switch_arm_body(arm.body)?,
    ))
}

fn resolve_comptime_switch_pattern(
    pattern: EarlyComptimeSwitchPattern,
) -> Result<ResolvedComptimeSwitchPattern, ComptimeLowerError> {
    match pattern {
        EarlyComptimeSwitchPattern::Default => Ok(ResolvedComptimeSwitchPattern::default()),
        EarlyComptimeSwitchPattern::OptionalSome {
            name,
            local_id,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::optional_some(
            name,
            local_id.ok_or_else(|| unresolved_error(span, "comptime switch pattern local"))?,
            span,
        )),
        EarlyComptimeSwitchPattern::OptionalNull { span } => {
            Ok(ResolvedComptimeSwitchPattern::optional_null(span))
        }
        EarlyComptimeSwitchPattern::ErrorOk {
            name,
            local_id,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::error_ok(
            name,
            local_id.ok_or_else(|| unresolved_error(span, "comptime switch pattern local"))?,
            span,
        )),
        EarlyComptimeSwitchPattern::ErrorErr {
            name,
            local_id,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::error_err(
            name,
            local_id.ok_or_else(|| unresolved_error(span, "comptime switch pattern local"))?,
            span,
        )),
        EarlyComptimeSwitchPattern::Expr(expr) => {
            resolve_expr(expr).map(ResolvedComptimeSwitchPattern::expr)
        }
        EarlyComptimeSwitchPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => Ok(ResolvedComptimeSwitchPattern::range(
            resolve_expr(start)?,
            resolve_expr(end)?,
            inclusive,
            span,
        )),
    }
}

fn resolve_comptime_switch_arm_body(
    body: EarlyComptimeSwitchArmBody,
) -> Result<ResolvedComptimeSwitchArmBody, ComptimeLowerError> {
    match body {
        EarlyComptimeSwitchArmBody::Expr(expr) => {
            resolve_expr(expr).map(ResolvedComptimeSwitchArmBody::expr)
        }
        EarlyComptimeSwitchArmBody::Stmt(stmt) => {
            resolve_comptime_stmt(stmt).map(ResolvedComptimeSwitchArmBody::stmt)
        }
        EarlyComptimeSwitchArmBody::Block(block) => {
            resolve_comptime_block(block).map(ResolvedComptimeSwitchArmBody::block)
        }
    }
}

fn resolve_comptime_array_elements(
    elems: EarlyComptimeArrayElements,
) -> Result<ResolvedComptimeArrayElements, ComptimeLowerError> {
    match elems {
        EarlyComptimeArrayElements::List(elems) => elems
            .into_iter()
            .map(resolve_expr)
            .collect::<Result<Vec<_>, _>>()
            .map(ResolvedComptimeArrayElements::list),
        EarlyComptimeArrayElements::Repeat { value, count } => Ok(
            ResolvedComptimeArrayElements::repeat(resolve_expr(*value)?, resolve_expr(*count)?),
        ),
    }
}

fn resolve_comptime_range(
    range: EarlyComptimeRange,
) -> Result<ResolvedComptimeRange, ComptimeLowerError> {
    Ok(ResolvedComptimeRange::new(
        range
            .start
            .map(|start| resolve_expr(*start).map(Box::new))
            .transpose()?,
        range
            .end
            .map(|end| resolve_expr(*end).map(Box::new))
            .transpose()?,
        range.inclusive,
    ))
}

fn resolve_comptime_slice_range(
    range: EarlyComptimeSliceRange,
) -> Result<ResolvedComptimeSliceRange, ComptimeLowerError> {
    Ok(ResolvedComptimeSliceRange::new(
        range
            .start
            .map(|start| resolve_expr(*start).map(Box::new))
            .transpose()?,
        range
            .end
            .map(|end| resolve_expr(*end).map(Box::new))
            .transpose()?,
        range.inclusive,
    ))
}

fn resolve_comptime_field_init(
    field: EarlyComptimeFieldInit,
) -> Result<ResolvedComptimeFieldInit, ComptimeLowerError> {
    Ok(ResolvedComptimeFieldInit::new(
        field.span,
        field.name,
        resolve_expr(field.value)?,
    ))
}

pub fn resolve_type_arg(
    type_arg: EarlyComptimeTypeArg,
) -> Result<ResolvedComptimeTypeArg, ComptimeLowerError> {
    Ok(ResolvedComptimeTypeArg::new(
        type_arg.span,
        type_arg.ty_span,
        type_arg
            .ty
            .ok_or_else(|| unresolved_error(type_arg.ty_span, "comptime type argument"))?,
    ))
}

fn unresolved_error(span: Span, what: &str) -> ComptimeLowerError {
    ComptimeLowerError {
        span,
        message: format!("failed to resolve {what}"),
    }
}

pub fn lower_function_early(
    function_span: Span,
    function: &nia_ast::FunctionItem,
) -> Result<EarlyComptimeFunction, ComptimeLowerError> {
    lower_function_internal(
        function_span,
        function,
        &EarlyComptimeLowerInputs::default(),
    )
}

fn lower_function_internal(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeFunction, ComptimeLowerError> {
    if !function.is_comptime || function.is_extern {
        return Err(ComptimeLowerError {
            span: function_span,
            message: "comptime expression can only call `comptime fn`".to_string(),
        });
    }
    let Some(body) = &function.body else {
        return Err(ComptimeLowerError {
            span: function_span,
            message: "comptime function requires a body".to_string(),
        });
    };
    let params = function
        .params
        .iter()
        .map(|param| {
            let Some(name) = &param.name else {
                return Err(ComptimeLowerError {
                    span: param.span,
                    message: "comptime function parameter requires a name".to_string(),
                });
            };
            Ok(EarlyComptimeParam {
                span: param.span,
                name: name.clone(),
                local_id: lower_local_id(context, &param.node_key, param.span)?,
                ty: param
                    .ty
                    .as_ref()
                    .map(|ty| lower_type_id(context, &ty.node_key, ty.span))
                    .transpose()?
                    .flatten(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EarlyComptimeFunction {
        span: function_span,
        params,
        body: lower_block_with_context(body, context)?,
    })
}

pub fn lower_function_resolved_with_context(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &ResolvedComptimeLowerInputs<'_>,
) -> Result<ResolvedComptimeFunction, ComptimeLowerError> {
    let function = lower_function_internal(function_span, function, context)?;
    ResolvedComptimeFunction::new(function)
}

fn lower_block_with_context(
    block: &nia_ast::Block,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeBlock, ComptimeLowerError> {
    Ok(EarlyComptimeBlock {
        span: block.span,
        stmts: block
            .stmts
            .iter()
            .map(|stmt| lower_stmt_with_context(stmt, context))
            .collect::<Result<Vec<_>, _>>()?,
        tail: block
            .tail
            .as_deref()
            .map(|tail| lower_expr_internal(tail, context))
            .transpose()?
            .map(Box::new),
    })
}

fn lower_stmt_with_context(
    stmt: &nia_ast::Stmt,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeStmt, ComptimeLowerError> {
    let kind = match &stmt.kind {
        nia_ast::StmtKind::Binding(binding) => {
            let Some(value) = &binding.value else {
                return Err(ComptimeLowerError {
                    span: stmt.span,
                    message: "comptime function binding requires an initializer".to_string(),
                });
            };
            EarlyComptimeStmtKind::Binding(EarlyComptimeBinding {
                span: stmt.span,
                name: binding.name.clone(),
                local_id: lower_local_id(context, &stmt.node_key, stmt.span)?,
                explicit_type: binding
                    .ty
                    .as_ref()
                    .map(|ty| lower_type_id(context, &ty.node_key, ty.span))
                    .transpose()?
                    .flatten(),
                is_mutable: !binding.is_let,
                value: lower_expr_internal(value, context)?,
            })
        }
        nia_ast::StmtKind::Expr(expr) => lower_expr_stmt_with_context(expr, context)?,
        nia_ast::StmtKind::Return(value) => EarlyComptimeStmtKind::Return(
            value
                .as_ref()
                .map(|value| lower_expr_internal(value, context))
                .transpose()?,
        ),
        nia_ast::StmtKind::Break => EarlyComptimeStmtKind::Break,
        nia_ast::StmtKind::Continue => EarlyComptimeStmtKind::Continue,
        nia_ast::StmtKind::ForIn(for_in) => EarlyComptimeStmtKind::ForIn(EarlyComptimeForIn {
            binding: EarlyComptimeForBinding {
                span: for_in.pattern.span,
                name: for_in.pattern.name.clone(),
                local_id: if for_in.pattern.name.is_some() {
                    lower_local_id(context, &for_in.pattern.node_key, for_in.pattern.span)?
                } else {
                    None
                },
                pattern_kind: for_in.pattern.kind,
            },
            iter: lower_expr_internal(&for_in.iter, context)?,
            body: lower_block_with_context(&for_in.body, context)?,
        }),
        nia_ast::StmtKind::While(while_stmt) => EarlyComptimeStmtKind::While {
            cond: lower_expr_internal(&while_stmt.cond, context)?,
            body: lower_block_with_context(&while_stmt.body, context)?,
        },
        nia_ast::StmtKind::Loop(loop_stmt) => EarlyComptimeStmtKind::Loop {
            body: lower_block_with_context(&loop_stmt.body, context)?,
        },
        _ => {
            return Err(ComptimeLowerError {
                span: stmt.span,
                message: "unsupported statement in comptime function body".to_string(),
            });
        }
    };
    Ok(EarlyComptimeStmt {
        span: stmt.span,
        kind,
    })
}

fn lower_expr_stmt_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeStmtKind, ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => Ok(EarlyComptimeStmtKind::If {
            cond: lower_expr_internal(cond, context)?,
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_if_stmt_else_branch_with_context(else_branch, context))
                .transpose()?,
        }),
        _ => Ok(EarlyComptimeStmtKind::Expr(lower_expr_internal(
            expr, context,
        )?)),
    }
}

fn lower_if_stmt_else_branch_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeBlock, ComptimeLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::Block(block) => lower_block_with_context(block, context),
        nia_ast::ExprKind::If { .. } => Ok(EarlyComptimeBlock {
            span: expr.span,
            stmts: vec![EarlyComptimeStmt {
                span: expr.span,
                kind: lower_expr_stmt_with_context(expr, context)?,
            }],
            tail: None,
        }),
        _ => Ok(EarlyComptimeBlock {
            span: expr.span,
            stmts: Vec::new(),
            tail: Some(Box::new(lower_expr_internal(expr, context)?)),
        }),
    }
}

fn lower_switch_with_context(
    span: Span,
    switch: &nia_ast::SwitchStmt,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeSwitch, ComptimeLowerError> {
    Ok(EarlyComptimeSwitch {
        span,
        target: lower_expr_internal(&switch.target, context)?,
        arms: switch
            .arms
            .iter()
            .map(|arm| lower_switch_arm_with_context(arm, context))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn lower_switch_arm_with_context(
    arm: &nia_ast::SwitchArm,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeSwitchArm, ComptimeLowerError> {
    Ok(EarlyComptimeSwitchArm {
        span: arm.span,
        patterns: arm
            .patterns
            .iter()
            .map(|pattern| lower_switch_pattern_with_context(pattern, context))
            .collect::<Result<Vec<_>, _>>()?,
        body: lower_switch_arm_body_with_context(&arm.body, context)?,
    })
}

fn lower_switch_pattern_with_context(
    pattern: &nia_ast::SwitchPattern,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeSwitchPattern, ComptimeLowerError> {
    match pattern {
        nia_ast::SwitchPattern::Default => Ok(EarlyComptimeSwitchPattern::Default),
        nia_ast::SwitchPattern::OptionalSome {
            name,
            span,
            node_key,
        } => Ok(EarlyComptimeSwitchPattern::OptionalSome {
            name: name.clone(),
            local_id: lower_local_id(context, node_key, *span)?,
            span: *span,
        }),
        nia_ast::SwitchPattern::OptionalNull { span } => {
            Ok(EarlyComptimeSwitchPattern::OptionalNull { span: *span })
        }
        nia_ast::SwitchPattern::ErrorOk {
            name,
            span,
            node_key,
        } => Ok(EarlyComptimeSwitchPattern::ErrorOk {
            name: name.clone(),
            local_id: lower_local_id(context, node_key, *span)?,
            span: *span,
        }),
        nia_ast::SwitchPattern::ErrorErr {
            name,
            span,
            node_key,
        } => Ok(EarlyComptimeSwitchPattern::ErrorErr {
            name: name.clone(),
            local_id: lower_local_id(context, node_key, *span)?,
            span: *span,
        }),
        nia_ast::SwitchPattern::Expr(expr) => {
            lower_expr_internal(expr, context).map(EarlyComptimeSwitchPattern::Expr)
        }
        nia_ast::SwitchPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => Ok(EarlyComptimeSwitchPattern::Range {
            start: lower_expr_internal(start, context)?,
            end: lower_expr_internal(end, context)?,
            inclusive: *inclusive,
            span: *span,
        }),
    }
}

fn lower_switch_arm_body_with_context(
    body: &nia_ast::SwitchArmBody,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeSwitchArmBody, ComptimeLowerError> {
    match body {
        nia_ast::SwitchArmBody::Expr(expr) => {
            lower_expr_internal(expr, context).map(EarlyComptimeSwitchArmBody::Expr)
        }
        nia_ast::SwitchArmBody::Stmt(stmt) => {
            lower_stmt_with_context(stmt, context).map(EarlyComptimeSwitchArmBody::Stmt)
        }
        nia_ast::SwitchArmBody::Block(block) => {
            lower_block_with_context(block, context).map(EarlyComptimeSwitchArmBody::Block)
        }
    }
}

fn lower_field_init_with_context(
    field: &nia_ast::FieldInit,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeFieldInit, ComptimeLowerError> {
    Ok(EarlyComptimeFieldInit {
        span: field.span,
        name: field.name.clone(),
        value: lower_expr_internal(&field.value, context)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_node_id::{NodeChildPath, SyntaxKind};
    use nia_source::{SourceId, SourceRevision, SourceVersion};

    fn span() -> Span {
        Span::new(0, 1)
    }

    fn other_span() -> Span {
        Span::new(2, 3)
    }

    fn int_expr(value: &str) -> EarlyComptimeExpr {
        EarlyComptimeExpr {
            span: span(),
            kind: EarlyComptimeExprKind::Integer(value.to_string()),
        }
    }

    fn node_key(kind: SyntaxKind, ordinal: u32) -> NodeKey {
        NodeKey::child_path(
            SourceVersion {
                id: SourceId(0),
                revision: SourceRevision::INITIAL,
            },
            kind,
            NodeChildPath::from_steps([ordinal]),
        )
    }

    fn expr_key(ordinal: u32) -> NodeKey {
        node_key(SyntaxKind::Expr, ordinal)
    }

    fn stmt_key(ordinal: u32) -> NodeKey {
        node_key(SyntaxKind::Stmt, ordinal)
    }

    fn type_key(ordinal: u32) -> NodeKey {
        node_key(SyntaxKind::Type, ordinal)
    }

    fn ast_ident(name: &str) -> nia_ast::Expr {
        nia_ast::Expr {
            span: span(),
            node_key: expr_key(0),
            kind: nia_ast::ExprKind::Ident(name.to_string()),
        }
    }

    #[test]
    fn resolved_expr_rejects_unresolved_names() {
        let expr = EarlyComptimeExpr {
            span: span(),
            kind: EarlyComptimeExprKind::Ident(EarlyComptimeName::unresolved("x".to_string())),
        };

        let err = ResolvedComptimeExpr::new(expr).expect_err("unresolved name must be rejected");
        assert_eq!(err.message, "failed to resolve comptime name");
    }

    #[test]
    fn resolved_expr_rejects_unresolved_assignment_targets() {
        let expr = EarlyComptimeExpr {
            span: span(),
            kind: EarlyComptimeExprKind::Assign(Box::new(EarlyComptimeAssign {
                lhs: EarlyComptimeAssignTarget::Local {
                    span: span(),
                    name: "x".to_string(),
                    local_id: None,
                    path: Vec::new(),
                },
                op: ComptimeAssignOp::Assign,
                rhs: int_expr("1"),
            })),
        };

        let err = ResolvedComptimeExpr::new(expr)
            .expect_err("unresolved assignment target must be rejected");
        assert_eq!(err.message, "failed to resolve comptime assignment target");
    }

    #[test]
    fn resolved_function_rejects_unresolved_locals() {
        let function = EarlyComptimeFunction {
            span: span(),
            params: vec![EarlyComptimeParam {
                span: span(),
                name: "x".to_string(),
                local_id: None,
                ty: None,
            }],
            body: EarlyComptimeBlock {
                span: span(),
                stmts: Vec::new(),
                tail: None,
            },
        };

        let err = ResolvedComptimeFunction::new(function)
            .expect_err("unresolved function parameter must be rejected");
        assert_eq!(
            err.message,
            "failed to resolve comptime function parameter local"
        );
    }

    #[test]
    fn resolved_expr_rejects_unresolved_type_args() {
        let expr = EarlyComptimeExpr {
            span: span(),
            kind: EarlyComptimeExprKind::LayoutBuiltin {
                builtin: LayoutBuiltin::Size,
                type_arg: EarlyComptimeTypeArg {
                    span: span(),
                    ty_span: span(),
                    ty: None,
                },
            },
        };

        let err =
            ResolvedComptimeExpr::new(expr).expect_err("unresolved type arg must be rejected");
        assert_eq!(err.message, "failed to resolve comptime type argument");
    }

    #[test]
    fn resolved_lowering_requires_name_resolution() {
        let semantic_uses = SemanticUseTable::default();
        let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
        let err = lower_expr_resolved_with_context(&ast_ident("x"), &context)
            .expect_err("resolved lowering must reject unresolved names");
        assert_eq!(err.message, "failed to resolve comptime name");
    }

    #[test]
    fn early_name_lowering_separates_unresolved_and_resolved_states() {
        let early =
            lower_expr_early(&ast_ident("x")).expect("early lowering should keep display name");
        let EarlyComptimeExprKind::Ident(name) = early.kind else {
            panic!("identifier should lower to early comptime name");
        };
        assert_eq!(name.display(), "x");
        assert_eq!(name.resolution(), None);

        let ident = ast_ident("x");
        let mut semantic_uses = SemanticUseTable::default();
        semantic_uses
            .node_value_uses
            .insert(ident.node_key.clone(), SemanticValueUse::Local(LocalId(0)));
        let context = EarlyComptimeLowerInputs::default().with_semantic_uses(&semantic_uses);
        let early = lower_expr_early_with_context(&ident, &context)
            .expect("early lowering with semantic inputs should resolve names");
        let EarlyComptimeExprKind::Ident(name) = early.kind else {
            panic!("identifier should lower to early comptime name");
        };
        assert_eq!(name.display(), "x");
        assert_eq!(
            name.resolution(),
            Some(ComptimeNameResolution::Local(LocalId(0)))
        );
    }

    #[test]
    fn resolved_lowering_requires_local_ids() {
        let block = nia_ast::Block {
            span: span(),
            stmts: vec![nia_ast::Stmt {
                span: span(),
                node_key: stmt_key(0),
                kind: nia_ast::StmtKind::Binding(nia_ast::BindingStmt {
                    name: "x".to_string(),
                    ty: None,
                    value: Some(ast_ident("x")),
                    is_let: true,
                    is_comptime: true,
                }),
            }],
            tail: None,
        };
        let expr = nia_ast::Expr {
            span: span(),
            node_key: expr_key(1),
            kind: nia_ast::ExprKind::Block(block),
        };
        let mut semantic_uses = SemanticUseTable::default();
        semantic_uses
            .node_value_uses
            .insert(expr_key(0), SemanticValueUse::Local(LocalId(0)));
        let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
        let err = lower_expr_resolved_with_context(&expr, &context)
            .expect_err("resolved lowering must reject unresolved local bindings");
        assert_eq!(err.message, "failed to resolve comptime local binding");
    }

    #[test]
    fn resolved_lowering_uses_local_uses_for_assignment_targets() {
        let assign_span = other_span();
        let lhs_key = expr_key(2);
        let expr = nia_ast::Expr {
            span: Span::new(0, 3),
            node_key: expr_key(3),
            kind: nia_ast::ExprKind::Assign {
                lhs: Box::new(nia_ast::Expr {
                    span: assign_span,
                    node_key: lhs_key.clone(),
                    kind: nia_ast::ExprKind::Ident("x".to_string()),
                }),
                op: nia_ast::AssignOp::Assign,
                rhs: Box::new(nia_ast::Expr {
                    span: span(),
                    node_key: expr_key(4),
                    kind: nia_ast::ExprKind::Integer("1".to_string()),
                }),
            },
        };
        let mut semantic_uses = SemanticUseTable::default();
        semantic_uses
            .node_value_uses
            .insert(lhs_key, SemanticValueUse::Local(LocalId(7)));
        let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
        let lowered = lower_expr_resolved_with_context(&expr, &context)
            .expect("assignment target should use local-use facts");

        let ResolvedComptimeExprKind::Assign(assign) = lowered.kind() else {
            panic!("expression should lower to assignment");
        };
        let ResolvedComptimeAssignTargetKind::Local { local_id, .. } = assign.lhs().kind();
        assert_eq!(*local_id, LocalId(7));
    }

    #[test]
    fn resolved_lowering_requires_type_ids() {
        let expr = nia_ast::Expr {
            span: span(),
            node_key: expr_key(5),
            kind: nia_ast::ExprKind::Cast {
                expr: Box::new(nia_ast::Expr {
                    span: span(),
                    node_key: expr_key(6),
                    kind: nia_ast::ExprKind::Integer("1".to_string()),
                }),
                ty: nia_ast::TypeRef {
                    span: span(),
                    node_key: type_key(0),
                    text: "i32".to_string(),
                    kind: nia_ast::TypeKind::Path {
                        segments: vec![nia_ast::TypePathSegment {
                            name: "i32".to_string(),
                            args: Vec::new(),
                        }],
                    },
                },
            },
        };
        let mut semantic_uses = SemanticUseTable::default();
        semantic_uses
            .node_value_uses
            .insert(expr_key(6), SemanticValueUse::Local(LocalId(0)));
        semantic_uses
            .node_local_defs
            .insert(stmt_key(0), LocalId(0));
        let context = ResolvedComptimeLowerInputs::new(&semantic_uses);
        let err = lower_expr_resolved_with_context(&expr, &context)
            .expect_err("resolved lowering must reject unresolved types");
        assert_eq!(err.message, "failed to resolve comptime type");
    }
}
