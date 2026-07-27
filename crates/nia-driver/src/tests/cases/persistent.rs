// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::tests::common::checked_program_from_output;

use super::support::{
    CaseManifest, case_expects_errors, copy_case_tree, diagnostic_snapshot, fixture_relative_path,
};

struct PersistentCheckCase {
    source: PathBuf,
    expects_errors: bool,
    cache_product: PathBuf,
}

pub(super) fn run(root: &Path) {
    let mut cases = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {} cases: {error}", root.display()))
        .map(|entry| entry.expect("read case entry").path())
        .collect::<Vec<_>>();
    cases.sort();
    assert!(!cases.is_empty(), "persistent suite must contain cases");

    for case_root in cases {
        assert!(
            case_root.is_dir(),
            "persistent case {} must be a directory",
            case_root.display()
        );
        run_case(&case_root);
    }
}

fn run_case(case_root: &Path) {
    let case = load_case(case_root);
    let workspace = crate::tests::common::temp_dir("persistent-check-case");
    copy_case_tree(case_root, &workspace);
    let source = workspace.join(&case.source);
    assert!(
        source.is_file(),
        "missing persistent source {}",
        source.display()
    );
    let cache = workspace.join("cache");
    let compile = |verify_frontend_cache| {
        let driver = crate::Driver::with_config(crate::DriverConfig {
            artifact_cache_dir: Some(cache.clone()),
            verify_frontend_cache,
            ..crate::DriverConfig::default()
        });
        checked_program_from_output(driver.check_entry(crate::CheckRequest::new(
            source.to_string_lossy().into_owned(),
        )))
    };

    let cold = compile(false);
    let warm = compile(false);
    let verified = compile(true);
    assert_eq!(cold.diagnostics, warm.diagnostics);
    assert_eq!(cold.diagnostics, verified.diagnostics);
    assert_eq!(
        nia_compiler_query::has_error_diagnostics(&cold.diagnostics),
        case.expects_errors,
        "{} error classification mismatch",
        source.display()
    );
    let snapshot_path = case_root.join(&case.source).with_extension("snap");
    let expected = fs::read_to_string(&snapshot_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", snapshot_path.display()));
    assert_eq!(
        diagnostic_snapshot(&cold.diagnostics, &workspace),
        expected,
        "{} persistent diagnostic snapshot changed",
        source.display()
    );
    let cache_product = cache.join(&case.cache_product);
    assert!(
        fs::read_dir(&cache_product)
            .unwrap_or_else(|error| panic!("read {}: {error}", cache_product.display()))
            .next()
            .is_some(),
        "{} cache product must contain an artifact",
        cache_product.display()
    );
}

fn load_case(case_root: &Path) -> PersistentCheckCase {
    let mut manifest = CaseManifest::load(case_root);
    let manifest_path = manifest.path.clone();
    manifest.expect("mode", "persistent-check");
    manifest.expect("resource", "compiler");
    let source = fixture_relative_path(&manifest_path, manifest.required("source"));
    let expects_errors = case_expects_errors(&manifest_path, "expect", manifest.required("expect"));
    let cache_product = fixture_relative_path(&manifest_path, manifest.required("cache-product"));
    manifest.finish();
    PersistentCheckCase {
        source,
        expects_errors,
        cache_product,
    }
}
