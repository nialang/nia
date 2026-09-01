// SPDX-License-Identifier: GPL-3.0-or-later
use nia_node_id::VersionedNodeKey;
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_identity_key};

use crate::{
    ArrayElements, AssignOp, BinaryOp, Block, BracketArg, Expr, ExprKind, FieldInit, IndexArg,
    MatchArmBody, NominalPatternFields, Pattern, PatternKind, SliceRange, Stmt, StmtKind,
    StringLiteral, UnaryOp, UsingGroupItem, UsingItem, UsingName, UsingSelector,
};

/// Kind of one source path segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathSegmentKind {
    /// Named path segment.
    Name(SymbolId),
    /// Package-root path segment.
    Package,
    /// Parent-module path segment.
    Super,
    /// Current-module or value path segment.
    SelfValue,
}

/// Parsed type reference with source spelling and identity.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    /// Source span covering the type.
    pub span: Span,
    /// Stable syntax identity for the type node.
    pub node_key: VersionedNodeKey,
    /// Original type spelling used by diagnostics.
    pub text: String,
    /// Type syntax payload.
    pub kind: TypeKind,
}

/// Kinds of type syntax produced by the parser.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// Recovery type for invalid syntax.
    Error,
    /// Qualified type path.
    Path {
        /// Path segments in source order.
        segments: Vec<TypePathSegment>,
    },
    /// Explicit associated-type projection.
    Projection {
        /// Projected self type.
        ty: Box<TypeRef>,
        /// Trait defining the associated type.
        trait_ref: Box<TypeRef>,
        /// Associated type name.
        name: SymbolId,
    },
    /// Pointer type.
    Pointer {
        /// Whether mutation through the pointer is forbidden.
        is_readonly: bool,
        /// Pointed-to type.
        elem: Box<TypeRef>,
    },
    /// Volatile pointer type.
    VolatilePointer {
        /// Whether mutation through the pointer is forbidden.
        is_readonly: bool,
        /// Pointed-to type.
        elem: Box<TypeRef>,
    },
    /// Slice view type.
    Slice {
        /// Whether mutation through the slice is forbidden.
        is_readonly: bool,
        /// Slice element type.
        elem: Box<TypeRef>,
    },
    /// Unsized slice pointee type.
    SlicePointee {
        /// Slice element type.
        elem: Box<TypeRef>,
    },
    /// Fixed-length array type.
    Array {
        /// Array length syntax.
        len: ArrayLen,
        /// Array element type.
        elem: Box<TypeRef>,
    },
    /// Tuple type.
    Tuple {
        /// Tuple elements in source order.
        elems: Vec<TypeRef>,
    },
    /// Type-level range.
    Range {
        /// Optional start type.
        start: Option<Box<TypeRef>>,
        /// Optional end type.
        end: Option<Box<TypeRef>>,
        /// Whether the end is included.
        inclusive: bool,
    },
    /// Concrete function pointer type.
    FunctionPointer {
        /// Parameter types.
        params: Vec<TypeRef>,
        /// Optional return type.
        return_type: Option<Box<TypeRef>>,
        /// Whether variadic arguments are accepted.
        is_variadic: bool,
    },
    /// Callable interface type.
    Callable {
        /// Parameter types.
        params: Vec<TypeRef>,
        /// Optional return type.
        return_type: Option<Box<TypeRef>>,
    },
    /// Optional type.
    Optional {
        /// Wrapped element type.
        elem: Box<TypeRef>,
    },
    /// Error-union type.
    ErrorUnion {
        /// Error type.
        error: Box<TypeRef>,
        /// Success value type.
        value: Box<TypeRef>,
    },
    /// `Self` type.
    SelfType,
    /// Opaque inferred implementation type.
    Opaque,
    /// Uninhabited never type.
    Never,
    /// Type inference placeholder.
    Infer,
}

/// One segment in a type path with generic arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct TypePathSegment {
    /// Segment kind.
    pub kind: PathSegmentKind,
    /// Generic arguments attached to the segment.
    pub args: Vec<TypeArg>,
}

/// Generic argument syntax.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeArg {
    /// Unambiguously parsed type argument.
    Type(TypeRef),
    /// Unambiguously parsed const expression argument.
    Const(Expr),
    /// Syntax retaining both type and const interpretations.
    TypeOrConst {
        /// Type interpretation.
        ty: TypeRef,
        /// Const-expression interpretation.
        expr: Expr,
    },
    /// Associated-type equality binding.
    AssocBinding {
        /// Named or projected binding key.
        key: AssocBindingKey,
        /// Bound type.
        ty: TypeRef,
        /// Source span covering the binding.
        span: Span,
    },
}

/// Left-hand key of an associated-type binding.
#[derive(Debug, Clone, PartialEq)]
pub enum AssocBindingKey {
    /// Direct associated-type name.
    Name(SymbolId),
    /// Explicit projected associated type.
    Projection(TypeRef),
}

/// Array length syntax.
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayLen {
    /// Inferred array length.
    Infer,
    /// Explicit const expression length.
    Expr(Box<Expr>),
}

/// Parsed where clause.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WhereClause {
    /// Predicates in source order.
    pub predicates: Vec<WherePredicate>,
}

/// One type-and-bounds where predicate.
#[derive(Debug, Clone, PartialEq)]
pub struct WherePredicate {
    /// Constrained type.
    pub ty: TypeRef,
    /// Required trait or type bounds.
    pub bounds: Vec<TypeRef>,
    /// Source span covering the predicate.
    pub span: Span,
}

/// Compares two type references by declaration syntax, ignoring spans and node keys.
pub fn type_ref_decl_eq(lhs: &TypeRef, rhs: &TypeRef) -> bool {
    type_kind_decl_eq(&lhs.kind, &rhs.kind)
}

/// Compares two ordered type-reference lists by declaration syntax.
pub fn type_refs_decl_eq(lhs: &[TypeRef], rhs: &[TypeRef]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs.iter())
            .all(|(lhs, rhs)| type_ref_decl_eq(lhs, rhs))
}

/// Compares optional type references by declaration syntax.
pub fn option_type_ref_decl_eq(lhs: Option<&TypeRef>, rhs: Option<&TypeRef>) -> bool {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => type_ref_decl_eq(lhs, rhs),
        (None, None) => true,
        _ => false,
    }
}

/// Compares where clauses by ordered declaration syntax.
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

/// Encodes one type reference as a stable syntax identity string.
pub fn type_ref_identity(ty: &TypeRef) -> String {
    let mut out = String::new();
    write_type_ref_identity(&mut out, ty);
    out
}

/// Encodes type references as stable syntax identities in source order.
pub fn type_refs_identity(types: &[TypeRef]) -> Vec<String> {
    types.iter().map(type_ref_identity).collect()
}

/// Encodes a where clause as stable constrained-type and bound identities.
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
        | (TypeKind::Opaque, TypeKind::Opaque)
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
        (TypeKind::Tuple { elems: lhs }, TypeKind::Tuple { elems: rhs }) => {
            type_refs_decl_eq(lhs, rhs)
        }
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
            TypeKind::Callable {
                params: lhs_params,
                return_type: lhs_return,
            },
            TypeKind::Callable {
                params: rhs_params,
                return_type: rhs_return,
            },
        ) => {
            type_refs_decl_eq(lhs_params, rhs_params)
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

/// Compares expression declaration structure while ignoring source locations.
pub fn expr_decl_eq(lhs: &Expr, rhs: &Expr) -> bool {
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
        TypeKind::Tuple { elems } => {
            out.push_str("tuple(");
            write_joined(out, elems, write_type_ref_identity);
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
        TypeKind::Callable {
            params,
            return_type,
        } => {
            out.push_str("callable(");
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
        TypeKind::Opaque => out.push_str("opaque"),
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
            out.push_str("const_eval:");
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
        ExprKind::Tuple(elems) => {
            out.push_str("tuple(");
            write_joined(out, elems, write_expr_identity);
            out.push(')');
        }
        ExprKind::Closure {
            captures,
            params,
            body,
        } => {
            out.push_str("closure(");
            for (index, capture) in captures.iter().enumerate() {
                if index != 0 {
                    out.push('|');
                }
                write_symbol_identity(out, capture.name);
                out.push('=');
                write_expr_identity(out, &capture.value);
            }
            out.push('|');
            for (index, param) in params.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                if let Some(ty) = &param.ty {
                    write_type_ref_identity(out, ty);
                } else {
                    out.push_str("receiver");
                }
            }
            out.push('|');
            write_expr_identity(out, body);
            out.push(')');
        }
        ExprKind::ArrayLiteral { elems } => write_array_elements_identity(out, "array", elems),
        ExprKind::TypedStructLiteral { ty, fields } => {
            out.push_str("typed_struct(");
            write_type_ref_identity(out, ty);
            out.push('|');
            write_fields_identity(out, "struct", fields);
            out.push(')');
        }
        ExprKind::QualifiedStructLiteral { target, fields } => {
            out.push_str("qualified_struct(");
            write_expr_identity(out, target);
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
        ExprKind::TupleField { lhs, index } => {
            out.push_str("tuple_field(");
            write_expr_identity(out, lhs);
            out.push('|');
            out.push_str(&index.to_string());
            out.push(')');
        }
        ExprKind::Index { lhs, index } => {
            out.push_str("index(");
            write_expr_identity(out, lhs);
            out.push('|');
            write_index_arg_identity(out, index);
            out.push(')');
        }
        ExprKind::Range(range) => write_slice_range_identity(out, "range_expr", range),
        ExprKind::Block(block) => write_block_identity(out, block),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            out.push_str("if(");
            write_expr_identity(out, cond);
            out.push('|');
            write_block_identity(out, then_branch);
            out.push('|');
            write_optional_expr_identity(out, else_branch.as_deref());
            out.push(')');
        }
        ExprKind::IfPattern(value) => {
            out.push_str("if_pattern(");
            write_expr_identity(out, &value.target);
            out.push('|');
            write_pattern_identity(out, &value.pattern);
            out.push('|');
            write_block_identity(out, &value.then_branch);
            out.push('|');
            write_optional_expr_identity(out, value.else_branch.as_deref());
            out.push(')');
        }
        ExprKind::Match(value) => {
            out.push_str("match(");
            write_expr_identity(out, &value.target);
            out.push('|');
            for (index, arm) in value.arms.iter().enumerate() {
                if index > 0 {
                    out.push(';');
                }
                write_joined(out, &arm.patterns, write_pattern_identity);
                out.push('=');
                write_match_arm_body_identity(out, &arm.body);
            }
            out.push(')');
        }
    }
}

fn write_block_identity(out: &mut String, block: &Block) {
    out.push_str("block(");
    for (index, stmt) in block.stmts.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        write_stmt_identity(out, stmt);
    }
    out.push('|');
    write_optional_expr_identity(out, block.tail.as_deref());
    out.push(')');
}

fn write_stmt_identity(out: &mut String, stmt: &Stmt) {
    match &stmt.kind {
        StmtKind::Binding(binding) => {
            out.push_str("binding(");
            write_pattern_identity(out, &binding.pattern);
            out.push('|');
            if let Some(ty) = &binding.ty {
                write_type_ref_identity(out, ty);
            } else {
                out.push_str("none");
            }
            out.push('|');
            write_optional_expr_identity(out, binding.value.as_ref());
            out.push('|');
            out.push_str(if binding.is_mutable() {
                "mut"
            } else if binding.is_const() {
                "const"
            } else {
                "let"
            });
            out.push(')');
        }
        StmtKind::Static(item) => {
            out.push_str("static(");
            out.push_str(&symbol_identity_key(item.name));
            out.push('|');
            if let Some(ty) = &item.ty {
                write_type_ref_identity(out, ty);
            } else {
                out.push_str("none");
            }
            out.push('|');
            write_optional_expr_identity(out, item.value.as_ref());
            out.push('|');
            out.push_str(if item.is_const() {
                "const"
            } else if item.is_mutable() {
                if item.is_extern() {
                    "mut_extern"
                } else {
                    "mut"
                }
            } else if item.is_extern() {
                "static_extern"
            } else {
                "static"
            });
            out.push(')');
        }
        StmtKind::Using(item) => {
            out.push_str("using(");
            write_using_identity(out, item);
            out.push(')');
        }
        StmtKind::Expr(expr) => {
            out.push_str("expr(");
            write_expr_identity(out, expr);
            out.push(')');
        }
        StmtKind::Return(expr) => {
            out.push_str("return(");
            write_optional_expr_identity(out, expr.as_deref());
            out.push(')');
        }
        StmtKind::Defer(expr) => {
            out.push_str("defer(");
            write_expr_identity(out, expr);
            out.push(')');
        }
        StmtKind::ForIn(value) => {
            out.push_str("for(");
            write_pattern_identity(out, &value.pattern);
            out.push('|');
            write_expr_identity(out, &value.iter);
            out.push('|');
            write_block_identity(out, &value.body);
            out.push(')');
        }
        StmtKind::While(value) => {
            out.push_str("while(");
            write_expr_identity(out, &value.cond);
            out.push('|');
            write_block_identity(out, &value.body);
            out.push(')');
        }
        StmtKind::Loop(value) => {
            out.push_str("loop(");
            write_block_identity(out, &value.body);
            out.push(')');
        }
        StmtKind::Break => out.push_str("break"),
        StmtKind::Continue => out.push_str("continue"),
    }
}

fn write_using_identity(out: &mut String, item: &UsingItem) {
    write_joined(out, &item.host, |out, segment| {
        write_path_segment_identity(out, segment.kind)
    });
    out.push('|');
    write_using_selector_identity(out, &item.selector);
}

fn write_using_selector_identity(out: &mut String, selector: &UsingSelector) {
    match selector {
        UsingSelector::Single(name) => write_using_name_identity(out, name),
        UsingSelector::Group(items) => {
            out.push_str("group(");
            write_joined(out, items, |out, item| match item {
                UsingGroupItem::Name(name) => write_using_name_identity(out, name),
                UsingGroupItem::Nested { host, selector } => {
                    out.push_str("nested(");
                    write_joined(out, host, |out, segment| {
                        write_path_segment_identity(out, segment.kind)
                    });
                    out.push('|');
                    write_using_selector_identity(out, selector);
                    out.push(')');
                }
            });
            out.push(')');
        }
        UsingSelector::Wildcard { .. } => out.push_str("wildcard"),
        UsingSelector::SelfName => out.push_str("self"),
    }
}

fn write_using_name_identity(out: &mut String, name: &UsingName) {
    write_symbol_identity(out, name.name);
    out.push(':');
    match name.alias {
        Some(alias) => write_symbol_identity(out, alias),
        None => out.push_str("none"),
    }
}

fn write_pattern_identity(out: &mut String, pattern: &Pattern) {
    match &pattern.kind {
        PatternKind::Wildcard => out.push_str("wildcard"),
        PatternKind::Bind {
            name, is_mutable, ..
        } => {
            out.push_str(if *is_mutable { "bind_mut(" } else { "bind(" });
            write_symbol_identity(out, *name);
            out.push(')');
        }
        PatternKind::Pointer(inner) => write_unary_pattern_identity(out, "ptr", inner),
        PatternKind::MutPointer(inner) => write_unary_pattern_identity(out, "mut_ptr", inner),
        PatternKind::OptionalSome(inner) => write_unary_pattern_identity(out, "some", inner),
        PatternKind::ErrorOk(inner) => write_unary_pattern_identity(out, "ok", inner),
        PatternKind::ErrorErr(inner) => write_unary_pattern_identity(out, "err", inner),
        PatternKind::OptionalNull => out.push_str("null"),
        PatternKind::Tuple(values) => {
            out.push_str("tuple(");
            write_joined(out, values, write_pattern_identity);
            out.push(')');
        }
        PatternKind::Nominal {
            constructor,
            fields,
        } => {
            out.push_str("nominal(");
            write_expr_identity(out, constructor);
            out.push('|');
            match fields {
                NominalPatternFields::Tuple(values) => {
                    write_joined(out, values, write_pattern_identity)
                }
                NominalPatternFields::Named { fields, rest } => {
                    for (index, field) in fields.iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        write_symbol_identity(out, field.name);
                        out.push('=');
                        write_pattern_identity(out, &field.pattern);
                    }
                    out.push('|');
                    out.push_str(if rest.is_some() { "rest" } else { "exact" });
                }
            }
            out.push(')');
        }
        PatternKind::Expr(expr) => {
            out.push_str("expr(");
            write_expr_identity(out, expr);
            out.push(')');
        }
        PatternKind::Range {
            start,
            end,
            inclusive,
        } => {
            out.push_str(if *inclusive { "range_inc(" } else { "range(" });
            write_expr_identity(out, start);
            out.push('|');
            write_expr_identity(out, end);
            out.push(')');
        }
    }
}

fn write_unary_pattern_identity(out: &mut String, tag: &str, pattern: &Pattern) {
    out.push_str(tag);
    out.push('(');
    write_pattern_identity(out, pattern);
    out.push(')');
}

fn write_match_arm_body_identity(out: &mut String, body: &MatchArmBody) {
    match body {
        MatchArmBody::Expr(expr) => write_expr_identity(out, expr),
        MatchArmBody::Stmt(stmt) => write_stmt_identity(out, stmt),
        MatchArmBody::Block(block) => write_block_identity(out, block),
    }
}

fn write_tagged_text(out: &mut String, tag: &str, text: &str) {
    out.push_str(tag);
    out.push('(');
    write_length_prefixed_text(out, text);
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
    write_joined(out, &literal.parts, |out, part| {
        write_length_prefixed_text(out, part);
    });
    out.push(')');
}

fn write_length_prefixed_text(out: &mut String, text: &str) {
    out.push_str(&text.len().to_string());
    out.push(':');
    out.push_str(text);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MatchExpr;
    use nia_node_id::{SyntaxKind, VersionedNodeKey};
    use nia_source::{SourceId, SourceRevision, SourceVersion};

    fn type_ref(kind: TypeKind, span: Span) -> TypeRef {
        TypeRef {
            span,
            node_key: VersionedNodeKey::span(
                SourceVersion {
                    id: SourceId(1),
                    revision: SourceRevision::INITIAL,
                },
                SyntaxKind::Type,
                span,
            ),
            text: String::new(),
            kind,
        }
    }

    fn expr(kind: ExprKind, span: Span) -> Expr {
        Expr {
            span,
            node_key: VersionedNodeKey::span(
                SourceVersion {
                    id: SourceId(1),
                    revision: SourceRevision::INITIAL,
                },
                SyntaxKind::Expr,
                span,
            ),
            kind,
        }
    }

    fn array_with_len(len: Expr) -> TypeRef {
        type_ref(
            TypeKind::Array {
                len: ArrayLen::Expr(Box::new(len)),
                elem: Box::new(type_ref(TypeKind::Error, Span::new(20, 21))),
            },
            Span::new(0, 21),
        )
    }

    #[test]
    fn declaration_equality_ignores_source_locations() {
        let left = type_ref(TypeKind::Error, Span::new(0, 1));
        let right = type_ref(TypeKind::Error, Span::new(9, 12));
        assert!(type_ref_decl_eq(&left, &right));

        let different = type_ref(TypeKind::SelfType, Span::new(0, 1));
        assert!(!type_ref_decl_eq(&left, &different));
    }

    #[test]
    fn where_clause_identity_preserves_order_and_bounds() {
        let first = type_ref(TypeKind::Error, Span::new(0, 1));
        let second = type_ref(TypeKind::SelfType, Span::new(2, 6));
        let clause = WhereClause {
            predicates: vec![WherePredicate {
                ty: first,
                bounds: vec![second],
                span: Span::new(0, 6),
            }],
        };
        assert_eq!(where_clause_identity(&clause).len(), 1);
        assert_eq!(where_clause_identity(&clause)[0].0, "error");
        assert_eq!(where_clause_identity(&clause)[0].1, vec!["self".to_owned()]);
    }

    #[test]
    fn raw_literal_identity_cannot_impersonate_adjacent_tuple_elements() {
        let adjacent = array_with_len(expr(
            ExprKind::Tuple(vec![
                expr(ExprKind::Raw("left".to_owned()), Span::new(1, 5)),
                expr(ExprKind::Raw("right".to_owned()), Span::new(7, 12)),
            ]),
            Span::new(0, 13),
        ));
        let embedded_separator = array_with_len(expr(
            ExprKind::Tuple(vec![expr(
                ExprKind::Raw("left),raw(right".to_owned()),
                Span::new(1, 16),
            )]),
            Span::new(0, 17),
        ));

        assert_ne!(
            type_ref_identity(&adjacent),
            type_ref_identity(&embedded_separator)
        );
        assert!(!type_ref_decl_eq(&adjacent, &embedded_separator));
    }

    #[test]
    fn string_literal_identity_cannot_impersonate_adjacent_tuple_elements() {
        let adjacent = array_with_len(expr(
            ExprKind::Tuple(vec![
                expr(
                    ExprKind::String(StringLiteral {
                        parts: vec!["left".to_owned()],
                    }),
                    Span::new(1, 7),
                ),
                expr(
                    ExprKind::String(StringLiteral {
                        parts: vec!["right".to_owned()],
                    }),
                    Span::new(9, 16),
                ),
            ]),
            Span::new(0, 17),
        ));
        let embedded_separator = array_with_len(expr(
            ExprKind::Tuple(vec![expr(
                ExprKind::String(StringLiteral {
                    parts: vec!["left),string(right".to_owned()],
                }),
                Span::new(1, 21),
            )]),
            Span::new(0, 22),
        ));

        assert_ne!(
            type_ref_identity(&adjacent),
            type_ref_identity(&embedded_separator)
        );
        assert!(!type_ref_decl_eq(&adjacent, &embedded_separator));
    }

    #[test]
    fn control_expression_identity_preserves_branch_structure() {
        let empty = || Block {
            span: Span::default(),
            stmts: Vec::new(),
            tail: None,
        };
        let if_true = expr(
            ExprKind::If {
                cond: Box::new(expr(ExprKind::Bool(true), Span::default())),
                then_branch: empty(),
                else_branch: None,
            },
            Span::default(),
        );
        let if_false = expr(
            ExprKind::If {
                cond: Box::new(expr(ExprKind::Bool(false), Span::default())),
                then_branch: empty(),
                else_branch: None,
            },
            Span::default(),
        );
        let matched = expr(
            ExprKind::Match(Box::new(MatchExpr {
                target: expr(ExprKind::Bool(true), Span::default()),
                arms: Vec::new(),
            })),
            Span::default(),
        );

        assert!(!expr_decl_eq(&if_true, &if_false));
        assert!(!expr_decl_eq(&if_true, &matched));
    }

    #[test]
    fn closure_identity_includes_body_structure() {
        let closure = |body| {
            expr(
                ExprKind::Closure {
                    captures: Vec::new(),
                    params: Vec::new(),
                    body: Box::new(expr(body, Span::default())),
                },
                Span::default(),
            )
        };
        let left = closure(ExprKind::Bool(true));
        let right = closure(ExprKind::Bool(false));
        assert!(!expr_decl_eq(&left, &right));
    }

    #[test]
    fn static_statement_identity_distinguishes_const_and_static_storage() {
        let item = |kind| crate::BindingItem {
            name: SymbolId::from_stable_hash(7),
            ty: None,
            value: None,
            kind,
            node_key: VersionedNodeKey::span(
                SourceVersion {
                    id: SourceId(1),
                    revision: SourceRevision::INITIAL,
                },
                SyntaxKind::Item,
                Span::default(),
            ),
        };
        let block = |kind| {
            expr(
                ExprKind::Block(Block {
                    span: Span::default(),
                    stmts: vec![Stmt {
                        span: Span::default(),
                        node_key: VersionedNodeKey::span(
                            SourceVersion {
                                id: SourceId(1),
                                revision: SourceRevision::INITIAL,
                            },
                            SyntaxKind::Stmt,
                            Span::default(),
                        ),
                        attributes: Vec::new(),
                        kind: StmtKind::Static(Box::new(item(kind))),
                    }],
                    tail: None,
                }),
                Span::default(),
            )
        };

        assert!(!expr_decl_eq(
            &block(crate::ItemBindingKind::Const),
            &block(crate::ItemBindingKind::Static {
                is_mutable: false,
                is_extern: false,
            },)
        ));
    }
}
