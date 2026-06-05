// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::NodeKey;
use nia_span::Span;

use crate::{Expr, ExprStub};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub span: Span,
    pub node_key: NodeKey,
    pub text: String,
    pub kind: TypeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    Error,
    Path {
        segments: Vec<TypePathSegment>,
    },
    Projection {
        ty: Box<TypeRef>,
        trait_ref: Box<TypeRef>,
        name: String,
    },
    Pointer {
        is_readonly: bool,
        elem: Box<TypeRef>,
    },
    Slice {
        is_readonly: bool,
        elem: Box<TypeRef>,
    },
    Array {
        len: ArrayLen,
        elem: Box<TypeRef>,
    },
    Range {
        start: Option<Box<TypeRef>>,
        end: Option<Box<TypeRef>>,
        inclusive: bool,
    },
    FunctionPointer {
        params: Vec<TypeRef>,
        return_type: Option<Box<TypeRef>>,
        is_variadic: bool,
    },
    Optional {
        elem: Box<TypeRef>,
    },
    ErrorUnion {
        error: Box<TypeRef>,
        value: Box<TypeRef>,
    },
    SelfType,
    Void,
    Never,
    Infer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypePathSegment {
    pub name: String,
    pub args: Vec<TypeArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeArg {
    Type(TypeRef),
    Const(ExprStub),
    AssocBinding {
        key: AssocBindingKey,
        ty: TypeRef,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssocBindingKey {
    Name(String),
    Projection(TypeRef),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArrayLen {
    Infer,
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WhereClause {
    pub predicates: Vec<WherePredicate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WherePredicate {
    pub ty: TypeRef,
    pub bounds: Vec<TypeRef>,
    pub span: Span,
}
