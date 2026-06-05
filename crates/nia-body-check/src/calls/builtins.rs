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
        let Some(type_arg) = type_arg else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("builtin `@{name}` requires a type argument"),
            ));
            return self.primitive(PrimitiveTy::Usize);
        };
        let ty = self.ty_for_type(type_arg);
        let builtin = match resolution {
            BuiltinResolution::Builtin => return self.error(),
            BuiltinResolution::SizeOf => {
                self.require_sized_type(type_arg.span, ty, name);
                LayoutBuiltin::Size
            }
            BuiltinResolution::AlignOf => {
                self.require_sized_type(type_arg.span, ty, name);
                LayoutBuiltin::Align
            }
            BuiltinResolution::Asm => {
                self.diagnostics.push(Diagnostic::error(
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
                    self.diagnostics.push(Diagnostic::error(
                        call_span,
                        "builtin `@builtin` does not take value arguments",
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                }
                self.interner.intern(TyKind::ComptimeOnly)
            }
            BuiltinResolution::SizeOf | BuiltinResolution::AlignOf => {
                if !args.is_empty() {
                    self.diagnostics.push(Diagnostic::error(
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
        self.diagnostics.push(Diagnostic::error(
            span,
            format!(
                "builtin `@{builtin_name}` requires {}: Sized",
                self.ty_name(ty)
            ),
        ));
    }
}
