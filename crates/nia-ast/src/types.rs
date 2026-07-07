// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_identity_key};

use crate::{
    ArrayElements, AssignOp, BinaryOp, BracketArg, Expr, ExprKind, FieldInit, IndexArg, SliceRange,
    StringLiteral, UnaryOp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathSegmentKind {
    Name(SymbolId),
    Package,
    Super,
    SelfValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    pub span: Span,
    pub node_key: VersionedNodeKey,
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
        name: SymbolId,
    },
    Pointer {
        is_readonly: bool,
        elem: Box<TypeRef>,
    },
    VolatilePointer {
        is_readonly: bool,
        elem: Box<TypeRef>,
    },
    Slice {
        is_readonly: bool,
        elem: Box<TypeRef>,
    },
    SlicePointee {
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
    pub kind: PathSegmentKind,
    pub args: Vec<TypeArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeArg {
    Type(TypeRef),
    Const(Expr),
    TypeOrConst {
        ty: TypeRef,
        expr: Expr,
    },
    AssocBinding {
        key: AssocBindingKey,
        ty: TypeRef,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AssocBindingKey {
    Name(SymbolId),
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

pub fn type_ref_decl_eq(lhs: &TypeRef, rhs: &TypeRef) -> bool {
    type_kind_decl_eq(&lhs.kind, &rhs.kind)
}

pub fn type_refs_decl_eq(lhs: &[TypeRef], rhs: &[TypeRef]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| type_ref_decl_eq(lhs, rhs))
}

pub fn option_type_ref_decl_eq(lhs: Option<&TypeRef>, rhs: Option<&TypeRef>) -> bool {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => type_ref_decl_eq(lhs, rhs),
        (None, None) => true,
        _ => false,
    }
}

pub fn where_clause_decl_eq(lhs: &WhereClause, rhs: &WhereClause) -> bool {
    lhs.predicates.len() == rhs.predicates.len()
        && lhs
            .predicates
            .iter()
            .zip(rhs.predicates.iter())
            .all(|(lhs, rhs)| {
                type_ref_decl_eq(&lhs.ty, &rhs.ty) && type_refs_decl_eq(&lhs.bounds, &rhs.bounds)
            })
}

pub fn type_ref_identity(ty: &TypeRef) -> String {
    let mut out = String::new();
    write_type_ref_identity(&mut out, ty);
    out
}

pub fn type_refs_identity(types: &[TypeRef]) -> Vec<String> {
    types.iter().map(type_ref_identity).collect()
}

pub fn where_clause_identity(where_clause: &WhereClause) -> Vec<(String, Vec<String>)> {
    where_clause
        .predicates
        .iter()
        .map(|predicate| {
            (
                type_ref_identity(&predicate.ty),
                type_refs_identity(&predicate.bounds),
            )
        })
        .collect()
}

fn type_kind_decl_eq(lhs: &TypeKind, rhs: &TypeKind) -> bool {
    match (lhs, rhs) {
        (TypeKind::Error, TypeKind::Error)
        | (TypeKind::SelfType, TypeKind::SelfType)
        | (TypeKind::Void, TypeKind::Void)
        | (TypeKind::Never, TypeKind::Never)
        | (TypeKind::Infer, TypeKind::Infer) => true,
        (TypeKind::Path { segments: lhs }, TypeKind::Path { segments: rhs }) => {
            type_path_segments_decl_eq(lhs, rhs)
        }
        (
            TypeKind::Projection {
                ty: lhs_ty,
                trait_ref: lhs_trait,
                name: lhs_name,
            },
            TypeKind::Projection {
                ty: rhs_ty,
                trait_ref: rhs_trait,
                name: rhs_name,
            },
        ) => {
            lhs_name == rhs_name
                && type_ref_decl_eq(lhs_ty, rhs_ty)
                && type_ref_decl_eq(lhs_trait, rhs_trait)
        }
        (
            TypeKind::Pointer {
                is_readonly: lhs_readonly,
                elem: lhs_elem,
            },
            TypeKind::Pointer {
                is_readonly: rhs_readonly,
                elem: rhs_elem,
            },
        )
        | (
            TypeKind::VolatilePointer {
                is_readonly: lhs_readonly,
                elem: lhs_elem,
            },
            TypeKind::VolatilePointer {
                is_readonly: rhs_readonly,
                elem: rhs_elem,
            },
        )
        | (
            TypeKind::Slice {
                is_readonly: lhs_readonly,
                elem: lhs_elem,
            },
            TypeKind::Slice {
                is_readonly: rhs_readonly,
                elem: rhs_elem,
            },
        ) => lhs_readonly == rhs_readonly && type_ref_decl_eq(lhs_elem, rhs_elem),
        (TypeKind::SlicePointee { elem: lhs }, TypeKind::SlicePointee { elem: rhs })
        | (TypeKind::Optional { elem: lhs }, TypeKind::Optional { elem: rhs }) => {
            type_ref_decl_eq(lhs, rhs)
        }
        (
            TypeKind::Array {
                len: lhs_len,
                elem: lhs_elem,
            },
            TypeKind::Array {
                len: rhs_len,
                elem: rhs_elem,
            },
        ) => array_len_decl_eq(lhs_len, rhs_len) && type_ref_decl_eq(lhs_elem, rhs_elem),
        (
            TypeKind::Range {
                start: lhs_start,
                end: lhs_end,
                inclusive: lhs_inclusive,
            },
            TypeKind::Range {
                start: rhs_start,
                end: rhs_end,
                inclusive: rhs_inclusive,
            },
        ) => {
            lhs_inclusive == rhs_inclusive
                && option_box_type_ref_decl_eq(lhs_start.as_deref(), rhs_start.as_deref())
                && option_box_type_ref_decl_eq(lhs_end.as_deref(), rhs_end.as_deref())
        }
        (
            TypeKind::FunctionPointer {
                params: lhs_params,
                return_type: lhs_return,
                is_variadic: lhs_variadic,
            },
            TypeKind::FunctionPointer {
                params: rhs_params,
                return_type: rhs_return,
                is_variadic: rhs_variadic,
            },
        ) => {
            lhs_variadic == rhs_variadic
                && type_refs_decl_eq(lhs_params, rhs_params)
                && option_box_type_ref_decl_eq(lhs_return.as_deref(), rhs_return.as_deref())
        }
        (
            TypeKind::ErrorUnion {
                error: lhs_error,
                value: lhs_value,
            },
            TypeKind::ErrorUnion {
                error: rhs_error,
                value: rhs_value,
            },
        ) => type_ref_decl_eq(lhs_error, rhs_error) && type_ref_decl_eq(lhs_value, rhs_value),
        _ => false,
    }
}

fn type_path_segments_decl_eq(lhs: &[TypePathSegment], rhs: &[TypePathSegment]) -> bool {
    lhs.len() == rhs.len()
        && lhs.iter().zip(rhs.iter()).all(|(lhs, rhs)| {
            lhs.kind == rhs.kind
                && lhs.args.len() == rhs.args.len()
                && lhs
                    .args
                    .iter()
                    .zip(rhs.args.iter())
                    .all(|(lhs, rhs)| type_arg_decl_eq(lhs, rhs))
        })
}

fn type_arg_decl_eq(lhs: &TypeArg, rhs: &TypeArg) -> bool {
    match (lhs, rhs) {
        (TypeArg::Type(lhs), TypeArg::Type(rhs)) => type_ref_decl_eq(lhs, rhs),
        (TypeArg::Const(lhs), TypeArg::Const(rhs)) => expr_decl_eq(lhs, rhs),
        (
            TypeArg::TypeOrConst {
                ty: lhs_ty,
                expr: lhs_expr,
            },
            TypeArg::TypeOrConst {
                ty: rhs_ty,
                expr: rhs_expr,
            },
        ) => type_ref_decl_eq(lhs_ty, rhs_ty) && expr_decl_eq(lhs_expr, rhs_expr),
        (
            TypeArg::AssocBinding {
                key: lhs_key,
                ty: lhs_ty,
                ..
            },
            TypeArg::AssocBinding {
                key: rhs_key,
                ty: rhs_ty,
                ..
            },
        ) => assoc_binding_key_decl_eq(lhs_key, rhs_key) && type_ref_decl_eq(lhs_ty, rhs_ty),
        _ => false,
    }
}

fn assoc_binding_key_decl_eq(lhs: &AssocBindingKey, rhs: &AssocBindingKey) -> bool {
    match (lhs, rhs) {
        (AssocBindingKey::Name(lhs), AssocBindingKey::Name(rhs)) => lhs == rhs,
        (AssocBindingKey::Projection(lhs), AssocBindingKey::Projection(rhs)) => {
            type_ref_decl_eq(lhs, rhs)
        }
        _ => false,
    }
}

fn array_len_decl_eq(lhs: &ArrayLen, rhs: &ArrayLen) -> bool {
    match (lhs, rhs) {
        (ArrayLen::Infer, ArrayLen::Infer) => true,
        (ArrayLen::Expr(lhs), ArrayLen::Expr(rhs)) => expr_decl_eq(lhs, rhs),
        _ => false,
    }
}

fn option_box_type_ref_decl_eq(lhs: Option<&TypeRef>, rhs: Option<&TypeRef>) -> bool {
    option_type_ref_decl_eq(lhs, rhs)
}

fn expr_decl_eq(lhs: &Expr, rhs: &Expr) -> bool {
    expr_identity(lhs) == expr_identity(rhs)
}

fn write_type_ref_identity(out: &mut String, ty: &TypeRef) {
    match &ty.kind {
        TypeKind::Error => out.push_str("error"),
        TypeKind::Path { segments } => {
            out.push_str("path(");
            write_joined(out, segments, |out, segment| {
                write_path_segment_identity(out, segment.kind);
                if !segment.args.is_empty() {
                    out.push('[');
                    write_joined(out, &segment.args, write_type_arg_identity);
                    out.push(']');
                }
            });
            out.push(')');
        }
        TypeKind::Projection {
            ty,
            trait_ref,
            name,
        } => {
            out.push_str("projection(");
            write_type_ref_identity(out, ty);
            out.push('|');
            write_type_ref_identity(out, trait_ref);
            out.push('|');
            write_symbol_identity(out, *name);
            out.push(')');
        }
        TypeKind::Pointer { is_readonly, elem } => {
            write_unary_type_identity(out, if *is_readonly { "ptr" } else { "mut_ptr" }, elem);
        }
        TypeKind::VolatilePointer { is_readonly, elem } => {
            write_unary_type_identity(
                out,
                if *is_readonly {
                    "volatile_ptr"
                } else {
                    "volatile_mut_ptr"
                },
                elem,
            );
        }
        TypeKind::Slice { is_readonly, elem } => {
            write_unary_type_identity(out, if *is_readonly { "slice" } else { "mut_slice" }, elem);
        }
        TypeKind::SlicePointee { elem } => write_unary_type_identity(out, "slice_pointee", elem),
        TypeKind::Array { len, elem } => {
            out.push_str("array(");
            write_array_len_identity(out, len);
            out.push('|');
            write_type_ref_identity(out, elem);
            out.push(')');
        }
        TypeKind::Range {
            start,
            end,
            inclusive,
        } => {
            out.push_str(if *inclusive { "range_inc(" } else { "range(" });
            write_optional_type_identity(out, start.as_deref());
            out.push('|');
            write_optional_type_identity(out, end.as_deref());
            out.push(')');
        }
        TypeKind::FunctionPointer {
            params,
            return_type,
            is_variadic,
        } => {
            out.push_str(if *is_variadic { "fn_var(" } else { "fn(" });
            write_joined(out, params, write_type_ref_identity);
            out.push('|');
            write_optional_type_identity(out, return_type.as_deref());
            out.push(')');
        }
        TypeKind::Optional { elem } => write_unary_type_identity(out, "optional", elem),
        TypeKind::ErrorUnion { error, value } => {
            out.push_str("error_union(");
            write_type_ref_identity(out, error);
            out.push('|');
            write_type_ref_identity(out, value);
            out.push(')');
        }
        TypeKind::SelfType => out.push_str("self"),
        TypeKind::Void => out.push_str("void"),
        TypeKind::Never => out.push_str("never"),
        TypeKind::Infer => out.push_str("infer"),
    }
}

fn write_unary_type_identity(out: &mut String, tag: &str, elem: &TypeRef) {
    out.push_str(tag);
    out.push('(');
    write_type_ref_identity(out, elem);
    out.push(')');
}

fn write_optional_type_identity(out: &mut String, ty: Option<&TypeRef>) {
    match ty {
        Some(ty) => write_type_ref_identity(out, ty),
        None => out.push_str("none"),
    }
}

fn write_type_arg_identity(out: &mut String, arg: &TypeArg) {
    match arg {
        TypeArg::Type(ty) => {
            out.push_str("type:");
            write_type_ref_identity(out, ty);
        }
        TypeArg::Const(expr) => {
            out.push_str("const:");
            out.push_str(&expr_identity(expr));
        }
        TypeArg::TypeOrConst { ty, expr } => {
            out.push_str("type_or_const:");
            write_type_ref_identity(out, ty);
            out.push('|');
            out.push_str(&expr_identity(expr));
        }
        TypeArg::AssocBinding { key, ty, .. } => {
            out.push_str("assoc:");
            write_assoc_binding_key_identity(out, key);
            out.push('=');
            write_type_ref_identity(out, ty);
        }
    }
}

fn write_assoc_binding_key_identity(out: &mut String, key: &AssocBindingKey) {
    match key {
        AssocBindingKey::Name(name) => write_symbol_identity(out, *name),
        AssocBindingKey::Projection(ty) => write_type_ref_identity(out, ty),
    }
}

fn write_array_len_identity(out: &mut String, len: &ArrayLen) {
    match len {
        ArrayLen::Infer => out.push_str("infer"),
        ArrayLen::Expr(expr) => out.push_str(&expr_identity(expr)),
    }
}

fn expr_identity(expr: &Expr) -> String {
    let mut out = String::new();
    write_expr_identity(&mut out, expr);
    out
}

fn write_expr_identity(out: &mut String, expr: &Expr) {
    match &expr.kind {
        ExprKind::Error => out.push_str("error"),
        ExprKind::Integer(value) => write_tagged_text(out, "int", value),
        ExprKind::Float(value) => write_tagged_text(out, "float", value),
        ExprKind::String(literal) => write_string_literal_identity(out, "string", literal),
        ExprKind::ByteString(literal) => write_string_literal_identity(out, "bytes", literal),
        ExprKind::Char(value) => write_tagged_text(out, "char", value),
        ExprKind::ByteChar(value) => write_tagged_text(out, "byte_char", value),
        ExprKind::Raw(value) => write_tagged_text(out, "raw", value),
        ExprKind::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        ExprKind::Null => out.push_str("null"),
        ExprKind::Ident(name) => write_tagged_symbol(out, "ident", *name),
        ExprKind::SelfValue => out.push_str("self_value"),
        ExprKind::PathRoot(kind) => {
            out.push_str("path_root(");
            write_path_segment_identity(out, *kind);
            out.push(')');
        }
        ExprKind::Underscore => out.push('_'),
        ExprKind::TypeTarget { ty } => {
            out.push_str("type_target(");
            write_type_ref_identity(out, ty);
            out.push(')');
        }
        ExprKind::TraitTarget { ty, trait_ref } => {
            out.push_str("trait_target(");
            write_type_ref_identity(out, ty);
            out.push('|');
            write_type_ref_identity(out, trait_ref);
            out.push(')');
        }
        ExprKind::BracketSuffix { callee, args } => {
            out.push_str("bracket(");
            write_expr_identity(out, callee);
            out.push('|');
            write_joined(out, args, write_bracket_arg_identity);
            out.push(')');
        }
        ExprKind::ArrayLiteral { elems } => write_array_elements_identity(out, "array", elems),
        ExprKind::StructLiteral { fields } => write_fields_identity(out, "struct", fields),
        ExprKind::TypedArrayLiteral { ty, elems } => {
            out.push_str("typed_array(");
            write_type_ref_identity(out, ty);
            out.push('|');
            write_array_elements_identity(out, "array", elems);
            out.push(')');
        }
        ExprKind::TypedStructLiteral { ty, fields } => {
            out.push_str("typed_struct(");
            write_type_ref_identity(out, ty);
            out.push('|');
            write_fields_identity(out, "struct", fields);
            out.push(')');
        }
        ExprKind::Unary { op, expr } => {
            out.push_str("unary(");
            out.push_str(unary_op_identity(*op));
            out.push('|');
            write_expr_identity(out, expr);
            out.push(')');
        }
        ExprKind::OptionalSome { expr } => write_unary_expr_identity(out, "some", expr),
        ExprKind::ErrorOk { expr } => write_unary_expr_identity(out, "ok", expr),
        ExprKind::ErrorErr { expr } => write_unary_expr_identity(out, "err", expr),
        ExprKind::Try { expr } => write_unary_expr_identity(out, "try", expr),
        ExprKind::Binary { lhs, op, rhs } => {
            out.push_str("binary(");
            write_expr_identity(out, lhs);
            out.push('|');
            out.push_str(binary_op_identity(*op));
            out.push('|');
            write_expr_identity(out, rhs);
            out.push(')');
        }
        ExprKind::Assign { lhs, op, rhs } => {
            out.push_str("assign(");
            write_expr_identity(out, lhs);
            out.push('|');
            out.push_str(assign_op_identity(*op));
            out.push('|');
            write_expr_identity(out, rhs);
            out.push(')');
        }
        ExprKind::Cast { expr, ty } => {
            out.push_str("cast(");
            write_expr_identity(out, expr);
            out.push('|');
            write_type_ref_identity(out, ty);
            out.push(')');
        }
        ExprKind::Call { callee, args } => {
            out.push_str("call(");
            write_expr_identity(out, callee);
            out.push('|');
            write_joined(out, args, write_expr_identity);
            out.push(')');
        }
        ExprKind::Qualified { lhs, name } => write_named_lhs_identity(out, "qualified", lhs, *name),
        ExprKind::Field { lhs, name } => write_named_lhs_identity(out, "field", lhs, *name),
        ExprKind::Index { lhs, index } => {
            out.push_str("index(");
            write_expr_identity(out, lhs);
            out.push('|');
            write_index_arg_identity(out, index);
            out.push(')');
        }
        ExprKind::Range(range) => write_slice_range_identity(out, "range_expr", range),
        ExprKind::Block(_) | ExprKind::If { .. } | ExprKind::IfPattern(_) | ExprKind::Switch(_) => {
            out.push_str("control_expr")
        }
    }
}

fn write_tagged_text(out: &mut String, tag: &str, text: &str) {
    out.push_str(tag);
    out.push('(');
    out.push_str(text);
    out.push(')');
}

fn write_tagged_symbol(out: &mut String, tag: &str, symbol: SymbolId) {
    out.push_str(tag);
    out.push('(');
    write_symbol_identity(out, symbol);
    out.push(')');
}

fn write_symbol_identity(out: &mut String, symbol: SymbolId) {
    out.push_str(&symbol_identity_key(symbol));
}

fn write_path_segment_identity(out: &mut String, segment: PathSegmentKind) {
    match segment {
        PathSegmentKind::Name(name) => write_symbol_identity(out, name),
        PathSegmentKind::Package => out.push_str("pkg"),
        PathSegmentKind::Super => out.push_str("super"),
        PathSegmentKind::SelfValue => out.push_str("self"),
    }
}

fn write_string_literal_identity(out: &mut String, tag: &str, literal: &StringLiteral) {
    out.push_str(tag);
    out.push('(');
    write_joined(out, &literal.parts, |out, part| out.push_str(part));
    out.push(')');
}

fn write_bracket_arg_identity(out: &mut String, arg: &BracketArg) {
    out.push_str("arg(");
    match (&arg.expr, &arg.ty) {
        (Some(expr), None) => write_expr_identity(out, expr),
        (None, Some(ty)) => write_type_ref_identity(out, ty),
        (Some(expr), Some(ty)) => {
            write_expr_identity(out, expr);
            out.push('|');
            write_type_ref_identity(out, ty);
        }
        (None, None) => out.push_str("empty"),
    }
    out.push(')');
}

fn write_array_elements_identity(out: &mut String, tag: &str, elems: &ArrayElements) {
    out.push_str(tag);
    out.push('(');
    match elems {
        ArrayElements::List(values) => write_joined(out, values, write_expr_identity),
        ArrayElements::Repeat { value, count } => {
            out.push_str("repeat:");
            write_expr_identity(out, value);
            out.push('|');
            write_expr_identity(out, count);
        }
    }
    out.push(')');
}

fn write_fields_identity(out: &mut String, tag: &str, fields: &[FieldInit]) {
    out.push_str(tag);
    out.push('(');
    write_joined(out, fields, |out, field| {
        write_symbol_identity(out, field.name);
        out.push('=');
        write_expr_identity(out, &field.value);
    });
    out.push(')');
}

fn write_unary_expr_identity(out: &mut String, tag: &str, expr: &Expr) {
    out.push_str(tag);
    out.push('(');
    write_expr_identity(out, expr);
    out.push(')');
}

fn write_named_lhs_identity(out: &mut String, tag: &str, lhs: &Expr, name: SymbolId) {
    out.push_str(tag);
    out.push('(');
    write_expr_identity(out, lhs);
    out.push('|');
    write_symbol_identity(out, name);
    out.push(')');
}

fn write_index_arg_identity(out: &mut String, index: &IndexArg) {
    match index {
        IndexArg::Expr(expr) => write_expr_identity(out, expr),
        IndexArg::Range(range) => write_slice_range_identity(out, "range", range),
    }
}

fn write_slice_range_identity(out: &mut String, tag: &str, range: &SliceRange) {
    out.push_str(tag);
    out.push('(');
    write_optional_expr_identity(out, range.start.as_deref());
    out.push('|');
    write_optional_expr_identity(out, range.end.as_deref());
    out.push('|');
    out.push_str(if range.inclusive {
        "inclusive"
    } else {
        "exclusive"
    });
    out.push(')');
}

fn write_optional_expr_identity(out: &mut String, expr: Option<&Expr>) {
    match expr {
        Some(expr) => write_expr_identity(out, expr),
        None => out.push_str("none"),
    }
}

fn unary_op_identity(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "bit_not",
        UnaryOp::RefReadOnly => "ref_readonly",
        UnaryOp::Ref => "ref",
        UnaryOp::Deref => "deref",
    }
}

fn binary_op_identity(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Rem => "rem",
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Shl => "shl",
        BinaryOp::Shr => "shr",
        BinaryOp::Lt => "lt",
        BinaryOp::Le => "le",
        BinaryOp::Gt => "gt",
        BinaryOp::Ge => "ge",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::BitAnd => "bit_and",
        BinaryOp::BitXor => "bit_xor",
        BinaryOp::BitOr => "bit_or",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn assign_op_identity(op: AssignOp) -> &'static str {
    match op {
        AssignOp::Assign => "assign",
        AssignOp::Add => "add",
        AssignOp::Sub => "sub",
        AssignOp::Shl => "shl",
        AssignOp::Shr => "shr",
        AssignOp::Mul => "mul",
        AssignOp::Div => "div",
        AssignOp::Rem => "rem",
        AssignOp::BitAnd => "bit_and",
        AssignOp::BitXor => "bit_xor",
        AssignOp::BitOr => "bit_or",
    }
}

fn write_joined<T>(out: &mut String, values: &[T], mut write: impl FnMut(&mut String, &T)) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write(out, value);
    }
}
