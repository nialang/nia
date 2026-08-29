use crate::{ConstAllocationOrigin, ConstPointerValue, ConstValue, ResolvedConstPlace};

use nia_const_ir::{
    ConstNameResolution, EarlyConstAssignTarget, EarlyConstBinding, EarlyConstExpr,
    EarlyConstGenericArg, EarlyConstName, EarlyConstParam, EarlyConstPatternBinding,
    EarlyConstTypeArg, ResolvedConstAssignTarget, ResolvedConstBinding, ResolvedConstExpr,
    ResolvedConstGenericArg, ResolvedConstParam, ResolvedConstPatternBinding, ResolvedConstTypeArg,
};
use nia_ids::{
    BuiltinConstValue, GlobalDefId, InternedTyId, LayoutBuiltin, ModuleId, ValueBuiltin,
};
use nia_span::Span;
use nia_symbol::{SymbolId, symbol_text_from_optional_resolver};
use nia_ty::ConstGenericArg;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Integer width and signedness selected by semantic type analysis.
pub struct ConstIntegerSemantics {
    /// Target integer width in bits.
    pub bits: u32,
    /// Whether values use signed interpretation.
    pub signed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Floating-point precision selected by semantic type analysis.
pub enum ConstFloatSemantics {
    /// IEEE-754 binary32 semantics.
    F32,
    /// IEEE-754 binary64 semantics.
    F64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A user-facing failure produced while interpreting a const expression.
pub struct ConstError {
    /// Source span that triggered the evaluation failure.
    pub span: Span,
    /// Stable user-facing error message.
    pub message: String,
}

/// Default total interpreter-step limit per outer evaluation session.
pub const DEFAULT_CONST_EVAL_STEP_LIMIT: usize = 1_000_000;
/// Default nested const-function call-depth limit.
pub const DEFAULT_CONST_EVAL_CALL_DEPTH_LIMIT: usize = 256;

#[derive(Debug, Clone)]
/// Shared step and recursion limits for one logical const evaluation.
///
/// Public evaluator entry points can nest when a const function calls another
/// const function. Nested sessions share the outer session's remaining steps
/// and call depth; only the outermost [`Self::begin_session`] resets the budget.
/// Every successful `begin_session`/`enter_call` must be paired with exactly one
/// `end_session`/`leave_call` respectively.
pub struct ConstEvalBudget {
    step_limit: usize,
    remaining_steps: usize,
    call_depth_limit: usize,
    call_depth: usize,
    session_depth: usize,
}

impl ConstEvalBudget {
    /// Creates a budget with explicit total-step and nested-call limits.
    pub fn new(step_limit: usize, call_depth_limit: usize) -> Self {
        Self {
            step_limit,
            remaining_steps: step_limit,
            call_depth_limit,
            call_depth: 0,
            session_depth: 0,
        }
    }

    /// Enters a possibly nested evaluation session.
    pub fn begin_session(&mut self) {
        // Nested entry points share the outer remaining budget. Resetting only
        // at depth zero prevents a recursive const call from replenishing its
        // parent's step allowance.
        if self.session_depth == 0 {
            assert_eq!(
                self.call_depth, 0,
                "const evaluation session began with active function calls"
            );
            self.remaining_steps = self.step_limit;
            self.call_depth = 0;
        }
        self.session_depth = self
            .session_depth
            .checked_add(1)
            .expect("const evaluation session depth overflow");
    }

    /// Leaves the current evaluation session.
    ///
    /// # Panics
    ///
    /// Panics when no session is active, or when the outermost session still
    /// owns function calls. Either condition is an evaluator cleanup bug.
    pub fn end_session(&mut self) {
        assert!(
            self.session_depth > 0,
            "const evaluation session ended without a matching begin"
        );
        self.session_depth -= 1;
        if self.session_depth == 0 {
            assert_eq!(
                self.call_depth, 0,
                "const evaluation session ended with active function calls"
            );
        }
    }

    /// Charges one interpreter operation to the current logical session.
    pub fn consume_step(&mut self, span: Span) -> Result<(), ConstError> {
        // Charge before executing the operation so a failing step cannot
        // partially mutate interpreter state and then be retried for free.
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

    /// Enters a const function call, rejecting recursion beyond the limit.
    pub fn enter_call(&mut self, span: Span) -> Result<(), ConstError> {
        // Check before incrementing; callers can therefore leave the budget
        // unchanged when recursion is rejected.
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

    /// Leaves a const function call previously accepted by [`Self::enter_call`].
    ///
    /// # Panics
    ///
    /// Panics when no call is active, which indicates unbalanced evaluator
    /// frame cleanup.
    pub fn leave_call(&mut self) {
        assert!(
            self.call_depth > 0,
            "const evaluation call ended without a matching entry"
        );
        self.call_depth -= 1;
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

/// State and semantic services shared by early and resolved const evaluation.
///
/// The evaluator brackets every public operation with `begin_const_eval` and
/// `end_const_eval`. Once a scope or function-frame push succeeds, the matching
/// pop is guaranteed on every ordinary `Result` and control-flow exit. An
/// implementation must therefore make a successful push fully usable; a
/// failing push must leave its state unchanged.
pub trait ConstCommonEnv {
    /// Begins a public evaluation session.
    fn begin_const_eval(&mut self) {}

    /// Ends the current public evaluation session.
    fn end_const_eval(&mut self) {}

    /// Charges one interpreter operation to the shared evaluation budget.
    fn consume_const_eval_step(&mut self, _span: Span) -> Result<(), ConstError> {
        Ok(())
    }

    /// Renders a symbol for diagnostics.
    fn symbol_name(&self, symbol: SymbolId) -> String {
        symbol_text_from_optional_resolver(None, symbol)
    }

    /// Reports whether a global definition denotes an enum variant.
    fn is_enum_variant(&self, _def_id: GlobalDefId) -> bool {
        false
    }

    /// Resolves a language builtin const value.
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

    /// Resolves a value builtin available during const evaluation.
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

    /// Loads an embedded resource for the `embed` builtin.
    fn resolve_embed(&mut self, span: Span, path: &str) -> Result<ConstValue, ConstError> {
        let _ = path;
        Err(ConstError {
            span,
            message: "builtin `embed` is not available in this context".to_string(),
        })
    }

    /// Casts a value to an already-lowered runtime type.
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

    /// Allocates a frozen pointer to an evaluated value.
    fn reference_const_value(
        &mut self,
        span: Span,
        value: ConstValue,
        is_readonly: bool,
    ) -> Result<ConstValue, ConstError> {
        Ok(ConstValue::Pointer(ConstPointerValue::Frozen {
            origin: ConstAllocationOrigin::new(None, span),
            is_readonly,
            pointee: Box::new(value),
        }))
    }

    /// Reads the value addressed by a const pointer.
    fn dereference_const_pointer(
        &mut self,
        span: Span,
        pointer: &ConstPointerValue,
    ) -> Result<ConstValue, ConstError> {
        match pointer {
            ConstPointerValue::Frozen { pointee, .. } => Ok((**pointee).clone()),
            ConstPointerValue::Place { .. } => Err(ConstError {
                span,
                message: "const place pointer is unavailable in this context".to_string(),
            }),
        }
    }

    /// Validates a value leaving a root expression evaluation.
    fn validate_const_root_result(
        &mut self,
        _span: Span,
        _value: &ConstValue,
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Validates a value crossing a const-function return boundary.
    fn validate_const_function_result(
        &mut self,
        _span: Span,
        _value: &ConstValue,
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Pushes a lexical local scope.
    fn push_const_scope(&mut self, span: Span) -> Result<(), ConstError> {
        Err(ConstError {
            span,
            message: "const local scopes are not available in this context".to_string(),
        })
    }

    /// Pops the most recently pushed lexical local scope.
    fn pop_const_scope(&mut self) {}

    /// Pushes an execution frame that hides caller locals and substitutions.
    fn push_function_frame(&mut self, span: Span) -> Result<(), ConstError> {
        self.push_const_scope(span)
    }

    /// Pops the most recently pushed execution frame.
    fn pop_function_frame(&mut self) {
        self.pop_const_scope();
    }

    /// Binds module, function, and generic context into the active frame.
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

/// Semantic services required while evaluating early, partially resolved IR.
///
/// Implementations may resolve source names lazily, but a name already carrying
/// a semantic identity must never fall back to spelling-based lookup.
pub trait EarlyConstEnv: ConstCommonEnv {
    /// Resolves an early name through its semantic identity or source context.
    fn resolve_name(&mut self, span: Span, name: &EarlyConstName)
    -> Result<ConstValue, ConstError>;

    /// Evaluates a layout builtin for a partially resolved type argument.
    fn resolve_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &EarlyConstTypeArg,
    ) -> Result<ConstValue, ConstError>;

    /// Evaluates a field-offset builtin for a partially resolved type argument.
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

    /// Dispatches an early const-function call after argument evaluation.
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

    /// Binds one early const-function parameter in the active frame.
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

    /// Binds one early local declaration in the active scope.
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

    /// Assigns a value to an early local or projected target.
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

    /// Binds one local introduced by an early pattern match.
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
            message: "const match pattern locals are not available in this context".to_string(),
        })
    }

    /// Binds a function-pattern local using its declaration metadata.
    fn bind_function_pattern_local(
        &mut self,
        span: Span,
        binding: &EarlyConstPatternBinding,
        name: &SymbolId,
        local_id: Option<nia_ids::LocalId>,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = binding;
        self.bind_pattern_local(span, name, local_id, value)
    }
}

/// Semantic services required while evaluating fully resolved const IR.
///
/// Resolved local and global identities are authoritative. This layer also owns
/// typed places, aggregate construction, iterator witnesses, and resolved call
/// dispatch that cannot be represented by the early environment.
pub trait ResolvedConstEnv: ConstCommonEnv {
    /// Creates a pointer retaining the identity of a resolved writable place.
    fn reference_resolved_place(
        &mut self,
        span: Span,
        _place: &ResolvedConstPlace,
        value: ConstValue,
        is_readonly: bool,
    ) -> Result<ConstValue, ConstError> {
        self.reference_const_value(span, value, is_readonly)
    }

    /// Prepares storage and type state before binding a resolved local.
    fn prepare_resolved_binding(
        &mut self,
        _binding: &ResolvedConstBinding,
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Prepares storage and type state before binding a pattern local.
    fn prepare_resolved_pattern_binding(
        &mut self,
        _binding: &ResolvedConstPatternBinding,
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Prepares the expected representation of a function result expression.
    fn prepare_resolved_function_result(
        &mut self,
        _expr: &ResolvedConstExpr,
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Prepares generic and expected-type state for resolved call arguments.
    fn prepare_resolved_call_arguments(
        &mut self,
        _span: Span,
        _callee: &ResolvedConstExpr,
        _generic_args: &[ResolvedConstGenericArg],
        _args: &[ResolvedConstExpr],
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Prepares target type and place metadata for a resolved assignment.
    fn prepare_resolved_assignment(
        &mut self,
        _assign: &nia_const_ir::ResolvedConstAssign,
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Prepares conversion metadata for a resolved try expression.
    fn prepare_resolved_try(
        &mut self,
        _span: Span,
        _expr: &ResolvedConstExpr,
    ) -> Result<(), ConstError> {
        Ok(())
    }

    /// Converts an error payload at a resolved try propagation boundary.
    fn convert_resolved_try_error(
        &mut self,
        _span: Span,
        value: ConstValue,
    ) -> Result<ConstValue, ConstError> {
        Ok(value)
    }

    /// Constructs a typed aggregate from evaluated named fields.
    fn build_resolved_aggregate(
        &mut self,
        _span: Span,
        _ty: InternedTyId,
        fields: BTreeMap<SymbolId, ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        Ok(ConstValue::Struct(fields))
    }

    /// Returns integer width and signedness for a resolved expression.
    fn resolved_integer_semantics(
        &mut self,
        _expr: &ResolvedConstExpr,
    ) -> Option<ConstIntegerSemantics> {
        None
    }

    /// Returns floating-point precision for a resolved expression.
    fn resolved_float_semantics(
        &mut self,
        _expr: &ResolvedConstExpr,
    ) -> Option<ConstFloatSemantics> {
        None
    }

    /// Returns floating-point precision of a resolved compound-assignment target.
    fn resolved_assignment_float_semantics(
        &mut self,
        _assign: &nia_const_ir::ResolvedConstAssign,
    ) -> Option<ConstFloatSemantics> {
        None
    }

    /// Returns integer width and signedness of a resolved compound-assignment target.
    fn resolved_assignment_integer_semantics(
        &mut self,
        _assign: &nia_const_ir::ResolvedConstAssign,
    ) -> Option<ConstIntegerSemantics> {
        None
    }

    /// Resolves a value from its authoritative semantic name identity.
    fn resolve_resolved_name(
        &mut self,
        span: Span,
        resolution: ConstNameResolution,
    ) -> Result<ConstValue, ConstError>;

    /// Evaluates a layout builtin for a resolved type argument.
    fn resolve_resolved_layout_builtin(
        &mut self,
        span: Span,
        builtin: LayoutBuiltin,
        type_arg: &ResolvedConstTypeArg,
    ) -> Result<ConstValue, ConstError>;

    /// Evaluates a field-offset builtin for a resolved type argument.
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

    /// Dispatches a resolved const-function call after argument evaluation.
    fn call_resolved_function(
        &mut self,
        span: Span,
        callee: &ResolvedConstExpr,
        generic_args: &[ResolvedConstGenericArg],
        arg_exprs: &[ResolvedConstExpr],
        receiver_place: Option<&crate::ResolvedConstPlace>,
        args: Vec<ConstValue>,
    ) -> Result<ConstValue, ConstError> {
        let _ = callee;
        let _ = generic_args;
        let _ = arg_exprs;
        let _ = receiver_place;
        let _ = args;
        Err(ConstError {
            span,
            message: "unsupported resolved const function call".to_string(),
        })
    }

    /// Converts an iterable value into interpreter iterator state.
    fn resolved_for_iterator(
        &mut self,
        span: Span,
        iterable: &ResolvedConstExpr,
        value: ConstValue,
    ) -> Result<crate::ResolvedConstIterator, ConstError> {
        let _ = iterable;
        let _ = value;
        Err(ConstError {
            span,
            message: "const Iterable execution is not available in this context".to_string(),
        })
    }

    /// Advances iterator state and returns the yielded value.
    fn resolved_iterator_next(
        &mut self,
        span: Span,
        iterator: crate::ResolvedConstIterator,
    ) -> Result<(crate::ResolvedConstIterator, ConstValue), ConstError> {
        let _ = iterator;
        Err(ConstError {
            span,
            message: "const Iterator execution is not available in this context".to_string(),
        })
    }

    /// Binds one resolved const-function parameter in the active frame.
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

    /// Binds one resolved local declaration in the active scope.
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

    /// Assigns a value to a resolved local or projected target.
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

    /// Writes a rebuilt root value back to a resolved local place.
    fn assign_resolved_place_local(
        &mut self,
        span: Span,
        local_id: nia_ids::LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = local_id;
        let _ = value;
        Err(ConstError {
            span,
            message: "resolved const place writeback is not available in this context".to_string(),
        })
    }

    /// Binds one local introduced by a resolved pattern match.
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
            message: "resolved const match pattern locals are not available in this context"
                .to_string(),
        })
    }

    /// Binds a resolved function-pattern local using declaration metadata.
    fn bind_resolved_function_pattern_local(
        &mut self,
        span: Span,
        binding: &ResolvedConstPatternBinding,
        name: &SymbolId,
        local_id: nia_ids::LocalId,
        value: ConstValue,
    ) -> Result<(), ConstError> {
        let _ = binding;
        self.bind_resolved_pattern_local(span, name, local_id, value)
    }
}

#[derive(Default)]
/// Minimal environment that rejects every compiler-dependent operation.
///
/// Useful for evaluating self-contained literals and for tests that need to
/// prove resolved identities do not silently fall back to source names.
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
