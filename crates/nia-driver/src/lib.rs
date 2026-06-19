// SPDX-License-Identifier: GPL-3.0-or-later
mod pipeline;

pub use nia_compiler_query::{
    BackendOptimizationChange, CheckedModule, CheckedProgram, LoadedModule, LoadedProgram,
    ProgramDiagnostic, TimingMode,
};
pub use nia_loader_query::{load_program, load_program_with_map};
pub use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
pub use pipeline::{
    CheckRequest, Driver, DriverError, DriverOutput, EmitLlvmRequest, EmitObjectRequest,
    ExecutableArtifact, ExecutableLinker, LinkExecutableRequest, LlvmIrArtifact, ObjectArtifact,
    ObjectOutput, Runtime, WriteObjectRequest, WrittenObjectArtifact,
    check_freestanding_executable_with_map_and_options,
    check_freestanding_executable_with_map_options_and_timings,
    check_freestanding_executable_with_options, check_program, check_program_request,
    check_program_with_map, check_program_with_map_and_options,
    check_program_with_map_options_and_timings, check_program_with_options,
};

#[cfg(test)]
mod tests;
