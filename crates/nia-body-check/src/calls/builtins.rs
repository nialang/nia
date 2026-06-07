// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{Expr, TypeRef};
use nia_diagnostic::Diagnostic;
use nia_ids::{InternedTyId, LayoutBuiltin, TraitId};
use nia_sema_ir::BuiltinValue;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};
use nia_value_resolve::BuiltinResolution;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_builtin(
        &mut self,
        expr: &Expr,
        name: &str,
        type_arg: &Option<TypeRef>,
    ) -> InternedTyId {
        let span = expr.span;
        let Some(resolution) = self.builtin_resolution(expr) else {
            return self.error();
        };
        if matches!(resolution, BuiltinResolution::Builtin) {
            return self.interner.intern(TyKind::ComptimeOnly);
        }
        if matches!(resolution, BuiltinResolution::ComptimeError) {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                span,
                "builtin `@error` must be called with a message",
            ));
            return self.error();
        }
        if matches!(resolution, BuiltinResolution::Trap) {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                span,
                "builtin `@trap` must be called",
            ));
            return self.error();
        }
        let Some(type_arg) = type_arg else {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                span,
                format!("builtin `@{name}` requires a type argument"),
            ));
            return self.primitive(PrimitiveTy::Usize);
        };
        let ty = self.ty_for_type(type_arg);
        let builtin = match resolution {
            BuiltinResolution::Builtin
            | BuiltinResolution::ComptimeError
            | BuiltinResolution::Trap => return self.error(),
            BuiltinResolution::SizeOf => {
                self.require_sized_type(type_arg.span, ty, name);
                LayoutBuiltin::Size
            }
            BuiltinResolution::AlignOf => {
                self.require_sized_type(type_arg.span, ty, name);
                LayoutBuiltin::Align
            }
            BuiltinResolution::Asm => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    span,
                    format!("builtin `@{name}` must be called with value arguments"),
                ));
                return self.error();
            }
            BuiltinResolution::MemCopy | BuiltinResolution::MemMove | BuiltinResolution::MemSet => {
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    span,
                    format!("builtin `@{name}` must be called with value arguments"),
                ));
                return self.error();
            }
            BuiltinResolution::Reserved => return self.error(),
        };
        if let Some(layout) = self.layout_of(ty) {
            self.record_builtin_node_value(
                expr,
                BuiltinValue::Usize(layout.builtin_value(builtin)),
            );
        } else {
            self.record_builtin_node_value(expr, BuiltinValue::Layout { builtin, ty });
        }
        self.primitive(PrimitiveTy::Usize)
    }

    pub(super) fn check_builtin_call(
        &mut self,
        call_span: Span,
        builtin: &Expr,
        name: &str,
        type_arg: &Option<TypeRef>,
        args: &[Expr],
    ) -> InternedTyId {
        let builtin_span = builtin.span;
        let Some(resolution) = self.builtin_resolution(builtin) else {
            return self.error();
        };
        match resolution {
            BuiltinResolution::Builtin => {
                if !args.is_empty() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0301",
                        call_span,
                        "builtin `@builtin` does not take value arguments",
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                }
                self.interner.intern(TyKind::ComptimeOnly)
            }
            BuiltinResolution::ComptimeError => {
                if type_arg.is_some() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0301",
                        builtin_span,
                        "builtin `@error` does not take a type argument",
                    ));
                }
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0301",
                        call_span,
                        "builtin `@error` requires exactly one message argument",
                    ));
                }
                for arg in args {
                    self.check_expr(arg);
                }
                self.diagnostics.push(Diagnostic::user_error_at(
                    "E0301",
                    call_span,
                    "builtin `@error` can only be evaluated at comptime",
                ));
                self.error()
            }
            BuiltinResolution::Trap => {
                if type_arg.is_some() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0301",
                        builtin_span,
                        "builtin `@trap` does not take a type argument",
                    ));
                }
                if !args.is_empty() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0301",
                        call_span,
                        "builtin `@trap` does not take value arguments",
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                }
                self.never()
            }
            BuiltinResolution::SizeOf | BuiltinResolution::AlignOf => {
                if !args.is_empty() {
                    self.diagnostics.push(Diagnostic::user_error_at(
                        "E0301",
                        call_span,
                        format!("builtin `@{name}` does not take value arguments"),
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                }
                self.check_builtin(builtin, name, type_arg)
            }
            BuiltinResolution::Asm => self.check_asm_builtin_call(call_span, builtin_span, args),
            BuiltinResolution::MemCopy => {
                self.check_memory_copy_builtin_call(call_span, builtin_span, name, type_arg, args)
            }
            BuiltinResolution::MemMove => {
                self.check_memory_copy_builtin_call(call_span, builtin_span, name, type_arg, args)
            }
            BuiltinResolution::MemSet => {
                self.check_memory_set_builtin_call(call_span, builtin_span, name, type_arg, args)
            }
            BuiltinResolution::Reserved => self.error(),
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
            "E0301",
            span,
            format!(
                "builtin `@{builtin_name}` requires {}: Sized",
                self.ty_name(ty)
            ),
        ));
    }

    fn check_memory_copy_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_arg: &Option<TypeRef>,
        args: &[Expr],
    ) -> InternedTyId {
        self.reject_memory_builtin_type_arg(builtin_span, name, type_arg);
        if args.len() != 2 {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                call_span,
                format!("builtin `@{name}` requires exactly two arguments"),
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.void();
        }

        let dest_actual = self.check_expr(&args[0]);
        let Some((elem_ty, dest_expected)) = self.memory_dest_slice_expected(&args[0], dest_actual)
        else {
            let src_expected = self.interner.intern(TyKind::Slice {
                is_readonly: true,
                elem: self.error(),
            });
            self.check_expr_with_expected(&args[1], Some(src_expected));
            return self.void();
        };
        self.expect_expr_type(
            &args[0],
            dest_expected,
            dest_actual,
            "memory intrinsic destination",
        );
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
        self.void()
    }

    fn check_memory_set_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_arg: &Option<TypeRef>,
        args: &[Expr],
    ) -> InternedTyId {
        self.reject_memory_builtin_type_arg(builtin_span, name, type_arg);
        if args.len() != 2 {
            self.diagnostics.push(Diagnostic::user_error_at(
                "E0301",
                call_span,
                "builtin `@memset` requires exactly two arguments",
            ));
            for arg in args {
                self.check_expr(arg);
            }
            return self.void();
        }

        let u8_ty = self.primitive(PrimitiveTy::U8);
        let dest_actual = self.check_expr(&args[0]);
        let Some((elem_ty, dest_expected)) = self.memory_dest_slice_expected(&args[0], dest_actual)
        else {
            self.check_expr_with_expected(&args[1], Some(u8_ty));
            return self.void();
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
        self.void()
    }

    fn reject_memory_builtin_type_arg(
        &mut self,
        builtin_span: Span,
        name: &str,
        type_arg: &Option<TypeRef>,
    ) {
        if type_arg.is_none() {
            return;
        }
        self.diagnostics.push(Diagnostic::user_error_at(
            "E0301",
            builtin_span,
            format!("builtin `@{name}` does not take a type argument"),
        ));
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
                    "E0301",
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
                    "E0301",
                    expr.span,
                    "memory intrinsic destination must be `&mut [T]`",
                ));
                None
            }
        }
    }
}
