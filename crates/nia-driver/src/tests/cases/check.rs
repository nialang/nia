// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use nia_test_support::{CaseManifest, case_directories, copy_case_tree, fixture_relative_path};

use super::support::{assert_check_case, assert_check_case_with_module_map, case_expects_errors};

struct CheckCase {
    source: PathBuf,
    expects_errors: bool,
    module_map: BTreeMap<String, PathBuf>,
}

struct IncrementalCheckCase {
    source: PathBuf,
    edited_source: PathBuf,
    initial_expects_errors: bool,
    edited_expects_errors: bool,
}

pub(super) fn run(driver: &crate::Driver, root: &Path) {
    run_check_suite(driver, root, "pass");
    run_check_suite(driver, root, "fail");
    run_incremental_check_suite(driver, root);
}

fn run_check_suite(driver: &crate::Driver, root: &Path, suite: &str) {
    for case_root in case_directories(&root.join(suite), suite) {
        let case = load_check_case(&case_root);
        let source = case_root.join(case.source);
        assert!(
            source.is_file(),
            "missing check source {}",
            source.display()
        );
        let snapshot_path = source.with_extension("snap");
        if case.module_map.is_empty() {
            assert_check_case(driver, root, &source, case.expects_errors, &snapshot_path);
        } else {
            let mut module_map = crate::ModuleMap::new();
            for (name, relative_path) in case.module_map {
                let path = case_root.join(relative_path);
                assert!(
                    path.is_file(),
                    "missing mapped module {} for {name}",
                    path.display()
                );
                module_map
                    .insert(
                        name,
                        crate::SourcePath::new(path.to_string_lossy().into_owned()),
                    )
                    .expect("insert module map entry");
            }
            assert_check_case_with_module_map(
                driver,
                root,
                &source,
                case.expects_errors,
                module_map,
                &snapshot_path,
            );
        }
    }
}

fn load_check_case(case_root: &Path) -> CheckCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path().to_owned();
    manifest.expect("mode", "check");
    manifest.expect("resource", "compiler");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let expects_errors = case_expects_errors(&manifest_path, "expect", manifest.required("expect"));
    let module_map = manifest.required_prefixed_paths("module.");
    manifest.finish();
    CheckCase {
        source,
        expects_errors,
        module_map,
    }
}

fn run_incremental_check_suite(driver: &crate::Driver, root: &Path) {
    let suite_root = root.join("incremental");
    for case_root in case_directories(&suite_root, "incremental") {
        run_incremental_check_case(driver, &case_root);
    }
}

fn run_incremental_check_case(driver: &crate::Driver, case_root: &Path) {
    let case = load_incremental_check_case(case_root);
    let workspace = crate::tests::common::temp_dir("check-incremental-case");
    copy_case_tree(case_root, &workspace);

    let source = workspace.join(&case.source);
    let edited_source = workspace.join(&case.edited_source);
    assert!(
        source.is_file(),
        "missing incremental source {}",
        source.display()
    );
    assert!(
        edited_source.is_file(),
        "missing incremental edit {}",
        edited_source.display()
    );
    let initial_snapshot = case_root.join(&case.source).with_extension("snap");
    assert_check_case(
        driver,
        &workspace,
        &source,
        case.initial_expects_errors,
        &initial_snapshot,
    );

    fs::copy(&edited_source, &source).unwrap_or_else(|error| {
        panic!(
            "replace incremental source {} with {}: {error}",
            source.display(),
            edited_source.display()
        )
    });
    let edited_text = fs::read_to_string(&source)
        .unwrap_or_else(|error| panic!("read edited source {}: {error}", source.display()));
    driver
        .set_source(source.to_string_lossy().into_owned(), edited_text)
        .expect("replace source");
    let edited_snapshot = case_root.join(&case.source).with_extension("after.snap");
    assert_check_case(
        driver,
        &workspace,
        &source,
        case.edited_expects_errors,
        &edited_snapshot,
    );
}

fn load_incremental_check_case(case_root: &Path) -> IncrementalCheckCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path().to_owned();
    manifest.expect("mode", "incremental-check");
    manifest.expect("resource", "compiler");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let edited_source = fixture_relative_path(&manifest_path, manifest.required("edit"));
    let case = IncrementalCheckCase {
        source,
        edited_source,
        initial_expects_errors: case_expects_errors(
            &manifest_path,
            "initial",
            manifest.required("initial"),
        ),
        edited_expects_errors: case_expects_errors(
            &manifest_path,
            "after",
            manifest.required("after"),
        ),
    };
    manifest.finish();
    case
}
