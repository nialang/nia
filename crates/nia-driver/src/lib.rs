// SPDX-License-Identifier: GPL-3.0-or-later
mod pipeline;

pub use nia_compiler_query::{
    CheckedModule, CheckedProgram, LoadedModule, LoadedProgram, ProgramDiagnostic,
};
pub use nia_loader_query::{load_program, load_program_with_map};
pub use pipeline::{check_program, check_program_with_map};

#[cfg(test)]
mod tests;
