use crate::*;

use nia_ids::{BuiltinTraitMethod, InternedTyId, LayoutBuiltin, LocalId};
use nia_node_id::VersionedNodeKey;
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_span::Span;

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

pub(crate) trait ComptimeLowerContext {
    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError>;

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError>;

    fn lower_local_id(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError>;

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<InternedTyId>, ComptimeLowerError>;
}

impl ComptimeLowerContext for EarlyComptimeLowerInputs<'_> {
    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
        Ok(self.semantic_uses.and_then(|semantic_uses| {
            semantic_uses
                .node_associated_comptime_projection(key)
                .cloned()
                .map(ComptimeNameResolution::AssociatedComptimeProjection)
                .or_else(|| {
                    semantic_uses
                        .node_builtin_associated_value(key)
                        .map(ComptimeNameResolution::BuiltinAssociatedValue)
                })
                .or_else(|| {
                    semantic_uses
                        .node_const_generic_use(key)
                        .map(|name| ComptimeNameResolution::GenericParam(name.to_string()))
                })
                .or_else(|| {
                    semantic_uses
                        .node_value_use(key)
                        .map(ComptimeNameResolution::from)
                })
        }))
    }

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
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
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_local_def(key)))
    }

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
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
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
        if let Some(projection) = self.semantic_uses.node_associated_comptime_projection(key) {
            return Ok(Some(ComptimeNameResolution::AssociatedComptimeProjection(
                projection.clone(),
            )));
        }
        if let Some(value) = self.semantic_uses.node_builtin_associated_value(key) {
            return Ok(Some(ComptimeNameResolution::BuiltinAssociatedValue(value)));
        }
        if let Some(name) = self.semantic_uses.node_const_generic_use(key) {
            return Ok(Some(ComptimeNameResolution::GenericParam(name.to_string())));
        }
        self.semantic_uses
            .node_value_use(key)
            .map(ComptimeNameResolution::from)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime name"))
    }

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
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
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ComptimeLowerError> {
        self.semantic_uses
            .node_local_def(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "comptime local binding"))
    }

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
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
        nia_ast::ExprKind::IfPattern(if_pattern) => EarlyComptimeExprKind::Switch(Box::new(
            lower_if_pattern_as_switch(expr.span, if_pattern, context)?,
        )),
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
    if let Some((name, type_arg, builtin_span)) = std_builtin_call(callee) {
        if name == "error" {
            if type_arg.is_some() {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: "builtin `error` does not take a type argument".to_string(),
                });
            }
            if args.len() != 1 {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: "builtin `error` requires exactly one message argument".to_string(),
                });
            }
            return Ok(EarlyComptimeExprKind::CompileError {
                message: Box::new(lower_expr_internal(&args[0], context)?),
            });
        }
        if let Some(builtin) = LayoutBuiltin::from_name(name) {
            let Some(type_arg) = type_arg else {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: format!("builtin `{name}` requires a type argument"),
                });
            };
            if !args.is_empty() {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: format!("builtin `{name}` does not take value arguments"),
                });
            }
            return Ok(EarlyComptimeExprKind::LayoutBuiltin {
                builtin,
                type_arg: EarlyComptimeTypeArg::from_type_ref(type_arg, context)?,
            });
        }
        if name == "offset" {
            let Some(type_arg) = type_arg else {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: "builtin `offset` requires an aggregate type argument".to_string(),
                });
            };
            let [arg] = args else {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: "builtin `offset` requires exactly one field name argument"
                        .to_string(),
                });
            };
            let nia_ast::ExprKind::String(field) = &arg.kind else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message: "builtin `offset` field name must be a string literal".to_string(),
                });
            };
            return Ok(EarlyComptimeExprKind::FieldOffsetBuiltin {
                type_arg: EarlyComptimeTypeArg::from_type_ref(type_arg, context)?,
                field: lower_string_literal(field),
            });
        }
        if name == "embed" {
            if type_arg.is_some() {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: "builtin `embed` does not take a type argument".to_string(),
                });
            }
            let [arg] = args else {
                return Err(ComptimeLowerError {
                    span: builtin_span,
                    message: "builtin `embed` requires exactly one path argument".to_string(),
                });
            };
            let nia_ast::ExprKind::String(path) = &arg.kind else {
                return Err(ComptimeLowerError {
                    span: arg.span,
                    message: "builtin `embed` path must be a string literal".to_string(),
                });
            };
            return Ok(EarlyComptimeExprKind::Embed {
                path: lower_string_literal(path),
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

fn std_builtin_call(callee: &nia_ast::Expr) -> Option<(&str, Option<&nia_ast::TypeRef>, Span)> {
    if let nia_ast::ExprKind::BracketSuffix { callee, args } = &callee.kind {
        let [arg] = args.as_slice() else {
            return None;
        };
        let (name, None, span) = std_builtin_call(callee)? else {
            return None;
        };
        return Some((name, arg.ty.as_ref(), span));
    }
    let nia_ast::ExprKind::Qualified { lhs, name } = &callee.kind else {
        return None;
    };
    let nia_ast::ExprKind::Qualified {
        lhs: std_expr,
        name: builtin_segment,
    } = &lhs.kind
    else {
        return None;
    };
    let nia_ast::ExprKind::Ident(root) = &std_expr.kind else {
        return None;
    };
    (root == "std" && builtin_segment == "builtin").then_some((name.as_str(), None, callee.span))
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
    key: &VersionedNodeKey,
    span: Span,
) -> Result<Option<ComptimeNameResolution>, ComptimeLowerError> {
    context.resolve_name(key, span)
}

fn lower_comptime_name(
    name: &str,
    key: &VersionedNodeKey,
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
    key: &VersionedNodeKey,
    span: Span,
) -> Result<Option<LocalId>, ComptimeLowerError> {
    context.lower_local_id(key, span)
}

fn lower_local_use(
    context: &dyn ComptimeLowerContext,
    key: &VersionedNodeKey,
    span: Span,
) -> Result<Option<LocalId>, ComptimeLowerError> {
    context.lower_local_use(key, span)
}

pub(crate) fn lower_type_id(
    context: &dyn ComptimeLowerContext,
    key: &VersionedNodeKey,
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
    Ok(ResolvedComptimeFunction::from_parts(
        function.span,
        params,
        body,
    ))
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
    Ok(ResolvedComptimeForIn::new(
        resolve_comptime_pattern(for_in.pattern)?,
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
        EarlyComptimeExprKind::BuiltinComptime(builtin) => {
            ResolvedComptimeExprKind::BuiltinComptime(builtin)
        }
        EarlyComptimeExprKind::BuiltinValue(builtin) => {
            ResolvedComptimeExprKind::BuiltinValue(builtin)
        }
        EarlyComptimeExprKind::LayoutBuiltin { builtin, type_arg } => {
            ResolvedComptimeExprKind::LayoutBuiltin {
                builtin,
                type_arg: resolve_type_arg(type_arg)?,
            }
        }
        EarlyComptimeExprKind::FieldOffsetBuiltin { type_arg, field } => {
            ResolvedComptimeExprKind::FieldOffsetBuiltin {
                type_arg: resolve_type_arg(type_arg)?,
                field,
            }
        }
        EarlyComptimeExprKind::Embed { path } => ResolvedComptimeExprKind::Embed { path },
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
    Ok(ResolvedComptimeExpr::from_parts(span, kind))
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
            .map(resolve_comptime_pattern)
            .collect::<Result<Vec<_>, _>>()?,
        resolve_comptime_switch_arm_body(arm.body)?,
    ))
}

fn resolve_comptime_pattern(
    pattern: EarlyComptimePattern,
) -> Result<ResolvedComptimePattern, ComptimeLowerError> {
    match pattern {
        EarlyComptimePattern::Wildcard { span } => Ok(ResolvedComptimePattern::wildcard(span)),
        EarlyComptimePattern::Bind {
            name,
            local_id,
            span,
        } => Ok(ResolvedComptimePattern::bind(
            name,
            local_id.ok_or_else(|| unresolved_error(span, "comptime switch pattern local"))?,
            span,
        )),
        EarlyComptimePattern::Pointer { pattern, span } => Ok(ResolvedComptimePattern::pointer(
            resolve_comptime_pattern(*pattern)?,
            span,
        )),
        EarlyComptimePattern::MutPointer { pattern, span } => Ok(
            ResolvedComptimePattern::mut_pointer(resolve_comptime_pattern(*pattern)?, span),
        ),
        EarlyComptimePattern::OptionalSome { pattern, span } => Ok(
            ResolvedComptimePattern::optional_some(resolve_comptime_pattern(*pattern)?, span),
        ),
        EarlyComptimePattern::OptionalNull { span } => {
            Ok(ResolvedComptimePattern::optional_null(span))
        }
        EarlyComptimePattern::ErrorOk { pattern, span } => Ok(ResolvedComptimePattern::error_ok(
            resolve_comptime_pattern(*pattern)?,
            span,
        )),
        EarlyComptimePattern::ErrorErr { pattern, span } => Ok(ResolvedComptimePattern::error_err(
            resolve_comptime_pattern(*pattern)?,
            span,
        )),
        EarlyComptimePattern::Expr(expr) => resolve_expr(expr).map(ResolvedComptimePattern::expr),
        EarlyComptimePattern::Range {
            start,
            end,
            inclusive,
            span,
        } => Ok(ResolvedComptimePattern::range(
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

pub(crate) fn unresolved_error(span: Span, what: &str) -> ComptimeLowerError {
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
            let (name, node_key) =
                single_pattern_binding(&binding.pattern).ok_or_else(|| ComptimeLowerError {
                    span: binding.pattern.span,
                    message: "comptime function binding requires a single binding pattern"
                        .to_string(),
                })?;
            EarlyComptimeStmtKind::Binding(EarlyComptimeBinding {
                span: stmt.span,
                name: name.to_string(),
                local_id: lower_local_id(context, node_key, binding.pattern.span)?,
                explicit_type: binding
                    .ty
                    .as_ref()
                    .map(|ty| lower_type_id(context, &ty.node_key, ty.span))
                    .transpose()?
                    .flatten(),
                is_mutable: binding.is_mutable,
                value: lower_expr_internal(value, context)?,
            })
        }
        nia_ast::StmtKind::Static(_) => {
            return Err(ComptimeLowerError {
                span: stmt.span,
                message: "static declarations are not supported in comptime function bodies"
                    .to_string(),
            });
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
            pattern: lower_pattern_with_context(&for_in.pattern, context)?,
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

fn lower_if_pattern_as_switch(
    span: Span,
    if_pattern: &nia_ast::IfPatternExpr,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimeSwitch, ComptimeLowerError> {
    let mut arms = if_pattern
        .arms
        .iter()
        .map(|arm| {
            Ok(EarlyComptimeSwitchArm {
                span: arm.span,
                patterns: vec![lower_pattern_with_context(&arm.pattern, context)?],
                body: EarlyComptimeSwitchArmBody::Block(lower_block_with_context(
                    &arm.body, context,
                )?),
            })
        })
        .collect::<Result<Vec<_>, ComptimeLowerError>>()?;
    if let Some(else_branch) = &if_pattern.else_branch {
        arms.push(EarlyComptimeSwitchArm {
            span: else_branch.span,
            patterns: vec![EarlyComptimePattern::Wildcard {
                span: else_branch.span,
            }],
            body: EarlyComptimeSwitchArmBody::Expr(lower_expr_internal(else_branch, context)?),
        });
    }
    Ok(EarlyComptimeSwitch {
        span,
        target: lower_expr_internal(&if_pattern.target, context)?,
        arms,
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
) -> Result<EarlyComptimePattern, ComptimeLowerError> {
    match &pattern.kind {
        nia_ast::SwitchPatternKind::Wildcard => {
            Ok(EarlyComptimePattern::Wildcard { span: pattern.span })
        }
        nia_ast::SwitchPatternKind::Expr(expr) => {
            lower_expr_internal(expr, context).map(EarlyComptimePattern::Expr)
        }
        nia_ast::SwitchPatternKind::Range {
            start,
            end,
            inclusive,
        } => Ok(EarlyComptimePattern::Range {
            start: lower_expr_internal(start, context)?,
            end: lower_expr_internal(end, context)?,
            inclusive: *inclusive,
            span: pattern.span,
        }),
    }
}

fn lower_pattern_with_context(
    pattern: &nia_ast::Pattern,
    context: &dyn ComptimeLowerContext,
) -> Result<EarlyComptimePattern, ComptimeLowerError> {
    match &pattern.kind {
        nia_ast::PatternKind::Wildcard => Ok(EarlyComptimePattern::Wildcard { span: pattern.span }),
        nia_ast::PatternKind::Bind { name, node_key, .. } => Ok(EarlyComptimePattern::Bind {
            name: name.clone(),
            local_id: lower_local_id(context, node_key, pattern.span)?,
            span: pattern.span,
        }),
        nia_ast::PatternKind::Pointer(inner) => Ok(EarlyComptimePattern::Pointer {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::MutPointer(inner) => Ok(EarlyComptimePattern::MutPointer {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::OptionalSome(inner) => Ok(EarlyComptimePattern::OptionalSome {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::OptionalNull => {
            Ok(EarlyComptimePattern::OptionalNull { span: pattern.span })
        }
        nia_ast::PatternKind::ErrorOk(inner) => Ok(EarlyComptimePattern::ErrorOk {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::ErrorErr(inner) => Ok(EarlyComptimePattern::ErrorErr {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::Expr(expr) => {
            lower_expr_internal(expr, context).map(EarlyComptimePattern::Expr)
        }
        nia_ast::PatternKind::Range {
            start,
            end,
            inclusive,
        } => Ok(EarlyComptimePattern::Range {
            start: lower_expr_internal(start, context)?,
            end: lower_expr_internal(end, context)?,
            inclusive: *inclusive,
            span: pattern.span,
        }),
    }
}

fn single_pattern_binding(
    pattern: &nia_ast::Pattern,
) -> Option<(&str, &nia_node_id::VersionedNodeKey)> {
    match &pattern.kind {
        nia_ast::PatternKind::Bind { name, node_key, .. } => Some((name, node_key)),
        nia_ast::PatternKind::Pointer(inner) | nia_ast::PatternKind::MutPointer(inner) => {
            single_pattern_binding(inner)
        }
        _ => None,
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
