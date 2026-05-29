// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::{Expr, TypeRef};
use nia_body_ir::BuiltinValue;
use nia_diagnostic::Diagnostic;
use nia_ids::InternedTyId;
use nia_span::Span;
use nia_ty::{PrimitiveTy, TyKind};
use nia_value_resolve::BuiltinResolution;

impl<'a> BodyChecker<'a> {
    pub(crate) fn check_builtin(
        &mut self,
        span: Span,
        name: &str,
        type_arg: &Option<TypeRef>,
    ) -> InternedTyId {
        let Some(resolution) = self.values.builtins.get(&span).copied() else {
            return self.error();
        };
        let Some(type_arg) = type_arg else {
            self.diagnostics.push(Diagnostic::error(
                span,
                format!("builtin `@{name}` requires a type argument"),
            ));
            return self.primitive(PrimitiveTy::Usize);
        };
        let ty = self.ty_for_span(type_arg.span);
        let Some(layout) = self.layout_of(ty) else {
            self.diagnostics.push(Diagnostic::error(
                type_arg.span,
                format!("cannot compute layout for builtin `@{name}`"),
            ));
            return self.primitive(PrimitiveTy::Usize);
        };
        let value = match resolution {
            BuiltinResolution::SizeOf => layout.size,
            BuiltinResolution::AlignOf => layout.align,
            BuiltinResolution::Len | BuiltinResolution::Ptr | BuiltinResolution::Asm => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("builtin `@{name}` must be called with value arguments"),
                ));
                return self.error();
            }
            BuiltinResolution::Reserved => return self.error(),
        };
        self.builtin_values.insert(span, BuiltinValue::Usize(value));
        self.primitive(PrimitiveTy::Usize)
    }

    pub(super) fn check_builtin_call(
        &mut self,
        call_span: Span,
        builtin_span: Span,
        name: &str,
        type_arg: &Option<TypeRef>,
        args: &[Expr],
    ) -> InternedTyId {
        let Some(resolution) = self.values.builtins.get(&builtin_span).copied() else {
            return self.error();
        };
        match resolution {
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
                self.check_builtin(builtin_span, name, type_arg)
            }
            BuiltinResolution::Len => {
                if type_arg.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        builtin_span,
                        "builtin `@len` does not take a type argument",
                    ));
                }
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::error(
                        call_span,
                        "builtin `@len` requires exactly one argument",
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                    return self.primitive(PrimitiveTy::Usize);
                }
                let arg_ty = self.check_expr(&args[0]);
                match self.interner.get(arg_ty) {
                    Some(TyKind::Array { .. } | TyKind::Slice { .. } | TyKind::Error) => {}
                    _ => self.diagnostics.push(Diagnostic::error(
                        args[0].span,
                        "builtin `@len` requires an array or slice",
                    )),
                }
                self.primitive(PrimitiveTy::Usize)
            }
            BuiltinResolution::Ptr => {
                if type_arg.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        builtin_span,
                        "builtin `@ptr` does not take a type argument",
                    ));
                }
                if args.len() != 1 {
                    self.diagnostics.push(Diagnostic::error(
                        call_span,
                        "builtin `@ptr` requires exactly one argument",
                    ));
                    for arg in args {
                        self.check_expr(arg);
                    }
                    return self.error();
                }
                let arg_ty = self.check_expr(&args[0]);
                match self.interner.get(arg_ty) {
                    Some(TyKind::Slice { is_const, elem }) => {
                        self.interner.intern(TyKind::Pointer {
                            is_const: *is_const,
                            elem: *elem,
                        })
                    }
                    Some(TyKind::Error) | None => self.error(),
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            args[0].span,
                            "builtin `@ptr` requires a slice",
                        ));
                        self.error()
                    }
                }
            }
            BuiltinResolution::Asm => self.check_asm_builtin_call(call_span, builtin_span, args),
            BuiltinResolution::Reserved => self.error(),
        }
    }
}
