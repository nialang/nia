// SPDX-License-Identifier: GPL-3.0-or-later
use std::{path::Path, process::Command};

#[allow(dead_code, unused_imports)]
mod support;

use nia_test_support::{
    CaseManifest, CommandExt, CommandStatusExt, TestWorkload, case_directories,
    fixture_relative_path,
};

#[test]
fn linker_cases_match_expectations() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Build);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/linker");
    for case_root in case_directories(&root, "linker") {
        let mut manifest = CaseManifest::load(&case_root);
        let manifest_path = manifest.path().to_owned();
        let mode = manifest.required("mode");
        manifest.expect("resource", TestWorkload::Build.as_str());
        let source = case_root.join(fixture_relative_path(
            &manifest_path,
            manifest.required("source"),
        ));
        match mode.as_str() {
            "linker-selection-errors" => {
                let reserved_error = manifest.required("reserved-error");
                let missing_error = manifest.required("missing-error");
                let bare_runtime_error = manifest.required("bare-runtime-error");
                manifest.finish();
                run_selection_errors(
                    &source,
                    &reserved_error,
                    &missing_error,
                    &bare_runtime_error,
                );
            }
            "linker-invocation" => {
                manifest.expect("platform", "unix");
                let raw = manifest.required_list("raw-args");
                let structured = manifest.required_list("structured-args");
                manifest.finish();
                run_invocation(&source, &raw, &structured);
            }
            "typed-link-cache" => {
                manifest.expect("platform", "unix");
                let edit = case_root.join(fixture_relative_path(
                    &manifest_path,
                    manifest.required("edit"),
                ));
                manifest.finish();
                run_typed_link_cache(&source, &edit);
            }
            _ => panic!(
                "unknown linker case mode {mode:?} in {}",
                manifest_path.display()
            ),
        }
    }
}

#[cfg(unix)]
fn run_typed_link_cache(initial: &Path, edit: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("nia-typed-link-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create typed link cache directory");
    let source = root.join("main.nia");
    std::fs::copy(initial, &source).expect("copy initial link cache source");
    let cache = root.join("cache");
    let linker = root.join("linker.sh");
    let invocation_log = root.join("linker-invocations");
    std::fs::write(
        &linker,
        format!(
            "#!/bin/sh\nprintf x >> '{}'\nexec ld \"$@\"\n",
            invocation_log.display()
        ),
    )
    .expect("write linker cache wrapper");
    let mut permissions = std::fs::metadata(&linker)
        .expect("linker metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&linker, permissions).expect("make linker executable");

    let first = root.join("first");
    let second = root.join("second");
    let mut timings = Vec::new();
    for output in [&first, &second] {
        let result = cached_link(&source, &cache, &linker, output);
        assert_success(&result);
        timings.push(String::from_utf8(result.stderr).expect("timings are UTF-8"));
    }
    assert_eq!(
        std::fs::read_to_string(&invocation_log).expect("read invocations"),
        "x"
    );
    assert_eq!(
        std::fs::read(&first).expect("read first"),
        std::fs::read(&second).expect("read second")
    );
    let keys = std::fs::read_dir(cache.join("artifacts/links/v2"))
        .expect("read link cache keys")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect link cache keys");
    assert_eq!(keys.len(), 1);
    assert_eq!(
        std::fs::read_dir(keys[0].path())
            .expect("read link entries")
            .count(),
        1
    );
    assert!(
        Command::new(&second)
            .status_timeout("run restored executable")
            .success()
    );

    assert_counters(
        &timings[0],
        &[
            "link.result_reuse_hits: 0",
            "link.result_reuse_misses: 1",
            "link.result_reuse_miss_not_found: 1",
            "llvm.object_reuse_miss_not_found:",
        ],
    );
    assert_counters(
        &timings[1],
        &[
            "link.result_reuse_hits: 1",
            "link.result_reuse_misses: 0",
            "llvm.object_reuse_miss_not_found: 0",
        ],
    );

    std::fs::copy(edit, &source).expect("apply link cache source edit");
    let changed = cached_link(&source, &cache, &linker, &root.join("changed"));
    assert_success(&changed);
    let changed = String::from_utf8(changed.stderr).expect("changed timings are UTF-8");
    assert_counters(
        &changed,
        &[
            "llvm.object_reuse_miss_invalidated: 1",
            "llvm.object_invalidation_definition: 1",
            "llvm.object_invalidation_policy: 0",
            "llvm.object_invalidation_declarations: 0",
            "llvm.object_invalidation_target: 0",
            "link.result_reuse_miss_invalidated: 1",
            "link.result_invalidation_inputs: 1",
            "link.result_invalidation_target: 0",
            "link.result_invalidation_linker: 0",
            "link.result_invalidation_options: 0",
        ],
    );
    assert!(
        !changed.contains("timing summary counter llvm.object_reuse_hits: 0"),
        "{changed}"
    );
    assert_eq!(
        std::fs::read_to_string(&invocation_log).expect("read changed invocations"),
        "xx"
    );
}

#[cfg(not(unix))]
fn run_typed_link_cache(_initial: &Path, _edit: &Path) {}

#[cfg(unix)]
fn cached_link(source: &Path, cache: &Path, linker: &Path, output: &Path) -> std::process::Output {
    support::nia_command()
        .arg("--timings")
        .env("NIA_LINKER", linker)
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--cache-dir")
        .arg(cache)
        .arg("-o")
        .arg(output)
        .output_timeout_without_resources("run typed link-result cache case")
}

fn assert_counters(report: &str, expected: &[&str]) {
    for expected in expected {
        let counter = format!("timing summary counter {expected}");
        assert!(report.contains(&counter), "missing {counter:?} in {report}");
    }
}

fn run_selection_errors(
    source: &Path,
    reserved_error: &str,
    missing_error: &str,
    bare_runtime_error: &str,
) {
    let output = std::env::temp_dir().join(format!("nia-linker-case-{}", std::process::id()));
    let reserved = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--linker-flavor")
        .arg("self-hosted-elf")
        .arg("-o")
        .arg(&output)
        .output_timeout_without_resources("run reserved linker flavor case");
    assert_error(&reserved, reserved_error);

    let missing = support::nia_command()
        .env("PATH", "")
        .env_remove("NIA_LLD")
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--linker-flavor")
        .arg("lld")
        .arg("-o")
        .arg(&output)
        .output_timeout_without_resources("run missing LLD case");
    assert_error(&missing, missing_error);

    let bare = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--runtime")
        .arg("bare")
        .arg("-o")
        .arg(&output)
        .output_timeout_without_resources("run bare executable runtime case");
    assert_error(&bare, bare_runtime_error);
}

#[cfg(unix)]
fn run_invocation(source: &Path, raw: &[String], structured: &[String]) {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("nia-linker-invocation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create linker invocation directory");
    let linker = root.join("linker.sh");
    let args_log = root.join("linker.args");
    std::fs::write(
        &linker,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            args_log.display()
        ),
    )
    .expect("write mock linker");
    let mut permissions = std::fs::metadata(&linker)
        .expect("mock linker metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&linker, permissions).expect("make mock linker executable");

    let executable = root.join("raw-main");
    let raw_output = support::nia_command()
        .env("NIA_LINKER", &linker)
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--link-arg")
        .arg("-lc")
        .arg("--link-arg=-lm")
        .arg("--link-arg")
        .arg("-Olinker")
        .arg("-o")
        .arg(&executable)
        .output_timeout_without_resources("run raw linker arguments case");
    assert_success(&raw_output);
    assert_args(&args_log, raw);

    let structured_output = support::nia_command()
        .env("NIA_LINKER", &linker)
        .arg("emit")
        .arg("--exe")
        .arg(source)
        .arg("--dynamic-linker")
        .arg("/loader")
        .arg("--linker")
        .arg(&linker)
        .arg("--linker-flavor")
        .arg("lld")
        .arg("-L")
        .arg("/native/lib")
        .arg("-l")
        .arg("native_api")
        .arg("--rpath")
        .arg("$ORIGIN")
        .arg("-o")
        .arg(root.join("structured-main"))
        .output_timeout_without_resources("run structured linker arguments case");
    assert_success(&structured_output);
    assert_args(&args_log, structured);
}

#[cfg(not(unix))]
fn run_invocation(_source: &Path, _raw: &[String], _structured: &[String]) {}

fn assert_args(path: &Path, expected: &[String]) {
    let args = std::fs::read_to_string(path).expect("read mock linker arguments");
    for expected in expected {
        assert!(
            args.lines().any(|arg| arg == expected),
            "missing {expected:?} in {args}"
        );
    }
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_error(output: &std::process::Output, expected: &str) {
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "missing {expected:?} in {stderr}"
    );
}
