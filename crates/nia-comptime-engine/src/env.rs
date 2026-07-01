use crate::ComptimeValue;

use nia_comptime_ir::{
    ComptimeNameResolution, EarlyComptimeAssignTarget, EarlyComptimeBinding, EarlyComptimeExpr,
    EarlyComptimeName, EarlyComptimeParam, EarlyComptimeTypeArg, ResolvedComptimeAssignTarget,
    ResolvedComptimeBinding, ResolvedComptimeExpr, ResolvedComptimeParam, ResolvedComptimeTypeArg,
};
use nia_ids::{GlobalDefId, InternedTyId, LayoutBuiltin, ModuleId, ValueBuiltin};
use nia_span::Span;
use nia_ty::ConstGenericArg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComptimeError {
    pub span: Span,
    pub message: String,
}

pub trait ComptimeCommonEnv {
    fn resolve_builtin_value(
        &mut self,
        span: Span,
        builtin: ValueBuiltin,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: format!(
                "unsupported builtin value in comptime expression: @{}",
                builtin.name()
            ),
        })
    }

    fn resolve_embed(&mut self, span: Span, path: &str) -> Result<ComptimeValue, ComptimeError> {
        let _ = path;
        Err(ComptimeError {
            span,
            message: "builtin `embed` is not available in this context".to_string(),
        })
    }

    fn cast_value(
        &mut self,
        span: Span,
        value: ComptimeValue,
        ty: InternedTyId,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = span;
        let _ = ty;
        Ok(value)
    }

    fn push_comptime_scope(&mut self, span: Span) -> Result<(), ComptimeError> {
        Err(ComptimeError {
            span,
            message: "comptime local scopes are not available in this context".to_string(),
        })
    }

    fn pop_comptime_scope(&mut self) {}

    fn push_function_frame(&mut self, span: Span) -> Result<(), ComptimeError> {
        self.push_comptime_scope(span)
    }

    fn pop_function_frame(&mut self) {
        self.pop_comptime_scope();
    }

    fn bind_function_context(
        &mut self,
        span: Span,
        module_id: ModuleId,
        function_id: Option<GlobalDefId>,
        substitutions: Vec<(String, InternedTyId)>,
        const_substitutions: Vec<(String, ConstGenericArg)>,
    ) -> Result<(), ComptimeError> {
        let _ = span;
        let _ = module_id;
        let _ = function_id;
        let _ = substitutions;
        let _ = const_substitutions;
        Ok(())
    }
}

pub trait EarlyComptimeEnv: ComptimeCommonEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyComptimeName,
    ) -> Result<ComptimeValue, ComptimeError>;

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &EarlyComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError>;

    fn resolve_field_offset_builtin(
        &mut self,
        span: Span,
        type_arg: &EarlyComptimeTypeArg,
        field: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = type_arg;
        let _ = field;
        Err(ComptimeError {
            span,
            message: "unsupported field offset builtin in comptime expression".to_string(),
        })
    }

    fn call_function(
        &mut self,
        span: Span,
        callee: &EarlyComptimeExpr,
        type_args: &[EarlyComptimeTypeArg],
        arg_exprs: &[EarlyComptimeExpr],
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = callee;
        let _ = type_args;
        let _ = arg_exprs;
        let _ = args;
        Err(ComptimeError {
            span,
            message: "unsupported comptime function call".to_string(),
        })
    }

    fn bind_function_param(
        &mut self,
        span: Span,
        param: &EarlyComptimeParam,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = param;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime function parameters are not available in this context".to_string(),
        })
    }

    fn bind_function_local(
        &mut self,
        span: Span,
        binding: &EarlyComptimeBinding,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = binding;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime function locals are not available in this context".to_string(),
        })
    }

    fn assign_local(
        &mut self,
        span: Span,
        target: &EarlyComptimeAssignTarget,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = target;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "comptime assignment is not available in this context".to_string(),
        })
    }

    fn bind_pattern_local(
        &mut self,
        span: Span,
        name: &str,
        local_id: Option<nia_ids::LocalId>,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = name;
        let _ = value;
        if local_id.is_none() {
            return Err(ComptimeError {
                span,
                message: format!("failed to resolve comptime pattern local `{name}`"),
            });
        }
        Err(ComptimeError {
            span,
            message: "comptime switch pattern locals are not available in this context".to_string(),
        })
    }
}

pub trait ResolvedComptimeEnv: ComptimeCommonEnv {
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: ComptimeNameResolution,
    ) -> Result<ComptimeValue, ComptimeError>;

    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &ResolvedComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError>;

    fn resolve_resolved_field_offset_builtin(
        &mut self,
        span: Span,
        type_arg: &ResolvedComptimeTypeArg,
        field: &str,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = type_arg;
        let _ = field;
        Err(ComptimeError {
            span,
            message: "unsupported field offset builtin in resolved comptime expression".to_string(),
        })
    }

    fn call_resolved_function(
        &mut self,
        span: Span,
        callee: &ResolvedComptimeExpr,
        type_args: &[ResolvedComptimeTypeArg],
        arg_exprs: &[ResolvedComptimeExpr],
        args: Vec<ComptimeValue>,
    ) -> Result<ComptimeValue, ComptimeError> {
        let _ = callee;
        let _ = type_args;
        let _ = arg_exprs;
        let _ = args;
        Err(ComptimeError {
            span,
            message: "unsupported resolved comptime function call".to_string(),
        })
    }

    fn bind_resolved_function_param(
        &mut self,
        span: Span,
        param: &ResolvedComptimeParam,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = param;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "resolved comptime function parameters are not available in this context"
                .to_string(),
        })
    }

    fn bind_resolved_function_local(
        &mut self,
        span: Span,
        binding: &ResolvedComptimeBinding,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = binding;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "resolved comptime function locals are not available in this context"
                .to_string(),
        })
    }

    fn assign_resolved_local(
        &mut self,
        span: Span,
        target: &ResolvedComptimeAssignTarget,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = target;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "resolved comptime assignment is not available in this context".to_string(),
        })
    }

    fn bind_resolved_pattern_local(
        &mut self,
        span: Span,
        name: &str,
        local_id: nia_ids::LocalId,
        value: ComptimeValue,
    ) -> Result<(), ComptimeError> {
        let _ = name;
        let _ = local_id;
        let _ = value;
        Err(ComptimeError {
            span,
            message: "resolved comptime switch pattern locals are not available in this context"
                .to_string(),
        })
    }
}

#[derive(Default)]
pub struct EmptyEnv;

impl ComptimeCommonEnv for EmptyEnv {}

impl EarlyComptimeEnv for EmptyEnv {
    fn resolve_name(
        &mut self,
        span: Span,
        name: &EarlyComptimeName,
    ) -> Result<ComptimeValue, ComptimeError> {
        match name {
            EarlyComptimeName::Unresolved(display) => Err(ComptimeError {
                span,
                message: format!("unknown comptime value `{display}`"),
            }),
            EarlyComptimeName::Resolved { display, .. } => Err(ComptimeError {
                span,
                message: format!(
                    "resolved comptime value `{display}` is not available in this context"
                ),
            }),
        }
    }

    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        _builtin: LayoutBuiltin,
        _type_arg: &EarlyComptimeTypeArg,
    ) -> Result<ComptimeValue, ComptimeError> {
        Err(ComptimeError {
            span,
            message: "layout builtins are not available in this comptime context".to_string(),
        })
    }
}
