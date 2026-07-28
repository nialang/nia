// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

mod backend;
mod check;
mod persistent;
mod support;

#[test]
fn compiler_cases_match_snapshots() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Compiler);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let driver = crate::Driver::new();
    check::run(&driver, &root.join("check"));
    persistent::run(&root.join("persistent"));
    backend::run_compiler(&driver, &root);
}

#[test]
fn build_cases_match_expectations() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    backend::run_build(&crate::Driver::new(), &root);
}
