// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{CheckedProgram, load_program, load_program_with_map};

use nia_imports::ModuleMap;
use nia_opt::NiaOptimizationLevel;

pub fn check_program(root_path: impl Into<String>) -> CheckedProgram {
    check_program_with_options(root_path, NiaOptimizationLevel::default())
}

pub fn check_program_with_options(
    root_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> CheckedProgram {
    nia_compiler_query::check_loaded_program_with_options(load_program(root_path), optimization)
}

pub fn check_program_with_map(
    root_path: impl Into<String>,
    module_map: ModuleMap,
) -> CheckedProgram {
    check_program_with_map_and_options(root_path, module_map, NiaOptimizationLevel::default())
}

pub fn check_program_with_map_and_options(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
) -> CheckedProgram {
    nia_compiler_query::check_loaded_program_with_options(
        load_program_with_map(root_path, module_map),
        optimization,
    )
}
