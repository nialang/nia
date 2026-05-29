// SPDX-License-Identifier: GPL-3.0-or-later
use nia_diagnostic::Diagnostic;
use nia_function_ir::{FunctionDeferBody, FunctionScopeId};
use nia_span::Span;

use super::FunctionCodegen;

#[derive(Debug, Clone)]
pub(super) struct DeferScope {
    pub(super) function_scope: Option<FunctionScopeId>,
    pub(super) bodies: Vec<FunctionDeferBody>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn push_function_defer_scope(&mut self, scope: FunctionScopeId) -> usize {
        let index = self.defer_scopes.len();
        self.defer_scopes.push(DeferScope {
            function_scope: Some(scope),
            bodies: Vec::new(),
        });
        index
    }

    pub(super) fn emit_function_scope_defers(
        &mut self,
        span: Span,
        scope: FunctionScopeId,
    ) -> Result<(), Diagnostic> {
        let Some(scope) = self
            .defer_scopes
            .iter()
            .rev()
            .find(|defer_scope| defer_scope.function_scope == Some(scope))
            .cloned()
        else {
            return Err(self.error(span, "missing function defer scope"));
        };
        self.emit_defer_scope(scope)
    }

    fn emit_defer_scope(&mut self, scope: DeferScope) -> Result<(), Diagnostic> {
        for body in scope.bodies.into_iter().rev() {
            self.emit_defer_function_body(&body)?;
        }
        Ok(())
    }

    pub(super) fn register_defer(
        &mut self,
        span: Span,
        body: &FunctionDeferBody,
    ) -> Result<(), Diagnostic> {
        if let Some(function_scope) = self.active_function_scope {
            let Some(index) = self.function_defer_scopes.get(&function_scope).copied() else {
                return Err(self.error(span, "missing active function defer scope"));
            };
            let Some(scope) = self.defer_scopes.get_mut(index) else {
                return Err(self.error(span, "missing active function defer storage"));
            };
            scope.bodies.push(body.clone());
            return Ok(());
        }
        let Some(scope) = self.defer_scopes.last_mut() else {
            return Err(self.error(span, "`defer` is not inside a block"));
        };
        scope.bodies.push(body.clone());
        Ok(())
    }
}
