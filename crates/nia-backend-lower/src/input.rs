// SPDX-License-Identifier: GPL-3.0-or-later
use std::collections::HashSet;

use nia_diagnostic::{Diagnostic, codes};
use nia_function_ir::validate_function_body;
use nia_ids::GlobalDefId;

use crate::BackendLowerModuleInput;

pub(crate) fn validate_backend_lowering_inputs(
    modules: &[BackendLowerModuleInput<'_>],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut validated = HashSet::new();
    for input in modules {
        validate_function_bodies(
            input
                .function_bodies
                .iter()
                .map(|(def_id, _, body)| (def_id, body)),
            &mut validated,
            &mut diagnostics,
        );
        validate_function_bodies(
            input
                .program_function_bodies
                .iter()
                .map(|(def_id, body)| (*def_id, *body)),
            &mut validated,
            &mut diagnostics,
        );
    }
    diagnostics
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
