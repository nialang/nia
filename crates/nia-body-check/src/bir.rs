// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{
    ArrayElements, AssignOp, BindingStmt, Block, Expr, ExprKind, IndexArg, SliceRange, Stmt,
    StmtKind, SwitchArmBody, SwitchPattern, UnaryOp,
};
use nia_body_ir::{
    AtomicOrder, AtomicRmwOp, BuiltinConst, BuiltinOperator, BuiltinPlaceMethod, MemoryIntrinsicOp,
    PlaceBase, PlaceElem, TypedArrayElements, TypedAtomic, TypedBinding, TypedBody, TypedCallee,
    TypedExpr, TypedExprKind, TypedFieldInit, TypedForBinding, TypedForIn, TypedLocal,
    TypedLocalKind, TypedLoop, TypedMemoryIntrinsic, TypedMemoryIntrinsicSource, TypedPlace,
    TypedRange, TypedSliceRange, TypedStmt, TypedStmtKind, TypedSwitch, TypedSwitchArm,
    TypedSwitchArmBody, TypedSwitchPattern, TypedWhile,
};
use nia_ids::{BuiltinReceiverKind, BuiltinTraitMethod, TraitId};
use nia_local_resolve::{LocalKind, LocalUse};
use nia_sema_ir::{
    BracketSuffixResolution, BuiltinOperatorOp, BuiltinValue, ComptimeIfSelection, ResolvedCall,
};
use nia_span::Span;
use nia_trait_solve::TraitResolution;
use nia_ty::{BuiltinTrait, TyKind};
use nia_value_resolve::ValueNameResolution;

use crate::literals::{
    decode_byte_string_literal, decode_c_string_literal, decode_char_literal,
    decode_string_literal, numeric_literal_body,
};

mod asm;

impl<'a> BodyChecker<'a> {
    pub(crate) fn lower_body(&mut self, block: &Block) -> TypedBody {
        self.lower_body_with_expected_tail(block, None)
    }

    fn lower_body_with_expected_tail(
        &mut self,
        block: &Block,
        expected_tail: Option<nia_ids::InternedTyId>,
    ) -> TypedBody {
        let stmts = block
            .stmts
            .iter()
            .filter(|stmt| {
                !matches!(&stmt.kind, StmtKind::Using(_))
                    && !matches!(&stmt.kind, StmtKind::Binding(binding) if binding.is_comptime)
            })
            .map(|stmt| self.lower_stmt(stmt))
            .collect();
        let tail = block
            .tail
            .as_ref()
            .map(|tail| Box::new(self.lower_expr_with_ty(tail, expected_tail)));
        let ty = tail
            .as_ref()
            .map(|tail| tail.ty)
            .or_else(|| self.block_terminating_never_ty(block))
            .unwrap_or_else(|| self.void());
        TypedBody {
            span: block.span,
            locals: self.lower_locals(block.span),
            stmts,
            tail,
            ty,
        }
    }

    fn block_terminating_never_ty(&self, block: &Block) -> Option<nia_ids::InternedTyId> {
        let stmt = block.stmts.last()?;
        match &stmt.kind {
            StmtKind::Return(_) | StmtKind::Break | StmtKind::Continue => Some(self.never()),
            StmtKind::Expr(expr) if self.expr_ty(expr).is_some_and(|ty| self.is_never(ty)) => {
                Some(self.never())
            }
            _ => None,
        }
    }

    fn lower_locals(&self, body_span: Span) -> Vec<TypedLocal> {
        // Runtime body IR intentionally excludes comptime bindings: their
        // values are available to later phases through evaluated constants, not
        // through storage-bearing locals.
        self.locals
            .locals
            .iter()
            .filter(|(id, local)| {
                self.current_param_locals.contains(id)
                    || (body_span.start <= local.span.start && local.span.end <= body_span.end)
            })
            .filter_map(|(id, local)| {
                let kind = match local.kind {
                    LocalKind::Param => TypedLocalKind::Param,
                    LocalKind::Binding => TypedLocalKind::Binding,
                    LocalKind::ConstBinding => TypedLocalKind::ConstBinding,
                    LocalKind::ComptimeBinding => return None,
                };
                Some(TypedLocal {
                    id,
                    name: local.name.clone(),
                    kind,
                    ty: self
                        .local_types
                        .get(&id)
                        .copied()
                        .unwrap_or_else(|| self.error()),
                    span: local.span,
                })
            })
            .collect()
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> TypedStmt {
        let kind = match &stmt.kind {
            StmtKind::Using(_) => TypedStmtKind::Expr(TypedExpr {
                span: stmt.span,
                ty: self.error(),
                kind: TypedExprKind::Error,
            }),
            StmtKind::Binding(binding) => self
                .lower_binding_stmt(stmt, binding)
                .map(TypedStmtKind::Binding)
                .unwrap_or_else(|| TypedStmtKind::Expr(self.error_expr(stmt.span))),
            StmtKind::Expr(expr) => TypedStmtKind::Expr(self.lower_expr(expr)),
            StmtKind::Return(value) => TypedStmtKind::Return(
                value
                    .as_ref()
                    .map(|value| self.lower_expr_with_ty(value, Some(self.current_return))),
            ),
            StmtKind::Break => TypedStmtKind::Break,
            StmtKind::Continue => TypedStmtKind::Continue,
            StmtKind::Defer(expr) => TypedStmtKind::Defer(self.lower_expr(expr)),
            StmtKind::ForIn(for_stmt) => {
                let local_id = self.local_def(&for_stmt.pattern.node_key);
                let binding = for_stmt.pattern.name().and_then(|name| {
                    Some(TypedForBinding {
                        local_id: local_id?,
                        name: name.to_string(),
                    })
                });
                TypedStmtKind::ForIn(Box::new(TypedForIn {
                    binding,
                    pattern_kind: for_stmt.pattern.kind,
                    item_ty: self
                        .expr_ty(&for_stmt.iter)
                        .map(|iter_ty| self.lower_for_iterator_item_type(iter_ty))
                        .unwrap_or_else(|| self.error()),
                    binding_ty: local_id
                        .and_then(|local_id| self.local_types.get(&local_id).copied())
                        .unwrap_or_else(|| self.error()),
                    iter: self.lower_expr(&for_stmt.iter),
                    body: self.lower_body(&for_stmt.body),
                }))
            }
            StmtKind::While(while_stmt) => TypedStmtKind::While(Box::new(TypedWhile {
                cond: self.lower_expr(&while_stmt.cond),
                body: self.lower_body(&while_stmt.body),
            })),
            StmtKind::Loop(loop_stmt) => TypedStmtKind::Loop(Box::new(TypedLoop {
                body: self.lower_body(&loop_stmt.body),
            })),
        };
        TypedStmt {
            span: stmt.span,
            kind,
        }
    }

    fn lower_binding_stmt(&mut self, stmt: &Stmt, binding: &BindingStmt) -> Option<TypedBinding> {
        let local_id = self.local_def(self.binding_pattern_node_key(stmt, binding))?;
        let ty = self.local_types.get(&local_id).copied().unwrap_or_else(|| {
            binding.ty.as_ref().map_or_else(
                || {
                    binding
                        .value
                        .as_ref()
                        .and_then(|value| self.expr_ty(value))
                        .unwrap_or_else(|| self.error())
                },
                |ty| self.ty_for_type(ty),
            )
        });
        Some(TypedBinding {
            local_id,
            name: binding.name.clone(),
            pattern_kind: binding.pattern_kind,
            ty,
            value: binding.value.as_ref().map(|value| {
                if matches!(
                    &value.kind,
                    ExprKind::Block(block) if block.stmts.is_empty() && block.tail.is_none()
                ) {
                    self.lower_expr_with_ty(value, Some(ty))
                } else {
                    self.lower_expr(value)
                }
            }),
            is_let: binding.is_let,
        })
    }

    fn binding_pattern_node_key<'b>(
        &self,
        stmt: &'b Stmt,
        binding: &'b BindingStmt,
    ) -> &'b nia_node_id::NodeKey {
        if matches!(binding.pattern_kind, nia_ast::ForPatternKind::Value) {
            &stmt.node_key
        } else {
            &binding.pattern_node_key
        }
    }

    fn error_expr(&mut self, span: Span) -> TypedExpr {
        TypedExpr {
            span,
            ty: self.error(),
            kind: TypedExprKind::Error,
        }
    }

    fn lower_switch(&mut self, switch: &nia_ast::SwitchStmt) -> TypedSwitch {
        let target = self.lower_expr(&switch.target);
        let target_ty = target.ty;
        TypedSwitch {
            target,
            bool_ty: self.bool(),
            arms: switch
                .arms
                .iter()
                .map(|arm| TypedSwitchArm {
                    patterns: arm
                        .patterns
                        .iter()
                        .map(|pattern| match pattern {
                            SwitchPattern::Default => TypedSwitchPattern::Default,
                            SwitchPattern::OptionalSome {
                                name,
                                span,
                                node_key,
                            } => {
                                let local_id = self
                                    .local_def(node_key)
                                    .unwrap_or(nia_ids::LocalId(u32::MAX));
                                TypedSwitchPattern::OptionalSome {
                                    local_id,
                                    name: name.clone(),
                                    ty: self
                                        .local_types
                                        .get(&local_id)
                                        .copied()
                                        .unwrap_or_else(|| self.error()),
                                    span: *span,
                                }
                            }
                            SwitchPattern::OptionalNull { span } => {
                                TypedSwitchPattern::OptionalNull { span: *span }
                            }
                            SwitchPattern::ErrorOk {
                                name,
                                span,
                                node_key,
                            } => {
                                let local_id = self
                                    .local_def(node_key)
                                    .unwrap_or(nia_ids::LocalId(u32::MAX));
                                TypedSwitchPattern::ErrorOk {
                                    local_id,
                                    name: name.clone(),
                                    ty: self
                                        .local_types
                                        .get(&local_id)
                                        .copied()
                                        .unwrap_or_else(|| self.error()),
                                    span: *span,
                                }
                            }
                            SwitchPattern::ErrorErr {
                                name,
                                span,
                                node_key,
                            } => {
                                let local_id = self
                                    .local_def(node_key)
                                    .unwrap_or(nia_ids::LocalId(u32::MAX));
                                TypedSwitchPattern::ErrorErr {
                                    local_id,
                                    name: name.clone(),
                                    ty: self
                                        .local_types
                                        .get(&local_id)
                                        .copied()
                                        .unwrap_or_else(|| self.error()),
                                    span: *span,
                                }
                            }
                            SwitchPattern::Expr(expr) => {
                                self.lower_switch_expr_pattern(expr, target_ty)
                            }
                            SwitchPattern::Range {
                                start,
                                end,
                                inclusive,
                                span,
                            } => self.lower_switch_range_pattern(
                                start, end, *inclusive, *span, target_ty,
                            ),
                        })
                        .collect(),
                    body: match &arm.body {
                        SwitchArmBody::Expr(expr) => {
                            TypedSwitchArmBody::Expr(self.lower_expr(expr))
                        }
                        SwitchArmBody::Stmt(stmt) => {
                            TypedSwitchArmBody::Stmt(Box::new(self.lower_stmt(stmt)))
                        }
                        SwitchArmBody::Block(block) => {
                            TypedSwitchArmBody::Block(Box::new(self.lower_body(block)))
                        }
                    },
                    span: arm.span,
                })
                .collect(),
        }
    }

    fn lower_switch_expr_pattern(
        &mut self,
        expr: &Expr,
        target_ty: nia_ids::InternedTyId,
    ) -> TypedSwitchPattern {
        if self.is_integer(target_ty) || self.is_bool(target_ty) {
            if let Some(value) = self.node_switch_pattern_values.get(&expr.node_key).copied() {
                return TypedSwitchPattern::CheckedInt {
                    value,
                    ty: target_ty,
                    span: expr.span,
                };
            }
            self.diagnostics
                .push(nia_diagnostic::Diagnostic::user_error_at(
                    "E0301",
                    expr.span,
                    "missing checked switch pattern value during body IR lowering",
                ));
        }
        TypedSwitchPattern::Expr(self.lower_expr(expr))
    }

    fn lower_switch_range_pattern(
        &mut self,
        start: &Expr,
        end: &Expr,
        inclusive: bool,
        span: Span,
        target_ty: nia_ids::InternedTyId,
    ) -> TypedSwitchPattern {
        if self.is_integer(target_ty) {
            let start_value = self
                .node_switch_pattern_values
                .get(&start.node_key)
                .copied();
            let end_value = self.node_switch_pattern_values.get(&end.node_key).copied();
            if let (Some(start), Some(end)) = (start_value, end_value) {
                return TypedSwitchPattern::CheckedIntRange {
                    start,
                    end,
                    inclusive,
                    ty: target_ty,
                    span,
                };
            }
            self.diagnostics
                .push(nia_diagnostic::Diagnostic::user_error_at(
                    "E0301",
                    span,
                    "missing checked switch range pattern values during body IR lowering",
                ));
        }
        TypedSwitchPattern::Range {
            start: Box::new(self.lower_expr(start)),
            end: Box::new(self.lower_expr(end)),
            inclusive,
            span,
        }
    }

    pub(crate) fn lower_expr(&mut self, expr: &Expr) -> TypedExpr {
        self.lower_expr_with_ty(expr, None)
    }

    fn lower_expr_with_ty(
        &mut self,
        expr: &Expr,
        forced_ty: Option<nia_ids::InternedTyId>,
    ) -> TypedExpr {
        if forced_ty.is_none()
            && let Some(coercion) = self
                .node_c_string_pointer_coercions
                .get(&expr.node_key)
                .copied()
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.pointer_ty,
                kind: TypedExprKind::CStringPointer {
                    array: Box::new(self.lower_expr_with_ty(expr, Some(coercion.array_ty))),
                    is_readonly: coercion.is_readonly,
                },
            };
        }
        if let Some(upcast) = forced_ty
            .is_none()
            .then(|| self.node_trait_object_upcasts.get(&expr.node_key).copied())
            .flatten()
        {
            return TypedExpr {
                span: expr.span,
                ty: upcast.target_ty,
                kind: TypedExprKind::TraitObjectUpcast {
                    expr: Box::new(self.lower_expr_with_ty(expr, Some(upcast.source_ty))),
                    source_ty: upcast.source_ty,
                    target_ty: upcast.target_ty,
                },
            };
        }
        if let Some(coercion) = forced_ty
            .is_none()
            .then(|| {
                self.node_trait_object_coercions
                    .get(&expr.node_key)
                    .copied()
            })
            .flatten()
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.target_ty,
                kind: TypedExprKind::TraitObjectCoercion {
                    expr: Box::new(self.lower_expr_with_ty(expr, Some(coercion.source_ty))),
                    target_ty: coercion.target_ty,
                    self_ty: self.trait_object_coercion_self_ty(coercion.source_ty),
                },
            };
        }
        if let Some(coercion) = self
            .node_array_to_slice_coercions
            .get(&expr.node_key)
            .copied()
            && forced_ty.is_none_or(|forced_ty| forced_ty == coercion.slice_ty)
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.slice_ty,
                kind: TypedExprKind::Slice {
                    lhs: Box::new(self.lower_expr_with_ty(expr, Some(coercion.array_ty))),
                    range: TypedSliceRange {
                        start: None,
                        end: None,
                        inclusive: false,
                    },
                    is_readonly: coercion.is_readonly,
                },
            };
        }
        if let Some(coercion) = self
            .node_pointer_array_to_slice_coercions
            .get(&expr.node_key)
            .copied()
            && forced_ty.is_none_or(|forced_ty| forced_ty == coercion.slice_ty)
        {
            return TypedExpr {
                span: expr.span,
                ty: coercion.slice_ty,
                kind: TypedExprKind::Slice {
                    lhs: Box::new(self.lower_expr_with_ty(expr, Some(coercion.pointer_ty))),
                    range: TypedSliceRange {
                        start: None,
                        end: None,
                        inclusive: false,
                    },
                    is_readonly: coercion.is_readonly,
                },
            };
        }
        let ty = forced_ty
            .or_else(|| self.expr_ty(expr))
            .unwrap_or_else(|| self.error());
        if let Some(def_id) = self.global_comptime_use(expr) {
            return self.lower_comptime_value_expr(
                expr.span,
                ty,
                self.global_comptime_value(def_id),
            );
        }
        if let Some(variant_id) = self.qualified_enum_variant(expr) {
            return TypedExpr {
                span: expr.span,
                ty,
                kind: TypedExprKind::EnumVariant(variant_id),
            };
        }
        if let Some(def_id) = self.qualified_value(expr) {
            let kind = match self.global_def_kind(def_id) {
                Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                    TypedExprKind::Function(def_id)
                }
                Some(nia_defs::DefKind::Global) => TypedExprKind::Global(def_id),
                Some(nia_defs::DefKind::Comptime) => {
                    return self.lower_comptime_value_expr(
                        expr.span,
                        ty,
                        self.global_comptime_value(def_id),
                    );
                }
                _ => TypedExprKind::Error,
            };
            return TypedExpr {
                span: expr.span,
                ty,
                kind,
            };
        }
        let kind = match &expr.kind {
            ExprKind::Error | ExprKind::Raw(_) | ExprKind::Underscore => TypedExprKind::Error,
            ExprKind::Integer(text) => {
                TypedExprKind::Integer(numeric_literal_body(text).to_string())
            }
            ExprKind::Float(text) => TypedExprKind::Float(numeric_literal_body(text).to_string()),
            ExprKind::String(literal) => {
                TypedExprKind::String(decode_string_literal(literal).unwrap_or_default())
            }
            ExprKind::ByteString(literal) => {
                TypedExprKind::ByteString(decode_byte_string_literal(literal).unwrap_or_default())
            }
            ExprKind::CString(literal) => {
                TypedExprKind::ByteString(decode_c_string_literal(literal).unwrap_or_default())
            }
            ExprKind::Char(text) => TypedExprKind::Char(decode_char_literal(text).unwrap_or(0)),
            ExprKind::ByteChar(text) => TypedExprKind::ByteChar(text.clone()),
            ExprKind::Bool(value) => TypedExprKind::Bool(*value),
            ExprKind::Null => TypedExprKind::Null,
            ExprKind::Ident(_) => {
                if let Some(local_id) = self.local_comptime_use(expr) {
                    return self.lower_comptime_value_expr(
                        expr.span,
                        ty,
                        self.comptime
                            .values
                            .get(&nia_comptime_check::ComptimeKey::Local(local_id))
                            .cloned(),
                    );
                }
                self.lower_ident_expr(expr)
            }
            ExprKind::Builtin { .. } => match self.builtin_value(expr) {
                Some(BuiltinValue::Int(value)) => {
                    TypedExprKind::BuiltinValue(BuiltinConst::Int(*value))
                }
                Some(BuiltinValue::Usize(value)) => {
                    TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                }
                Some(BuiltinValue::Layout { builtin, ty }) => {
                    TypedExprKind::BuiltinValue(BuiltinConst::Layout {
                        builtin: *builtin,
                        ty: *ty,
                    })
                }
                None => TypedExprKind::Error,
            },
            ExprKind::Qualified { .. } if self.builtin_value(expr).is_some() => {
                match self.builtin_value(expr) {
                    Some(BuiltinValue::Int(value)) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Int(*value))
                    }
                    Some(BuiltinValue::Usize(value)) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Usize(*value))
                    }
                    Some(BuiltinValue::Layout { builtin, ty }) => {
                        TypedExprKind::BuiltinValue(BuiltinConst::Layout {
                            builtin: *builtin,
                            ty: *ty,
                        })
                    }
                    None => TypedExprKind::Error,
                }
            }
            ExprKind::TypeTarget { .. } => TypedExprKind::Error,
            ExprKind::BracketSuffix { callee, args } => {
                match self.bracket_suffix_resolution(expr) {
                    Some(BracketSuffixResolution::Index) => {
                        if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                            self.lower_index_read_expr(callee, index)
                                .unwrap_or_else(|| TypedExprKind::Index {
                                    lhs: Box::new(self.lower_expr(callee)),
                                    index: Box::new(self.lower_expr(index)),
                                })
                        } else {
                            TypedExprKind::Error
                        }
                    }
                    Some(BracketSuffixResolution::GenericCall) => {
                        if let Some(reference) = self.function_reference(expr) {
                            TypedExprKind::FunctionInstance {
                                def_id: reference.def_id,
                                arg_module_id: reference.arg_module_id,
                                args: reference.args.clone(),
                            }
                        } else {
                            TypedExprKind::Error
                        }
                    }
                    Some(BracketSuffixResolution::TypePrefixInstantiation) | None => {
                        TypedExprKind::Error
                    }
                }
            }
            ExprKind::Field { lhs, name } => {
                let lhs_expr = self.lower_expr(lhs);
                self.field_def_for_base_ty(lhs_expr.ty, name)
                    .map(|field| TypedExprKind::Field {
                        lhs: Box::new(lhs_expr),
                        field,
                    })
                    .unwrap_or(TypedExprKind::Error)
            }
            ExprKind::ArrayLiteral { elems } => TypedExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems),
            },
            ExprKind::TypedArrayLiteral { elems, .. } => TypedExprKind::ArrayLiteral {
                elems: self.lower_array_elements(elems),
            },
            ExprKind::StructLiteral { fields } | ExprKind::TypedStructLiteral { fields, .. } => {
                let Some(def_id) = self.nominal_global_def(ty) else {
                    return TypedExpr {
                        span: expr.span,
                        ty,
                        kind: TypedExprKind::Error,
                    };
                };
                if self.is_union_def(def_id) {
                    let field = fields.first().map(|field| TypedFieldInit {
                        field: self.field_def_for_struct_ty(ty, &field.name),
                        name: field.name.clone(),
                        value: self.lower_expr(&field.value),
                        span: field.span,
                    });
                    TypedExprKind::UnionLiteral {
                        def_id,
                        field: Box::new(field.unwrap_or_else(|| TypedFieldInit {
                            field: None,
                            name: String::new(),
                            value: TypedExpr {
                                span: expr.span,
                                ty: self.error(),
                                kind: TypedExprKind::Error,
                            },
                            span: expr.span,
                        })),
                    }
                } else {
                    TypedExprKind::StructLiteral {
                        def_id,
                        fields: fields
                            .iter()
                            .map(|field| TypedFieldInit {
                                field: self.field_def_for_struct_ty(ty, &field.name),
                                name: field.name.clone(),
                                value: self.lower_expr(&field.value),
                                span: field.span,
                            })
                            .collect(),
                    }
                }
            }
            ExprKind::Unary { op, expr: inner }
                if let Some(trait_id) = BuiltinOperatorOp::Unary(*op).trait_id() =>
            {
                TypedExprKind::Call {
                    callee: TypedCallee::BuiltinOperator(BuiltinOperator {
                        trait_id,
                        op: BuiltinOperatorOp::Unary(*op),
                    }),
                    args: vec![self.lower_expr(inner)],
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                if let ExprKind::Index {
                    lhs,
                    index: IndexArg::Range(range),
                } = &inner.kind
                    && matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                {
                    self.lower_slice_expr(lhs, range, matches!(op, UnaryOp::RefReadOnly))
                } else if matches!(op, UnaryOp::Ref | UnaryOp::RefReadOnly)
                    && let Some(function_item) = self.lower_function_item_ref(inner)
                {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(TypedExpr {
                            span: inner.span,
                            ty: self.expr_ty(inner).unwrap_or_else(|| self.error()),
                            kind: function_item,
                        }),
                    }
                } else if matches!(op, UnaryOp::Deref)
                    && let Some(pointer) = self.lower_deref_read_pointer(inner)
                {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(pointer),
                    }
                } else {
                    TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(self.lower_expr(inner)),
                    }
                }
            }
            ExprKind::OptionalSome { expr: inner } => {
                let inner_ty = self.optional_elem_ty(ty).or_else(|| self.expr_ty(inner));
                TypedExprKind::OptionalSome {
                    expr: Box::new(self.lower_expr_with_ty(inner, inner_ty)),
                }
            }
            ExprKind::ErrorOk { expr: inner } => {
                let inner_ty = self
                    .error_union_parts(ty)
                    .map(|(_, value)| value)
                    .or_else(|| self.expr_ty(inner));
                TypedExprKind::ErrorOk {
                    expr: Box::new(self.lower_expr_with_ty(inner, inner_ty)),
                }
            }
            ExprKind::ErrorErr { expr: inner } => {
                let inner_ty = self
                    .error_union_parts(ty)
                    .map(|(error, _)| error)
                    .or_else(|| self.expr_ty(inner));
                TypedExprKind::ErrorErr {
                    expr: Box::new(self.lower_expr_with_ty(inner, inner_ty)),
                }
            }
            ExprKind::Try { expr: inner } => TypedExprKind::Try {
                expr: Box::new(self.lower_expr(inner)),
            },
            ExprKind::Binary { lhs, op, rhs }
                if let Some(trait_id) = BuiltinOperatorOp::Binary(*op).trait_id() =>
            {
                TypedExprKind::Call {
                    callee: TypedCallee::BuiltinOperator(BuiltinOperator {
                        trait_id,
                        op: BuiltinOperatorOp::Binary(*op),
                    }),
                    args: vec![self.lower_expr(lhs), self.lower_expr(rhs)],
                }
            }
            ExprKind::Binary { lhs, op, rhs } => TypedExprKind::Binary {
                lhs: Box::new(self.lower_expr(lhs)),
                op: *op,
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ExprKind::Assign {
                lhs,
                op: AssignOp::Assign,
                rhs,
            } if matches!(lhs.kind, ExprKind::Underscore) => {
                TypedExprKind::Discard(Box::new(self.lower_expr(rhs)))
            }
            ExprKind::Assign { lhs, op, rhs } => TypedExprKind::Assign {
                place: self.lower_place(lhs),
                op: *op,
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ExprKind::Cast { expr: inner, ty } => TypedExprKind::Cast {
                expr: Box::new(self.lower_expr(inner)),
                ty: self.ty_for_type(ty),
            },
            ExprKind::Call { callee, args } => {
                if let ExprKind::Builtin { name, .. } = &callee.kind {
                    match (name.as_str(), args.as_slice()) {
                        ("trap", []) => TypedExprKind::Trap,
                        (_, []) => self.lower_expr(callee).kind,
                        ("asm", [arg]) => self.lower_inline_asm(arg),
                        ("memcpy", [dest, source]) => {
                            TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                                op: MemoryIntrinsicOp::Copy,
                                elem_ty: self.memory_intrinsic_elem_ty(expr),
                                dest: Box::new(self.lower_expr(dest)),
                                source: TypedMemoryIntrinsicSource::Slice(Box::new(
                                    self.lower_expr(source),
                                )),
                            })
                        }
                        ("memmove", [dest, source]) => {
                            TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                                op: MemoryIntrinsicOp::Move,
                                elem_ty: self.memory_intrinsic_elem_ty(expr),
                                dest: Box::new(self.lower_expr(dest)),
                                source: TypedMemoryIntrinsicSource::Slice(Box::new(
                                    self.lower_expr(source),
                                )),
                            })
                        }
                        ("memset", [dest, value]) => {
                            TypedExprKind::MemoryIntrinsic(TypedMemoryIntrinsic {
                                op: MemoryIntrinsicOp::Set,
                                elem_ty: self.memory_intrinsic_elem_ty(expr),
                                dest: Box::new(self.lower_expr(dest)),
                                source: TypedMemoryIntrinsicSource::Byte(Box::new(
                                    self.lower_expr(value),
                                )),
                            })
                        }
                        ("load_unaligned", [ptr]) => TypedExprKind::LoadUnaligned {
                            ty: self.builtin_atomic_type_arg(callee),
                            ptr: Box::new(self.lower_expr(ptr)),
                        },
                        ("splat", [value]) => TypedExprKind::Splat {
                            value: Box::new(self.lower_expr(value)),
                        },
                        ("extract", [vector, index]) => TypedExprKind::ExtractElement {
                            vector: Box::new(self.lower_expr(vector)),
                            index: Box::new(self.lower_expr(index)),
                        },
                        ("insert", [vector, index, value]) => TypedExprKind::InsertElement {
                            vector: Box::new(self.lower_expr(vector)),
                            index: Box::new(self.lower_expr(index)),
                            value: Box::new(self.lower_expr(value)),
                        },
                        ("bitmask", [vector]) => TypedExprKind::Bitmask {
                            vector: Box::new(self.lower_expr(vector)),
                        },
                        ("ctz", [value]) => TypedExprKind::BitIntrinsic {
                            op: nia_body_ir::TypedBitIntrinsicOp::Ctz,
                            value: Box::new(self.lower_expr(value)),
                        },
                        ("clz", [value]) => TypedExprKind::BitIntrinsic {
                            op: nia_body_ir::TypedBitIntrinsicOp::Clz,
                            value: Box::new(self.lower_expr(value)),
                        },
                        ("popcount", [value]) => TypedExprKind::BitIntrinsic {
                            op: nia_body_ir::TypedBitIntrinsicOp::Popcount,
                            value: Box::new(self.lower_expr(value)),
                        },
                        ("atomic_load", [ptr, order]) => TypedExprKind::Atomic(TypedAtomic::Load {
                            ty: self.builtin_atomic_type_arg(callee),
                            ptr: Box::new(self.lower_expr(ptr)),
                            order: self.lower_atomic_order(order),
                        }),
                        ("atomic_store", [ptr, value, order]) => {
                            TypedExprKind::Atomic(TypedAtomic::Store {
                                ty: self.builtin_atomic_type_arg(callee),
                                ptr: Box::new(self.lower_expr(ptr)),
                                value: Box::new(self.lower_expr(value)),
                                order: self.lower_atomic_order(order),
                            })
                        }
                        ("atomic_rmw", [ptr, op, value, order]) => {
                            TypedExprKind::Atomic(TypedAtomic::Rmw {
                                ty: self.builtin_atomic_type_arg(callee),
                                ptr: Box::new(self.lower_expr(ptr)),
                                op: self.lower_atomic_rmw_op(op),
                                value: Box::new(self.lower_expr(value)),
                                order: self.lower_atomic_order(order),
                            })
                        }
                        ("cmpxchg_strong", [ptr, expected, desired, success, failure]) => {
                            TypedExprKind::Atomic(TypedAtomic::Cmpxchg {
                                ty: self.builtin_atomic_type_arg(callee),
                                ptr: Box::new(self.lower_expr(ptr)),
                                expected: Box::new(self.lower_expr(expected)),
                                desired: Box::new(self.lower_expr(desired)),
                                success: self.lower_atomic_order(success),
                                failure: self.lower_atomic_order(failure),
                                weak: false,
                            })
                        }
                        ("cmpxchg_weak", [ptr, expected, desired, success, failure]) => {
                            TypedExprKind::Atomic(TypedAtomic::Cmpxchg {
                                ty: self.builtin_atomic_type_arg(callee),
                                ptr: Box::new(self.lower_expr(ptr)),
                                expected: Box::new(self.lower_expr(expected)),
                                desired: Box::new(self.lower_expr(desired)),
                                success: self.lower_atomic_order(success),
                                failure: self.lower_atomic_order(failure),
                                weak: true,
                            })
                        }
                        ("fence", [order]) => TypedExprKind::Atomic(TypedAtomic::Fence {
                            order: self.lower_atomic_order(order),
                        }),
                        _ => TypedExprKind::Call {
                            callee: self.lower_callee(expr, callee),
                            args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                        },
                    }
                } else if let Some(ResolvedCall::BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                }) = self.resolved_call(expr)
                {
                    let (receiver, lowered_args) = self.lower_builtin_call_receiver(callee, args);
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                            trait_id,
                            method,
                            self_ty,
                            trait_args,
                            receiver: Box::new(self.lower_typed_builtin_place_method_receiver(
                                &receiver, self_ty, method,
                            )),
                        }),
                        args: lowered_args,
                    }
                } else if let Some(ResolvedCall::BuiltinMethod { method, self_ty }) =
                    self.resolved_call(expr)
                {
                    let (receiver, lowered_args) = self.lower_builtin_call_receiver(callee, args);
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinMethod {
                            method,
                            self_ty,
                            receiver: Box::new(receiver),
                        },
                        args: lowered_args,
                    }
                } else if let Some(ResolvedCall::BuiltinTraitMethod { trait_id, op }) =
                    self.resolved_call(expr)
                {
                    let lowered_args = if let Some(receiver) = self.lower_receiver_expr(callee) {
                        std::iter::once(receiver)
                            .chain(args.iter().map(|arg| self.lower_expr(arg)))
                            .collect()
                    } else {
                        args.iter().map(|arg| self.lower_expr(arg)).collect()
                    };
                    TypedExprKind::Call {
                        callee: TypedCallee::BuiltinOperator(BuiltinOperator { trait_id, op }),
                        args: lowered_args,
                    }
                } else {
                    TypedExprKind::Call {
                        callee: self.lower_callee(expr, callee),
                        args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
                    }
                }
            }
            ExprKind::Qualified { lhs, name } => {
                if let Some(variant) = self
                    .qualified_enum_variant(expr)
                    .or_else(|| self.enum_variant_for_qualified(lhs, name))
                {
                    TypedExprKind::EnumVariant(variant)
                } else {
                    let lhs_expr = self.lower_expr(lhs);
                    self.field_def_for_base_ty(lhs_expr.ty, name)
                        .map(|field| TypedExprKind::Field {
                            lhs: Box::new(lhs_expr),
                            field,
                        })
                        .unwrap_or(TypedExprKind::Error)
                }
            }
            ExprKind::Index { lhs, index } => match index {
                IndexArg::Expr(index) => {
                    self.lower_index_read_expr(lhs, index)
                        .unwrap_or_else(|| TypedExprKind::Index {
                            lhs: Box::new(self.lower_expr(lhs)),
                            index: Box::new(self.lower_expr(index)),
                        })
                }
                IndexArg::Range(range) => self.lower_slice_read_expr(lhs, range),
            },
            ExprKind::Range(range) => TypedExprKind::Range(self.lower_range(range)),
            ExprKind::Block(block) if self.empty_struct_literal_expr(ty, block) => self
                .nominal_global_def(ty)
                .map(|def_id| TypedExprKind::StructLiteral {
                    def_id,
                    fields: Vec::new(),
                })
                .unwrap_or(TypedExprKind::Error),
            ExprKind::Block(block) => TypedExprKind::Block(self.lower_body_with_expected_tail(
                block,
                if self.is_never(ty) { None } else { Some(ty) },
            )),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => TypedExprKind::If {
                cond: Box::new(self.lower_expr(cond)),
                then_branch: self.lower_body_with_expected_tail(
                    then_branch,
                    if self.is_never(ty) { None } else { Some(ty) },
                ),
                else_branch: else_branch.as_ref().map(|else_branch| {
                    Box::new(self.lower_expr_with_ty(
                        else_branch,
                        if self.is_never(ty) { None } else { Some(ty) },
                    ))
                }),
            },
            ExprKind::ComptimeIf(comptime_if) => {
                match self
                    .node_comptime_if_selections
                    .get(&expr.node_key)
                    .copied()
                {
                    Some(ComptimeIfSelection::Then) => {
                        TypedExprKind::Block(self.lower_body_with_expected_tail(
                            &comptime_if.then_branch,
                            if self.is_never(ty) { None } else { Some(ty) },
                        ))
                    }
                    Some(ComptimeIfSelection::Else) => comptime_if
                        .else_branch
                        .as_deref()
                        .map(|else_branch| {
                            self.lower_expr_with_ty(
                                else_branch,
                                if self.is_never(ty) { None } else { Some(ty) },
                            )
                            .kind
                        })
                        .unwrap_or_else(|| self.lower_void_block(expr.span).kind),
                    Some(ComptimeIfSelection::None) => self.lower_void_block(expr.span).kind,
                    None => TypedExprKind::Error,
                }
            }
            ExprKind::Switch(switch) => TypedExprKind::Switch(Box::new(self.lower_switch(switch))),
        };
        TypedExpr {
            span: expr.span,
            ty,
            kind,
        }
    }

    fn memory_intrinsic_elem_ty(&self, expr: &Expr) -> nia_ids::InternedTyId {
        let ExprKind::Call { args, .. } = &expr.kind else {
            return self.error();
        };
        let Some(dest) = args.first() else {
            return self.error();
        };
        let Some(dest_ty) = self.expr_ty(dest) else {
            return self.error();
        };
        match self.interner.get(self.normalization.normalize(dest_ty)) {
            Some(TyKind::Slice { elem, .. }) | Some(TyKind::Array { elem, .. }) => *elem,
            _ => self.error(),
        }
    }

    fn builtin_atomic_type_arg(&self, builtin: &Expr) -> nia_ids::InternedTyId {
        let ExprKind::Builtin {
            type_arg: Some(type_arg),
            ..
        } = &builtin.kind
        else {
            return self.error();
        };
        self.ty_for_type(type_arg)
    }

    fn lower_atomic_order(&mut self, expr: &Expr) -> AtomicOrder {
        match self.lower_comptime_int(expr) {
            Some(0) => AtomicOrder::Unordered,
            Some(1) => AtomicOrder::Monotonic,
            Some(2) => AtomicOrder::Acquire,
            Some(3) => AtomicOrder::Release,
            Some(4) => AtomicOrder::AcqRel,
            Some(5) => AtomicOrder::SeqCst,
            _ => AtomicOrder::Monotonic,
        }
    }

    fn lower_atomic_rmw_op(&mut self, expr: &Expr) -> AtomicRmwOp {
        match self.lower_comptime_int(expr) {
            Some(0) => AtomicRmwOp::Xchg,
            Some(1) => AtomicRmwOp::Add,
            Some(2) => AtomicRmwOp::Sub,
            Some(3) => AtomicRmwOp::And,
            Some(4) => AtomicRmwOp::Nand,
            Some(5) => AtomicRmwOp::Or,
            Some(6) => AtomicRmwOp::Xor,
            Some(7) => AtomicRmwOp::Max,
            Some(8) => AtomicRmwOp::Min,
            Some(9) => AtomicRmwOp::UMax,
            Some(10) => AtomicRmwOp::UMin,
            _ => AtomicRmwOp::Xchg,
        }
    }

    fn lower_comptime_int(&mut self, expr: &Expr) -> Option<i128> {
        match self
            .with_comptime_context(|this| {
                let expr = this.lower_comptime_expr(expr).map_err(|err| {
                    nia_comptime_engine::ComptimeError {
                        span: err.span,
                        message: err.message,
                    }
                })?;
                nia_comptime_engine::eval_resolved_comptime_expr(&expr, this)
            })
            .ok()?
        {
            nia_comptime_engine::ComptimeValue::Int(value) => value.as_i128(),
            _ => None,
        }
    }

    fn lower_comptime_value_expr(
        &self,
        span: Span,
        ty: nia_ids::InternedTyId,
        value: Option<nia_comptime_check::ComptimeValue>,
    ) -> TypedExpr {
        match value {
            Some(nia_comptime_check::ComptimeValue::Int(value)) => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::BuiltinValue(BuiltinConst::Int(value)),
            },
            Some(_) | None => TypedExpr {
                span,
                ty,
                kind: TypedExprKind::Error,
            },
        }
    }

    fn lower_void_block(&self, span: Span) -> TypedExpr {
        TypedExpr {
            span,
            ty: self.void(),
            kind: TypedExprKind::Block(TypedBody {
                span,
                locals: Vec::new(),
                stmts: Vec::new(),
                tail: None,
                ty: self.void(),
            }),
        }
    }

    pub(crate) fn global_comptime_value(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_comptime_check::ComptimeValue> {
        self.global_comptime_value_for_env(def_id)
    }

    fn global_comptime_value_for_env(
        &self,
        def_id: nia_ids::GlobalDefId,
    ) -> Option<nia_comptime_check::ComptimeValue> {
        let key = nia_comptime_check::ComptimeKey::Global(def_id);
        if def_id.module_id == self.defs.module_id {
            return self.comptime.values.get(&key).cloned();
        }
        self.program_comptime
            .get(&def_id.module_id)
            .and_then(|comptime| comptime.values.get(&key))
            .cloned()
    }

    fn empty_struct_literal_expr(&mut self, ty: nia_ids::InternedTyId, block: &Block) -> bool {
        if !block.stmts.is_empty() || block.tail.is_some() {
            return false;
        }
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(ty) else {
            return false;
        };
        self.resolved_struct_signature(*def_id)
            .is_some_and(|resolved| resolved.signature.fields.is_empty())
    }

    fn lower_deref_read_pointer(&mut self, expr: &Expr) -> Option<TypedExpr> {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        self.lower_builtin_deref_method_call(expr, ty, false)
    }

    fn lower_index_read_expr(&mut self, lhs: &Expr, index: &Expr) -> Option<TypedExprKind> {
        let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
        let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
        let pointer = self.lower_builtin_index_method_call(lhs, index, lhs_ty, index_ty, false)?;
        Some(TypedExprKind::Unary {
            op: UnaryOp::Deref,
            expr: Box::new(pointer),
        })
    }

    fn nominal_global_def(&self, ty: nia_ids::InternedTyId) -> Option<nia_ids::GlobalDefId> {
        match self.interner.get(ty) {
            Some(TyKind::Nominal { def_id, .. }) => Some(*def_id),
            _ => None,
        }
    }

    pub(crate) fn field_def_for_struct_ty(
        &self,
        ty: nia_ids::InternedTyId,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let def_id = self.nominal_global_def(ty)?;
        self.field_def_for_nominal(def_id, name)
    }

    pub(crate) fn field_def_for_base_ty(
        &self,
        ty: nia_ids::InternedTyId,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let base = self.receiver_base_type(ty)?;
        self.field_def_for_nominal(base.def_id, name)
    }

    fn field_def_for_nominal(
        &self,
        def_id: nia_ids::GlobalDefId,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let defs = self.defs_for_module(def_id.module_id)?;
        defs.scopes
            .struct_members
            .get(&def_id.def_id)
            .and_then(|members| members.fields.get(name))
            .or_else(|| {
                defs.scopes
                    .union_members
                    .get(&def_id.def_id)
                    .and_then(|members| members.fields.get(name))
            })
            .map(|field| nia_ids::GlobalDefId {
                module_id: def_id.module_id,
                def_id: field,
            })
    }

    fn qualified_enum_variant(&self, expr: &Expr) -> Option<nia_ids::GlobalDefId> {
        self.variant_enum(expr)
            .and_then(|_| self.qualified_value(expr))
    }

    fn enum_variant_for_qualified(
        &mut self,
        lhs: &Expr,
        name: &str,
    ) -> Option<nia_ids::GlobalDefId> {
        let enum_id = self.type_prefix_def_id(lhs)?;
        if !self.is_enum_def(enum_id) {
            return None;
        }
        let scope = self.enum_variant_scope(enum_id)?;
        let variant_id = scope
            .iter()
            .find(|(variant_name, _)| variant_name == name)
            .map(|(_, def_id)| *def_id)?;
        Some(nia_ids::GlobalDefId {
            module_id: enum_id.module_id,
            def_id: variant_id,
        })
    }

    fn lower_ident_expr(&self, expr: &Expr) -> TypedExprKind {
        match self.local_use(expr) {
            Some(LocalUse::Local(local)) => {
                if self
                    .locals
                    .locals
                    .get(local)
                    .is_some_and(|local| local.kind == LocalKind::ComptimeBinding)
                {
                    return TypedExprKind::Error;
                }
                TypedExprKind::Local(local)
            }
            Some(LocalUse::ModuleValue) => {
                if let Some(variant_id) = self.qualified_value(expr)
                    && self.variant_enum(expr).is_some()
                {
                    return TypedExprKind::EnumVariant(variant_id);
                }
                if let Some(global_id) = self.qualified_value(expr) {
                    match self.global_def_kind(global_id) {
                        Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                            return TypedExprKind::Function(global_id);
                        }
                        Some(nia_defs::DefKind::Global) => return TypedExprKind::Global(global_id),
                        Some(nia_defs::DefKind::Comptime) => return TypedExprKind::Error,
                        _ => return TypedExprKind::Error,
                    }
                }
                match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        match self.defs.defs.get(def_id).map(|def| def.kind) {
                            Some(nia_defs::DefKind::Function | nia_defs::DefKind::Method) => {
                                TypedExprKind::Function(self.global_def_id(def_id))
                            }
                            Some(nia_defs::DefKind::Global) => {
                                TypedExprKind::Global(self.global_def_id(def_id))
                            }
                            Some(nia_defs::DefKind::Comptime) => TypedExprKind::Error,
                            _ => TypedExprKind::Error,
                        }
                    }
                    _ => TypedExprKind::Error,
                }
            }
            _ => TypedExprKind::Error,
        }
    }

    fn lower_function_item_ref(&mut self, expr: &Expr) -> Option<TypedExprKind> {
        let reference = self.function_reference(expr)?;
        if reference.args.is_empty() {
            Some(TypedExprKind::Function(reference.def_id))
        } else {
            Some(TypedExprKind::FunctionInstance {
                def_id: reference.def_id,
                arg_module_id: reference.arg_module_id,
                args: reference.args.clone(),
            })
        }
    }

    fn lower_slice_range(&mut self, range: &SliceRange) -> TypedSliceRange {
        TypedSliceRange {
            start: range
                .start
                .as_ref()
                .map(|start| Box::new(self.lower_expr(start))),
            end: range.end.as_ref().map(|end| Box::new(self.lower_expr(end))),
            inclusive: range.inclusive,
        }
    }

    fn lower_range(&mut self, range: &SliceRange) -> TypedRange {
        TypedRange {
            start: range
                .start
                .as_ref()
                .map(|start| Box::new(self.lower_expr(start))),
            end: range.end.as_ref().map(|end| Box::new(self.lower_expr(end))),
            inclusive: range.inclusive,
        }
    }

    fn lower_array_elements(&mut self, elems: &ArrayElements) -> TypedArrayElements {
        match elems {
            ArrayElements::List(elems) => {
                TypedArrayElements::List(elems.iter().map(|elem| self.lower_expr(elem)).collect())
            }
            ArrayElements::Repeat { value, count } => TypedArrayElements::Repeat {
                value: Box::new(self.lower_expr(value)),
                count: self.lower_array_repeat_count(count),
            },
        }
    }

    pub(crate) fn lower_array_repeat_count(&mut self, count: &Expr) -> u64 {
        if let Some(value) = self.node_array_repeat_counts.get(&count.node_key).copied() {
            return value;
        }
        self.diagnostics
            .push(nia_diagnostic::Diagnostic::user_error_at(
                "E0301",
                count.span,
                "missing checked array repeat count during body IR lowering",
            ));
        0
    }

    fn lower_callee(&mut self, call: &Expr, callee: &Expr) -> TypedCallee {
        if let Some(resolved) = self.resolved_call(call) {
            return self.lower_resolved_callee(callee, resolved);
        }
        TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
    }

    fn lower_resolved_callee(&mut self, callee: &Expr, resolved: ResolvedCall) -> TypedCallee {
        match resolved {
            ResolvedCall::Function(def_id) => TypedCallee::Function(def_id),
            ResolvedCall::FunctionInstance {
                def_id,
                arg_module_id,
                args,
            } => TypedCallee::FunctionInstance {
                def_id,
                arg_module_id,
                args,
            },
            ResolvedCall::Method {
                def_id,
                args,
                receiver_kind,
            } => TypedCallee::Method {
                def_id,
                args,
                receiver_kind,
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
            ResolvedCall::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver_kind,
            } => TypedCallee::TraitMethod {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
                receiver_kind,
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
            ResolvedCall::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
            } => TypedCallee::TraitAssociatedFunction {
                trait_id,
                method_id,
                method_name,
                self_ty,
                trait_args,
                args,
            },
            ResolvedCall::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver_kind,
            } => TypedCallee::DynamicTraitMethod {
                object_ty,
                trait_id,
                method_id,
                method_name,
                trait_args,
                slot,
                params,
                return_type,
                receiver_kind,
                receiver: Box::new(
                    self.lower_receiver_expr(callee)
                        .unwrap_or_else(|| self.lower_expr(callee)),
                ),
            },
            ResolvedCall::BuiltinTraitMethod { trait_id, op } => {
                TypedCallee::BuiltinOperator(BuiltinOperator { trait_id, op })
            }
            ResolvedCall::BuiltinMethod { method, self_ty } => {
                let receiver = self
                    .lower_receiver_expr(callee)
                    .unwrap_or_else(|| self.lower_expr(callee));
                TypedCallee::BuiltinMethod {
                    method,
                    self_ty,
                    receiver: Box::new(receiver),
                }
            }
            ResolvedCall::BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty,
                trait_args,
            } => {
                let receiver = self
                    .lower_receiver_expr(callee)
                    .unwrap_or_else(|| self.lower_expr(callee));
                TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty,
                    trait_args,
                    receiver: Box::new(
                        self.lower_typed_builtin_place_method_receiver(&receiver, self_ty, method),
                    ),
                })
            }
            ResolvedCall::FunctionPointer => {
                TypedCallee::FunctionPointer(Box::new(self.lower_expr(callee)))
            }
        }
    }

    fn lower_receiver_expr(&mut self, callee: &Expr) -> Option<TypedExpr> {
        let field_callee = match &callee.kind {
            ExprKind::Field { .. } => callee,
            ExprKind::BracketSuffix {
                callee: generic_callee,
                ..
            } if matches!(
                self.bracket_suffix_resolution(callee),
                Some(BracketSuffixResolution::GenericCall)
            ) =>
            {
                generic_callee.as_ref()
            }
            _ => return None,
        };
        let ExprKind::Field { lhs, .. } = &field_callee.kind else {
            return None;
        };
        Some(self.lower_expr(lhs))
    }

    fn lower_builtin_call_receiver(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> (TypedExpr, Vec<TypedExpr>) {
        if let Some(receiver) = self.lower_receiver_expr(callee) {
            return (
                receiver,
                args.iter().map(|arg| self.lower_expr(arg)).collect(),
            );
        }
        if let Some((receiver, args)) = args.split_first() {
            return (
                self.lower_expr(receiver),
                args.iter().map(|arg| self.lower_expr(arg)).collect(),
            );
        }
        (self.lower_expr(callee), Vec::new())
    }

    pub(crate) fn lower_place(&mut self, expr: &Expr) -> TypedPlace {
        let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
        let mut elems = Vec::new();
        let base = self.lower_place_inner(expr, &mut elems, true);
        TypedPlace {
            span: expr.span,
            ty,
            base,
            elems,
        }
    }

    fn lower_place_inner(
        &mut self,
        expr: &Expr,
        elems: &mut Vec<PlaceElem>,
        mutable: bool,
    ) -> PlaceBase {
        if self.variant_enum(expr).is_some() {
            return PlaceBase::Error;
        }
        if let Some(def_id) = self.qualified_value(expr) {
            return PlaceBase::Global(def_id);
        }
        match &expr.kind {
            ExprKind::Ident(_) => match self.local_use(expr) {
                Some(LocalUse::Local(local)) => PlaceBase::Local(local),
                Some(LocalUse::ModuleValue) => match self.value_name(expr) {
                    Some(ValueNameResolution::Def(def_id)) => {
                        PlaceBase::Global(self.global_def_id(def_id))
                    }
                    _ => PlaceBase::Error,
                },
                _ => PlaceBase::Error,
            },
            ExprKind::Unary {
                op: nia_ast::UnaryOp::Deref,
                expr,
            } => {
                let ty = self.expr_ty(expr).unwrap_or_else(|| self.error());
                if let Some(pointer) = self.lower_builtin_deref_method_call(expr, ty, mutable) {
                    PlaceBase::Deref(Box::new(pointer))
                } else {
                    PlaceBase::Deref(Box::new(self.lower_expr(expr)))
                }
            }
            ExprKind::Field { lhs, name } | ExprKind::Qualified { lhs, name } => {
                let base = self.lower_place_inner(lhs, elems, mutable);
                let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
                let field = self
                    .field_def_for_base_ty(lhs_ty, name)
                    .map(PlaceElem::Field)
                    .unwrap_or(PlaceElem::Error);
                elems.push(field);
                base
            }
            ExprKind::Index { lhs, index } => {
                if let IndexArg::Expr(index) = index {
                    let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
                    let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
                    if let Some(pointer) =
                        self.lower_builtin_index_method_call(lhs, index, lhs_ty, index_ty, mutable)
                    {
                        return PlaceBase::Deref(Box::new(pointer));
                    }
                    let base = self.lower_place_inner(lhs, elems, mutable);
                    elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                    return base;
                }
                PlaceBase::Error
            }
            ExprKind::BracketSuffix { callee, args } => {
                if matches!(
                    self.bracket_suffix_resolution(expr),
                    Some(BracketSuffixResolution::Index)
                ) {
                    if let Some(index) = args.first().and_then(|arg| arg.expr.as_ref()) {
                        let lhs_ty = self.expr_ty(callee).unwrap_or_else(|| self.error());
                        let index_ty = self.expr_ty(index).unwrap_or_else(|| self.error());
                        if let Some(pointer) = self.lower_builtin_index_method_call(
                            callee, index, lhs_ty, index_ty, mutable,
                        ) {
                            return PlaceBase::Deref(Box::new(pointer));
                        }
                        let base = self.lower_place_inner(callee, elems, mutable);
                        elems.push(PlaceElem::Index(Box::new(self.lower_expr(index))));
                        return base;
                    }
                    PlaceBase::Error
                } else {
                    PlaceBase::Error
                }
            }
            _ => PlaceBase::Error,
        }
    }

    fn lower_builtin_deref_method_call(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        let (trait_id, method, target_const) = if mutable {
            (BuiltinTrait::Deref, BuiltinTraitMethod::Deref, false)
        } else {
            (BuiltinTrait::DerefRead, BuiltinTraitMethod::DerefRead, true)
        };
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(trait_id),
            Vec::new(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let target = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: Vec::new(),
            name: BuiltinTrait::TARGET_ASSOC_TYPE.to_string(),
        });
        let target = self.normalize_projection(target);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: target_const,
            elem: target,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty: receiver_ty,
                    trait_args: Vec::new(),
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        method,
                    )),
                }),
                args: Vec::new(),
            },
        })
    }

    fn lower_builtin_index_method_call(
        &mut self,
        receiver: &Expr,
        index: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        index_ty: nia_ids::InternedTyId,
        mutable: bool,
    ) -> Option<TypedExpr> {
        let (trait_id, method, output_const) = if mutable {
            (BuiltinTrait::Index, BuiltinTraitMethod::Index, false)
        } else {
            (BuiltinTrait::IndexRead, BuiltinTraitMethod::IndexRead, true)
        };
        let trait_args = vec![index_ty];
        let resolution = self.current_context_resolve_trait_obligation(
            receiver_ty,
            TraitId::Builtin(trait_id),
            trait_args.clone(),
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return None;
        }
        let output = self.interner.intern(TyKind::Projection {
            self_ty: receiver_ty,
            trait_id: TraitId::Builtin(trait_id),
            trait_args: trait_args.clone(),
            name: BuiltinTrait::OUTPUT_ASSOC_TYPE.to_string(),
        });
        let output = self.normalize_projection(output);
        let pointer_ty = self.interner.intern(TyKind::Pointer {
            is_readonly: output_const,
            elem: output,
        });
        Some(TypedExpr {
            span: receiver.span,
            ty: pointer_ty,
            kind: TypedExprKind::Call {
                callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                    trait_id,
                    method,
                    self_ty: receiver_ty,
                    trait_args,
                    receiver: Box::new(self.lower_builtin_place_method_receiver(
                        receiver,
                        receiver_ty,
                        method,
                    )),
                }),
                args: vec![self.lower_expr(index)],
            },
        })
    }

    fn lower_slice_read_expr(&mut self, lhs: &Expr, range: &SliceRange) -> TypedExprKind {
        self.lower_slice_expr(lhs, range, true)
    }

    fn lower_slice_expr(
        &mut self,
        lhs: &Expr,
        range: &SliceRange,
        is_readonly: bool,
    ) -> TypedExprKind {
        let lhs_ty = self.expr_ty(lhs).unwrap_or_else(|| self.error());
        let range_ty = self.check_slice_range_bounds(range);
        let (trait_id, method) = if is_readonly {
            (BuiltinTrait::SliceRead, BuiltinTraitMethod::SliceRead)
        } else {
            (BuiltinTrait::Slice, BuiltinTraitMethod::Slice)
        };
        let resolution = self.current_context_resolve_trait_obligation(
            lhs_ty,
            TraitId::Builtin(trait_id),
            vec![range_ty],
        );
        if !matches!(
            resolution,
            TraitResolution::User(_) | TraitResolution::Assumed(_)
        ) {
            return TypedExprKind::Slice {
                lhs: Box::new(self.lower_expr(lhs)),
                range: self.lower_slice_range(range),
                is_readonly,
            };
        }
        TypedExprKind::Call {
            callee: TypedCallee::BuiltinPlaceMethod(BuiltinPlaceMethod {
                trait_id,
                method,
                self_ty: lhs_ty,
                trait_args: vec![range_ty],
                receiver: Box::new(self.lower_builtin_place_method_receiver(lhs, lhs_ty, method)),
            }),
            args: vec![self.lower_range_as_expr(range, range_ty)],
        }
    }

    fn lower_range_as_expr(&mut self, range: &SliceRange, ty: nia_ids::InternedTyId) -> TypedExpr {
        TypedExpr {
            span: self.slice_range_span(range),
            ty,
            kind: TypedExprKind::Range(self.lower_range(range)),
        }
    }

    fn slice_range_span(&self, range: &SliceRange) -> Span {
        match (&range.start, &range.end) {
            (Some(start), Some(end)) => Span::new(start.span.start, end.span.end),
            (Some(start), None) => start.span,
            (None, Some(end)) => end.span,
            (None, None) => Span::default(),
        }
    }

    fn lower_builtin_place_method_receiver(
        &mut self,
        receiver: &Expr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            BuiltinReceiverKind::RefReadOnly => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefReadOnly,
                        expr: Box::new(self.lower_expr(receiver)),
                    },
                }
            }
            BuiltinReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Ref,
                        expr: Box::new(self.lower_expr(receiver)),
                    },
                }
            }
            BuiltinReceiverKind::Value => self.lower_expr(receiver),
        }
    }

    fn lower_typed_builtin_place_method_receiver(
        &mut self,
        receiver: &TypedExpr,
        receiver_ty: nia_ids::InternedTyId,
        method: BuiltinTraitMethod,
    ) -> TypedExpr {
        let receiver_kind = method
            .place_receiver_kind()
            .unwrap_or_else(|| method.receiver_kind());
        match receiver_kind {
            BuiltinReceiverKind::RefReadOnly => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: true,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::RefReadOnly,
                        expr: Box::new(receiver.clone()),
                    },
                }
            }
            BuiltinReceiverKind::Ref => {
                let pointer_ty = self.interner.intern(TyKind::Pointer {
                    is_readonly: false,
                    elem: receiver_ty,
                });
                TypedExpr {
                    span: receiver.span,
                    ty: pointer_ty,
                    kind: TypedExprKind::Unary {
                        op: UnaryOp::Ref,
                        expr: Box::new(receiver.clone()),
                    },
                }
            }
            BuiltinReceiverKind::Value => receiver.clone(),
        }
    }
}
