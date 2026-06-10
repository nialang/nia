// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{CheckedProgram, load_program, load_program_with_map};

use nia_compiler_query::TimingMode;
use nia_imports::ModuleMap;
use nia_loader_query::{EntryRuntime, load_program_with_map_and_entry_runtime};
use nia_opt::NiaOptimizationLevel;

pub fn check_program(root_path: impl Into<String>) -> CheckedProgram {
    check_program_with_options(root_path, NiaOptimizationLevel::default())
}

pub fn check_program_with_options(
    root_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> CheckedProgram {
    let _permit = check_test_permit();
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
    check_program_with_map_options_and_timings(root_path, module_map, optimization, TimingMode::Off)
}

pub fn check_program_with_map_options_and_timings(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: TimingMode,
) -> CheckedProgram {
    let _permit = check_test_permit();
    nia_compiler_query::check_loaded_program_with_options_and_timings(
        load_program_with_map(root_path, module_map),
        optimization,
        timings,
    )
}

pub fn check_freestanding_executable_with_map_and_options(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
) -> CheckedProgram {
    check_freestanding_executable_with_map_options_and_timings(
        root_path,
        module_map,
        optimization,
        TimingMode::Off,
    )
}

pub fn check_freestanding_executable_with_map_options_and_timings(
    root_path: impl Into<String>,
    module_map: ModuleMap,
    optimization: NiaOptimizationLevel,
    timings: TimingMode,
) -> CheckedProgram {
    let _permit = check_test_permit();
    nia_compiler_query::check_loaded_program_with_options_and_timings(
        load_program_with_map_and_entry_runtime(root_path, module_map, EntryRuntime::Freestanding),
        optimization,
        timings,
    )
}

pub fn check_freestanding_executable_with_options(
    root_path: impl Into<String>,
    optimization: NiaOptimizationLevel,
) -> CheckedProgram {
    check_freestanding_executable_with_map_and_options(
        root_path,
        ModuleMap::default(),
        optimization,
    )
}

#[cfg(test)]
fn check_test_permit() -> CheckTestPermit {
    const MAX_CHECKS: usize = 4;

    let (running, available) = check_test_limit();
    let mut count = running
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while *count >= MAX_CHECKS {
        count = available
            .wait(count)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    *count += 1;
    CheckTestPermit
}

#[cfg(not(test))]
fn check_test_permit() -> CheckTestPermit {
    CheckTestPermit
}

struct CheckTestPermit;

#[cfg(test)]
impl Drop for CheckTestPermit {
    fn drop(&mut self) {
        let (running, available) = check_test_limit();
        let mut count = running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *count -= 1;
        available.notify_one();
    }
}

#[cfg(test)]
fn check_test_limit() -> &'static (std::sync::Mutex<usize>, std::sync::Condvar) {
    use std::sync::{Condvar, Mutex, OnceLock};

    static LIMIT: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    LIMIT.get_or_init(|| (Mutex::new(0), Condvar::new()))
}
