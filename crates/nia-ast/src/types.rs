// SPDX-License-Identifier: GPL-3.0-or-later
use nia_span::Span;

use crate::{Expr, ExprStub};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub span: Span,
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
        is_const: bool,
        elem: Box<TypeRef>,
    },
    Slice {
        is_const: bool,
        elem: Box<TypeRef>,
    },
    Array {
        len: ArrayLen,
        elem: Box<TypeRef>,
    },
    FunctionPointer {
        params: Vec<TypeRef>,
        return_type: Option<Box<TypeRef>>,
        is_variadic: bool,
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
        name: String,
        ty: TypeRef,
        span: Span,
    },
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
