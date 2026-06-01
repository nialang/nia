// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{CheckedProgram, load_program, load_program_with_map};

use nia_imports::ModuleMap;
use nia_opt::OptimizationLevel;

pub fn check_program(root_path: impl Into<String>) -> CheckedProgram {
    check_program_with_options(root_path, OptimizationLevel::default())
}

pub fn check_program_with_options(
    root_path: impl Into<String>,
    optimization: OptimizationLevel,
) -> CheckedProgram {
    nia_compiler_query::check_loaded_program_with_options(load_program(root_path), optimization)
}

pub fn check_program_with_map(
    root_path: impl Into<String>,
    module_map: ModuleMap,
) -> CheckedProgram {
    check_program_with_map_and_options(root_path, module_map, OptimizationLevel::default())
}

pub fn check_program_with_map_and_options(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    optimization: OptimizationLevel,
) -> CheckedProgram {
    nia_compiler_query::check_loaded_program_with_options(
        load_program_with_map(root_path, module_map),
        optimization,
    )
}
