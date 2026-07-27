// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::support::{
    CaseManifest, assert_check_case, case_expects_errors, copy_case_tree, fixture_relative_path,
};

struct CheckSuite {
    name: &'static str,
    expects_errors: bool,
}

struct IncrementalCheckCase {
    source: PathBuf,
    edited_source: PathBuf,
    initial_expects_errors: bool,
    edited_expects_errors: bool,
}

pub(super) fn run(driver: &crate::Driver, root: &Path) {
    for suite in [
        CheckSuite {
            name: "pass",
            expects_errors: false,
        },
        CheckSuite {
            name: "fail",
            expects_errors: true,
        },
    ] {
        run_check_suite(driver, root, suite);
    }
    run_incremental_check_suite(driver, root);
}

fn run_check_suite(driver: &crate::Driver, root: &Path, suite: CheckSuite) {
    let mut cases = fs::read_dir(root.join(suite.name))
        .unwrap_or_else(|error| panic!("read {} cases: {error}", suite.name))
        .filter_map(|entry| {
            let path = entry.expect("read case entry").path();
            if path.extension().is_some_and(|extension| extension == "nia") {
                Some(path)
            } else if path.is_dir() {
                let entry = path.join("main.nia");
                assert!(
                    entry.is_file(),
                    "{} case must contain main.nia",
                    path.display()
                );
                Some(entry)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "{} suite must contain cases", suite.name);
    for source in cases {
        let snapshot_path = source.with_extension("snap");
        assert_check_case(driver, root, &source, suite.expects_errors, &snapshot_path);
    }
}

fn run_incremental_check_suite(driver: &crate::Driver, root: &Path) {
    let suite_root = root.join("incremental");
    let mut cases = fs::read_dir(&suite_root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", suite_root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(
        !cases.is_empty(),
        "incremental suite must contain directory cases"
    );

    for case_root in cases {
        assert!(
            case_root.is_dir(),
            "incremental case {} must be a directory",
            case_root.display()
        );
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
    driver.set_source(source.to_string_lossy().into_owned(), edited_text);
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
    let manifest_path = manifest.path.clone();
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
