// SPDX-License-Identifier: GPL-3.0-or-later
use nia_backend_ir::TypedExpr;
use nia_diagnostic::Diagnostic;
use nia_span::Span;

use super::FunctionCodegen;

#[derive(Debug, Clone)]
pub(super) struct DeferScope {
    pub(super) loop_depth: usize,
    pub(super) exprs: Vec<TypedExpr>,
}

impl<'m, 'ctx, 'a> FunctionCodegen<'m, 'ctx, 'a> {
    pub(super) fn push_defer_scope(&mut self) -> usize {
        let index = self.defer_scopes.len();
        self.defer_scopes.push(DeferScope {
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
        while self
            .defer_scopes
            .last()
            .is_some_and(|scope| scope.loop_depth >= loop_depth)
        {
            let scope = self
                .defer_scopes
                .pop()
                .ok_or_else(|| self.error(span, "missing defer scope"))?;
            self.emit_defer_scope(scope)?;
        }
        Ok(())
    }
}
