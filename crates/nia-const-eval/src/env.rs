use crate::ConstValue;

use nia_const_ir::{
    ConstNameResolution, EarlyConstAssignTarget, EarlyConstBinding, EarlyConstExpr,
    EarlyConstGenericArg, EarlyConstName, EarlyConstParam, EarlyConstTypeArg,
    ResolvedConstAssignTarget, ResolvedConstBinding, ResolvedConstExpr, ResolvedConstGenericArg,
    ResolvedConstParam, ResolvedConstTypeArg,
};
use nia_ids::{
    BuiltinConstValue, GlobalDefId, InternedTyId, LayoutBuiltin, ModuleId, ValueBuiltin,
};
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_text_from_optional_resolver};
use nia_ty::ConstGenericArg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstIntegerSemantics {
    pub bits: u32,
    pub signed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstError {
    pub span: Span,
    pub message: String,
}

pub const DEFAULT_CONST_EVAL_STEP_LIMIT: usize = 1_000_000;
pub const DEFAULT_CONST_EVAL_CALL_DEPTH_LIMIT: usize = 256;

#[derive(Debug, Clone)]
pub struct ConstEvalBudget {
    step_limit: usize,
    remaining_steps: usize,
    call_depth_limit: usize,
    call_depth: usize,
    session_depth: usize,
}

impl ConstEvalBudget {
    pub fn new(step_limit: usize, call_depth_limit: usize) -> Self {
        Self {
            step_limit,
            remaining_steps: step_limit,
            call_depth_limit,
            call_depth: 0,
            session_depth: 0,
        }
    }

    pub fn begin_session(&mut self) {
        if self.session_depth == 0 {
            self.remaining_steps = self.step_limit;
            self.call_depth = 0;
        }
        self.session_depth += 1;
    }

    pub fn end_session(&mut self) {
        self.session_depth = self.session_depth.saturating_sub(1);
        if self.session_depth == 0 {
            self.call_depth = 0;
        }
    }

    pub fn consume_step(&mut self, span: Span) -> Result<(), ConstError> {
        let Some(remaining) = self.remaining_steps.checked_sub(1) else {
            return Err(ConstError {
                span,
                message: format!(
                    "const evaluation exceeded the {} step limit",
                    self.step_limit
                ),
            });
        };
        self.remaining_steps = remaining;
        Ok(())
    }

    pub fn enter_call(&mut self, span: Span) -> Result<(), ConstError> {
        if self.call_depth >= self.call_depth_limit {
            return Err(ConstError {
                span,
                message: format!(
                    "const evaluation exceeded the {} call depth limit",
                    self.call_depth_limit
                ),
            });
        }
        self.call_depth += 1;
        Ok(())
    }

    pub fn leave_call(&mut self) {
        self.call_depth = self.call_depth.saturating_sub(1);
    }
}

impl Default for ConstEvalBudget {
    fn default() -> Self {
        Self::new(
            DEFAULT_CONST_EVAL_STEP_LIMIT,
            DEFAULT_CONST_EVAL_CALL_DEPTH_LIMIT,
        )
    }
}

pub trait ConstCommonEnv {
    fn begin_const_eval(&mut self) {}

    fn end_const_eval(&mut self) {}

    fn consume_const_eval_step(&mut self, _span: Span) -> Result<(), ConstError> {
        Ok(())
    }

    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(None, symbol)
    }

    fn is_enum_variant(&self, _def_id: GlobalDefId) -> bool {
        false
    }

    fn resolve_builtin_const(
        &mut self,
        span: Span,
        builtin: BuiltinConstValue,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: format!(
                "unsupported builtin const in const expression: {}",
                builtin.name()
            ),
        })
    }

    fn resolve_builtin_value(
        &mut self,
        span: Span,
        builtin: ValueBuiltin,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: format!(
                "unsupported builtin value in const expression: @{}",
                builtin.name()
            ),
        })
    }

    fn resolve_embed(&mut self, span: Span, path: &str) -> Result<ConstValue, ConstError> {
        let _ = path;
        Err(ConstError {
            span,
            message: "builtin `embed` is not available in this context".to_string(),
        })
    }

    fn cast_value(
        &mut self,
        span: Span,
        value: ConstValue,
        ty: InternedTyId,
    ) -> Result<ConstValue, ConstError> {
        let _ = span;
        let _ = ty;
        Ok(value)
    }

    fn push_const_scope(&mut self, span: Span) -> Result<(), ConstError> {
        Err(ConstError {
            span,
            message: "const local scopes are not available in this context".to_string(),
        })
    }

    fn pop_const_scope(&mut self) {}

    fn push_function_frame(&mut self, span: Span) -> Result<(), ConstError> {
        self.push_const_scope(span)
    }

    fn pop_function_frame(&mut self) {
        self.pop_const_scope();
    }

    fn bind_function_context(
        &mut self,
        span: Span,
        module_id: ModuleId,
        function_id: Option<GlobalDefId>,
        substitutions: Vec<(SymbolId, InternedTyId)>,
        const_substitutions: Vec<(SymbolId, ConstGenericArg)>,
    ) -> Result<(), ConstError> {
        let _ = span;
        let _ = module_id;
        let _ = function_id;
        let _ = substitutions;
        let _ = const_substitutions;
        Ok(())
    }
}

pub trait EarlyConstEnv: ConstCommonEnv {
    fn resolve_name(&mut self, span: Span, name: &EarlyConstName)
    -> Result<ConstValue, ConstError>;

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &EarlyConstTypeArg,
    ) -> Result<ConstValue, ConstError>;

    fn resolve_field_offset_builtin(
        &mut self,
        span: Span,
        type_arg: &EarlyConstTypeArg,
        field: &SymbolId,
    ) -> Result<ConstValue, ConstError> {
        let _ = type_arg;
        let _ = field;
        Err(ConstError {
            span,
            message: "unsupported field offset builtin in const expression".to_string(),
        })
    }

    fn call_function(
        &mut self,
        span: Span,
        callee: &EarlyConstExpr,
        generic_args: &[EarlyConstGenericArg],
        arg_exprs: &[EarlyConstExpr],
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let _ = callee;
        let _ = generic_args;
        let _ = arg_exprs;
        let _ = args;
        Err(ConstError {
            span,
            message: "unsupported const function call".to_string(),
        })
    }

    fn bind_function_param(
        &mut self,
        span: Span,
        param: &EarlyConstParam,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = param;
        let _ = value;
        Err(ConstError {
            span,
            message: "const function parameters are not available in this context".to_string(),
        })
    }

    fn bind_function_local(
        &mut self,
        span: Span,
        binding: &EarlyConstBinding,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = binding;
        let _ = value;
        Err(ConstError {
            span,
            message: "const function locals are not available in this context".to_string(),
        })
    }

    fn assign_local(
        &mut self,
        span: Span,
        target: &EarlyConstAssignTarget,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = target;
        let _ = value;
        Err(ConstError {
            span,
            message: "const assignment is not available in this context".to_string(),
        })
    }

    fn bind_pattern_local(
        &mut self,
        span: Span,
        name: &SymbolId,
        local_id: Option<nia_ids::LocalId>,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = name;
        let _ = value;
        if local_id.is_none() {
            return Err(ConstError {
                span,
                message: format!(
                    "failed to resolve const pattern local `{}`",
                    self.symbol_name(*name)
                ),
            });
        }
        Err(ConstError {
            span,
            message: "const switch pattern locals are not available in this context".to_string(),
        })
    }
}

pub trait ResolvedConstEnv: ConstCommonEnv {
    fn resolved_integer_semantics(
        &mut self,
        _expr: &ResolvedConstExpr,
    ) -> Option<ConstIntegerSemantics> {
        None
    }

    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: ConstNameResolution,
    ) -> Result<ConstValue, ConstError>;

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &ResolvedConstTypeArg,
    ) -> Result<ConstValue, ConstError>;

    fn resolve_resolved_field_offset_builtin(
        &mut self,
        span: Span,
        type_arg: &ResolvedConstTypeArg,
        field: &SymbolId,
    ) -> Result<ConstValue, ConstError> {
        let _ = type_arg;
        let _ = field;
        Err(ConstError {
            span,
            message: "unsupported field offset builtin in resolved const expression".to_string(),
        })
    }

    fn call_resolved_function(
        &mut self,
        span: Span,
        callee: &ResolvedConstExpr,
        generic_args: &[ResolvedConstGenericArg],
        arg_exprs: &[ResolvedConstExpr],
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let _ = callee;
        let _ = generic_args;
        let _ = arg_exprs;
        let _ = args;
        Err(ConstError {
            span,
            message: "unsupported resolved const function call".to_string(),
        })
    }

    fn bind_resolved_function_param(
        &mut self,
        span: Span,
        param: &ResolvedConstParam,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = param;
        let _ = value;
        Err(ConstError {
            span,
            message: "resolved const function parameters are not available in this context"
                .to_string(),
        })
    }

    fn bind_resolved_function_local(
        &mut self,
        span: Span,
        binding: &ResolvedConstBinding,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = binding;
        let _ = value;
        Err(ConstError {
            span,
            message: "resolved const function locals are not available in this context".to_string(),
        })
    }

    fn assign_resolved_local(
        &mut self,
        span: Span,
        target: &ResolvedConstAssignTarget,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = target;
        let _ = value;
        Err(ConstError {
            span,
            message: "resolved const assignment is not available in this context".to_string(),
        })
    }

    fn bind_resolved_pattern_local(
        &mut self,
        span: Span,
        name: &SymbolId,
        local_id: nia_ids::LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = name;
        let _ = local_id;
        let _ = value;
        Err(ConstError {
            span,
            message: "resolved const switch pattern locals are not available in this context"
                .to_string(),
        })
    }
}

#[derive(Default)]
pub struct EmptyEnv;

impl ConstCommonEnv for EmptyEnv {}

impl EarlyConstEnv for EmptyEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyConstName,
    ) -> Result<ConstValue, ConstError> {
        match name {
            EarlyConstName::Unresolved(display) => Err(ConstError {
                span,
                message: format!("unknown const value `{}`", self.symbol_name(*display)),
            }),
            EarlyConstName::Resolved { display, .. } => Err(ConstError {
                span,
                message: format!(
                    "resolved const value `{}` is not available in this context",
                    self.symbol_name(*display)
                ),
            }),
        }
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg: &EarlyConstTypeArg,
    ) -> Result<ConstValue, ConstError> {
        Err(ConstError {
            span,
            message: "layout builtins are not available in this const context".to_string(),
        })
    }
}
