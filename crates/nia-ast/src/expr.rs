// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_symbol::SymbolId;

use crate::{PathSegmentKind, TypeRef};

/// Compound statement block with an optional tail expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Source span covering the block.
    pub span: Span,
    /// Statements in source order.
    pub stmts: Vec<Stmt>,
    /// Optional value-producing tail expression.
    pub tail: Option<Box<Expr>>,
}

/// Statement node with source identity and attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    /// Source span covering the statement.
    pub span: Span,
    /// Stable syntax identity for the statement.
    pub node_key: VersionedNodeKey,
    /// Attributes attached to the statement.
    pub attributes: Vec<crate::Attribute>,
    /// Statement-specific payload.
    pub kind: StmtKind,
}

/// Kinds of statements accepted by the language grammar.
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// Local binding statement.
    Binding(Box<BindingStmt>),
    /// Static binding declaration in statement position.
    Static(Box<crate::BindingItem>),
    /// Using/import statement.
    Using(crate::UsingItem),
    /// Expression statement.
    Expr(Box<Expr>),
    /// Return with an optional value.
    Return(Option<Box<Expr>>),
    /// Break from the innermost loop.
    Break,
    /// Continue the innermost loop.
    Continue,
    /// Deferred expression evaluated on scope exit.
    Defer(Box<Expr>),
    /// For-in loop statement.
    ForIn(Box<ForInStmt>),
    /// While loop statement.
    While(Box<WhileStmt>),
    /// Unconditional loop statement.
    Loop(Box<LoopStmt>),
}

/// Local binding statement payload.
#[derive(Debug, Clone, PartialEq)]
pub struct BindingStmt {
    /// Pattern receiving the bound value.
    pub pattern: Pattern,
    /// Optional explicit binding type.
    pub ty: Option<TypeRef>,
    /// Optional initializer expression.
    pub value: Option<Expr>,
    /// Mutable or const binding mode.
    pub kind: LocalBindingKind,
}

/// Mutability mode for a local binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalBindingKind {
    /// A `let` binding, optionally mutable.
    Let {
        /// Whether writes to the binding are permitted.
        is_mutable: bool,
    },
    /// An immutable compile-time binding.
    Const,
}

impl BindingStmt {
    /// Reports whether this binding is a const declaration.
    pub fn is_const(&self) -> bool {
        matches!(self.kind, LocalBindingKind::Const)
    }

    /// Reports whether this binding is mutable.
    pub fn is_mutable(&self) -> bool {
        matches!(self.kind, LocalBindingKind::Let { is_mutable: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nia_node_id::{SyntaxKind, VersionedNodeKey};
    use nia_source::{SourceId, SourceRevision, SourceVersion};
    use nia_symbol::SymbolId;

    #[test]
    fn binding_kind_reports_const_and_mutability() {
        let immutable = BindingStmt {
            pattern: Pattern {
                span: Span::new(0, 1),
                kind: PatternKind::Wildcard,
            },
            ty: None,
            value: None,
            kind: LocalBindingKind::Let { is_mutable: false },
        };
        assert!(!immutable.is_const());
        assert!(!immutable.is_mutable());

        let mutable = BindingStmt {
            kind: LocalBindingKind::Let { is_mutable: true },
            ..immutable.clone()
        };
        assert!(mutable.is_mutable());

        let constant = BindingStmt {
            kind: LocalBindingKind::Const,
            ..immutable
        };
        assert!(constant.is_const());
        assert!(!constant.is_mutable());
    }

    #[test]
    fn nested_pattern_binding_is_detected_recursively() {
        let wildcard = Pattern {
            span: Span::new(0, 1),
            kind: PatternKind::Wildcard,
        };
        assert!(!wildcard.contains_binding());
        let nested = Pattern {
            span: Span::new(0, 3),
            kind: PatternKind::OptionalSome(Box::new(Pattern {
                span: Span::new(1, 2),
                kind: PatternKind::Tuple(vec![wildcard]),
            })),
        };
        assert!(!nested.contains_binding());

        let binding = Pattern {
            span: Span::new(1, 2),
            kind: PatternKind::Bind {
                name: SymbolId::from_stable_hash(1),
                node_key: VersionedNodeKey::span(
                    SourceVersion {
                        id: SourceId(1),
                        revision: SourceRevision::INITIAL,
                    },
                    SyntaxKind::Pattern,
                    Span::new(1, 2),
                ),
                is_mutable: false,
            },
        };
        let nested_binding = Pattern {
            span: Span::new(0, 3),
            kind: PatternKind::Pointer(Box::new(binding)),
        };
        assert!(nested_binding.contains_binding());
    }
}

/// For-in loop with a binding pattern, iterator expression, and body.
#[derive(Debug, Clone, PartialEq)]
pub struct ForInStmt {
    /// Pattern receiving each iterator item.
    pub pattern: Pattern,
    /// Expression producing the iterator.
    pub iter: Expr,
    /// Loop body.
    pub body: Block,
}

/// While loop with condition and body.
#[derive(Debug, Clone, PartialEq)]
pub struct WhileStmt {
    /// Loop condition expression.
    pub cond: Expr,
    /// Loop body.
    pub body: Block,
}

/// Unconditional loop body.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopStmt {
    /// Loop body.
    pub body: Block,
}

/// Conditional expression that branches on pattern matching.
#[derive(Debug, Clone, PartialEq)]
pub struct IfPatternExpr {
    /// Expression being matched.
    pub target: Expr,
    /// Pattern tested against the target.
    pub pattern: Pattern,
    /// Branch taken on a successful match.
    pub then_branch: Block,
    /// Optional expression for the failed branch.
    pub else_branch: Option<Box<Expr>>,
}

/// Match expression and its ordered arms.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchExpr {
    /// Expression being matched.
    pub target: Expr,
    /// Match arms in source order.
    pub arms: Vec<MatchArm>,
}

/// One pattern set and body in a match expression.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// Patterns accepted by this arm.
    pub patterns: Vec<Pattern>,
    /// Arm result body.
    pub body: MatchArmBody,
    /// Source span covering the arm.
    pub span: Span,
}

/// Pattern node with source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    /// Source span covering the pattern.
    pub span: Span,
    /// Pattern-specific payload.
    pub kind: PatternKind,
}

impl Pattern {
    /// Reports whether this pattern introduces a local binding.
    pub fn contains_binding(&self) -> bool {
        match &self.kind {
            PatternKind::Bind { .. } => true,
            PatternKind::Pointer(pattern) | PatternKind::MutPointer(pattern) => {
                pattern.contains_binding()
            }
            PatternKind::OptionalSome(pattern)
            | PatternKind::ErrorOk(pattern)
            | PatternKind::ErrorErr(pattern) => pattern.contains_binding(),
            PatternKind::Tuple(patterns) => patterns.iter().any(Pattern::contains_binding),
            PatternKind::Nominal { fields, .. } => fields.contains_binding(),
            PatternKind::Wildcard
            | PatternKind::OptionalNull
            | PatternKind::Expr(_)
            | PatternKind::Range { .. } => false,
        }
    }
}

/// Kinds of patterns recognized by the parser.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// Wildcard pattern matching any value.
    Wildcard,
    /// Binding pattern with identity and mutability metadata.
    Bind {
        /// Bound symbol identity.
        name: SymbolId,
        /// Stable syntax identity of the binding.
        node_key: VersionedNodeKey,
        /// Whether the introduced binding is mutable.
        is_mutable: bool,
    },
    /// Read-only pointer pattern.
    Pointer(Box<Pattern>),
    /// Mutable pointer pattern.
    MutPointer(Box<Pattern>),
    /// Pattern matching the populated optional case.
    OptionalSome(Box<Pattern>),
    /// Pattern matching a null optional value.
    OptionalNull,
    /// Pattern matching the successful error-union case.
    ErrorOk(Box<Pattern>),
    /// Pattern matching the error error-union case.
    ErrorErr(Box<Pattern>),
    /// Tuple destructuring pattern.
    Tuple(Vec<Pattern>),
    /// Nominal constructor pattern.
    Nominal {
        /// Constructor expression.
        constructor: Box<Expr>,
        /// Constructor fields being matched.
        fields: NominalPatternFields,
    },
    /// Explicit expression pattern.
    Expr(Box<Expr>),
    /// Inclusive or exclusive range pattern.
    Range {
        /// Range start expression.
        start: Box<Expr>,
        /// Range end expression.
        end: Box<Expr>,
        /// Whether the end bound is included.
        inclusive: bool,
    },
}

/// Field layout used by a nominal pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum NominalPatternFields {
    /// Positional constructor fields.
    Tuple(Vec<Pattern>),
    /// Named constructor fields.
    Named {
        /// Explicitly selected named fields.
        fields: Vec<NamedPatternField>,
        /// Explicit permission to ignore declaration fields omitted by the pattern.
        rest: Option<Span>,
    },
}

impl NominalPatternFields {
    fn contains_binding(&self) -> bool {
        match self {
            Self::Tuple(fields) => fields.iter().any(Pattern::contains_binding),
            Self::Named { fields, .. } => {
                fields.iter().any(|field| field.pattern.contains_binding())
            }
        }
    }
}

/// One named field selected by a nominal pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedPatternField {
    /// Field symbol identity.
    pub name: SymbolId,
    /// Pattern applied to the field.
    pub pattern: Pattern,
    /// Source span covering the field pattern.
    pub span: Span,
}

/// Body form of a match arm.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchArmBody {
    /// Expression body.
    Expr(Box<Expr>),
    /// Single-statement body.
    Stmt(Box<Stmt>),
    /// Block body.
    Block(Box<Block>),
}

impl MatchArmBody {
    /// Returns the source span of the selected body form.
    pub fn span(&self) -> Span {
        match self {
            Self::Expr(expr) => expr.span,
            Self::Stmt(stmt) => stmt.span,
            Self::Block(block) => block.span,
        }
    }
}

/// Expression node with source identity.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// Source span covering the expression.
    pub span: Span,
    /// Stable syntax identity for the expression.
    pub node_key: VersionedNodeKey,
    /// Expression-specific payload.
    pub kind: ExprKind,
}

/// String literal represented as one or more source parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringLiteral {
    /// Literal parts in source order.
    pub parts: Vec<String>,
}

/// Kinds of expressions produced by the parser.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Recovery expression for invalid syntax.
    Error,
    /// Integer literal text.
    Integer(String),
    /// Floating-point literal text.
    Float(String),
    /// String literal.
    String(StringLiteral),
    /// Byte string literal.
    ByteString(StringLiteral),
    /// Character literal text.
    Char(String),
    /// Byte character literal text.
    ByteChar(String),
    /// Raw string literal text.
    Raw(String),
    /// Boolean literal.
    Bool(bool),
    /// Null literal.
    Null,
    /// Identifier expression.
    Ident(SymbolId),
    /// `self` value expression.
    SelfValue,
    /// Root-qualified path expression.
    PathRoot(PathSegmentKind),
    /// Underscore placeholder expression.
    Underscore,
    /// Type target expression.
    TypeTarget {
        /// Target type.
        ty: TypeRef,
    },
    /// Trait target expression.
    TraitTarget {
        /// Target type.
        ty: TypeRef,
        /// Trait reference.
        trait_ref: TypeRef,
    },
    /// Bracket suffix with expression/type arguments.
    BracketSuffix {
        /// Expression receiving the suffix.
        callee: Box<Expr>,
        /// Suffix arguments.
        args: Vec<BracketArg>,
    },
    /// Tuple expression.
    Tuple(Vec<Expr>),
    /// Anonymous closure value with optional explicit captures.
    ///
    /// The capture expressions are evaluated at the closure site and their
    /// names are visible only in the closure body. The concrete state type is
    /// intentionally anonymous; dynamic callable views are a later semantic
    /// coercion rather than a second source spelling.
    Closure {
        /// Explicit capture bindings evaluated at the closure site.
        captures: Vec<ClosureCapture>,
        /// Closure parameters.
        params: Vec<crate::Param>,
        /// The expression after `->`; a multi-statement body is an ordinary
        /// `ExprKind::Block`, not a closure-specific body form.
        body: Box<Expr>,
    },
    /// Array literal expression.
    ArrayLiteral {
        /// Explicit or repeated elements.
        elems: ArrayElements,
    },
    /// Struct literal with an explicit type target.
    TypedStructLiteral {
        /// Constructed type.
        ty: TypeRef,
        /// Field initializers.
        fields: Vec<FieldInit>,
    },
    /// Struct literal qualified by an expression target.
    QualifiedStructLiteral {
        /// Qualifying target expression.
        target: Box<Expr>,
        /// Field initializers.
        fields: Vec<FieldInit>,
    },
    /// Struct literal whose nominal type is supplied by expected-type context.
    OmittedAggregateLiteral {
        /// Field initializers.
        fields: Vec<FieldInit>,
    },
    /// Enum variant or associated item whose nominal owner is omitted.
    OmittedMember {
        /// Unqualified member name.
        name: SymbolId,
    },
    /// Unary operation.
    Unary {
        /// Unary operator.
        op: UnaryOp,
        /// Operand expression.
        expr: Box<Expr>,
    },
    /// Populated optional construction.
    OptionalSome {
        /// Wrapped value.
        expr: Box<Expr>,
    },
    /// Successful error-union construction.
    ErrorOk {
        /// Success value.
        expr: Box<Expr>,
    },
    /// Error error-union construction.
    ErrorErr {
        /// Error value.
        expr: Box<Expr>,
    },
    /// Fallible propagation expression.
    Try {
        /// Expression whose error is propagated.
        expr: Box<Expr>,
    },
    /// Binary operation.
    Binary {
        /// Left operand.
        lhs: Box<Expr>,
        /// Binary operator.
        op: BinaryOp,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// Assignment or compound assignment.
    Assign {
        /// Assignment target.
        lhs: Box<Expr>,
        /// Assignment operator.
        op: AssignOp,
        /// Assigned value.
        rhs: Box<Expr>,
    },
    /// Explicit type cast.
    Cast {
        /// Value being cast.
        expr: Box<Expr>,
        /// Destination type syntax.
        ty: TypeRef,
    },
    /// Function or callable invocation.
    Call {
        /// Callee expression.
        callee: Box<Expr>,
        /// Call arguments in source order.
        args: Vec<Expr>,
    },
    /// Qualified name lookup.
    Qualified {
        /// Qualifying expression.
        lhs: Box<Expr>,
        /// Selected symbol.
        name: SymbolId,
    },
    /// Named field projection.
    Field {
        /// Projected expression.
        lhs: Box<Expr>,
        /// Field symbol.
        name: SymbolId,
    },
    /// Tuple field projection.
    TupleField {
        /// Projected expression.
        lhs: Box<Expr>,
        /// Zero-based tuple field index.
        index: usize,
    },
    /// Index or slice expression.
    Index {
        /// Indexed expression.
        lhs: Box<Expr>,
        /// Index or slice range.
        index: IndexArg,
    },
    /// Standalone range expression.
    Range(SliceRange),
    /// Block expression.
    Block(Block),
    /// Conditional expression.
    If {
        /// Condition expression.
        cond: Box<Expr>,
        /// Branch taken when the condition is true.
        then_branch: Block,
        /// Optional false branch.
        else_branch: Option<Box<Expr>>,
    },
    /// Conditional pattern-match expression.
    IfPattern(Box<IfPatternExpr>),
    /// Match expression.
    Match(Box<MatchExpr>),
}

/// Syntax-preserving bracket argument before semantic disambiguation.
#[derive(Debug, Clone, PartialEq)]
pub struct BracketArg {
    /// Source span covering the argument.
    pub span: Span,
    /// Expression interpretation, when syntactically available.
    pub expr: Option<Expr>,
    /// Type interpretation, when syntactically available.
    pub ty: Option<TypeRef>,
}

/// Explicit closure capture binding.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureCapture {
    /// Name visible in the closure body.
    pub name: SymbolId,
    /// Capture expression evaluated at the closure site.
    pub value: Expr,
    /// Source span covering the capture entry.
    pub span: Span,
    /// Stable syntax identity for the capture.
    pub node_key: VersionedNodeKey,
}

/// Index suffix argument.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexArg {
    /// Single index expression.
    Expr(Box<Expr>),
    /// Slice range.
    Range(SliceRange),
}

/// Optional-bounds range used by slice and range expressions.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceRange {
    /// Optional start bound.
    pub start: Option<Box<Expr>>,
    /// Optional end bound.
    pub end: Option<Box<Expr>>,
    /// Whether the end bound is included.
    pub inclusive: bool,
}

/// Element form of an array literal.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayElements {
    /// Explicit list of array elements.
    List(Vec<Expr>),
    /// One value repeated a source-specified number of times.
    Repeat {
        /// Repeated value expression.
        value: Box<Expr>,
        /// Repetition count expression.
        count: Box<Expr>,
    },
}

/// Named initializer in a struct literal.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInit {
    /// Field symbol identity.
    pub name: SymbolId,
    /// Field value expression.
    pub value: Expr,
    /// Source span covering the initializer.
    pub span: Span,
}

/// Unary expression operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Arithmetic negation.
    Neg,
    /// Logical negation.
    Not,
    /// Bitwise complement.
    BitNot,
    /// Read-only reference creation.
    RefReadOnly,
    /// Mutable-capable reference creation.
    Ref,
    /// Pointer/reference dereference.
    Deref,
}

/// Binary expression operators, ordered by parser precedence groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
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
    /// Bitwise conjunction.
    BitAnd,
    /// Bitwise exclusive-or.
    BitXor,
    /// Bitwise disjunction.
    BitOr,
    /// Logical conjunction.
    And,
    /// Logical disjunction.
    Or,
}

/// Assignment expression operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// Simple assignment.
    Assign,
    /// Addition assignment.
    Add,
    /// Subtraction assignment.
    Sub,
    /// Left-shift assignment.
    Shl,
    /// Right-shift assignment.
    Shr,
    /// Multiplication assignment.
    Mul,
    /// Division assignment.
    Div,
    /// Remainder assignment.
    Rem,
    /// Bitwise-and assignment.
    BitAnd,
    /// Bitwise-xor assignment.
    BitXor,
    /// Bitwise-or assignment.
    BitOr,
}
