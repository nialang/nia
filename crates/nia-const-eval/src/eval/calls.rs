// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

struct EarlyConstCall<'a> {
    span: Span,
    function_module_id: ModuleId,
    params: &'a [EarlyConstParam],
    body: &'a EarlyConstBlock,
    type_substitutions: Vec<(SymbolId, InternedTyId)>,
    const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    args: Vec<ConstValue>,
}

fn eval_const_function_call(
    call: EarlyConstCall<'_>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    let EarlyConstCall {
        span,
        function_module_id,
        params,
        body,
        type_substitutions,
        const_substitutions,
        args,
    } = call;
    check_call_arity(span, params.len(), args.len())?;
    env.push_function_frame(span)?;
    // Once a frame exists, keep all fallible work inside one result boundary.
    // This makes frame restoration independent of which operation fails.
    let result = (|| {
        env.bind_function_context(
            span,
            function_module_id,
            None,
            type_substitutions,
            const_substitutions,
        )?;
        for (param, value) in params.iter().zip(args) {
            env.bind_function_param(param.span, param, value)?;
        }
        let value = call_result_value(body.span, super::eval_function_block(body, env)?)?;
        env.validate_const_function_result(span, &value)?;
        Ok(value)
    })();
    env.pop_function_frame();
    result
}

/// Evaluates an early const function in a fresh function frame.
///
/// Arity is checked before the frame is pushed. After a successful push, every
/// context/parameter/body/result failure passes through the shared cleanup
/// boundary before it is returned.
pub fn eval_early_const_function_call(
    span: Span,
    function_module_id: ModuleId,
    function: &EarlyConstFunction,
    type_substitutions: Vec<(SymbolId, InternedTyId)>,
    args: Vec<ConstValue>,
    env: &mut impl EarlyConstEnv,
) -> Result<ConstValue, ConstError> {
    super::with_const_eval_session(env, |env| {
        eval_const_function_call(
            EarlyConstCall {
                span,
                function_module_id,
                params: &function.params,
                body: &function.body,
                type_substitutions,
                const_substitutions: Vec::new(),
                args,
            },
            env,
        )
    })
}

/// Fully resolved inputs needed to execute one const function invocation.
pub struct ResolvedConstCallInput<'a> {
    pub span: Span,
    pub function_id: GlobalDefId,
    pub function_module_id: ModuleId,
    pub function: &'a ResolvedConstFunction,
    pub type_substitutions: Vec<(SymbolId, InternedTyId)>,
    pub const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    pub args: Vec<ConstValue>,
}

/// Evaluates a resolved const function and returns its value plus receiver state.
///
/// A mutable receiver is returned separately so the caller can write it back to
/// the original resolved place after the callee frame has been destroyed.
pub fn eval_resolved_const_function_call(
    input: ResolvedConstCallInput<'_>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ResolvedConstCallOutput, ConstError> {
    let ResolvedConstCallInput {
        span,
        function_id,
        function_module_id,
        function,
        type_substitutions,
        const_substitutions,
        args,
    } = input;
    super::with_const_eval_session(env, |env| {
        eval_resolved_const_function_call_inner(
            ResolvedConstCall {
                span,
                function_id,
                function_module_id,
                params: function.params(),
                body: function.body(),
                type_substitutions,
                const_substitutions,
                args,
            },
            env,
        )
    })
}

struct ResolvedConstCall<'a> {
    span: Span,
    function_id: GlobalDefId,
    function_module_id: ModuleId,
    params: &'a [ResolvedConstParam],
    body: &'a ResolvedConstBlock,
    type_substitutions: Vec<(SymbolId, InternedTyId)>,
    const_substitutions: Vec<(SymbolId, nia_ty::ConstGenericArg)>,
    args: Vec<ConstValue>,
}

fn eval_resolved_const_function_call_inner(
    call: ResolvedConstCall<'_>,
    env: &mut impl ResolvedConstEnv,
) -> Result<ResolvedConstCallOutput, ConstError> {
    let ResolvedConstCall {
        span,
        function_id,
        function_module_id,
        params,
        body,
        type_substitutions,
        const_substitutions,
        args,
    } = call;
    check_call_arity(span, params.len(), args.len())?;
    env.push_function_frame(span)?;
    let result = (|| {
        env.bind_function_context(
            span,
            function_module_id,
            Some(function_id),
            type_substitutions,
            const_substitutions,
        )?;
        for (param, value) in params.iter().zip(args) {
            env.bind_resolved_function_param(param.span(), param, value)?;
        }
        let value =
            call_result_value(body.span(), super::eval_resolved_function_block(body, env)?)?;
        env.validate_const_function_result(span, &value)?;
        let mutable_receiver = params
            .iter()
            .find(|param| param.receiver() == Some(nia_ids::ReceiverKind::Ref))
            .map(|param| {
                env.resolve_resolved_name(
                    param.span(),
                    ConstNameResolution::Local(param.local_id()),
                )
            })
            .transpose()?;
        if let Some(receiver) = &mutable_receiver {
            env.validate_const_function_result(span, receiver)?;
        }
        Ok(ResolvedConstCallOutput {
            value,
            mutable_receiver,
        })
    })();
    env.pop_function_frame();
    result
}

fn check_call_arity(span: Span, expected: usize, actual: usize) -> Result<(), ConstError> {
    if let ArityCheck::Mismatch { actual, .. } = check_exact_arity(expected, actual) {
        return Err(ConstError {
            span,
            message: format!(
                "const function argument count mismatch: expected {expected}, got {actual}"
            ),
        });
    }
    Ok(())
}

fn call_result_value(span: Span, flow: ConstEvalFlow) -> Result<ConstValue, ConstError> {
    match flow {
        ConstEvalFlow::Value(value)
        | ConstEvalFlow::Return(value)
        | ConstEvalFlow::Propagate(value) => Ok(value),
        ConstEvalFlow::Break | ConstEvalFlow::Continue => Err(ConstError {
            span,
            message: "const loop control flow escaped its loop".to_string(),
        }),
        ConstEvalFlow::Void => Err(ConstError {
            span,
            message: "const function must return a value".to_string(),
        }),
    }
}
