// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_diagnostic::{Diagnostic, codes};
use nia_function_ir::{validate_function_body, validate_function_closure_entry};
use nia_ids::{ClosureId, GlobalDefId};

use crate::BackendLowerModuleInput;

pub(crate) fn validate_backend_lowering_inputs(
    modules: &[BackendLowerModuleInput<'_>],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut validated = HashSet::new();
    if let Some(input) = modules.first() {
        validate_function_bodies(
            input
                .program
                .function_body_ids()
                .iter()
                .filter_map(|def_id| {
                    input
                        .program
                        .function_body(*def_id)
                        .map(|body| (*def_id, body))
                }),
            &mut validated,
            &mut diagnostics,
        );
        validate_closure_entries(input, &mut diagnostics);
    }
    diagnostics
}

fn validate_closure_entries(
    input: &BackendLowerModuleInput<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut validated = HashSet::<ClosureId>::new();
    for def_id in input.program.function_body_ids() {
        for entry in input.program.closure_entries(*def_id) {
            if entry.closure_id.owner != *def_id {
                diagnostics.push(
                    Diagnostic::internal_error(
                        codes::INVALID_FUNCTION_IR,
                        "closure entry owner does not match its source function",
                    )
                    .primary(
                        entry.body.span,
                        "generated closure entry has an invalid owner",
                    )
                    .debug("function_def_id", def_id)
                    .debug("closure_id", entry.closure_id)
                    .finish(),
                );
            }
            if !validated.insert(entry.closure_id) {
                diagnostics.push(
                    Diagnostic::internal_error(
                        codes::INVALID_FUNCTION_IR,
                        "duplicate closure entry identity passed to backend lowering",
                    )
                    .primary(entry.body.span, "closure identity is not unique")
                    .debug("closure_id", entry.closure_id)
                    .finish(),
                );
            }
            if let Err(error) = validate_function_closure_entry(entry) {
                diagnostics.push(
                    Diagnostic::internal_error(
                        codes::INVALID_FUNCTION_IR,
                        "invalid closure entry IR passed to backend lowering",
                    )
                    .primary(error.span, error.message)
                    .debug("closure_id", entry.closure_id)
                    .finish(),
                );
            }
        }
    }
}

pub(crate) fn unreachable_invalid_function_ir(node: &'static str) -> ! {
    panic!("Nia ICE: invalid function IR reached backend lowering pass: {node}");
}

fn validate_function_bodies<'a>(
    bodies: impl IntoIterator<Item = (GlobalDefId, &'a nia_function_ir::FunctionBody)>,
    validated: &mut HashSet<GlobalDefId>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (def_id, body) in bodies {
        if !validated.insert(def_id) {
            continue;
        }
        if let Err(error) = validate_function_body(body) {
            diagnostics.push(
                Diagnostic::internal_error(
                    codes::INVALID_FUNCTION_IR,
                    "invalid function IR passed to backend lowering",
                )
                .primary(error.span, error.message)
                .debug("function_def_id", def_id)
                .finish(),
            );
        }
    }
}
