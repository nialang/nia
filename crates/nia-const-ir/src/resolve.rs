// SPDX-License-Identifier: GPL-3.0-or-later
//! Validation and conversion from early const IR into resolved const IR.
use crate::*;
use nia_span::Span;

// This module is the validation boundary between the permissive early IR and
// the resolved IR consumed by checking and evaluation. Every recursive branch
// must either preserve an intentionally optional type (for an inferred literal
// or unannotated binding) or reject a missing name, local, or required type id.

/// Converts an early const function and rejects unresolved parameter, pattern,
/// assignment, expression, and required type identities anywhere in its body.
pub fn resolve_function(
    function: EarlyConstFunction,
) -> Result<ResolvedConstFunction, ConstLowerError> {
    let params = function
        .params
        .into_iter()
        .map(resolve_const_param)
        .collect::<Result<Vec<_>, _>>()?;
    let body = resolve_const_block(function.body)?;
    Ok(ResolvedConstFunction::from_parts(
        function.span,
        params,
        body,
    ))
}

fn resolve_const_param(param: EarlyConstParam) -> Result<ResolvedConstParam, ConstLowerError> {
    let local_id = param
        .local_id
        .ok_or_else(|| unresolved_error(param.span, "const function parameter local"))?;
    Ok(ResolvedConstParam::new(
        param.span,
        param.name,
        local_id,
        resolve_optional_explicit_type(param.ty, "const function parameter type")?,
        param.receiver,
    ))
}

fn resolve_const_block(block: EarlyConstBlock) -> Result<ResolvedConstBlock, ConstLowerError> {
    let stmts = block
        .stmts
        .into_iter()
        .map(resolve_const_stmt)
        .collect::<Result<Vec<_>, _>>()?;
    let tail = block
        .tail
        .map(|tail| resolve_expr(*tail).map(Box::new))
        .transpose()?;
    Ok(ResolvedConstBlock::new(block.span, stmts, tail))
}

fn resolve_const_stmt(stmt: EarlyConstStmt) -> Result<ResolvedConstStmt, ConstLowerError> {
    let kind = match stmt.kind {
        EarlyConstStmtKind::Binding(binding) => {
            ResolvedConstStmtKind::Binding(resolve_const_binding(binding)?)
        }
        EarlyConstStmtKind::PatternBinding(binding) => {
            ResolvedConstStmtKind::PatternBinding(resolve_const_pattern_binding(*binding)?)
        }
        EarlyConstStmtKind::Expr(expr) => ResolvedConstStmtKind::Expr(resolve_expr(expr)?),
        EarlyConstStmtKind::Return(expr) => {
            ResolvedConstStmtKind::Return(expr.map(resolve_expr).transpose()?)
        }
        EarlyConstStmtKind::Break => ResolvedConstStmtKind::Break,
        EarlyConstStmtKind::Continue => ResolvedConstStmtKind::Continue,
        EarlyConstStmtKind::If {
            cond,
            then_branch,
            else_branch,
        } => ResolvedConstStmtKind::If {
            cond: resolve_expr(cond)?,
            then_branch: resolve_const_block(then_branch)?,
            else_branch: else_branch.map(resolve_const_block).transpose()?,
        },
        EarlyConstStmtKind::ForIn(for_in) => {
            ResolvedConstStmtKind::ForIn(resolve_const_for_in(*for_in)?)
        }
        EarlyConstStmtKind::While { cond, body } => ResolvedConstStmtKind::While {
            cond: resolve_expr(cond)?,
            body: resolve_const_block(body)?,
        },
        EarlyConstStmtKind::Loop { body } => ResolvedConstStmtKind::Loop {
            body: resolve_const_block(body)?,
        },
    };
    Ok(ResolvedConstStmt::new(stmt.span, kind))
}

fn resolve_const_pattern_binding(
    binding: EarlyConstPatternBinding,
) -> Result<ResolvedConstPatternBinding, ConstLowerError> {
    Ok(ResolvedConstPatternBinding::new(
        binding.span,
        resolve_const_pattern(binding.pattern)?,
        resolve_optional_explicit_type(binding.explicit_type, "const pattern binding type")?,
        binding.is_mutable,
        resolve_expr(binding.value)?,
    ))
}

fn resolve_const_binding(
    binding: EarlyConstBinding,
) -> Result<ResolvedConstBinding, ConstLowerError> {
    let local_id = binding
        .local_id
        .ok_or_else(|| unresolved_error(binding.span, "const local binding"))?;
    Ok(ResolvedConstBinding::new(
        binding.span,
        binding.name,
        local_id,
        resolve_optional_explicit_type(binding.explicit_type, "const local binding type")?,
        binding.is_mutable,
        resolve_expr(binding.value)?,
    ))
}

fn resolve_const_for_in(for_in: EarlyConstForIn) -> Result<ResolvedConstForIn, ConstLowerError> {
    Ok(ResolvedConstForIn::new(
        resolve_const_pattern(for_in.pattern)?,
        resolve_expr(for_in.iter)?,
        resolve_const_block(for_in.body)?,
    ))
}

/// Converts an early expression into the identity-complete IR required by
/// const checking and evaluation.
pub fn resolve_expr(expr: EarlyConstExpr) -> Result<ResolvedConstExpr, ConstLowerError> {
    let span = expr.span;
    let kind = match expr.kind {
        EarlyConstExprKind::Integer(value) => ResolvedConstExprKind::Integer(value),
        EarlyConstExprKind::Char(value) => ResolvedConstExprKind::Char(value),
        EarlyConstExprKind::ByteChar(value) => ResolvedConstExprKind::ByteChar(value),
        EarlyConstExprKind::Float(value) => ResolvedConstExprKind::Float(value),
        EarlyConstExprKind::String(value) => ResolvedConstExprKind::String(value),
        EarlyConstExprKind::ByteString(value) => ResolvedConstExprKind::ByteString(value),
        EarlyConstExprKind::Bool(value) => ResolvedConstExprKind::Bool(value),
        EarlyConstExprKind::Null => ResolvedConstExprKind::Null,
        EarlyConstExprKind::Ident(name) | EarlyConstExprKind::Qualified(name) => {
            ResolvedConstExprKind::Name(name.into_resolution(span)?)
        }
        EarlyConstExprKind::Field { lhs, name } => ResolvedConstExprKind::Field {
            lhs: Box::new(resolve_expr(*lhs)?),
            name,
        },
        EarlyConstExprKind::Method { receiver, name } => ResolvedConstExprKind::Method {
            receiver: Box::new(resolve_expr(*receiver)?),
            name,
        },
        EarlyConstExprKind::AssociatedFunction { target, name } => {
            ResolvedConstExprKind::AssociatedFunction {
                target: match target {
                    EarlyConstAssociatedTarget::Type(target) => {
                        ResolvedConstAssociatedTarget::Type(resolve_type_arg(target)?)
                    }
                    EarlyConstAssociatedTarget::Nominal { def_id, args } => {
                        ResolvedConstAssociatedTarget::Nominal {
                            def_id,
                            args: args
                                .into_iter()
                                .map(resolve_type_arg)
                                .collect::<Result<Vec<_>, _>>()?,
                        }
                    }
                },
                name,
            }
        }
        EarlyConstExprKind::Index { lhs, index } => ResolvedConstExprKind::Index {
            lhs: Box::new(resolve_expr(*lhs)?),
            index: Box::new(resolve_expr(*index)?),
        },
        EarlyConstExprKind::Slice { lhs, range } => ResolvedConstExprKind::Slice {
            lhs: Box::new(resolve_expr(*lhs)?),
            range: resolve_const_slice_range(range)?,
        },
        EarlyConstExprKind::Tuple(elems) => ResolvedConstExprKind::Tuple(
            elems
                .into_iter()
                .map(resolve_expr)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        EarlyConstExprKind::TupleField { lhs, index } => ResolvedConstExprKind::TupleField {
            lhs: Box::new(resolve_expr(*lhs)?),
            index,
        },
        EarlyConstExprKind::ArrayLiteral { elems } => ResolvedConstExprKind::ArrayLiteral {
            elems: resolve_const_array_elements(elems)?,
        },
        EarlyConstExprKind::StructLiteral { ty, fields } => ResolvedConstExprKind::StructLiteral {
            ty: resolve_type_arg(ty)?.ty(),
            fields: fields
                .into_iter()
                .map(resolve_const_field_init)
                .collect::<Result<Vec<_>, _>>()?,
        },
        EarlyConstExprKind::TupleStructLiteral {
            def_id,
            generic_args,
            fields,
        } => ResolvedConstExprKind::TupleStructLiteral {
            def_id,
            generic_args: generic_args
                .into_iter()
                .map(|arg| match arg {
                    EarlyConstGenericArg::Infer(span) => Ok(ResolvedConstGenericArg::Infer(span)),
                    EarlyConstGenericArg::Type(arg) => {
                        resolve_type_arg(arg).map(ResolvedConstGenericArg::Type)
                    }
                    EarlyConstGenericArg::Const(expr) => {
                        resolve_expr(expr).map(ResolvedConstGenericArg::Const)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?,
            fields: fields
                .into_iter()
                .map(resolve_const_field_init)
                .collect::<Result<Vec<_>, _>>()?,
        },
        EarlyConstExprKind::EnumStructLiteral { variant, fields } => {
            ResolvedConstExprKind::EnumStructLiteral {
                variant: Box::new(resolve_expr(*variant)?),
                fields: fields
                    .into_iter()
                    .map(resolve_const_field_init)
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        EarlyConstExprKind::CompileError { message } => ResolvedConstExprKind::CompileError {
            message: Box::new(resolve_expr(*message)?),
        },
        EarlyConstExprKind::Trap => ResolvedConstExprKind::Trap,
        EarlyConstExprKind::BuiltinConstValue(builtin) => {
            ResolvedConstExprKind::BuiltinConstValue(builtin)
        }
        EarlyConstExprKind::BuiltinValue(builtin) => ResolvedConstExprKind::BuiltinValue(builtin),
        EarlyConstExprKind::LayoutBuiltin { builtin, type_arg } => {
            ResolvedConstExprKind::LayoutBuiltin {
                builtin,
                type_arg: resolve_type_arg(type_arg)?,
            }
        }
        EarlyConstExprKind::FieldOffsetBuiltin { type_arg, field } => {
            ResolvedConstExprKind::FieldOffsetBuiltin {
                type_arg: resolve_type_arg(type_arg)?,
                field,
            }
        }
        EarlyConstExprKind::Embed { path } => ResolvedConstExprKind::Embed { path },
        EarlyConstExprKind::Call {
            callee,
            generic_args,
            args,
        } => ResolvedConstExprKind::Call {
            callee: Box::new(resolve_expr(*callee)?),
            generic_args: generic_args
                .into_iter()
                .map(|arg| match arg {
                    EarlyConstGenericArg::Infer(span) => Ok(ResolvedConstGenericArg::Infer(span)),
                    EarlyConstGenericArg::Type(arg) => {
                        resolve_type_arg(arg).map(ResolvedConstGenericArg::Type)
                    }
                    EarlyConstGenericArg::Const(expr) => {
                        resolve_expr(expr).map(ResolvedConstGenericArg::Const)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?,
            args: args
                .into_iter()
                .map(resolve_expr)
                .collect::<Result<Vec<_>, _>>()?,
        },
        EarlyConstExprKind::Unary { op, expr } => ResolvedConstExprKind::Unary {
            op,
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyConstExprKind::OptionalSome { expr } => ResolvedConstExprKind::OptionalSome {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyConstExprKind::ErrorOk { expr } => ResolvedConstExprKind::ErrorOk {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyConstExprKind::ErrorErr { expr } => ResolvedConstExprKind::ErrorErr {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyConstExprKind::Try { expr } => ResolvedConstExprKind::Try {
            expr: Box::new(resolve_expr(*expr)?),
        },
        EarlyConstExprKind::Binary { lhs, op, rhs } => ResolvedConstExprKind::Binary {
            lhs: Box::new(resolve_expr(*lhs)?),
            op,
            rhs: Box::new(resolve_expr(*rhs)?),
        },
        EarlyConstExprKind::Assign(assign) => {
            ResolvedConstExprKind::Assign(Box::new(resolve_const_assign(*assign)?))
        }
        EarlyConstExprKind::Range(range) => {
            ResolvedConstExprKind::Range(resolve_const_range(range)?)
        }
        EarlyConstExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => ResolvedConstExprKind::If {
            cond: Box::new(resolve_expr(*cond)?),
            then_branch: resolve_const_block(then_branch)?,
            else_branch: else_branch
                .map(|else_branch| resolve_expr(*else_branch).map(Box::new))
                .transpose()?,
        },
        EarlyConstExprKind::Match(matched) => {
            ResolvedConstExprKind::Match(Box::new(resolve_const_switch(*matched)?))
        }
        EarlyConstExprKind::Cast { expr, ty } => ResolvedConstExprKind::Cast {
            expr: Box::new(resolve_expr(*expr)?),
            ty: ty.ok_or_else(|| unresolved_error(span, "const cast type"))?,
        },
        EarlyConstExprKind::Block(block) => {
            ResolvedConstExprKind::Block(resolve_const_block(block)?)
        }
    };
    Ok(ResolvedConstExpr::from_parts(span, kind))
}

fn resolve_const_assign(assign: EarlyConstAssign) -> Result<ResolvedConstAssign, ConstLowerError> {
    Ok(ResolvedConstAssign::new(
        resolve_const_assign_target(assign.lhs)?,
        assign.op,
        resolve_expr(assign.rhs)?,
    ))
}

fn resolve_const_assign_target(
    target: EarlyConstAssignTarget,
) -> Result<ResolvedConstAssignTarget, ConstLowerError> {
    match target {
        EarlyConstAssignTarget::Local {
            span,
            name,
            local_id,
            path,
        } => {
            let local_id =
                local_id.ok_or_else(|| unresolved_error(span, "const assignment target"))?;
            Ok(ResolvedConstAssignTarget::local(
                span,
                name,
                local_id,
                path.into_iter()
                    .map(resolve_const_assign_path_elem)
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
    }
}

fn resolve_const_assign_path_elem(
    elem: EarlyConstAssignPathElem,
) -> Result<ResolvedConstAssignPathElem, ConstLowerError> {
    match elem {
        EarlyConstAssignPathElem::Field { span, name } => {
            Ok(ResolvedConstAssignPathElem::field(span, name))
        }
        EarlyConstAssignPathElem::Index { span, index } => Ok(ResolvedConstAssignPathElem::index(
            span,
            resolve_expr(index)?,
        )),
    }
}

fn resolve_const_switch(matched: EarlyConstMatch) -> Result<ResolvedConstMatch, ConstLowerError> {
    Ok(ResolvedConstMatch::new(
        matched.span,
        resolve_expr(matched.target)?,
        matched
            .arms
            .into_iter()
            .map(resolve_const_match_arm)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

fn resolve_const_match_arm(
    arm: EarlyConstMatchArm,
) -> Result<ResolvedConstMatchArm, ConstLowerError> {
    Ok(ResolvedConstMatchArm::new(
        arm.span,
        arm.patterns
            .into_iter()
            .map(resolve_const_pattern)
            .collect::<Result<Vec<_>, _>>()?,
        resolve_const_match_arm_body(arm.body)?,
    ))
}

fn resolve_const_pattern(
    pattern: EarlyConstPattern,
) -> Result<ResolvedConstPattern, ConstLowerError> {
    match pattern {
        EarlyConstPattern::Wildcard { span } => Ok(ResolvedConstPattern::wildcard(span)),
        EarlyConstPattern::Bind {
            name,
            local_id,
            span,
        } => Ok(ResolvedConstPattern::bind(
            name,
            local_id.ok_or_else(|| unresolved_error(span, "const match pattern local"))?,
            span,
        )),
        EarlyConstPattern::Pointer { pattern, span } => Ok(ResolvedConstPattern::pointer(
            resolve_const_pattern(*pattern)?,
            span,
        )),
        EarlyConstPattern::MutPointer { pattern, span } => Ok(ResolvedConstPattern::mut_pointer(
            resolve_const_pattern(*pattern)?,
            span,
        )),
        EarlyConstPattern::OptionalSome { pattern, span } => Ok(
            ResolvedConstPattern::optional_some(resolve_const_pattern(*pattern)?, span),
        ),
        EarlyConstPattern::OptionalNull { span } => Ok(ResolvedConstPattern::optional_null(span)),
        EarlyConstPattern::ErrorOk { pattern, span } => Ok(ResolvedConstPattern::error_ok(
            resolve_const_pattern(*pattern)?,
            span,
        )),
        EarlyConstPattern::ErrorErr { pattern, span } => Ok(ResolvedConstPattern::error_err(
            resolve_const_pattern(*pattern)?,
            span,
        )),
        EarlyConstPattern::Tuple { patterns, span } => Ok(ResolvedConstPattern::tuple(
            patterns
                .into_iter()
                .map(resolve_const_pattern)
                .collect::<Result<Vec<_>, _>>()?,
            span,
        )),
        EarlyConstPattern::EnumVariant {
            variant,
            fields,
            span,
        } => Ok(ResolvedConstPattern::enum_variant(
            resolve_expr(variant)?,
            match fields {
                ConstEnumPatternFields::Tuple(fields) => ConstEnumPatternFields::Tuple(
                    fields
                        .into_iter()
                        .map(resolve_const_pattern)
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                ConstEnumPatternFields::Named { fields, rest } => ConstEnumPatternFields::Named {
                    fields: fields
                        .into_iter()
                        .map(|field| {
                            Ok(ConstNamedPatternField {
                                name: field.name,
                                pattern: resolve_const_pattern(field.pattern)?,
                                span: field.span,
                            })
                        })
                        .collect::<Result<Vec<_>, ConstLowerError>>()?,
                    rest,
                },
            },
            span,
        )),
        EarlyConstPattern::Struct {
            def_id,
            fields,
            rest,
            span,
        } => Ok(ResolvedConstPattern::struct_pattern(
            def_id,
            fields
                .into_iter()
                .map(|field| {
                    Ok(ConstNamedPatternField {
                        name: field.name,
                        pattern: resolve_const_pattern(field.pattern)?,
                        span: field.span,
                    })
                })
                .collect::<Result<Vec<_>, ConstLowerError>>()?,
            rest,
            span,
        )),
        EarlyConstPattern::Expr(expr) => resolve_expr(expr).map(ResolvedConstPattern::expr),
        EarlyConstPattern::Range {
            start,
            end,
            inclusive,
            span,
        } => Ok(ResolvedConstPattern::range(
            resolve_expr(start)?,
            resolve_expr(end)?,
            inclusive,
            span,
        )),
    }
}

fn resolve_const_match_arm_body(
    body: EarlyConstMatchArmBody,
) -> Result<ResolvedConstMatchArmBody, ConstLowerError> {
    match body {
        EarlyConstMatchArmBody::Expr(expr) => {
            resolve_expr(expr).map(ResolvedConstMatchArmBody::expr)
        }
        EarlyConstMatchArmBody::Stmt(stmt) => {
            resolve_const_stmt(*stmt).map(ResolvedConstMatchArmBody::stmt)
        }
        EarlyConstMatchArmBody::Block(block) => {
            resolve_const_block(block).map(ResolvedConstMatchArmBody::block)
        }
    }
}

fn resolve_const_array_elements(
    elems: EarlyConstArrayElements,
) -> Result<ResolvedConstArrayElements, ConstLowerError> {
    match elems {
        EarlyConstArrayElements::List(elems) => elems
            .into_iter()
            .map(resolve_expr)
            .collect::<Result<Vec<_>, _>>()
            .map(ResolvedConstArrayElements::list),
        EarlyConstArrayElements::Repeat { value, count } => Ok(ResolvedConstArrayElements::repeat(
            resolve_expr(*value)?,
            resolve_expr(*count)?,
        )),
    }
}

fn resolve_const_range(range: EarlyConstRange) -> Result<ResolvedConstRange, ConstLowerError> {
    Ok(ResolvedConstRange::new(
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

fn resolve_const_slice_range(
    range: EarlyConstSliceRange,
) -> Result<ResolvedConstSliceRange, ConstLowerError> {
    Ok(ResolvedConstSliceRange::new(
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

fn resolve_const_field_init(
    field: EarlyConstFieldInit,
) -> Result<ResolvedConstFieldInit, ConstLowerError> {
    Ok(ResolvedConstFieldInit::new(
        field.span,
        field.name,
        resolve_expr(field.value)?,
    ))
}

/// Resolves a type argument whose type identity is mandatory at this boundary.
pub fn resolve_type_arg(
    type_arg: EarlyConstTypeArg,
) -> Result<ResolvedConstTypeArg, ConstLowerError> {
    Ok(ResolvedConstTypeArg::new(
        type_arg.span,
        type_arg.ty_span,
        type_arg
            .ty
            .ok_or_else(|| unresolved_error(type_arg.ty_span, "const type argument"))?,
    ))
}

fn resolve_optional_explicit_type(
    type_arg: Option<EarlyConstTypeArg>,
    what: &str,
) -> Result<Option<nia_ids::InternedTyId>, ConstLowerError> {
    type_arg
        .map(|type_arg| {
            type_arg
                .ty
                .ok_or_else(|| unresolved_error(type_arg.ty_span, what))
        })
        .transpose()
}

pub(crate) fn unresolved_error(span: Span, what: &str) -> ConstLowerError {
    ConstLowerError {
        span,
        message: format!("failed to resolve {what}"),
    }
}
