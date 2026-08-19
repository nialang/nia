// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

pub(super) fn eval_assign_expr_flow(
    span: Span,
    assign: &EarlyConstAssign,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let rhs = eval_value_or_return_flow!(&assign.rhs, env);
    let value = if matches!(assign.op, ConstAssignOp::Assign) {
        assign_target_writeback_value(span, &assign.lhs, rhs, env)?
    } else {
        let (lhs, path) = eval_assign_target_value(span, &assign.lhs, env)?;
        let value = eval_compound_assignment_value(span, lhs, assign.op, rhs)?;
        assign_target_writeback_value_with_path(span, &assign.lhs, &path, value, env)?
    };
    env.assign_local(span, &assign.lhs, value)?;
    Ok(ConstEvalFlow::Void)
}

pub(super) fn eval_resolved_assign_expr_flow(
    span: Span,
    assign: &ResolvedConstAssign,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstEvalFlow, ConstError> {
    let rhs = eval_resolved_value_or_return_flow!(assign.rhs(), env);
    let value = if matches!(assign.op(), ConstAssignOp::Assign) {
        resolved_assign_target_writeback_value(span, assign.lhs(), rhs, env)?
    } else {
        let (lhs, path) = eval_resolved_assign_target_value(span, assign.lhs(), env)?;
        let value = eval_compound_assignment_value(span, lhs, assign.op(), rhs)?;
        resolved_assign_target_writeback_value_with_path(span, assign.lhs(), &path, value, env)?
    };
    env.assign_resolved_local(span, assign.lhs(), value)?;
    Ok(ConstEvalFlow::Void)
}

fn eval_compound_assignment_value(
    span: Span,
    lhs: ConstValue,
    op: ConstAssignOp,
    rhs: ConstValue,
) -> Result<ConstValue, ConstError> {
    let op = assign_op_binary(op).ok_or_else(|| ConstError {
        span,
        message: "unsupported const assignment operator".to_string(),
    })?;
    eval_numeric_binary_value(lhs, op, rhs).map_err(|message| ConstError { span, message })
}

fn assign_op_binary(op: ConstAssignOp) -> Option<ConstBinaryOp> {
    Some(match op {
        ConstAssignOp::Assign => return None,
        ConstAssignOp::Add => ConstBinaryOp::Add,
        ConstAssignOp::Sub => ConstBinaryOp::Sub,
        ConstAssignOp::Shl => ConstBinaryOp::Shl,
        ConstAssignOp::Shr => ConstBinaryOp::Shr,
        ConstAssignOp::Mul => ConstBinaryOp::Mul,
        ConstAssignOp::Div => ConstBinaryOp::Div,
        ConstAssignOp::Rem => ConstBinaryOp::Rem,
        ConstAssignOp::BitAnd => ConstBinaryOp::BitAnd,
        ConstAssignOp::BitXor => ConstBinaryOp::BitXor,
        ConstAssignOp::BitOr => ConstBinaryOp::BitOr,
    })
}

#[derive(Clone, Copy)]
enum EvaluatedAssignPathElem {
    Field { span: Span, name: SymbolId },
    Index { span: Span, index: usize },
}

// Plain assignment reaches writeback without reading the target, so its path
// expressions are evaluated here. Compound assignment instead freezes every
// dynamic index into `EvaluatedAssignPathElem` while reading the old leaf and
// uses the `_with_path` variants below. Re-evaluating those expressions during
// writeback would duplicate their const-visible side effects.
pub(super) fn assign_target_writeback_value(
    span: Span,
    target: &EarlyConstAssignTarget,
    value: ConstValue,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    match target {
        EarlyConstAssignTarget::Local { path, .. } => {
            if path.is_empty() {
                return Ok(value);
            }
            let root = eval_assign_target_root_value(span, target, env)?;
            write_assign_path_value(span, root, path, value, env)
        }
    }
}

fn assign_target_writeback_value_with_path(
    span: Span,
    target: &EarlyConstAssignTarget,
    path: &[EvaluatedAssignPathElem],
    value: ConstValue,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    if path.is_empty() {
        return Ok(value);
    }
    let root = eval_assign_target_root_value(span, target, env)?;
    write_evaluated_assign_path_value(span, root, path, value, env)
}

pub(super) fn resolved_assign_target_writeback_value(
    span: Span,
    target: &ResolvedConstAssignTarget,
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    match target.kind() {
        ResolvedConstAssignTargetKind::Local { path, .. } => {
            if path.is_empty() {
                return Ok(value);
            }
            let root = eval_resolved_assign_target_root_value(target, env)?;
            write_resolved_assign_path_value(span, root, path, value, env)
        }
    }
}

fn resolved_assign_target_writeback_value_with_path(
    span: Span,
    target: &ResolvedConstAssignTarget,
    path: &[EvaluatedAssignPathElem],
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    if path.is_empty() {
        return Ok(value);
    }
    let root = eval_resolved_assign_target_root_value(target, env)?;
    write_resolved_evaluated_assign_path_value(span, root, path, value, env)
}

/// Rebuilds and writes a resolved projected place from leaf to local root.
///
/// Aggregate values are immutable snapshots in the evaluator, so updating a
/// nested field or index reconstructs every parent and performs one final
/// writeback through [`ResolvedConstEnv::assign_resolved_place_local`].
pub fn write_resolved_const_place(
    span: Span,
    place: &ResolvedConstPlace,
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<(), ConstError> {
    let root = env.resolve_resolved_name(span, ConstNameResolution::Local(place.local_id))?;
    let value = write_resolved_const_place_path(span, root, &place.path, value, env)?;
    env.assign_resolved_place_local(span, place.local_id, value)
}

fn eval_assign_target_root_value(
    span: Span,
    target: &EarlyConstAssignTarget,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    match target {
        EarlyConstAssignTarget::Local {
            span: target_span,
            name,
            local_id,
            ..
        } => {
            let Some(local_id) = local_id else {
                return Err(ConstError {
                    span,
                    message: format!(
                        "failed to resolve const assignment target `{}`",
                        env.symbol_name(*name)
                    ),
                });
            };
            env.resolve_name(
                *target_span,
                &EarlyConstName::resolved(*name, ConstNameResolution::Local(*local_id)),
            )
        }
    }
}

fn eval_resolved_assign_target_root_value(
    target: &ResolvedConstAssignTarget,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    match target.kind() {
        ResolvedConstAssignTargetKind::Local { span, local_id, .. } => {
            env.resolve_resolved_name(*span, ConstNameResolution::Local(*local_id))
        }
    }
}

fn eval_assign_path_value(
    span: Span,
    mut value: ConstValue,
    path: &[EarlyConstAssignPathElem],
    env: &mut impl EarlyConstEnv,
) -> Result<(ConstValue, Vec<EvaluatedAssignPathElem>), ConstError> {
    let mut evaluated_path = Vec::with_capacity(path.len());
    for elem in path {
        value = match elem {
            EarlyConstAssignPathElem::Field { span, name } => {
                evaluated_path.push(EvaluatedAssignPathElem::Field {
                    span: *span,
                    name: *name,
                });
                match value {
                    ConstValue::Struct(fields) => {
                        fields.get(name).cloned().ok_or_else(|| ConstError {
                            span: *span,
                            message: format!(
                                "unknown const assignment field `{}`",
                                env.symbol_name(*name)
                            ),
                        })?
                    }
                    _ => {
                        return Err(ConstError {
                            span: *span,
                            message: "const field assignment requires a struct value".to_string(),
                        });
                    }
                }
            }
            EarlyConstAssignPathElem::Index {
                span: elem_span,
                index,
            } => match value {
                ConstValue::Array(values) => {
                    let index = eval_assign_path_index(*elem_span, index, env)?;
                    evaluated_path.push(EvaluatedAssignPathElem::Index {
                        span: *elem_span,
                        index,
                    });
                    values.get(index).cloned().ok_or_else(|| ConstError {
                        span,
                        message: format!("const array assignment index {index} is out of bounds"),
                    })?
                }
                _ => {
                    return Err(ConstError {
                        span: *elem_span,
                        message: "const index assignment requires an array value".to_string(),
                    });
                }
            },
        };
    }
    Ok((value, evaluated_path))
}

fn eval_assign_target_value(
    span: Span,
    target: &EarlyConstAssignTarget,
    env: &mut impl EarlyConstEnv,
) -> Result<(ConstValue, Vec<EvaluatedAssignPathElem>), ConstError> {
    let value = eval_assign_target_root_value(span, target, env)?;
    match target {
        EarlyConstAssignTarget::Local { path, .. } => {
            eval_assign_path_value(span, value, path, env)
        }
    }
}

fn eval_resolved_assign_target_value(
    span: Span,
    target: &ResolvedConstAssignTarget,
    env: &mut impl ResolvedConstEnv,
) -> Result<(ConstValue, Vec<EvaluatedAssignPathElem>), ConstError> {
    let value = eval_resolved_assign_target_root_value(target, env)?;
    match target.kind() {
        ResolvedConstAssignTargetKind::Local { path, .. } => {
            eval_resolved_assign_path_value(span, value, path, env)
        }
    }
}

fn eval_resolved_assign_path_value(
    span: Span,
    mut value: ConstValue,
    path: &[ResolvedConstAssignPathElem],
    env: &mut impl ResolvedConstEnv,
) -> Result<(ConstValue, Vec<EvaluatedAssignPathElem>), ConstError> {
    let mut evaluated_path = Vec::with_capacity(path.len());
    for elem in path {
        value = match elem.kind() {
            ResolvedConstAssignPathElemKind::Field { span, name } => {
                evaluated_path.push(EvaluatedAssignPathElem::Field {
                    span: *span,
                    name: *name,
                });
                match value {
                    ConstValue::Struct(fields) => {
                        fields.get(name).cloned().ok_or_else(|| ConstError {
                            span: *span,
                            message: format!(
                                "unknown const assignment field `{}`",
                                env.symbol_name(*name)
                            ),
                        })?
                    }
                    ConstValue::Union(value) => {
                        value.read(*name).map_err(|message| ConstError {
                            span: *span,
                            message: format!("{message} `{}`", env.symbol_name(*name)),
                        })?
                    }
                    _ => {
                        return Err(ConstError {
                            span: *span,
                            message: "const field assignment requires a struct value".to_string(),
                        });
                    }
                }
            }
            ResolvedConstAssignPathElemKind::Index {
                span: elem_span,
                index,
            } => match value {
                ConstValue::Array(values) => {
                    let index = eval_resolved_assign_path_index(*elem_span, index, env)?;
                    evaluated_path.push(EvaluatedAssignPathElem::Index {
                        span: *elem_span,
                        index,
                    });
                    values.get(index).cloned().ok_or_else(|| ConstError {
                        span,
                        message: format!("const array assignment index {index} is out of bounds"),
                    })?
                }
                _ => {
                    return Err(ConstError {
                        span: *elem_span,
                        message: "const index assignment requires an array value".to_string(),
                    });
                }
            },
        };
    }
    Ok((value, evaluated_path))
}

fn write_resolved_const_place_path(
    span: Span,
    root: ConstValue,
    path: &[ResolvedConstPlaceElem],
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head {
        ResolvedConstPlaceElem::Field(name) => match root {
            ConstValue::Struct(mut fields) => {
                let current = fields.remove(name).ok_or_else(|| ConstError {
                    span,
                    message: format!("unknown const writeback field `{}`", env.symbol_name(*name)),
                })?;
                let updated = write_resolved_const_place_path(span, current, tail, value, env)?;
                fields.insert(*name, updated);
                Ok(ConstValue::Struct(fields))
            }
            ConstValue::Union(mut union) => {
                let updated = if tail.is_empty() {
                    value
                } else {
                    let current = union.read(*name).map_err(|message| ConstError {
                        span,
                        message: format!("{message} `{}`", env.symbol_name(*name)),
                    })?;
                    write_resolved_const_place_path(span, current, tail, value, env)?
                };
                union.write(*name, updated).map_err(|message| ConstError {
                    span,
                    message: format!("{message} `{}`", env.symbol_name(*name)),
                })?;
                Ok(ConstValue::Union(union))
            }
            _ => Err(ConstError {
                span,
                message: "const field writeback requires an aggregate value".to_string(),
            }),
        },
        ResolvedConstPlaceElem::Index(index) => {
            let ConstValue::Array(mut values) = root else {
                return Err(ConstError {
                    span,
                    message: "const index writeback requires an array value".to_string(),
                });
            };
            if *index >= values.len() {
                return Err(ConstError {
                    span,
                    message: format!("const array index {index} is out of bounds"),
                });
            }
            let current = values.remove(*index);
            let updated = write_resolved_const_place_path(span, current, tail, value, env)?;
            values.insert(*index, updated);
            Ok(ConstValue::Array(values))
        }
    }
}

fn write_assign_path_value(
    span: Span,
    root: ConstValue,
    path: &[EarlyConstAssignPathElem],
    value: ConstValue,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head {
        EarlyConstAssignPathElem::Field {
            span: field_span,
            name,
        } => {
            let ConstValue::Struct(mut fields) = root else {
                return Err(ConstError {
                    span: *field_span,
                    message: "const field assignment requires a struct value".to_string(),
                });
            };
            let current = fields.remove(name).ok_or_else(|| ConstError {
                span: *field_span,
                message: format!(
                    "unknown const assignment field `{}`",
                    env.symbol_name(*name)
                ),
            })?;
            let updated = write_assign_path_value(span, current, tail, value, env)?;
            fields.insert(*name, updated);
            Ok(ConstValue::Struct(fields))
        }
        EarlyConstAssignPathElem::Index {
            span: index_span,
            index,
        } => {
            let ConstValue::Array(mut values) = root else {
                return Err(ConstError {
                    span: *index_span,
                    message: "const index assignment requires an array value".to_string(),
                });
            };
            let index = eval_assign_path_index(*index_span, index, env)?;
            if index >= values.len() {
                return Err(ConstError {
                    span,
                    message: format!("const array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(index);
            let updated = write_assign_path_value(span, current, tail, value, env)?;
            values.insert(index, updated);
            Ok(ConstValue::Array(values))
        }
    }
}

fn write_evaluated_assign_path_value(
    span: Span,
    root: ConstValue,
    path: &[EvaluatedAssignPathElem],
    value: ConstValue,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head {
        EvaluatedAssignPathElem::Field {
            span: field_span,
            name,
        } => {
            let ConstValue::Struct(mut fields) = root else {
                return Err(ConstError {
                    span: *field_span,
                    message: "const field assignment requires a struct value".to_string(),
                });
            };
            let current = fields.remove(name).ok_or_else(|| ConstError {
                span: *field_span,
                message: format!(
                    "unknown const assignment field `{}`",
                    env.symbol_name(*name)
                ),
            })?;
            let updated = write_evaluated_assign_path_value(span, current, tail, value, env)?;
            fields.insert(*name, updated);
            Ok(ConstValue::Struct(fields))
        }
        EvaluatedAssignPathElem::Index {
            span: index_span,
            index,
        } => {
            let ConstValue::Array(mut values) = root else {
                return Err(ConstError {
                    span: *index_span,
                    message: "const index assignment requires an array value".to_string(),
                });
            };
            if *index >= values.len() {
                return Err(ConstError {
                    span,
                    message: format!("const array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(*index);
            let updated = write_evaluated_assign_path_value(span, current, tail, value, env)?;
            values.insert(*index, updated);
            Ok(ConstValue::Array(values))
        }
    }
}

fn write_resolved_assign_path_value(
    span: Span,
    root: ConstValue,
    path: &[ResolvedConstAssignPathElem],
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head.kind() {
        ResolvedConstAssignPathElemKind::Field {
            span: field_span,
            name,
        } => match root {
            ConstValue::Struct(mut fields) => {
                let current = fields.remove(name).ok_or_else(|| ConstError {
                    span: *field_span,
                    message: format!(
                        "unknown const assignment field `{}`",
                        env.symbol_name(*name)
                    ),
                })?;
                let updated = write_resolved_assign_path_value(span, current, tail, value, env)?;
                fields.insert(*name, updated);
                Ok(ConstValue::Struct(fields))
            }
            ConstValue::Union(mut union) => {
                let updated = if tail.is_empty() {
                    value
                } else {
                    let current = union.read(*name).map_err(|message| ConstError {
                        span: *field_span,
                        message: format!("{message} `{}`", env.symbol_name(*name)),
                    })?;
                    write_resolved_assign_path_value(span, current, tail, value, env)?
                };
                union.write(*name, updated).map_err(|message| ConstError {
                    span: *field_span,
                    message: format!("{message} `{}`", env.symbol_name(*name)),
                })?;
                Ok(ConstValue::Union(union))
            }
            _ => Err(ConstError {
                span: *field_span,
                message: "const field assignment requires a struct value".to_string(),
            }),
        },
        ResolvedConstAssignPathElemKind::Index {
            span: index_span,
            index,
        } => {
            let ConstValue::Array(mut values) = root else {
                return Err(ConstError {
                    span: *index_span,
                    message: "const index assignment requires an array value".to_string(),
                });
            };
            let index = eval_resolved_assign_path_index(*index_span, index, env)?;
            if index >= values.len() {
                return Err(ConstError {
                    span,
                    message: format!("const array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(index);
            let updated = write_resolved_assign_path_value(span, current, tail, value, env)?;
            values.insert(index, updated);
            Ok(ConstValue::Array(values))
        }
    }
}

fn write_resolved_evaluated_assign_path_value(
    span: Span,
    root: ConstValue,
    path: &[EvaluatedAssignPathElem],
    value: ConstValue,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
    let Some((head, tail)) = path.split_first() else {
        return Ok(value);
    };
    match head {
        EvaluatedAssignPathElem::Field {
            span: field_span,
            name,
        } => match root {
            ConstValue::Struct(mut fields) => {
                let current = fields.remove(name).ok_or_else(|| ConstError {
                    span: *field_span,
                    message: format!(
                        "unknown const assignment field `{}`",
                        env.symbol_name(*name)
                    ),
                })?;
                let updated =
                    write_resolved_evaluated_assign_path_value(span, current, tail, value, env)?;
                fields.insert(*name, updated);
                Ok(ConstValue::Struct(fields))
            }
            ConstValue::Union(mut union) => {
                let updated = if tail.is_empty() {
                    value
                } else {
                    let current = union.read(*name).map_err(|message| ConstError {
                        span: *field_span,
                        message: format!("{message} `{}`", env.symbol_name(*name)),
                    })?;
                    write_resolved_evaluated_assign_path_value(span, current, tail, value, env)?
                };
                union.write(*name, updated).map_err(|message| ConstError {
                    span: *field_span,
                    message: format!("{message} `{}`", env.symbol_name(*name)),
                })?;
                Ok(ConstValue::Union(union))
            }
            _ => Err(ConstError {
                span: *field_span,
                message: "const field assignment requires a struct value".to_string(),
            }),
        },
        EvaluatedAssignPathElem::Index {
            span: index_span,
            index,
        } => {
            let ConstValue::Array(mut values) = root else {
                return Err(ConstError {
                    span: *index_span,
                    message: "const index assignment requires an array value".to_string(),
                });
            };
            if *index >= values.len() {
                return Err(ConstError {
                    span,
                    message: format!("const array assignment index {index} is out of bounds"),
                });
            }
            let current = values.remove(*index);
            let updated =
                write_resolved_evaluated_assign_path_value(span, current, tail, value, env)?;
            values.insert(*index, updated);
            Ok(ConstValue::Array(values))
        }
    }
}

fn eval_assign_path_index(
    span: Span,
    index: &EarlyConstExpr,
    env: &mut impl EarlyConstEnv,
) -> Result<usize, ConstError> {
    let index_span = index.span;
    let value = match super::eval_const_expr_flow(index, env)? {
        ConstEvalFlow::Value(ConstValue::Int(value)) => value,
        ConstEvalFlow::Value(_) => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index must be an integer".to_string(),
            });
        }
        ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index cannot contain control flow".to_string(),
            });
        }
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index requires a value".to_string(),
            });
        }
    };
    let index = super::int_to_array_len(span, value)?;
    usize::try_from(index).map_err(|_| ConstError {
        span,
        message: "const array assignment index is too large".to_string(),
    })
}

pub(super) fn eval_resolved_assign_path_index(
    span: Span,
    index: &ResolvedConstExpr,
    env: &mut impl ResolvedConstEnv,
) -> Result<usize, ConstError> {
    let index_span = index.span();
    let value = match super::eval_resolved_const_expr_flow(index, env)? {
        ConstEvalFlow::Value(ConstValue::Int(value)) => value,
        ConstEvalFlow::Value(_) => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index must be an integer".to_string(),
            });
        }
        ConstEvalFlow::Return(_)
        | ConstEvalFlow::Propagate(_)
        | ConstEvalFlow::Break
        | ConstEvalFlow::Continue => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index cannot contain control flow".to_string(),
            });
        }
        ConstEvalFlow::Void => {
            return Err(ConstError {
                span: index_span,
                message: "const array assignment index requires a value".to_string(),
            });
        }
    };
    let index = super::int_to_array_len(span, value)?;
    usize::try_from(index).map_err(|_| ConstError {
        span,
        message: "const array assignment index is too large".to_string(),
    })
}
