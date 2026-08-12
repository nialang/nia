// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

/// Rebuilds an aggregate from the leaf value back toward the assigned local.
/// Reading and writing are deliberately separate: compound assignment reads
/// the old leaf first, then this routine reconstructs every projection in the
/// original order without mutating an intermediate value in place.
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
) -> Result<ConstValue, ConstError> {
    for elem in path {
        value = match elem {
            EarlyConstAssignPathElem::Field { span, name } => match value {
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
            },
            EarlyConstAssignPathElem::Index {
                span: elem_span,
                index,
            } => match value {
                ConstValue::Array(values) => {
                    let index = eval_assign_path_index(*elem_span, index, env)?;
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
    Ok(value)
}

pub(super) fn eval_assign_target_value(
    span: Span,
    target: &EarlyConstAssignTarget,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    let value = eval_assign_target_root_value(span, target, env)?;
    match target {
        EarlyConstAssignTarget::Local { path, .. } => {
            eval_assign_path_value(span, value, path, env)
        }
    }
}

pub(super) fn eval_resolved_assign_target_value(
    span: Span,
    target: &ResolvedConstAssignTarget,
    env: &mut impl ResolvedConstEnv,
) -> Result<ConstValue, ConstError> {
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
) -> Result<ConstValue, ConstError> {
    for elem in path {
        value = match elem.kind() {
            ResolvedConstAssignPathElemKind::Field { span, name } => match value {
                ConstValue::Struct(fields) => {
                    fields.get(name).cloned().ok_or_else(|| ConstError {
                        span: *span,
                        message: format!(
                            "unknown const assignment field `{}`",
                            env.symbol_name(*name)
                        ),
                    })?
                }
                ConstValue::Union(value) => value.read(*name).map_err(|message| ConstError {
                    span: *span,
                    message: format!("{message} `{}`", env.symbol_name(*name)),
                })?,
                _ => {
                    return Err(ConstError {
                        span: *span,
                        message: "const field assignment requires a struct value".to_string(),
                    });
                }
            },
            ResolvedConstAssignPathElemKind::Index {
                span: elem_span,
                index,
            } => match value {
                ConstValue::Array(values) => {
                    let index = eval_resolved_assign_path_index(*elem_span, index, env)?;
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
    Ok(value)
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
