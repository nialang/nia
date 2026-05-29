// SPDX-License-Identifier: GPL-3.0-or-later
use nia_body_ir::TypedExpr;
use nia_control_ir::ControlScopeId;
use nia_diagnostic::Diagnostic;
use nia_span::Span;

use super::FunctionCodegen;

#[derive(Debug, Clone)]
pub(super) struct DeferScope {
    pub(super) control_scope: Option<ControlScopeId>,
    pub(super) loop_depth: usize,
    pub(super) exprs: Vec<TypedExpr>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn push_defer_scope(&mut self) -> usize {
        let index = self.defer_scopes.len();
        self.defer_scopes.push(DeferScope {
            control_scope: None,
            loop_depth: self.loops.len(),
            exprs: Vec::new(),
        });
        index
    }

    pub(super) fn push_control_defer_scope(&mut self, scope: ControlScopeId) -> usize {
        let index = self.defer_scopes.len();
        self.defer_scopes.push(DeferScope {
            control_scope: Some(scope),
            loop_depth: self.loops.len(),
            exprs: Vec::new(),
        });
        index
    }

    pub(super) fn pop_defer_scope_to(
        &mut self,
        index: usize,
        execute: bool,
    ) -> Result<(), Diagnostic> {
        while self.defer_scopes.len() > index {
            let scope = self
                .defer_scopes
                .pop()
                .ok_or_else(|| self.error(Span::default(), "missing defer scope"))?;
            if execute {
                self.emit_defer_scope(scope)?;
            }
        }
        Ok(())
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
        for expr in scope.exprs.into_iter().rev() {
            self.emit_void_expr(&expr)?;
        }
        Ok(())
    }

    pub(super) fn register_defer(
        &mut self,
        span: Span,
        expr: &TypedExpr,
    ) -> Result<(), Diagnostic> {
        if let Some(control_scope) = self.active_control_scope {
            let Some(index) = self.control_defer_scopes.get(&control_scope).copied() else {
                return Err(self.error(span, "missing active control defer scope"));
            };
            let Some(scope) = self.defer_scopes.get_mut(index) else {
                return Err(self.error(span, "missing active control defer storage"));
            };
            scope.exprs.push(expr.clone());
            return Ok(());
        }
        let Some(scope) = self.defer_scopes.last_mut() else {
            return Err(self.error(span, "`defer` is not inside a block"));
        };
        scope.exprs.push(expr.clone());
        Ok(())
    }

    pub(super) fn emit_all_defers(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.emit_defers_to_loop_depth(span, 0)
    }

    pub(super) fn emit_loop_exit_defers(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.emit_defers_to_loop_depth(span, self.loops.len())
    }

    fn emit_defers_to_loop_depth(
        &mut self,
        span: Span,
        loop_depth: usize,
    ) -> Result<(), Diagnostic> {
        let scopes = self
            .defer_scopes
            .iter()
            .rev()
            .take_while(|scope| scope.loop_depth >= loop_depth)
            .cloned()
            .collect::<Vec<_>>();
        if scopes.is_empty() {
            return Err(self.error(span, "missing defer scope"));
        }
        for scope in scopes {
            self.emit_defer_scope(scope)?;
        }
        Ok(())
    }
}
