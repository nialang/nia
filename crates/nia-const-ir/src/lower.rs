use crate::*;

use nia_ids::{BuiltinTraitMethod, InternedTyId, LayoutBuiltin, LocalId};
use nia_node_id::VersionedNodeKey;
use nia_sema_ir::{SemanticUseTable, SemanticValueUse};
use nia_span::Span;
use nia_symbol::{SymbolId, known, symbol_text_from_optional_resolver};
use nia_symbol_table::SymbolTable;

pub fn lower_expr_early(expr: &nia_ast::Expr) -> Result<EarlyConstExpr, ConstLowerError> {
    lower_expr_internal(expr, &EarlyConstLowerInputs::default())
}

pub fn lower_expr_early_with_context(
    expr: &nia_ast::Expr,
    context: &EarlyConstLowerInputs<'_>,
) -> Result<EarlyConstExpr, ConstLowerError> {
    lower_expr_internal(expr, context)
}

#[derive(Clone, Copy, Default)]
pub struct EarlyConstLowerInputs<'a> {
    pub semantic_uses: Option<&'a SemanticUseTable>,
    pub symbols: Option<&'a SymbolTable>,
}

impl<'a> EarlyConstLowerInputs<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_semantic_uses(mut self, semantic_uses: &'a SemanticUseTable) -> Self {
        self.semantic_uses = Some(semantic_uses);
        self
    }

    pub fn with_symbols(mut self, symbols: &'a SymbolTable) -> Self {
        self.symbols = Some(symbols);
        self
    }
}

#[derive(Clone, Copy)]
pub struct ResolvedConstLowerInputs<'a> {
    pub semantic_uses: &'a SemanticUseTable,
    pub symbols: Option<&'a SymbolTable>,
}

impl<'a> ResolvedConstLowerInputs<'a> {
    pub fn new(semantic_uses: &'a SemanticUseTable) -> Self {
        Self {
            semantic_uses,
            symbols: None,
        }
    }

    pub fn with_symbols(mut self, symbols: &'a SymbolTable) -> Self {
        self.symbols = Some(symbols);
        self
    }
}

pub(crate) trait ConstLowerContext {
    fn has_semantic_facts(&self) -> bool;

    fn probe_name_resolution(&self, key: &VersionedNodeKey) -> Option<ConstNameResolution>;

    fn probe_type_id(&self, key: &VersionedNodeKey) -> Option<InternedTyId>;

    fn probe_type_prefix(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId>;

    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<ConstNameResolution>, ConstLowerError>;

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError>;

    fn lower_local_id(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError>;

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<InternedTyId>, ConstLowerError>;

    fn intern_name(&self, text: &str, span: Span) -> Result<Option<SymbolId>, ConstLowerError>;

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(None, symbol)
    }
}

impl ConstLowerContext for EarlyConstLowerInputs<'_> {
    fn has_semantic_facts(&self) -> bool {
        self.semantic_uses.is_some()
    }

    fn probe_name_resolution(&self, key: &VersionedNodeKey) -> Option<ConstNameResolution> {
        self.semantic_uses.and_then(|semantic_uses| {
            semantic_uses
                .node_associated_const_projection(key)
                .cloned()
                .map(ConstNameResolution::AssociatedConstProjection)
                .or_else(|| {
                    semantic_uses
                        .node_builtin_associated_value(key)
                        .map(ConstNameResolution::BuiltinAssociatedValue)
                })
                .or_else(|| {
                    semantic_uses
                        .node_const_generic_use(key)
                        .map(|name| ConstNameResolution::GenericParam(*name))
                })
                .or_else(|| {
                    semantic_uses
                        .node_value_use(key)
                        .map(ConstNameResolution::from)
                })
        })
    }

    fn probe_type_id(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_type_use(key))
    }

    fn probe_type_prefix(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId> {
        self.semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_type_prefix(key))
    }

    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<ConstNameResolution>, ConstLowerError> {
        Ok(self.probe_name_resolution(key))
    }

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError> {
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
    ) -> Result<Option<LocalId>, ConstLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_local_def(key)))
    }

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
        _span: Span,
    ) -> Result<Option<InternedTyId>, ConstLowerError> {
        Ok(self
            .semantic_uses
            .and_then(|semantic_uses| semantic_uses.node_type_use(key)))
    }

    fn intern_name(&self, text: &str, span: Span) -> Result<Option<SymbolId>, ConstLowerError> {
        self.symbols
            .map(|symbols| {
                symbols.intern(text).map_err(|collision| ConstLowerError {
                    span,
                    message: collision.to_string(),
                })
            })
            .transpose()
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols.map(|symbols| symbols as _), symbol)
    }
}

impl ConstLowerContext for ResolvedConstLowerInputs<'_> {
    fn has_semantic_facts(&self) -> bool {
        true
    }

    fn probe_name_resolution(&self, key: &VersionedNodeKey) -> Option<ConstNameResolution> {
        self.semantic_uses
            .node_associated_const_projection(key)
            .cloned()
            .map(ConstNameResolution::AssociatedConstProjection)
            .or_else(|| {
                self.semantic_uses
                    .node_builtin_associated_value(key)
                    .map(ConstNameResolution::BuiltinAssociatedValue)
            })
            .or_else(|| {
                self.semantic_uses
                    .node_const_generic_use(key)
                    .map(|name| ConstNameResolution::GenericParam(*name))
            })
            .or_else(|| {
                self.semantic_uses
                    .node_value_use(key)
                    .map(ConstNameResolution::from)
            })
    }

    fn probe_type_id(&self, key: &VersionedNodeKey) -> Option<InternedTyId> {
        self.semantic_uses.node_type_use(key)
    }

    fn probe_type_prefix(&self, key: &VersionedNodeKey) -> Option<nia_ids::GlobalDefId> {
        self.semantic_uses.node_type_prefix(key)
    }

    fn resolve_name(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<ConstNameResolution>, ConstLowerError> {
        self.probe_name_resolution(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "const name"))
    }

    fn lower_local_use(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError> {
        match self.semantic_uses.node_value_use(key) {
            Some(SemanticValueUse::Local(local_id)) => Ok(Some(local_id)),
            Some(SemanticValueUse::Global(_)) | None => {
                Err(unresolved_error(span, "const assignment target"))
            }
        }
    }

    fn lower_local_id(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<LocalId>, ConstLowerError> {
        self.semantic_uses
            .node_local_def(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "const local binding"))
    }

    fn lower_type_id(
        &self,
        key: &VersionedNodeKey,
        span: Span,
    ) -> Result<Option<InternedTyId>, ConstLowerError> {
        self.semantic_uses
            .node_type_use(key)
            .map(Some)
            .ok_or_else(|| unresolved_error(span, "const type"))
    }

    fn intern_name(&self, text: &str, span: Span) -> Result<Option<SymbolId>, ConstLowerError> {
        let Some(symbols) = self.symbols else {
            return Err(ConstLowerError {
                span,
                message: "const lowering requires a symbol table for dynamic field names"
                    .to_string(),
            });
        };
        symbols
            .intern(text)
            .map(Some)
            .map_err(|collision| ConstLowerError {
                span,
                message: collision.to_string(),
            })
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(self.symbols.map(|symbols| symbols as _), symbol)
    }
}

fn lower_expr_internal(
    expr: &nia_ast::Expr,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstExpr, ConstLowerError> {
    let kind = match &expr.kind {
        nia_ast::ExprKind::Integer(text) => EarlyConstExprKind::Integer(text.clone()),
        nia_ast::ExprKind::Char(text) => EarlyConstExprKind::Char(text.clone()),
        nia_ast::ExprKind::ByteChar(text) => EarlyConstExprKind::ByteChar(text.clone()),
        nia_ast::ExprKind::Float(text) => EarlyConstExprKind::Float(text.clone()),
        nia_ast::ExprKind::String(literal) => {
            EarlyConstExprKind::String(lower_string_literal(literal))
        }
        nia_ast::ExprKind::ByteString(literal) => {
            EarlyConstExprKind::ByteString(lower_string_literal(literal))
        }
        nia_ast::ExprKind::Bool(value) => EarlyConstExprKind::Bool(*value),
        nia_ast::ExprKind::Null => EarlyConstExprKind::Null,
        nia_ast::ExprKind::Ident(name) => {
            EarlyConstExprKind::Ident(lower_const_name(name, &expr.node_key, expr.span, context)?)
        }
        nia_ast::ExprKind::SelfValue => {
            let Some(name) = context.intern_name("self", expr.span)? else {
                return Err(ConstLowerError {
                    span: expr.span,
                    message: "const receiver lowering requires a symbol table".to_string(),
                });
            };
            let Some(local_id) = lower_local_use(context, &expr.node_key, expr.span)? else {
                return Err(unresolved_error(expr.span, "const receiver"));
            };
            EarlyConstExprKind::Ident(EarlyConstName::resolved(
                name,
                ConstNameResolution::Local(local_id),
            ))
        }
        nia_ast::ExprKind::Qualified { name, .. } => EarlyConstExprKind::Qualified(
            lower_const_name(name, &expr.node_key, expr.span, context)?,
        ),
        nia_ast::ExprKind::Field { lhs, name } => EarlyConstExprKind::Field {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
            name: *name,
        },
        nia_ast::ExprKind::BracketSuffix { callee, args } => {
            let [arg] = args.as_slice() else {
                return Err(ConstLowerError {
                    span: expr.span,
                    message: "const bracket suffix requires exactly one index argument".to_string(),
                });
            };
            let Some(index) = &arg.expr else {
                return Err(ConstLowerError {
                    span: arg.span,
                    message: "const bracket suffix requires an expression index".to_string(),
                });
            };
            EarlyConstExprKind::Index {
                lhs: Box::new(lower_expr_internal(callee, context)?),
                index: Box::new(lower_expr_internal(index, context)?),
            }
        }
        nia_ast::ExprKind::Index { lhs, index } => match index {
            nia_ast::IndexArg::Expr(index) => EarlyConstExprKind::Index {
                lhs: Box::new(lower_expr_internal(lhs, context)?),
                index: Box::new(lower_expr_internal(index, context)?),
            },
            nia_ast::IndexArg::Range(range) => EarlyConstExprKind::Slice {
                lhs: Box::new(lower_expr_internal(lhs, context)?),
                range: lower_slice_range_with_context(range, context)?,
            },
        },
        nia_ast::ExprKind::ArrayLiteral { elems } => EarlyConstExprKind::ArrayLiteral {
            ty: None,
            elems: lower_array_elements_with_context(elems, context)?,
        },
        nia_ast::ExprKind::TypedArrayLiteral { ty, elems } => EarlyConstExprKind::ArrayLiteral {
            ty: lower_type_id(context, &ty.node_key, ty.span)?,
            elems: lower_array_elements_with_context(elems, context)?,
        },
        nia_ast::ExprKind::StructLiteral { fields } => EarlyConstExprKind::StructLiteral {
            ty: None,
            fields: fields
                .iter()
                .map(|field| lower_field_init_with_context(field, context))
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::TypedStructLiteral { ty, fields } => EarlyConstExprKind::StructLiteral {
            ty: lower_type_id(context, &ty.node_key, ty.span)?,
            fields: fields
                .iter()
                .map(|field| lower_field_init_with_context(field, context))
                .collect::<Result<Vec<_>, _>>()?,
        },
        nia_ast::ExprKind::QualifiedStructLiteral { target, fields } => {
            EarlyConstExprKind::EnumStructLiteral {
                variant: Box::new(lower_expr_internal(target, context)?),
                fields: fields
                    .iter()
                    .map(|field| lower_field_init_with_context(field, context))
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        nia_ast::ExprKind::Call { callee, args } => lower_call_with_context(callee, args, context)?,
        nia_ast::ExprKind::Unary { op, expr } => EarlyConstExprKind::Unary {
            op: lower_unary_op(*op),
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::OptionalSome { expr } => EarlyConstExprKind::OptionalSome {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::ErrorOk { expr } => EarlyConstExprKind::ErrorOk {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::ErrorErr { expr } => EarlyConstExprKind::ErrorErr {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::Try { expr } => EarlyConstExprKind::Try {
            expr: Box::new(lower_expr_internal(expr, context)?),
        },
        nia_ast::ExprKind::Binary { lhs, op, rhs } => EarlyConstExprKind::Binary {
            lhs: Box::new(lower_expr_internal(lhs, context)?),
            op: lower_binary_op(*op),
            rhs: Box::new(lower_expr_internal(rhs, context)?),
        },
        nia_ast::ExprKind::Assign { lhs, op, rhs } => {
            EarlyConstExprKind::Assign(Box::new(EarlyConstAssign {
                lhs: lower_assign_target_with_context(lhs, context)?,
                op: lower_assign_op(*op),
                rhs: lower_expr_internal(rhs, context)?,
            }))
        }
        nia_ast::ExprKind::Range(range) => {
            EarlyConstExprKind::Range(lower_const_range_with_context(range, context)?)
        }
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => EarlyConstExprKind::If {
            cond: Box::new(lower_expr_internal(cond, context)?),
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_expr_internal(else_branch, context))
                .transpose()?
                .map(Box::new),
        },
        nia_ast::ExprKind::IfPattern(if_pattern) => EarlyConstExprKind::Switch(Box::new(
            lower_if_pattern_as_switch(expr.span, if_pattern, context)?,
        )),
        nia_ast::ExprKind::Switch(switch) => EarlyConstExprKind::Switch(Box::new(
            lower_switch_with_context(expr.span, switch, context)?,
        )),
        nia_ast::ExprKind::Cast { expr, ty } => EarlyConstExprKind::Cast {
            expr: Box::new(lower_expr_internal(expr, context)?),
            ty: lower_type_id(context, &ty.node_key, ty.span)?,
        },
        nia_ast::ExprKind::Block(block) => {
            EarlyConstExprKind::Block(lower_block_with_context(block, context)?)
        }
        _ => {
            return Err(ConstLowerError {
                span: expr.span,
                message: "unsupported const expression".to_string(),
            });
        }
    };
    Ok(EarlyConstExpr {
        span: expr.span,
        kind,
    })
}

pub fn lower_expr_resolved_with_context(
    expr: &nia_ast::Expr,
    context: &ResolvedConstLowerInputs<'_>,
) -> Result<ResolvedConstExpr, ConstLowerError> {
    let expr = lower_expr_internal(expr, context)?;
    ResolvedConstExpr::new(expr)
}

fn lower_string_literal(literal: &nia_ast::StringLiteral) -> ConstStringLiteral {
    ConstStringLiteral {
        parts: literal.parts.clone(),
    }
}

fn lower_string_literal_name(
    literal: &nia_ast::StringLiteral,
    span: Span,
    context: &dyn ConstLowerContext,
) -> Result<SymbolId, ConstLowerError> {
    let text = nia_literals::eval_string_literal_parts(literal.parts.iter().map(String::as_str))
        .ok_or_else(|| ConstLowerError {
            span,
            message: "invalid string literal in const field name".to_string(),
        })?;
    context
        .intern_name(text.as_str(), span)?
        .ok_or_else(|| ConstLowerError {
            span,
            message: "const field name lowering requires a symbol table".to_string(),
        })
}

fn lower_unary_op(op: nia_ast::UnaryOp) -> ConstUnaryOp {
    match op {
        nia_ast::UnaryOp::Neg => ConstUnaryOp::Neg,
        nia_ast::UnaryOp::Not => ConstUnaryOp::Not,
        nia_ast::UnaryOp::BitNot => ConstUnaryOp::BitNot,
        nia_ast::UnaryOp::RefReadOnly => ConstUnaryOp::RefReadOnly,
        nia_ast::UnaryOp::Ref => ConstUnaryOp::Ref,
        nia_ast::UnaryOp::Deref => ConstUnaryOp::Deref,
    }
}

fn lower_binary_op(op: nia_ast::BinaryOp) -> ConstBinaryOp {
    match op {
        nia_ast::BinaryOp::Mul => ConstBinaryOp::Mul,
        nia_ast::BinaryOp::Div => ConstBinaryOp::Div,
        nia_ast::BinaryOp::Rem => ConstBinaryOp::Rem,
        nia_ast::BinaryOp::Add => ConstBinaryOp::Add,
        nia_ast::BinaryOp::Sub => ConstBinaryOp::Sub,
        nia_ast::BinaryOp::Shl => ConstBinaryOp::Shl,
        nia_ast::BinaryOp::Shr => ConstBinaryOp::Shr,
        nia_ast::BinaryOp::Lt => ConstBinaryOp::Lt,
        nia_ast::BinaryOp::Le => ConstBinaryOp::Le,
        nia_ast::BinaryOp::Gt => ConstBinaryOp::Gt,
        nia_ast::BinaryOp::Ge => ConstBinaryOp::Ge,
        nia_ast::BinaryOp::Eq => ConstBinaryOp::Eq,
        nia_ast::BinaryOp::Ne => ConstBinaryOp::Ne,
        nia_ast::BinaryOp::BitAnd => ConstBinaryOp::BitAnd,
        nia_ast::BinaryOp::BitXor => ConstBinaryOp::BitXor,
        nia_ast::BinaryOp::BitOr => ConstBinaryOp::BitOr,
        nia_ast::BinaryOp::And => ConstBinaryOp::And,
        nia_ast::BinaryOp::Or => ConstBinaryOp::Or,
    }
}

fn lower_assign_op(op: nia_ast::AssignOp) -> ConstAssignOp {
    match op {
        nia_ast::AssignOp::Assign => ConstAssignOp::Assign,
        nia_ast::AssignOp::Add => ConstAssignOp::Add,
        nia_ast::AssignOp::Sub => ConstAssignOp::Sub,
        nia_ast::AssignOp::Shl => ConstAssignOp::Shl,
        nia_ast::AssignOp::Shr => ConstAssignOp::Shr,
        nia_ast::AssignOp::Mul => ConstAssignOp::Mul,
        nia_ast::AssignOp::Div => ConstAssignOp::Div,
        nia_ast::AssignOp::Rem => ConstAssignOp::Rem,
        nia_ast::AssignOp::BitAnd => ConstAssignOp::BitAnd,
        nia_ast::AssignOp::BitXor => ConstAssignOp::BitXor,
        nia_ast::AssignOp::BitOr => ConstAssignOp::BitOr,
    }
}

fn lower_call_with_context(
    callee: &nia_ast::Expr,
    args: &[nia_ast::Expr],
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstExprKind, ConstLowerError> {
    if let Some((name, type_arg, builtin_span)) = std_builtin_call(callee) {
        if name == known::ERROR {
            if type_arg.is_some() {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `error` does not take a type argument".to_string(),
                });
            }
            if args.len() != 1 {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `error` requires exactly one message argument".to_string(),
                });
            }
            return Ok(EarlyConstExprKind::CompileError {
                message: Box::new(lower_expr_internal(&args[0], context)?),
            });
        }
        if name == known::TRAP {
            if type_arg.is_some() {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `trap` does not take a type argument".to_string(),
                });
            }
            if !args.is_empty() {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `trap` does not take value arguments".to_string(),
                });
            }
            return Ok(EarlyConstExprKind::Trap);
        }
        if let Some(builtin) = layout_builtin_from_symbol(name) {
            let Some(type_arg) = type_arg else {
                let name = context.symbol_name(name);
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: format!("builtin `{name}` requires a type argument"),
                });
            };
            if !args.is_empty() {
                let name = context.symbol_name(name);
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: format!("builtin `{name}` does not take value arguments"),
                });
            }
            return Ok(EarlyConstExprKind::LayoutBuiltin {
                builtin,
                type_arg: EarlyConstTypeArg::from_type_ref(type_arg, context)?,
            });
        }
        if name == known::OFFSET {
            let Some(type_arg) = type_arg else {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `offset` requires an aggregate type argument".to_string(),
                });
            };
            let [arg] = args else {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `offset` requires exactly one field name argument"
                        .to_string(),
                });
            };
            let nia_ast::ExprKind::String(field) = &arg.kind else {
                return Err(ConstLowerError {
                    span: arg.span,
                    message: "builtin `offset` field name must be a string literal".to_string(),
                });
            };
            return Ok(EarlyConstExprKind::FieldOffsetBuiltin {
                type_arg: EarlyConstTypeArg::from_type_ref(type_arg, context)?,
                field: lower_string_literal_name(field, arg.span, context)?,
            });
        }
        if name == known::EMBED {
            if type_arg.is_some() {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `embed` does not take a type argument".to_string(),
                });
            }
            let [arg] = args else {
                return Err(ConstLowerError {
                    span: builtin_span,
                    message: "builtin `embed` requires exactly one path argument".to_string(),
                });
            };
            let nia_ast::ExprKind::String(path) = &arg.kind else {
                return Err(ConstLowerError {
                    span: arg.span,
                    message: "builtin `embed` path must be a string literal".to_string(),
                });
            };
            return Ok(EarlyConstExprKind::Embed {
                path: lower_string_literal(path),
            });
        }
    }
    let (callee, generic_args) = match &callee.kind {
        nia_ast::ExprKind::BracketSuffix {
            callee: generic_callee,
            args: bracket_args,
        } => (
            generic_callee.as_ref(),
            lower_generic_args_with_context(bracket_args, context)?,
        ),
        _ => (callee, Vec::new()),
    };
    if let nia_ast::ExprKind::Field { lhs, name } = &callee.kind {
        if args.is_empty()
            && generic_args.is_empty()
            && let Some(method) = const_builtin_method_name(*name)
        {
            return Ok(EarlyConstExprKind::BuiltinMethod {
                method,
                lhs: Box::new(lower_expr_internal(lhs, context)?),
            });
        }
        return Ok(EarlyConstExprKind::Call {
            callee: Box::new(EarlyConstExpr {
                span: callee.span,
                kind: EarlyConstExprKind::Method {
                    receiver: Box::new(lower_expr_internal(lhs, context)?),
                    name: *name,
                },
            }),
            generic_args,
            args: args
                .iter()
                .map(|arg| lower_expr_internal(arg, context))
                .collect::<Result<Vec<_>, _>>()?,
        });
    }
    if let nia_ast::ExprKind::Qualified { lhs, name } = &callee.kind
        && context.probe_name_resolution(&callee.node_key).is_none()
    {
        let nominal_instance = match &lhs.kind {
            nia_ast::ExprKind::BracketSuffix {
                callee: nominal,
                args,
            } => context
                .probe_type_prefix(&nominal.node_key)
                .or_else(|| context.probe_type_prefix(&lhs.node_key))
                .map(|def_id| {
                    lower_type_args_with_context(args, context)
                        .map(|args| EarlyConstAssociatedTarget::Nominal { def_id, args })
                }),
            _ => None,
        }
        .transpose()?;
        let target = nominal_instance.or_else(|| {
            context
                .probe_type_id(&lhs.node_key)
                .or_else(|| context.probe_type_id(&callee.node_key))
                .map(|target_ty| {
                    EarlyConstAssociatedTarget::Type(EarlyConstTypeArg {
                        span: lhs.span,
                        ty_span: lhs.span,
                        ty: Some(target_ty),
                    })
                })
                .or_else(|| {
                    context.probe_type_prefix(&lhs.node_key).map(|def_id| {
                        EarlyConstAssociatedTarget::Nominal {
                            def_id,
                            args: Vec::new(),
                        }
                    })
                })
        });
        let Some(target) = target else {
            return Ok(EarlyConstExprKind::Call {
                callee: Box::new(lower_expr_internal(callee, context)?),
                generic_args,
                args: args
                    .iter()
                    .map(|arg| lower_expr_internal(arg, context))
                    .collect::<Result<Vec<_>, _>>()?,
            });
        };
        return Ok(EarlyConstExprKind::Call {
            callee: Box::new(EarlyConstExpr {
                span: callee.span,
                kind: EarlyConstExprKind::AssociatedFunction {
                    target,
                    name: *name,
                },
            }),
            generic_args,
            args: args
                .iter()
                .map(|arg| lower_expr_internal(arg, context))
                .collect::<Result<Vec<_>, _>>()?,
        });
    }
    Ok(EarlyConstExprKind::Call {
        callee: Box::new(lower_expr_internal(callee, context)?),
        generic_args,
        args: args
            .iter()
            .map(|arg| lower_expr_internal(arg, context))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn layout_builtin_from_symbol(name: SymbolId) -> Option<LayoutBuiltin> {
    match name {
        known::SIZE => Some(LayoutBuiltin::Size),
        known::ALIGN => Some(LayoutBuiltin::Align),
        _ => None,
    }
}

fn std_builtin_call(callee: &nia_ast::Expr) -> Option<(SymbolId, Option<&nia_ast::TypeRef>, Span)> {
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
    (*root == known::STD && *builtin_segment == known::BUILTIN).then_some((
        *name,
        None,
        callee.span,
    ))
}

fn const_builtin_method_name(name: SymbolId) -> Option<BuiltinTraitMethod> {
    match name {
        known::LEN => Some(BuiltinTraitMethod::Len),
        known::START => Some(BuiltinTraitMethod::Start),
        known::END => Some(BuiltinTraitMethod::End),
        _ => None,
    }
}

fn lower_const_range_with_context(
    range: &nia_ast::SliceRange,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstRange, ConstLowerError> {
    Ok(EarlyConstRange {
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
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstSliceRange, ConstLowerError> {
    let range = lower_const_range_with_context(range, context)?;
    Ok(EarlyConstSliceRange {
        start: range.start,
        end: range.end,
        inclusive: range.inclusive,
    })
}

fn lower_type_args_with_context(
    args: &[nia_ast::BracketArg],
    context: &dyn ConstLowerContext,
) -> Result<Vec<EarlyConstTypeArg>, ConstLowerError> {
    args.iter()
        .map(|arg| {
            let Some(ty) = &arg.ty else {
                return Err(ConstLowerError {
                    span: arg.span,
                    message: "const generic function arguments must be types".to_string(),
                });
            };
            Ok(EarlyConstTypeArg {
                span: arg.span,
                ty_span: ty.span,
                ty: lower_type_id(context, &ty.node_key, ty.span)?,
            })
        })
        .collect()
}

fn lower_generic_args_with_context(
    args: &[nia_ast::BracketArg],
    context: &dyn ConstLowerContext,
) -> Result<Vec<EarlyConstGenericArg>, ConstLowerError> {
    args.iter()
        .map(|arg| {
            if let Some(ty) = &arg.ty
                && (context.probe_type_id(&ty.node_key).is_some()
                    || !context.has_semantic_facts()
                    || arg.expr.is_none())
            {
                return Ok(EarlyConstGenericArg::Type(EarlyConstTypeArg {
                    span: arg.span,
                    ty_span: ty.span,
                    ty: lower_type_id(context, &ty.node_key, ty.span)?,
                }));
            }
            if let Some(expr) = &arg.expr {
                return lower_expr_internal(expr, context).map(EarlyConstGenericArg::Const);
            }
            let Some(ty) = &arg.ty else {
                return Err(ConstLowerError {
                    span: arg.span,
                    message: "generic argument must be a type or const value".to_string(),
                });
            };
            Ok(EarlyConstGenericArg::Type(EarlyConstTypeArg {
                span: arg.span,
                ty_span: ty.span,
                ty: lower_type_id(context, &ty.node_key, ty.span)?,
            }))
        })
        .collect()
}

fn lower_assign_target_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstAssignTarget, ConstLowerError> {
    let mut path = Vec::new();
    let (span, name, local_id) = lower_assign_target_base_with_context(expr, context, &mut path)?;
    Ok(EarlyConstAssignTarget::Local {
        span,
        name,
        local_id,
        path,
    })
}

fn lower_assign_target_base_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ConstLowerContext,
    path: &mut Vec<EarlyConstAssignPathElem>,
) -> Result<(Span, SymbolId, Option<LocalId>), ConstLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::Ident(name) => Ok((
            expr.span,
            *name,
            lower_local_use(context, &expr.node_key, expr.span)?,
        )),
        nia_ast::ExprKind::SelfValue => {
            let Some(name) = context.intern_name("self", expr.span)? else {
                return Err(ConstLowerError {
                    span: expr.span,
                    message: "const receiver lowering requires a symbol table".to_string(),
                });
            };
            Ok((
                expr.span,
                name,
                lower_local_use(context, &expr.node_key, expr.span)?,
            ))
        }
        nia_ast::ExprKind::Field { lhs, name } => {
            let base = lower_assign_target_base_with_context(lhs, context, path)?;
            path.push(EarlyConstAssignPathElem::Field {
                span: expr.span,
                name: *name,
            });
            Ok(base)
        }
        nia_ast::ExprKind::Index { lhs, index } => {
            let base = lower_assign_target_base_with_context(lhs, context, path)?;
            let nia_ast::IndexArg::Expr(index) = index else {
                return Err(ConstLowerError {
                    span: expr.span,
                    message: "const assignment target does not support slicing".to_string(),
                });
            };
            path.push(EarlyConstAssignPathElem::Index {
                span: expr.span,
                index: lower_expr_internal(index, context)?,
            });
            Ok(base)
        }
        nia_ast::ExprKind::BracketSuffix { callee, args } => {
            let base = lower_assign_target_base_with_context(callee, context, path)?;
            let [arg] = args.as_slice() else {
                return Err(ConstLowerError {
                    span: expr.span,
                    message:
                        "const assignment target bracket suffix requires exactly one index argument"
                            .to_string(),
                });
            };
            let Some(index) = &arg.expr else {
                return Err(ConstLowerError {
                    span: arg.span,
                    message: "const assignment target bracket suffix requires an expression index"
                        .to_string(),
                });
            };
            path.push(EarlyConstAssignPathElem::Index {
                span: expr.span,
                index: lower_expr_internal(index, context)?,
            });
            Ok(base)
        }
        _ => Err(ConstLowerError {
            span: expr.span,
            message: "unsupported const assignment target".to_string(),
        }),
    }
}

fn lower_array_elements_with_context(
    elems: &nia_ast::ArrayElements,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstArrayElements, ConstLowerError> {
    match elems {
        nia_ast::ArrayElements::List(elems) => Ok(EarlyConstArrayElements::List(
            elems
                .iter()
                .map(|elem| lower_expr_internal(elem, context))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        nia_ast::ArrayElements::Repeat { value, count } => Ok(EarlyConstArrayElements::Repeat {
            value: Box::new(lower_expr_internal(value, context)?),
            count: Box::new(lower_expr_internal(count, context)?),
        }),
    }
}

fn resolve_name(
    context: &dyn ConstLowerContext,
    key: &VersionedNodeKey,
    span: Span,
) -> Result<Option<ConstNameResolution>, ConstLowerError> {
    context.resolve_name(key, span)
}

fn lower_const_name(
    name: &SymbolId,
    key: &VersionedNodeKey,
    span: Span,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstName, ConstLowerError> {
    match resolve_name(context, key, span)? {
        Some(resolution) => Ok(EarlyConstName::resolved(*name, resolution)),
        None => Ok(EarlyConstName::unresolved(*name)),
    }
}

fn lower_local_id(
    context: &dyn ConstLowerContext,
    key: &VersionedNodeKey,
    span: Span,
) -> Result<Option<LocalId>, ConstLowerError> {
    context.lower_local_id(key, span)
}

fn lower_local_use(
    context: &dyn ConstLowerContext,
    key: &VersionedNodeKey,
    span: Span,
) -> Result<Option<LocalId>, ConstLowerError> {
    context.lower_local_use(key, span)
}

pub(crate) fn lower_type_id(
    context: &dyn ConstLowerContext,
    key: &VersionedNodeKey,
    span: Span,
) -> Result<Option<InternedTyId>, ConstLowerError> {
    context.lower_type_id(key, span)
}

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
        param.ty,
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
        binding.explicit_type,
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
        EarlyConstExprKind::BuiltinMethod { method, lhs } => ResolvedConstExprKind::BuiltinMethod {
            method,
            lhs: Box::new(resolve_expr(*lhs)?),
        },
        EarlyConstExprKind::Index { lhs, index } => ResolvedConstExprKind::Index {
            lhs: Box::new(resolve_expr(*lhs)?),
            index: Box::new(resolve_expr(*index)?),
        },
        EarlyConstExprKind::Slice { lhs, range } => ResolvedConstExprKind::Slice {
            lhs: Box::new(resolve_expr(*lhs)?),
            range: resolve_const_slice_range(range)?,
        },
        EarlyConstExprKind::ArrayLiteral { ty, elems } => ResolvedConstExprKind::ArrayLiteral {
            ty,
            elems: resolve_const_array_elements(elems)?,
        },
        EarlyConstExprKind::StructLiteral { ty, fields } => ResolvedConstExprKind::StructLiteral {
            ty,
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
        EarlyConstExprKind::Switch(switch) => {
            ResolvedConstExprKind::Switch(Box::new(resolve_const_switch(*switch)?))
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

fn resolve_const_switch(switch: EarlyConstSwitch) -> Result<ResolvedConstSwitch, ConstLowerError> {
    Ok(ResolvedConstSwitch::new(
        switch.span,
        resolve_expr(switch.target)?,
        switch
            .arms
            .into_iter()
            .map(resolve_const_switch_arm)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

fn resolve_const_switch_arm(
    arm: EarlyConstSwitchArm,
) -> Result<ResolvedConstSwitchArm, ConstLowerError> {
    Ok(ResolvedConstSwitchArm::new(
        arm.span,
        arm.patterns
            .into_iter()
            .map(resolve_const_pattern)
            .collect::<Result<Vec<_>, _>>()?,
        resolve_const_switch_arm_body(arm.body)?,
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
            local_id.ok_or_else(|| unresolved_error(span, "const switch pattern local"))?,
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
                ConstEnumPatternFields::Named(fields) => ConstEnumPatternFields::Named(
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
                ),
            },
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

fn resolve_const_switch_arm_body(
    body: EarlyConstSwitchArmBody,
) -> Result<ResolvedConstSwitchArmBody, ConstLowerError> {
    match body {
        EarlyConstSwitchArmBody::Expr(expr) => {
            resolve_expr(expr).map(ResolvedConstSwitchArmBody::expr)
        }
        EarlyConstSwitchArmBody::Stmt(stmt) => {
            resolve_const_stmt(*stmt).map(ResolvedConstSwitchArmBody::stmt)
        }
        EarlyConstSwitchArmBody::Block(block) => {
            resolve_const_block(block).map(ResolvedConstSwitchArmBody::block)
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

pub(crate) fn unresolved_error(span: Span, what: &str) -> ConstLowerError {
    ConstLowerError {
        span,
        message: format!("failed to resolve {what}"),
    }
}

pub fn lower_function_early(
    function_span: Span,
    function: &nia_ast::FunctionItem,
) -> Result<EarlyConstFunction, ConstLowerError> {
    lower_function_internal(function_span, function, &EarlyConstLowerInputs::default())
}

fn lower_function_internal(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstFunction, ConstLowerError> {
    if !function.is_const || function.is_extern {
        return Err(ConstLowerError {
            span: function_span,
            message: "const expression can only call `const fn`".to_string(),
        });
    }
    let Some(body) = &function.body else {
        return Err(ConstLowerError {
            span: function_span,
            message: "const function requires a body".to_string(),
        });
    };
    let params = function
        .params
        .iter()
        .map(|param| {
            let name = match param.name {
                Some(name) => Some(name),
                None if param.receiver.is_some() => context.intern_name("self", param.span)?,
                None => None,
            };
            let Some(name) = name else {
                return Err(ConstLowerError {
                    span: param.span,
                    message: "const function parameter requires a name".to_string(),
                });
            };
            Ok(EarlyConstParam {
                span: param.span,
                name,
                local_id: lower_local_id(context, &param.node_key, param.span)?,
                ty: param
                    .ty
                    .as_ref()
                    .map(|ty| lower_type_id(context, &ty.node_key, ty.span))
                    .transpose()?
                    .flatten(),
                receiver: param.receiver,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EarlyConstFunction {
        span: function_span,
        params,
        body: lower_block_with_context(body, context)?,
    })
}

pub fn lower_function_resolved_with_context(
    function_span: Span,
    function: &nia_ast::FunctionItem,
    context: &ResolvedConstLowerInputs<'_>,
) -> Result<ResolvedConstFunction, ConstLowerError> {
    let function = lower_function_internal(function_span, function, context)?;
    ResolvedConstFunction::new(function)
}

fn lower_block_with_context(
    block: &nia_ast::Block,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstBlock, ConstLowerError> {
    Ok(EarlyConstBlock {
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
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstStmt, ConstLowerError> {
    let kind = match &stmt.kind {
        nia_ast::StmtKind::Binding(binding) => {
            let Some(value) = &binding.value else {
                return Err(ConstLowerError {
                    span: stmt.span,
                    message: "const function binding requires an initializer".to_string(),
                });
            };
            let (name, node_key) =
                single_pattern_binding(&binding.pattern).ok_or_else(|| ConstLowerError {
                    span: binding.pattern.span,
                    message: "const function binding requires a single binding pattern".to_string(),
                })?;
            EarlyConstStmtKind::Binding(EarlyConstBinding {
                span: stmt.span,
                name: *name,
                local_id: lower_local_id(context, node_key, binding.pattern.span)?,
                explicit_type: binding
                    .ty
                    .as_ref()
                    .map(|ty| lower_type_id(context, &ty.node_key, ty.span))
                    .transpose()?
                    .flatten(),
                is_mutable: binding.is_mutable(),
                value: lower_expr_internal(value, context)?,
            })
        }
        nia_ast::StmtKind::Static(_) => {
            return Err(ConstLowerError {
                span: stmt.span,
                message: "static declarations are not supported in const function bodies"
                    .to_string(),
            });
        }
        nia_ast::StmtKind::Expr(expr) => lower_expr_stmt_with_context(expr, context)?,
        nia_ast::StmtKind::Return(value) => EarlyConstStmtKind::Return(
            value
                .as_ref()
                .map(|value| lower_expr_internal(value, context))
                .transpose()?,
        ),
        nia_ast::StmtKind::Break => EarlyConstStmtKind::Break,
        nia_ast::StmtKind::Continue => EarlyConstStmtKind::Continue,
        nia_ast::StmtKind::ForIn(for_in) => EarlyConstStmtKind::ForIn(Box::new(EarlyConstForIn {
            pattern: lower_pattern_with_context(&for_in.pattern, context)?,
            iter: lower_expr_internal(&for_in.iter, context)?,
            body: lower_block_with_context(&for_in.body, context)?,
        })),
        nia_ast::StmtKind::While(while_stmt) => EarlyConstStmtKind::While {
            cond: lower_expr_internal(&while_stmt.cond, context)?,
            body: lower_block_with_context(&while_stmt.body, context)?,
        },
        nia_ast::StmtKind::Loop(loop_stmt) => EarlyConstStmtKind::Loop {
            body: lower_block_with_context(&loop_stmt.body, context)?,
        },
        _ => {
            return Err(ConstLowerError {
                span: stmt.span,
                message: "unsupported statement in const function body".to_string(),
            });
        }
    };
    Ok(EarlyConstStmt {
        span: stmt.span,
        kind,
    })
}

fn lower_expr_stmt_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstStmtKind, ConstLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => Ok(EarlyConstStmtKind::If {
            cond: lower_expr_internal(cond, context)?,
            then_branch: lower_block_with_context(then_branch, context)?,
            else_branch: else_branch
                .as_deref()
                .map(|else_branch| lower_if_stmt_else_branch_with_context(else_branch, context))
                .transpose()?,
        }),
        _ => Ok(EarlyConstStmtKind::Expr(lower_expr_internal(
            expr, context,
        )?)),
    }
}

fn lower_if_stmt_else_branch_with_context(
    expr: &nia_ast::Expr,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstBlock, ConstLowerError> {
    match &expr.kind {
        nia_ast::ExprKind::Block(block) => lower_block_with_context(block, context),
        nia_ast::ExprKind::If { .. } => Ok(EarlyConstBlock {
            span: expr.span,
            stmts: vec![EarlyConstStmt {
                span: expr.span,
                kind: lower_expr_stmt_with_context(expr, context)?,
            }],
            tail: None,
        }),
        _ => Ok(EarlyConstBlock {
            span: expr.span,
            stmts: Vec::new(),
            tail: Some(Box::new(lower_expr_internal(expr, context)?)),
        }),
    }
}

fn lower_switch_with_context(
    span: Span,
    switch: &nia_ast::SwitchStmt,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstSwitch, ConstLowerError> {
    Ok(EarlyConstSwitch {
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
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstSwitch, ConstLowerError> {
    let mut arms = vec![EarlyConstSwitchArm {
        span: if_pattern.then_branch.span,
        patterns: vec![lower_pattern_with_context(&if_pattern.pattern, context)?],
        body: EarlyConstSwitchArmBody::Block(lower_block_with_context(
            &if_pattern.then_branch,
            context,
        )?),
    }];
    if let Some(else_branch) = &if_pattern.else_branch {
        arms.push(EarlyConstSwitchArm {
            span: else_branch.span,
            patterns: vec![EarlyConstPattern::Wildcard {
                span: else_branch.span,
            }],
            body: EarlyConstSwitchArmBody::Expr(lower_expr_internal(else_branch, context)?),
        });
    }
    Ok(EarlyConstSwitch {
        span,
        target: lower_expr_internal(&if_pattern.target, context)?,
        arms,
    })
}

fn lower_switch_arm_with_context(
    arm: &nia_ast::SwitchArm,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstSwitchArm, ConstLowerError> {
    Ok(EarlyConstSwitchArm {
        span: arm.span,
        patterns: arm
            .patterns
            .iter()
            .map(|pattern| lower_pattern_with_context(pattern, context))
            .collect::<Result<Vec<_>, _>>()?,
        body: lower_switch_arm_body_with_context(&arm.body, context)?,
    })
}

fn lower_pattern_with_context(
    pattern: &nia_ast::Pattern,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstPattern, ConstLowerError> {
    match &pattern.kind {
        nia_ast::PatternKind::Wildcard => Ok(EarlyConstPattern::Wildcard { span: pattern.span }),
        nia_ast::PatternKind::Bind { name, node_key, .. } => Ok(EarlyConstPattern::Bind {
            name: *name,
            local_id: lower_local_id(context, node_key, pattern.span)?,
            span: pattern.span,
        }),
        nia_ast::PatternKind::Pointer(inner) => Ok(EarlyConstPattern::Pointer {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::MutPointer(inner) => Ok(EarlyConstPattern::MutPointer {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::OptionalSome(inner) => Ok(EarlyConstPattern::OptionalSome {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::OptionalNull => {
            Ok(EarlyConstPattern::OptionalNull { span: pattern.span })
        }
        nia_ast::PatternKind::ErrorOk(inner) => Ok(EarlyConstPattern::ErrorOk {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::ErrorErr(inner) => Ok(EarlyConstPattern::ErrorErr {
            pattern: Box::new(lower_pattern_with_context(inner, context)?),
            span: pattern.span,
        }),
        nia_ast::PatternKind::EnumVariant { variant, fields } => {
            Ok(EarlyConstPattern::EnumVariant {
                variant: lower_expr_internal(variant, context)?,
                fields: match fields {
                    nia_ast::EnumVariantPatternFields::Tuple(fields) => {
                        ConstEnumPatternFields::Tuple(
                            fields
                                .iter()
                                .map(|field| lower_pattern_with_context(field, context))
                                .collect::<Result<Vec<_>, _>>()?,
                        )
                    }
                    nia_ast::EnumVariantPatternFields::Named(fields) => {
                        ConstEnumPatternFields::Named(
                            fields
                                .iter()
                                .map(|field| {
                                    Ok(ConstNamedPatternField {
                                        name: field.name,
                                        pattern: lower_pattern_with_context(
                                            &field.pattern,
                                            context,
                                        )?,
                                        span: field.span,
                                    })
                                })
                                .collect::<Result<Vec<_>, ConstLowerError>>()?,
                        )
                    }
                },
                span: pattern.span,
            })
        }
        nia_ast::PatternKind::Expr(expr) => {
            lower_expr_internal(expr, context).map(EarlyConstPattern::Expr)
        }
        nia_ast::PatternKind::Range {
            start,
            end,
            inclusive,
        } => Ok(EarlyConstPattern::Range {
            start: lower_expr_internal(start, context)?,
            end: lower_expr_internal(end, context)?,
            inclusive: *inclusive,
            span: pattern.span,
        }),
    }
}

fn single_pattern_binding(
    pattern: &nia_ast::Pattern,
) -> Option<(&SymbolId, &nia_node_id::VersionedNodeKey)> {
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
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstSwitchArmBody, ConstLowerError> {
    match body {
        nia_ast::SwitchArmBody::Expr(expr) => {
            lower_expr_internal(expr, context).map(EarlyConstSwitchArmBody::Expr)
        }
        nia_ast::SwitchArmBody::Stmt(stmt) => lower_stmt_with_context(stmt, context)
            .map(Box::new)
            .map(EarlyConstSwitchArmBody::Stmt),
        nia_ast::SwitchArmBody::Block(block) => {
            lower_block_with_context(block, context).map(EarlyConstSwitchArmBody::Block)
        }
    }
}

fn lower_field_init_with_context(
    field: &nia_ast::FieldInit,
    context: &dyn ConstLowerContext,
) -> Result<EarlyConstFieldInit, ConstLowerError> {
    Ok(EarlyConstFieldInit {
        span: field.span,
        name: field.name,
        value: lower_expr_internal(&field.value, context)?,
    })
}
