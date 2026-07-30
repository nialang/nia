use crate::ConstValue;

use nia_const_ir::{
    ConstNameResolution, EarlyConstAssignTarget, EarlyConstBinding, EarlyConstExpr, EarlyConstName,
    EarlyConstParam, EarlyConstTypeArg, ResolvedConstAssignTarget, ResolvedConstBinding,
    ResolvedConstExpr, ResolvedConstParam, ResolvedConstTypeArg,
};
use nia_ids::{
    BuiltinConstValue, GlobalDefId, InternedTyId, LayoutBuiltin, ModuleId, ValueBuiltin,
};
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_text_from_optional_resolver};
use nia_ty::ConstGenericArg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstError {
    pub span: Span,
    pub message: String,
}

pub trait ConstCommonEnv {
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
        type_args: &[EarlyConstTypeArg],
        arg_exprs: &[EarlyConstExpr],
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let _ = callee;
        let _ = type_args;
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
        type_args: &[ResolvedConstTypeArg],
        arg_exprs: &[ResolvedConstExpr],
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let _ = callee;
        let _ = type_args;
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
