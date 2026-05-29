// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{CheckedProgram, load_program, load_program_with_map, query::check_loaded_program};

use nia_imports::ModuleMap;

pub fn check_program(root_path: impl Into<String>) -> CheckedProgram {
    check_loaded_program(load_program(root_path))
}

pub fn check_program_with_map(
    root_path: impl Into<String>,
    module_map: ModuleMap,
) -> CheckedProgram {
    check_loaded_program(load_program_with_map(root_path, module_map))
}
