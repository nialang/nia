// SPDX-License-Identifier: GPL-3.0-or-later
mod loader;
mod pipeline;

use nia_diagnostic::Diagnostic;
use nia_imports::SourcePath;

pub use loader::{load_program, load_program_with_map};
pub use nia_compiler_query::{
    CheckedModule, CheckedProgram, LoadedModule, LoadedProgram, ProgramDiagnostic,
};
pub use pipeline::{check_program, check_program_with_map};

pub(crate) fn module_diagnostics(
    path: &SourcePath,
    diagnostics: &[Diagnostic],
) -> Vec<ProgramDiagnostic> {
    diagnostics
        .iter()
        .cloned()
        .map(|diagnostic| ProgramDiagnostic {
            path: path.clone(),
            diagnostic,
        })
        .collect()
}

#[cfg(test)]
mod tests;
