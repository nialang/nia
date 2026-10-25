// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyChecker;
use nia_ast::Expr;
use nia_diagnostic::{Diagnostic, codes};
use nia_ids::{GlobalDefId, InternedTyId};
use nia_sema::{ArityCheck, ArityRequirement, check_call_arity};
use nia_span::Span;

/// What a call's parameters belong to. A declared callee lets an argument
/// mismatch point at the parameter that set the expectation, in the callee's
/// own source file, so an error in a call the user did not write, such as the
/// runtime's call to `main`, names the declaration the user did write.
#[derive(Clone, Copy)]
pub(crate) enum CallTarget {
    /// A function or method declaration. `first_param` skips a receiver that
    /// the call supplies outside its argument list.
    Declared {
        callee: GlobalDefId,
        first_param: usize,
    },
    /// A function pointer, closure, or callable value with no declaration.
    Value,
}

impl CallTarget {
    pub(crate) const fn function(callee: GlobalDefId) -> Self {
        Self::Declared {
            callee,
            first_param: 0,
        }
    }

    pub(crate) const fn method(callee: GlobalDefId) -> Self {
        Self::Declared {
            callee,
            first_param: 1,
        }
    }
}

impl<'a> BodyChecker<'a> {
    pub(super) fn check_direct_call_args(
        &mut self,
        span: Span,
        target: CallTarget,
        args: &[Expr],
        params: &[InternedTyId],
        is_variadic: bool,
    ) {
        self.check_call_arg_count(span, args.len(), params.len(), is_variadic);

        for (index, arg) in args.iter().enumerate() {
            if let Some(expected) = params.get(index).copied() {
                // A static callable generic carries its callable shape in a
                // `where F: Fn(...)` bound. Expose that shape while checking
                // the argument so closure parameters can be inferred without
                // requiring redundant annotations.
                let expected = self
                    .static_callable_bound(expected)
                    .map(|(params, return_type)| {
                        self.interner.intern(nia_ty::TyKind::CallablePointee {
                            params,
                            return_type,
                        })
                    })
                    .unwrap_or(expected);
                self.seed_closure_params_from_callable_pattern(expected, arg);
                let arg_ty = self.check_expr_with_expected(arg, Some(expected));
                self.expect_call_arg_type(target, index, arg, expected, arg_ty);
            } else {
                self.check_expr(arg);
            }
        }
    }

    /// Checks one argument against its parameter. A mismatch names the
    /// callee's parameter declaration, wherever it is declared.
    pub(crate) fn expect_call_arg_type(
        &mut self,
        target: CallTarget,
        index: usize,
        arg: &Expr,
        expected: InternedTyId,
        actual: InternedTyId,
    ) {
        let reported = self.diagnostics.len();
        self.expect_expr_type(
            arg,
            expected,
            actual,
            &format!("call argument {}", index + 1),
        );
        if self.diagnostics.len() == reported + 1
            && let Some(declaration) = self.call_param_declaration(target, index)
        {
            let diagnostic = self
                .diagnostics
                .pop()
                .expect("argument mismatch was pushed");
            let diagnostic = if declaration.relocates {
                self.relocated_argument_mismatch(diagnostic, declaration, expected, actual)
            } else {
                declaration.attach(diagnostic)
            };
            self.diagnostics.push(diagnostic);
        }
    }

    /// Toolchain code calling user code (the runtime calling `main`) cannot be
    /// edited by the user, so the mismatch is the declaration's: the report is
    /// filed at the user's parameter, and the toolchain call becomes context.
    fn relocated_argument_mismatch(
        &self,
        mismatch: Diagnostic,
        declaration: ParamDeclaration,
        expected: InternedTyId,
        actual: InternedTyId,
    ) -> Diagnostic {
        let call_span = mismatch.primary_span().unwrap_or_default();
        let required = self.ty_name(actual);
        let summary = format!(
            "{} has type {}, but its caller passes {required}",
            declaration.subject,
            self.ty_name(expected),
        );
        let mut builder = Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
            .primary(
                declaration.span,
                format!("declare this parameter as `{required}`"),
            )
            .related_at(
                self.source_path.as_str(),
                call_span,
                format!("{} is called here with `{required}`", declaration.function),
            )
            .help(format!(
                "change the parameter type of `{}` to `{required}`",
                declaration.function
            ));
        if let Some(path) = declaration.source_path {
            builder = builder.in_source(path);
        }
        builder.finish()
    }

    fn call_param_declaration(
        &mut self,
        target: CallTarget,
        index: usize,
    ) -> Option<ParamDeclaration> {
        let CallTarget::Declared {
            callee,
            first_param,
        } = target
        else {
            return None;
        };
        let signature = self.resolved_function_signature(callee)?.signature;
        let param = signature.params.get(first_param + index)?;
        let function = self.symbol_name(signature.name);
        let subject = match param.name {
            Some(name) => format!("parameter `{}` of `{function}`", self.symbol_name(name)),
            None => format!("parameter {} of `{function}`", index + 1),
        };
        let source_path = if callee.module_id == self.defs.module_id {
            None
        } else {
            Some(self.module_path_text(callee.module_id)?)
        };
        let relocates = self.is_toolchain_module(self.defs.module_id)
            && !self.is_toolchain_module(callee.module_id);
        Some(ParamDeclaration {
            source_path,
            span: param.span,
            subject,
            function,
            relocates,
        })
    }

    /// A pattern that a toolchain call's result cannot match is the callee's
    /// return type at fault when the callee is user code: the report moves to
    /// that return type, and the toolchain pattern becomes context.
    pub(crate) fn relocate_call_result_mismatches(&mut self, target: &Expr, reported: usize) {
        let Some(callee) = self.direct_callee(target) else {
            return;
        };
        if !self.is_toolchain_module(self.defs.module_id)
            || self.is_toolchain_module(callee.module_id)
        {
            return;
        }
        let Some(signature) = self
            .resolved_function_signature(callee)
            .map(|resolved| resolved.signature)
        else {
            return;
        };
        let Some(source_path) = self.module_path_text(callee.module_id) else {
            return;
        };
        let function = self.symbol_name(signature.name);
        let returned = self.ty_name(signature.return_type);
        for index in reported..self.diagnostics.len() {
            let pattern_span = self.diagnostics[index].primary_span().unwrap_or_default();
            let summary = format!(
                "`{function}` returns {returned}, which its caller cannot accept: {}",
                self.diagnostics[index].summary
            );
            self.diagnostics[index] = Diagnostic::user_error(codes::TYPE_CHECK, summary)
                .primary(signature.return_span, "change this return type")
                .related_at(
                    self.source_path.as_str(),
                    pattern_span,
                    format!("the result of `{function}` is matched here"),
                )
                .in_source(source_path.clone())
                .finish();
        }
    }

    /// The declared function a call expression invokes directly, if any.
    fn direct_callee(&self, expr: &Expr) -> Option<GlobalDefId> {
        match self.node_resolved_calls.get(&expr.node_key)? {
            nia_sema_ir::ResolvedCall::Function(def_id)
            | nia_sema_ir::ResolvedCall::FunctionInstance { def_id, .. } => Some(*def_id),
            _ => None,
        }
    }

    fn module_path_text(&self, module_id: nia_ids::ModuleId) -> Option<String> {
        if module_id == self.defs.module_id {
            return Some(self.source_path.as_str().to_owned());
        }
        self.program
            .module_source_path
            .and_then(|path| path(module_id))
            .map(|path| path.as_str().to_owned())
    }

    /// Whether a module is part of the shipped standard library or runtime,
    /// which users call but do not edit.
    fn is_toolchain_module(&self, module_id: nia_ids::ModuleId) -> bool {
        let package = if module_id == self.defs.module_id {
            self.current_module_package()
        } else {
            self.program
                .module_path
                .and_then(|module_path| module_path(module_id))
                .map(|path| path.package)
        };
        package.is_some_and(|package| {
            package == nia_symbol::known::STD || package == nia_symbol::known::RUNTIME_TREE
        })
    }

    fn current_module_package(&self) -> Option<nia_symbol::SymbolId> {
        self.program
            .module_path
            .and_then(|module_path| module_path(self.defs.module_id))
            .map(|path| path.package)
    }

    pub(super) fn check_call_arg_count(
        &mut self,
        span: Span,
        actual: usize,
        expected: usize,
        is_variadic: bool,
    ) {
        let ArityCheck::Mismatch {
            requirement,
            actual,
        } = check_call_arity(expected, actual, is_variadic)
        else {
            return;
        };
        let expected = match requirement {
            ArityRequirement::Exact(expected) => expected.to_string(),
            ArityRequirement::AtLeast(expected) => format!("at least {expected}"),
        };
        let summary = format!("argument count mismatch: expected {expected}, got {actual}");
        let help = match requirement {
            ArityRequirement::Exact(expected) if actual < expected => {
                "add the missing arguments to match the call signature"
            }
            ArityRequirement::Exact(_) => "remove the extra arguments to match the call signature",
            ArityRequirement::AtLeast(_) => "add arguments until the call meets the minimum arity",
        };
        self.diagnostics.push(
            Diagnostic::user_error(codes::TYPE_CHECK, summary.clone())
                .primary(span, summary)
                .note(format!(
                    "this call accepts {expected} argument(s), but received {actual}"
                ))
                .help(help)
                .finish(),
        );
    }
}

/// A callee parameter's declaration, attached to an argument mismatch.
struct ParamDeclaration {
    /// The callee's source file, when it is not the caller's.
    source_path: Option<String>,
    span: Span,
    /// `parameter `name` of `function``.
    subject: String,
    function: String,
    /// The caller is toolchain code and the callee is not.
    relocates: bool,
}

impl ParamDeclaration {
    fn attach(self, mut diagnostic: Diagnostic) -> Diagnostic {
        diagnostic.related.push(nia_diagnostic::RelatedDiagnostic {
            span: self.span,
            source_path: self.source_path,
            message: format!("{} is declared here", self.subject),
        });
        diagnostic
    }
}
