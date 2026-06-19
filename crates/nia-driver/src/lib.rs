// SPDX-License-Identifier: GPL-3.0-or-later
mod pipeline;

pub use nia_compiler_query::{
    BackendOptimizationChange, CheckedModule, CheckedProgram, LoadedModule, LoadedProgram,
    ProgramDiagnostic, TimingMode,
};
pub use nia_opt::{NiaOptimizationLevel, OptimizationPolicy};
pub use pipeline::{
    CheckRequest, Driver, DriverConfig, DriverError, DriverOutput, EmitLlvmRequest,
    EmitObjectRequest, ExecutableArtifact, ExecutableLinker, LinkExecutableRequest, LlvmIrArtifact,
    ObjectArtifact, ObjectOutput, Runtime, WriteObjectRequest, WrittenObjectArtifact,
};

#[cfg(test)]
mod tests;
