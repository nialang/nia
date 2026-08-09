// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{BracketArg, Expr, ExprKind};
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{BuiltinFunction, GlobalDefId, InternedTyId, LayoutBuiltin, TraitId};
use nia_sema_ir::BuiltinValue;
use nia_span::Span;
use nia_symbol::SymbolId;
use nia_symbol_table::SymbolCollision;
use nia_ty::{PrimitiveTy, TyKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckedAtomicOrder {
    Unordered,
    Monotonic,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckedAtomicRmwOp {
    Xchg,
    Add,
    Sub,
    And,
    Nand,
    Or,
    Xor,
    Max,
    Min,
    UMax,
    UMin,
}

#[derive(Clone, Copy)]
pub(super) enum BuiltinCallTypeArgs<'a> {
    Bracket(&'a [BracketArg]),
}

#[derive(Clone, Copy)]
struct CheckedBuiltinTypeArg {
    ty: InternedTyId,
    span: Span,
}

impl<'a> BodyChecker<'a> {
    pub(super) fn check_builtin_function_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        value_node: &Expr,
        builtin: BuiltinFunction,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let name = builtin.name();
        match builtin {
            BuiltinFunction::ConstError => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.reject_builtin_type_arg(builtin_span, name, type_args);
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        call_span,
                        "builtin `error` requires exactly one message argument",
                    ));
                }
                for arg in args {
                    self.check_expr(arg);
                }
                if self.body_filter.checks_const_declarations() {
                    self.never()
                } else {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        call_span,
                        "builtin `error` can only be evaluated at const",
                    ));
                    self.error()
                }
            }
            BuiltinFunction::Trap => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.reject_builtin_type_arg(builtin_span, name, type_args);
                if !args.is_empty() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        call_span,
                        "builtin `trap` does not take value arguments",
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                }
                self.never()
            }
            BuiltinFunction::Embed => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.reject_builtin_type_arg(builtin_span, name, type_args);
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        call_span,
                        "builtin `embed` requires exactly one path argument",
                    ));
                }
                for arg in args {
                    self.check_expr(arg);
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    call_span,
                    "builtin `embed` can only be evaluated at const",
                ));
                self.error()
            }
            BuiltinFunction::SizeOf | BuiltinFunction::AlignOf => {
                if !args.is_empty() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        call_span,
                        format!("builtin `{name}` does not take value arguments"),
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                }
                let Some(type_arg) = self.require_builtin_type_arg(builtin_span, name, type_args)
                else {
                    return self.primitive(PrimitiveTy::Usize);
                };
                self.record_builtin_function_call(
                    call_span,
                    value_node,
                    builtin,
                    Some(type_arg.ty),
                );
                let layout_builtin = match builtin {
                    BuiltinFunction::SizeOf => LayoutBuiltin::Size,
                    BuiltinFunction::AlignOf => LayoutBuiltin::Align,
                    _ => unreachable!(),
                };
                self.require_sized_type(type_arg.span, type_arg.ty, name);
                if let Some(layout) = self.layout_of(type_arg.ty) {
                    self.record_builtin_node_value(
                        value_node,
                        BuiltinValue::Usize(layout.builtin_value(layout_builtin)),
                    );
                } else {
                    self.record_builtin_node_value(
                        value_node,
                        BuiltinValue::Layout {
                            builtin: layout_builtin,
                            ty: type_arg.ty,
                        },
                    );
                }
                self.primitive(PrimitiveTy::Usize)
            }
            BuiltinFunction::Offset => self.check_offset_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::Asm => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.reject_builtin_type_arg(builtin_span, name, type_args);
                self.check_asm_builtin_call(call_span, builtin_span, args)
            }
            BuiltinFunction::MemCopy => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.check_memory_copy_builtin_call(call_span, builtin_span, name, type_args, args)
            }
            BuiltinFunction::MemMove => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.check_memory_copy_builtin_call(call_span, builtin_span, name, type_args, args)
            }
            BuiltinFunction::MemSet => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.check_memory_set_builtin_call(call_span, builtin_span, name, type_args, args)
            }
            BuiltinFunction::LoadUnaligned => self.check_load_unaligned_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::Splat => self.check_splat_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::Extract => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.check_extract_builtin_call(call_span, builtin_span, name, type_args, args)
            }
            BuiltinFunction::Insert => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.check_insert_builtin_call(call_span, builtin_span, name, type_args, args)
            }
            BuiltinFunction::Bitmask => {
                self.record_builtin_function_call(call_span, value_node, builtin, None);
                self.check_bitmask_builtin_call(call_span, builtin_span, name, type_args, args)
            }
            BuiltinFunction::Ctz | BuiltinFunction::Clz | BuiltinFunction::Popcount => self
                .check_bit_intrinsic_builtin_call(
                    call_span,
                    value_node,
                    builtin_span,
                    name,
                    type_args,
                    args,
                ),
            BuiltinFunction::AtomicLoad => self.check_atomic_load_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::AtomicStore => self.check_atomic_store_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::AtomicRmw => self.check_atomic_rmw_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::CmpxchgStrong | BuiltinFunction::CmpxchgWeak => self
                .check_cmpxchg_builtin_call(
                    call_span,
                    value_node,
                    builtin_span,
                    name,
                    type_args,
                    args,
                ),
            BuiltinFunction::Fence => self.check_fence_builtin_call(
                call_span,
                builtin_span,
                value_node,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::CharFromU32 => self.check_char_from_u32_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
            BuiltinFunction::SliceLen => self.check_slice_len_builtin_call(
                call_span,
                value_node,
                builtin_span,
                name,
                type_args,
                args,
            ),
        }
    }

    fn check_slice_len_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let explicit_args = match type_args {
            BuiltinCallTypeArgs::Bracket(args) => self.lower_bracket_type_args(args),
        };
        if explicit_args.len() > 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                builtin_span,
                format!("builtin `{name}` accepts at most one type argument"),
            ));
        }
        let mut elem_ty = None;
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly one slice pointer argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
        } else {
            let actual = self.check_expr(&args[0]);
            if let Some(TyKind::Pointer { elem, .. }) =
                self.interner.get(self.normalization.normalize(actual))
                && let Some(TyKind::Slice { elem, .. }) =
                    self.interner.get(self.normalization.normalize(*elem))
            {
                elem_ty = Some(*elem);
            }
            if elem_ty.is_none() {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    args[0].span,
                    format!("builtin `{name}` requires a slice pointer"),
                ));
            }
        }
        if let ([expected], Some(actual)) = (explicit_args.as_slice(), elem_ty) {
            self.expect_type(call_span, *expected, actual, "slice element type");
        }
        self.record_builtin_function_call(
            call_span,
            value_node,
            BuiltinFunction::SliceLen,
            elem_ty,
        );
        self.primitive(PrimitiveTy::Usize)
    }

    fn check_char_from_u32_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        self.record_builtin_function_call(
            call_span,
            value_node,
            BuiltinFunction::CharFromU32,
            None,
        );
        self.reject_builtin_type_arg(builtin_span, name, type_args);
        let u32_ty = self.primitive(PrimitiveTy::U32);
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly one value argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
        } else {
            let actual = self.check_expr_with_expected(&args[0], Some(u32_ty));
            self.expect_expr_type(&args[0], u32_ty, actual, "Unicode scalar value");
        }
        let char_ty = self.primitive(PrimitiveTy::Char);
        self.interner.intern(TyKind::Optional { elem: char_ty })
    }

    fn require_builtin_type_arg(
        &mut self,
        span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
    ) -> Option<CheckedBuiltinTypeArg> {
        match type_args {
            BuiltinCallTypeArgs::Bracket(args) => {
                let lowered = self.lower_bracket_type_args(args);
                if lowered.len() != 1 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!("builtin `{name}` requires exactly one type argument"),
                    ));
                    return None;
                }
                Some(CheckedBuiltinTypeArg {
                    ty: lowered[0],
                    span: args.first().map_or(span, |arg| arg.span),
                })
            }
        }
    }

    fn record_builtin_function_call(
        &mut self,
        span: Span,
        value_node: &Expr,
        builtin: BuiltinFunction,
        type_arg: Option<InternedTyId>,
    ) {
        self.record_resolved_node_call(
            span,
            &value_node.node_key,
            nia_sema_ir::ResolvedCall::BuiltinFunction { builtin, type_arg },
        );
    }

    fn reject_builtin_type_arg(
        &mut self,
        span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
    ) {
        let has_type_arg = match type_args {
            BuiltinCallTypeArgs::Bracket(args) => {
                if args.is_empty() {
                    false
                } else {
                    let _ = self.lower_bracket_type_args(args);
                    true
                }
            }
        };
        if has_type_arg {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("builtin `{name}` does not take a type argument"),
            ));
        }
    }

    fn optional_builtin_type_arg(
        &mut self,
        span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
    ) -> Option<CheckedBuiltinTypeArg> {
        match type_args {
            BuiltinCallTypeArgs::Bracket(args) => {
                if args.is_empty() {
                    return None;
                }
                let lowered = self.lower_bracket_type_args(args);
                if lowered.len() != 1 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        span,
                        format!("builtin `{name}` takes at most one type argument"),
                    ));
                    return None;
                }
                Some(CheckedBuiltinTypeArg {
                    ty: lowered[0],
                    span: args.first().map_or(span, |arg| arg.span),
                })
            }
        }
    }

    fn check_offset_builtin_call(
        &mut self,
        call_span: Span,
        builtin: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let Some(type_arg) = self.require_builtin_type_arg(builtin_span, name, type_args) else {
            for arg in args {
                self.check_expr(arg);
            }
            return self.primitive(PrimitiveTy::Usize);
        };
        self.record_builtin_function_call(
            call_span,
            builtin,
            BuiltinFunction::Offset,
            Some(type_arg.ty),
        );
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly one field name argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.primitive(PrimitiveTy::Usize);
        }

        let ty = type_arg.ty;
        let Some(field_name) = self.offset_field_name(&args[0]) else {
            self.check_expr(&args[0]);
            return self.primitive(PrimitiveTy::Usize);
        };
        let Some((nominal, field)) = self.offset_field_def(type_arg.span, ty, &field_name) else {
            return self.primitive(PrimitiveTy::Usize);
        };
        if let Some(offset) = self.field_offset_of(ty, nominal, field) {
            self.record_builtin_node_value(builtin, BuiltinValue::Usize(offset));
        } else {
            self.record_builtin_node_value(builtin, BuiltinValue::FieldOffset { ty, field });
        }
        self.primitive(PrimitiveTy::Usize)
    }

    fn offset_field_name(&mut self, arg: &Expr) -> Option<SymbolId> {
        let ExprKind::String(literal) = &arg.kind else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                arg.span,
                "builtin `offset` field name must be a string literal",
            ));
            return None;
        };
        let Some(scalars) = crate::literals::decode_string_literal(literal) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                arg.span,
                "invalid string literal in `offset` field name",
            ));
            return None;
        };
        let mut name = String::new();
        for scalar in scalars {
            let Some(ch) = char::from_u32(scalar) else {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    arg.span,
                    "invalid string scalar in `offset` field name",
                ));
                return None;
            };
            name.push(ch);
        }
        match self.symbols.intern(&name) {
            Ok(symbol) => Some(symbol),
            Err(SymbolCollision {
                symbol,
                existing,
                incoming,
            }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    arg.span,
                    format!(
                        "symbol collision for {}: `{existing}` and `{incoming}`",
                        nia_symbol::symbol_identity_key(symbol)
                    ),
                ));
                None
            }
        }
    }

    fn offset_field_def(
        &mut self,
        span: Span,
        ty: InternedTyId,
        name: &SymbolId,
    ) -> Option<(GlobalDefId, GlobalDefId)> {
        let ty = self.normalization.normalize(ty);
        let Some(TyKind::Nominal { def_id, .. }) = self.interner.get(ty) else {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "builtin `offset` requires a struct or union type argument",
            ));
            return None;
        };
        let Some(field) = self.field_def_for_nominal(*def_id, name) else {
            let name = self.symbol_name(*name);
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                format!("type has no field `{name}` for builtin `offset`"),
            ));
            return None;
        };
        Some((*def_id, field))
    }

    fn field_offset_of(
        &self,
        ty: InternedTyId,
        nominal: GlobalDefId,
        field: GlobalDefId,
    ) -> Option<u64> {
        let ty = self.normalization.normalize(ty);
        let Some(TyKind::Nominal { args, .. }) = self.interner.get(ty) else {
            return None;
        };
        if nominal.module_id == self.defs.module_id {
            return self.layouts.field_offset(nominal, args, field);
        }
        let layouts = (self.program.layouts?)(nominal.module_id)?;
        layouts.field_offset(nominal, args, field)
    }

    fn check_splat_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let Some(type_arg) = self.require_builtin_type_arg(builtin_span, name, type_args) else {
            for arg in args {
                self.check_expr(arg);
            }
            return self.error();
        };
        self.record_builtin_function_call(
            call_span,
            value_node,
            BuiltinFunction::Splat,
            Some(type_arg.ty),
        );
        let vector_ty = type_arg.ty;
        let lane_ty = match self.interner.get(vector_ty).cloned() {
            Some(TyKind::Vector { elem, .. }) => self.primitive(elem),
            Some(_) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    type_arg.span,
                    format!(
                        "builtin `{name}` requires a SIMD vector type, got {}",
                        self.ty_name(vector_ty)
                    ),
                ));
                self.error()
            }
            None => self.error(),
        };
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly one value argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return vector_ty;
        }
        let actual = self.check_expr_with_expected(&args[0], Some(lane_ty));
        self.expect_expr_type(&args[0], lane_ty, actual, "splat builtin argument");
        vector_ty
    }

    fn check_extract_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        self.reject_builtin_type_arg(builtin_span, name, type_args);
        if args.len() != 2 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly two value arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.error();
        }
        let vector_ty = self.check_expr(&args[0]);
        let lane_ty = self.vector_lane_ty(args[0].span, name, vector_ty);
        let index_ty = self.check_expr(&args[1]);
        self.expect_integer(args[1].span, index_ty, "SIMD lane index");
        lane_ty
    }

    fn check_insert_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        self.reject_builtin_type_arg(builtin_span, name, type_args);
        if args.len() != 3 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly three value arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.error();
        }
        let vector_ty = self.check_expr(&args[0]);
        let lane_ty = self.vector_lane_ty(args[0].span, name, vector_ty);
        let index_ty = self.check_expr(&args[1]);
        self.expect_integer(args[1].span, index_ty, "SIMD lane index");
        let value_ty = self.check_expr_with_expected(&args[2], Some(lane_ty));
        self.expect_expr_type(&args[2], lane_ty, value_ty, "SIMD lane value");
        vector_ty
    }

    fn check_bitmask_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        self.reject_builtin_type_arg(builtin_span, name, type_args);
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly one value argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.primitive(PrimitiveTy::Usize);
        }
        let vector_ty = self.check_expr(&args[0]);
        match self.interner.get(vector_ty).cloned() {
            Some(TyKind::Vector {
                elem: PrimitiveTy::Bool,
                lanes,
            }) => {
                if lanes > 64 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        codes::TYPE_CHECK,
                        args[0].span,
                        "builtin `bitmask` supports at most 64 SIMD mask lanes",
                    ));
                }
            }
            Some(TyKind::Vector { .. }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    args[0].span,
                    format!(
                        "builtin `{name}` requires a bool SIMD mask vector, got {}",
                        self.ty_name(vector_ty)
                    ),
                ));
            }
            Some(_) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    args[0].span,
                    format!(
                        "builtin `{name}` requires a SIMD vector argument, got {}",
                        self.ty_name(vector_ty)
                    ),
                ));
            }
            None => {}
        }
        self.primitive(PrimitiveTy::Usize)
    }

    fn check_load_unaligned_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let Some(type_arg) = self.require_builtin_type_arg(builtin_span, name, type_args) else {
            for arg in args {
                self.check_expr(arg);
            }
            return self.error();
        };
        let ty = type_arg.ty;
        self.record_builtin_function_call(
            call_span,
            value_node,
            BuiltinFunction::LoadUnaligned,
            Some(ty),
        );
        self.require_sized_type(type_arg.span, ty, name);
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly one pointer argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return ty;
        }
        let ptr_ty = self.check_expr(&args[0]);
        let u8_ty = self.primitive(PrimitiveTy::U8);
        match self.interner.get(ptr_ty).cloned() {
            Some(TyKind::Pointer { elem, .. }) if self.types_match(elem, u8_ty) => {}
            Some(_) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    args[0].span,
                    format!(
                        "builtin `{name}` requires a byte pointer argument, got {}",
                        self.ty_name(ptr_ty)
                    ),
                ));
            }
            None => {}
        }
        ty
    }

    fn check_bit_intrinsic_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let result_ty = self.check_integer_builtin_type_arg(builtin_span, name, type_args);
        let builtin = BuiltinFunction::from_name(name).unwrap_or(BuiltinFunction::Ctz);
        self.record_builtin_function_call(call_span, value_node, builtin, Some(result_ty));
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly one value argument"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return result_ty;
        }
        let actual = self.check_expr_with_expected(&args[0], Some(result_ty));
        self.expect_expr_type(&args[0], result_ty, actual, "bit intrinsic argument");
        result_ty
    }

    fn check_integer_builtin_type_arg(
        &mut self,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
    ) -> InternedTyId {
        let Some(type_arg) = self.require_builtin_type_arg(builtin_span, name, type_args) else {
            return self.error();
        };
        let ty = type_arg.ty;
        match self.interner.get(ty).cloned() {
            Some(TyKind::Primitive(primitive)) if primitive.is_integer() => ty,
            Some(TyKind::Primitive(_)) | Some(_) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    type_arg.span,
                    format!(
                        "builtin `{name}` requires an integer type argument, got {}",
                        self.ty_name(ty)
                    ),
                ));
                self.error()
            }
            None => self.error(),
        }
    }

    fn vector_lane_ty(&mut self, span: Span, name: &str, vector_ty: InternedTyId) -> InternedTyId {
        match self.interner.get(vector_ty).cloned() {
            Some(TyKind::Vector { elem, .. }) => self.primitive(elem),
            Some(_) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    span,
                    format!(
                        "builtin `{name}` requires a SIMD vector argument, got {}",
                        self.ty_name(vector_ty)
                    ),
                ));
                self.error()
            }
            None => self.error(),
        }
    }

    fn require_sized_type(&mut self, span: Span, ty: InternedTyId, builtin_name: &str) {
        if self.current_context_proves_trait_obligation(
            ty,
            TraitId::Builtin(nia_ty::BuiltinTrait::Sized),
            Vec::new(),
        ) {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            codes::TYPE_CHECK,
            span,
            format!(
                "builtin `{builtin_name}` requires {}: Sized",
                self.ty_name(ty)
            ),
        ));
    }

    fn check_memory_copy_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let explicit_elem_ty = self.optional_builtin_type_arg(builtin_span, name, type_args);
        if args.len() != 2 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly two arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.unit();
        }

        let dest_actual = self.check_expr(&args[0]);
        let Some((mut elem_ty, dest_expected)) =
            self.memory_dest_slice_expected(&args[0], dest_actual)
        else {
            let elem = explicit_elem_ty.map_or_else(|| self.error(), |arg| arg.ty);
            let src_expected = self.interner.intern(TyKind::Slice {
                is_readonly: true,
                elem,
            });
            self.check_expr_with_expected(&args[1], Some(src_expected));
            return self.unit();
        };
        self.expect_expr_type(
            &args[0],
            dest_expected,
            dest_actual,
            "memory intrinsic destination",
        );
        if let Some(type_arg) = explicit_elem_ty {
            self.expect_type(
                type_arg.span,
                type_arg.ty,
                elem_ty,
                "memory intrinsic element type argument",
            );
            elem_ty = type_arg.ty;
        }
        self.require_sized_type(args[0].span, elem_ty, name);

        let src_expected = self.interner.intern(TyKind::Slice {
            is_readonly: true,
            elem: elem_ty,
        });
        let src_actual = self.check_expr_with_expected(&args[1], Some(src_expected));
        self.expect_expr_type(
            &args[1],
            src_expected,
            src_actual,
            "memory intrinsic source",
        );
        self.unit()
    }

    fn check_memory_set_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        self.reject_builtin_type_arg(builtin_span, name, type_args);
        if args.len() != 2 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                "builtin `memset` requires exactly two arguments",
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.unit();
        }

        let u8_ty = self.primitive(PrimitiveTy::U8);
        let dest_actual = self.check_expr(&args[0]);
        let Some((elem_ty, dest_expected)) = self.memory_dest_slice_expected(&args[0], dest_actual)
        else {
            self.check_expr_with_expected(&args[1], Some(u8_ty));
            return self.unit();
        };
        self.expect_expr_type(
            &args[0],
            dest_expected,
            dest_actual,
            "memory intrinsic destination",
        );
        self.expect_type(
            args[0].span,
            u8_ty,
            elem_ty,
            "memory intrinsic destination element",
        );

        let value_actual = self.check_expr_with_expected(&args[1], Some(u8_ty));
        self.expect_expr_type(&args[1], u8_ty, value_actual, "memory intrinsic byte value");
        self.unit()
    }

    fn memory_dest_slice_expected(
        &mut self,
        expr: &Expr,
        actual: InternedTyId,
    ) -> Option<(InternedTyId, InternedTyId)> {
        let actual = self.normalization.normalize(actual);
        match self.interner.get(actual).cloned() {
            Some(TyKind::Slice {
                is_readonly: false,
                elem,
            }) => Some((elem, actual)),
            Some(TyKind::Slice {
                is_readonly: true, ..
            }) => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "memory intrinsic destination must be mutable",
                ));
                None
            }
            Some(TyKind::Array { elem, .. }) => {
                let expected = self.interner.intern(TyKind::Slice {
                    is_readonly: false,
                    elem,
                });
                Some((elem, expected))
            }
            Some(TyKind::Error) => None,
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    "memory intrinsic destination must be `&mut [T]`",
                ));
                None
            }
        }
    }

    fn check_atomic_load_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let ty = self.atomic_type_arg(builtin_span, name, type_args);
        self.record_builtin_function_call(
            call_span,
            value_node,
            BuiltinFunction::AtomicLoad,
            Some(ty),
        );
        if args.len() != 2 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly two arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return ty;
        }
        self.check_atomic_value_type(builtin_span, name, ty);
        self.check_atomic_ptr_arg(&args[0], ty, true, name);
        self.check_atomic_order_arg(&args[1], name, AtomicOrderContext::Load);
        ty
    }

    fn check_atomic_store_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let ty = self.atomic_type_arg(builtin_span, name, type_args);
        self.record_builtin_function_call(
            call_span,
            value_node,
            BuiltinFunction::AtomicStore,
            Some(ty),
        );
        if args.len() != 3 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly three arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.unit();
        }
        self.check_atomic_value_type(builtin_span, name, ty);
        self.check_atomic_ptr_arg(&args[0], ty, false, name);
        let value_actual = self.check_expr_with_expected(&args[1], Some(ty));
        self.expect_expr_type(&args[1], ty, value_actual, "atomic store value");
        self.check_atomic_order_arg(&args[2], name, AtomicOrderContext::Store);
        self.unit()
    }

    fn check_atomic_rmw_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let ty = self.atomic_type_arg(builtin_span, name, type_args);
        self.record_builtin_function_call(
            call_span,
            value_node,
            BuiltinFunction::AtomicRmw,
            Some(ty),
        );
        if args.len() != 4 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly four arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return ty;
        }
        self.check_atomic_value_type(builtin_span, name, ty);
        self.check_atomic_ptr_arg(&args[0], ty, false, name);
        self.check_atomic_rmw_op_arg(&args[1], name, ty);
        let value_actual = self.check_expr_with_expected(&args[2], Some(ty));
        self.expect_expr_type(&args[2], ty, value_actual, "atomic read-modify-write value");
        self.check_atomic_order_arg(&args[3], name, AtomicOrderContext::Rmw);
        ty
    }

    fn check_cmpxchg_builtin_call(
        &mut self,
        call_span: Span,
        value_node: &Expr,
        builtin_span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        let ty = self.atomic_type_arg(builtin_span, name, type_args);
        let builtin = BuiltinFunction::from_name(name).unwrap_or(BuiltinFunction::CmpxchgStrong);
        self.record_builtin_function_call(call_span, value_node, builtin, Some(ty));
        let optional_ty = self.interner.intern(TyKind::Optional { elem: ty });
        if args.len() != 5 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                format!("builtin `{name}` requires exactly five arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return optional_ty;
        }
        self.check_atomic_value_type(builtin_span, name, ty);
        self.check_atomic_ptr_arg(&args[0], ty, false, name);
        let expected_actual = self.check_expr_with_expected(&args[1], Some(ty));
        self.expect_expr_type(&args[1], ty, expected_actual, "cmpxchg expected value");
        let desired_actual = self.check_expr_with_expected(&args[2], Some(ty));
        self.expect_expr_type(&args[2], ty, desired_actual, "cmpxchg desired value");
        let success =
            self.check_atomic_order_arg(&args[3], name, AtomicOrderContext::CmpxchgSuccess);
        let failure =
            self.check_atomic_order_arg(&args[4], name, AtomicOrderContext::CmpxchgFailure);
        if let (Some(success), Some(failure)) = (success, failure) {
            self.check_cmpxchg_order_pair(args[4].span, success, failure);
        }
        optional_ty
    }

    fn check_fence_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        value_node: &Expr,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
        args: &[Expr],
    ) -> InternedTyId {
        self.record_builtin_function_call(call_span, value_node, BuiltinFunction::Fence, None);
        self.reject_builtin_type_arg(builtin_span, name, type_args);
        if args.len() != 1 {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                call_span,
                "builtin `fence` requires exactly one argument",
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.unit();
        }
        self.check_atomic_order_arg(&args[0], name, AtomicOrderContext::Fence);
        self.unit()
    }

    fn atomic_type_arg(
        &mut self,
        span: Span,
        name: &str,
        type_args: BuiltinCallTypeArgs<'_>,
    ) -> InternedTyId {
        let Some(type_arg) = self.require_builtin_type_arg(span, name, type_args) else {
            return self.error();
        };
        type_arg.ty
    }

    fn check_atomic_ptr_arg(
        &mut self,
        expr: &Expr,
        ty: InternedTyId,
        allow_readonly: bool,
        name: &str,
    ) {
        let expected = self.interner.intern(TyKind::Pointer {
            is_readonly: allow_readonly,
            elem: ty,
        });
        let actual = self.check_expr_with_expected(expr, Some(expected));
        let actual = self.normalization.normalize(actual);
        match self.interner.get(actual) {
            Some(TyKind::Pointer { is_readonly, elem })
                if *elem == ty && (allow_readonly || !*is_readonly) => {}
            Some(TyKind::Pointer {
                is_readonly: true, ..
            }) if !allow_readonly => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!("builtin `{name}` pointer argument must be mutable"),
                ));
            }
            Some(TyKind::Error) => {}
            _ => {
                self.expect_expr_type(expr, expected, actual, "atomic pointer argument");
            }
        }
    }

    fn check_atomic_value_type(&mut self, span: Span, name: &str, ty: InternedTyId) {
        let ty = self.normalization.normalize(ty);
        if matches!(self.interner.get(ty), Some(TyKind::GenericParam(_))) {
            return;
        }
        if self.atomic_value_bits(ty).is_some() {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(codes::TYPE_CHECK,
            span,
            format!(
                "builtin `{name}` supports only bool, integer, enum, and pointer types up to the native pointer width"
            ),
        ));
    }

    fn atomic_value_bits(&mut self, ty: InternedTyId) -> Option<u32> {
        match self.interner.get(ty)? {
            TyKind::Primitive(primitive) => match primitive {
                PrimitiveTy::Bool => Some(1),
                PrimitiveTy::I8 | PrimitiveTy::U8 => Some(8),
                PrimitiveTy::I16 | PrimitiveTy::U16 => Some(16),
                PrimitiveTy::I32 | PrimitiveTy::U32 | PrimitiveTy::Char => Some(32),
                PrimitiveTy::I64 | PrimitiveTy::U64 | PrimitiveTy::Isize | PrimitiveTy::Usize => {
                    Some(64)
                }
                PrimitiveTy::I128
                | PrimitiveTy::U128
                | PrimitiveTy::F32
                | PrimitiveTy::F64
                | PrimitiveTy::Never => None,
            },
            TyKind::Pointer { .. } => Some(self.target.pointer_width),
            TyKind::GenericParam(_) => Some(self.target.pointer_width),
            TyKind::Nominal { .. } if self.is_enum(ty) => {
                let enum_id = self.enum_global_def_id(ty)?;
                let backing_type = self
                    .resolved_enum_signature(enum_id)
                    .map(|resolved| resolved.signature.backing_type)?;
                self.atomic_value_bits(self.normalization.normalize(backing_type))
            }
            _ => None,
        }
        .filter(|bits| *bits <= self.target.pointer_width)
    }

    fn check_atomic_order_arg(
        &mut self,
        expr: &Expr,
        name: &str,
        context: AtomicOrderContext,
    ) -> Option<CheckedAtomicOrder> {
        self.check_expr(expr);
        let value = self.const_int_arg(expr, "atomic ordering")?;
        let order = match value {
            0 => CheckedAtomicOrder::Unordered,
            1 => CheckedAtomicOrder::Monotonic,
            2 => CheckedAtomicOrder::Acquire,
            3 => CheckedAtomicOrder::Release,
            4 => CheckedAtomicOrder::AcqRel,
            5 => CheckedAtomicOrder::SeqCst,
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!("invalid atomic ordering `{value}` for builtin `{name}`"),
                ));
                return None;
            }
        };
        if !context.allows(order) {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "atomic ordering `{}` is invalid for {}",
                    order.name(),
                    context.name()
                ),
            ));
            return None;
        }
        Some(order)
    }

    fn check_atomic_rmw_op_arg(
        &mut self,
        expr: &Expr,
        name: &str,
        ty: InternedTyId,
    ) -> Option<CheckedAtomicRmwOp> {
        self.check_expr(expr);
        let value = self.const_int_arg(expr, "atomic read-modify-write operation")?;
        let op = match value {
            0 => CheckedAtomicRmwOp::Xchg,
            1 => CheckedAtomicRmwOp::Add,
            2 => CheckedAtomicRmwOp::Sub,
            3 => CheckedAtomicRmwOp::And,
            4 => CheckedAtomicRmwOp::Nand,
            5 => CheckedAtomicRmwOp::Or,
            6 => CheckedAtomicRmwOp::Xor,
            7 => CheckedAtomicRmwOp::Max,
            8 => CheckedAtomicRmwOp::Min,
            9 => CheckedAtomicRmwOp::UMax,
            10 => CheckedAtomicRmwOp::UMin,
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!("invalid atomic RMW operation `{value}` for builtin `{name}`"),
                ));
                return None;
            }
        };
        if matches!(
            op,
            CheckedAtomicRmwOp::Add
                | CheckedAtomicRmwOp::Sub
                | CheckedAtomicRmwOp::And
                | CheckedAtomicRmwOp::Nand
                | CheckedAtomicRmwOp::Or
                | CheckedAtomicRmwOp::Xor
                | CheckedAtomicRmwOp::Max
                | CheckedAtomicRmwOp::Min
                | CheckedAtomicRmwOp::UMax
                | CheckedAtomicRmwOp::UMin
        ) && !self.atomic_rmw_integer_like(ty)
        {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                expr.span,
                format!(
                    "atomic RMW operation `{}` requires an integer, bool, or enum type",
                    op.name()
                ),
            ));
            return None;
        }
        Some(op)
    }

    fn atomic_rmw_integer_like(&self, ty: InternedTyId) -> bool {
        match self.interner.get(self.normalization.normalize(ty)) {
            Some(TyKind::GenericParam(_)) => true,
            Some(TyKind::Primitive(
                PrimitiveTy::Bool
                | PrimitiveTy::I8
                | PrimitiveTy::I16
                | PrimitiveTy::I32
                | PrimitiveTy::I64
                | PrimitiveTy::Isize
                | PrimitiveTy::U8
                | PrimitiveTy::U16
                | PrimitiveTy::U32
                | PrimitiveTy::U64
                | PrimitiveTy::Usize
                | PrimitiveTy::Char,
            )) => true,
            Some(TyKind::Nominal { .. }) => self.is_enum(ty),
            _ => false,
        }
    }

    fn check_cmpxchg_order_pair(
        &mut self,
        span: Span,
        success: CheckedAtomicOrder,
        failure: CheckedAtomicOrder,
    ) {
        if failure.strength() > success.strength() {
            self.diagnostics.push(Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                span,
                "cmpxchg failure ordering cannot be stronger than success ordering",
            ));
        }
    }

    fn const_int_arg(&mut self, expr: &Expr, context: &str) -> Option<i128> {
        let value = self
            .with_const_context(|this| {
                let expr =
                    this.lower_const_expr(expr)
                        .map_err(|err| nia_const_eval::ConstError {
                            span: err.span,
                            message: err.message,
                        })?;
                nia_const_eval::eval_resolved_const_expr(&expr, this)
            })
            .ok();
        match value {
            Some(nia_const_eval::ConstValue::Int(value)) => value.as_i128(),
            _ => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    codes::TYPE_CHECK,
                    expr.span,
                    format!("{context} must be a compile-time integer constant"),
                ));
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomicOrderContext {
    Load,
    Store,
    Rmw,
    CmpxchgSuccess,
    CmpxchgFailure,
    Fence,
}

impl AtomicOrderContext {
    fn name(self) -> &'static str {
        match self {
            Self::Load => "atomic load",
            Self::Store => "atomic store",
            Self::Rmw => "atomic read-modify-write",
            Self::CmpxchgSuccess => "cmpxchg success",
            Self::CmpxchgFailure => "cmpxchg failure",
            Self::Fence => "atomic fence",
        }
    }

    fn allows(self, order: CheckedAtomicOrder) -> bool {
        match self {
            Self::Load => matches!(
                order,
                CheckedAtomicOrder::Unordered
                    | CheckedAtomicOrder::Monotonic
                    | CheckedAtomicOrder::Acquire
                    | CheckedAtomicOrder::SeqCst
            ),
            Self::Store => matches!(
                order,
                CheckedAtomicOrder::Unordered
                    | CheckedAtomicOrder::Monotonic
                    | CheckedAtomicOrder::Release
                    | CheckedAtomicOrder::SeqCst
            ),
            Self::Rmw | Self::CmpxchgSuccess => matches!(
                order,
                CheckedAtomicOrder::Monotonic
                    | CheckedAtomicOrder::Acquire
                    | CheckedAtomicOrder::Release
                    | CheckedAtomicOrder::AcqRel
                    | CheckedAtomicOrder::SeqCst
            ),
            Self::CmpxchgFailure => matches!(
                order,
                CheckedAtomicOrder::Monotonic
                    | CheckedAtomicOrder::Acquire
                    | CheckedAtomicOrder::SeqCst
            ),
            Self::Fence => matches!(
                order,
                CheckedAtomicOrder::Acquire
                    | CheckedAtomicOrder::Release
                    | CheckedAtomicOrder::AcqRel
                    | CheckedAtomicOrder::SeqCst
            ),
        }
    }
}

impl CheckedAtomicOrder {
    fn name(self) -> &'static str {
        match self {
            Self::Unordered => "Unordered",
            Self::Monotonic => "Monotonic",
            Self::Acquire => "Acquire",
            Self::Release => "Release",
            Self::AcqRel => "AcqRel",
            Self::SeqCst => "SeqCst",
        }
    }

    fn strength(self) -> u8 {
        match self {
            Self::Unordered => 0,
            Self::Monotonic => 1,
            Self::Acquire | Self::Release => 2,
            Self::AcqRel => 3,
            Self::SeqCst => 4,
        }
    }
}

impl CheckedAtomicRmwOp {
    fn name(self) -> &'static str {
        match self {
            Self::Xchg => "Xchg",
            Self::Add => "Add",
            Self::Sub => "Sub",
            Self::And => "And",
            Self::Nand => "Nand",
            Self::Or => "Or",
            Self::Xor => "Xor",
            Self::Max => "Max",
            Self::Min => "Min",
            Self::UMax => "UMax",
            Self::UMin => "UMin",
        }
    }
}
