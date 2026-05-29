// SPDX-License-Identifier: GPL-3.0-or-later
use nia_control_ir::{ControlDeferBody, ControlScopeId};
use nia_diagnostic::Diagnostic;
use nia_span::Span;

use super::FunctionCodegen;

#[derive(Debug, Clone)]
pub(super) struct DeferScope {
    pub(super) control_scope: Option<ControlScopeId>,
    pub(super) bodies: Vec<ControlDeferBody>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn push_control_defer_scope(&mut self, scope: ControlScopeId) -> usize {
        let index = self.defer_scopes.len();
        self.defer_scopes.push(DeferScope {
            control_scope: Some(scope),
            bodies: Vec::new(),
        });
        index
    }

    pub(super) fn emit_control_scope_defers(
        &mut self,
        span: Span,
        scope: ControlScopeId,
    ) -> Result<(), Diagnostic> {
        let Some(scope) = self
            .defer_scopes
            .iter()
            .rev()
            .find(|defer_scope| defer_scope.control_scope == Some(scope))
            .cloned()
        else {
            return Err(self.error(span, "missing control defer scope"));
        };
        self.emit_defer_scope(scope)
    }

    fn emit_defer_scope(&mut self, scope: DeferScope) -> Result<(), Diagnostic> {
        for body in scope.bodies.into_iter().rev() {
            self.emit_defer_control_body(&body)?;
        }
        Ok(())
    }

    pub(super) fn register_defer(
        &mut self,
        span: Span,
        body: &ControlDeferBody,
    ) -> Result<(), Diagnostic> {
        if let Some(control_scope) = self.active_control_scope {
            let Some(index) = self.control_defer_scopes.get(&control_scope).copied() else {
                return Err(self.error(span, "missing active control defer scope"));
            };
            let Some(scope) = self.defer_scopes.get_mut(index) else {
                return Err(self.error(span, "missing active control defer storage"));
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
