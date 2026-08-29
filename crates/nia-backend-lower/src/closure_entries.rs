// SPDX-License-Identifier: GPL-3.0-or-later
//! Backend closure-entry ABI materialization shared by source and generic owners.

use std::collections::HashMap;

use crate::ModuleLowerer;
use nia_backend_ir::{
    BackendClosureEntry, BackendClosureEntryAbi, BackendClosureEntryKey, BackendClosureEntryOwner,
};
use nia_diagnostic::Diagnostic;
use nia_function_ir::{FunctionBody, FunctionClosureEntry};
use nia_ids::{InternedTyId, LocalId};

impl ModuleLowerer<'_> {
    /// Materializes the ABI and body metadata for one generated closure entry.
    ///
    /// `state_type`, `return_type`, and `body` must all belong to the same
    /// substitution context. Source closures pass their original values;
    /// generic function instances pass the three values after applying the
    /// instance substitution. Keeping local lookup here prevents the source
    /// and instance paths from assigning different ABI parameter order or
    /// types to the same closure entry shape.
    pub(crate) fn materialize_closure_entry(
        &mut self,
        entry: &FunctionClosureEntry,
        owner: BackendClosureEntryOwner,
        owner_symbol: &str,
        state_type: InternedTyId,
        return_type: InternedTyId,
        body: FunctionBody,
    ) -> Option<BackendClosureEntry> {
        let local_types = body
            .locals
            .iter()
            .map(|local| (local.id, local.ty))
            .collect::<HashMap<_, _>>();
        let state_pointer_type = closure_param_type(
            &local_types,
            entry.state_param,
            entry,
            "state parameter",
            &mut self.diagnostics,
        )?;
        let mut params = Vec::with_capacity(entry.params.len());
        for param in &entry.params {
            params.push(closure_param_type(
                &local_types,
                *param,
                entry,
                "parameter",
                &mut self.diagnostics,
            )?);
        }
        Some(BackendClosureEntry {
            key: BackendClosureEntryKey {
                closure_id: entry.closure_id,
                owner,
            },
            symbol: nia_mangle::mangle_closure_entry_symbol(owner_symbol, entry.closure_id),
            abi: BackendClosureEntryAbi {
                state_type,
                state_pointer_type,
                params,
                return_type,
            },
            state_param: entry.state_param,
            params: entry.params.clone(),
            local_names: self.function_local_names(&body),
            span: body.span,
            function_body: body,
        })
    }
}

fn closure_param_type(
    local_types: &HashMap<LocalId, InternedTyId>,
    local_id: LocalId,
    entry: &FunctionClosureEntry,
    role: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<InternedTyId> {
    local_types.get(&local_id).copied().or_else(|| {
        diagnostics.push(
            Diagnostic::internal_error(
                nia_diagnostic::codes::INVALID_BACKEND_IR,
                format!("closure entry is missing {role} local"),
            )
            .debug("closure_id", entry.closure_id)
            .debug("local_id", local_id)
            .finish(),
        );
        None
    })
}
