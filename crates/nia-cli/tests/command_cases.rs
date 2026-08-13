// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

#[allow(dead_code, unused_imports)]
mod support;

use nia_test_support::{
    CaseManifest, CommandExt, TestWorkload, case_directories, fixture_relative_path,
};

#[test]
fn command_cases_match_expectations() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Compiler);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/commands");
    for case_root in case_directories(&root, "commands") {
        let mut manifest = CaseManifest::load(&case_root);
        let manifest_path = manifest.path().to_owned();
        let mode = manifest.required("mode");
        manifest.expect("resource", TestWorkload::Compiler.as_str());
        let source = case_root.join(fixture_relative_path(
            &manifest_path,
            manifest.required("source"),
        ));
        match mode.as_str() {
            "option-errors" => {
                manifest.finish();
                run_option_errors(&source);
            }
            "option-placement" => {
                let mapped_entry = case_root.join(fixture_relative_path(
                    &manifest_path,
                    manifest.required("mapped-entry"),
                ));
                let mapped = case_root.join(fixture_relative_path(
                    &manifest_path,
                    manifest.required("mapped-source"),
                ));
                manifest.finish();
                run_option_placement(&source, &mapped_entry, &mapped);
            }
            "inspection-contracts" => {
                manifest.finish();
                run_inspection_contracts(&source);
            }
            "check-configuration" => {
                let cache_source = case_root.join(fixture_relative_path(
                    &manifest_path,
                    manifest.required("cache-source"),
                ));
                let private_source = case_root.join(fixture_relative_path(
                    &manifest_path,
                    manifest.required("private-source"),
                ));
                let public_source = case_root.join(fixture_relative_path(
                    &manifest_path,
                    manifest.required("public-source"),
                ));
                manifest.finish();
                run_check_configuration(&source, &cache_source, &private_source, &public_source);
            }
            _ => panic!(
                "unknown command case mode {mode:?} in {}",
                manifest_path.display()
            ),
        }
    }
}

fn run_check_configuration(
    default_std_source: &Path,
    cache_source: &Path,
    private_source: &Path,
    public_source: &Path,
) {
    assert_success(&command(["check"], default_std_source));

    let cache_root = support::temp_dir("command-cache");
    let cache = cache_root.join("cache");
    let mut first = support::nia_command();
    let first = first
        .arg("check")
        .arg(cache_source)
        .arg("--cache-dir")
        .arg(&cache)
        .output_timeout_in_session("run first persistent cache check");
    assert_success(&first);
    assert_frontend_cache_kind_has_entry(&cache, "public-surface-facts");
    let mut second = support::nia_command();
    let second = second
        .arg("check")
        .arg(format!("--cache-dir={}", cache.display()))
        .arg(cache_source)
        .output_timeout_in_session("run warm persistent cache check");
    assert_success(&second);

    assert_success(&command(["check"], private_source));
    let mut private_runtime = support::nia_command();
    let private_runtime = private_runtime
        .arg("check")
        .arg(private_source)
        .arg("--runtime")
        .arg("freestanding")
        .output_timeout_in_session("run private freestanding entry check");
    assert_failure(private_runtime, &["private", "entry::main"]);

    for _ in 0..2 {
        let mut public_runtime = support::nia_command();
        let public_runtime = public_runtime
            .arg("check")
            .arg(public_source)
            .arg("--runtime")
            .arg("freestanding")
            .output_timeout_in_session("run public freestanding entry check");
        assert_success(&public_runtime);
    }
    let mut repeated = support::nia_command();
    let repeated = repeated
        .arg("check")
        .arg(public_source)
        .arg("--runtime")
        .arg("freestanding")
        .arg("--runtime=bare")
        .output_timeout_in_session("run repeated runtime option check");
    assert_success(&repeated);

    let mut removed_alias = support::nia_command();
    let removed_alias = removed_alias
        .arg("check")
        .arg("--exe")
        .arg(public_source)
        .output_timeout_in_session("run removed check --exe alias");
    assert_failure(removed_alias, &["unknown `nia check` option `--exe`"]);
}

fn assert_frontend_cache_kind_has_entry(cache: &Path, kind: &str) {
    let frontend = cache.join("artifacts/frontend");
    for schema in std::fs::read_dir(&frontend).expect("read frontend cache") {
        let schema = schema.expect("read frontend cache schema entry");
        if !schema
            .file_type()
            .expect("inspect frontend cache schema entry")
            .is_dir()
        {
            continue;
        }
        let entries = match std::fs::read_dir(schema.path().join(kind)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("read frontend cache kind `{kind}`: {error}"),
        };
        if entries
            .map(|entry| entry.expect("read frontend cache kind entry"))
            .next()
            .is_some()
        {
            return;
        }
    }
    panic!(
        "frontend cache kind `{kind}` has no entries under `{}`",
        frontend.display()
    );
}

fn run_option_errors(source: &Path) {
    assert_failure(
        command(["--timings=trace", "check"], source),
        &["unknown timings mode `--timings=trace`", "--timings=detail"],
    );
    assert_failure(
        command(["--timings-format=csv", "check"], source),
        &["unknown timings format `csv`", "expected text or json"],
    );
    assert_failure(
        command(["--timing-trace=spans", "check"], source),
        &[
            "unknown timing trace mode `--timing-trace=spans`",
            "--timing-trace=events",
        ],
    );
    assert_failure(
        command(["-O9", "check"], source),
        &["unknown optimization level `-O9`", "-Oz"],
    );
    for reserved in ["entry", "pkg", "builtin"] {
        let mut process = support::nia_command();
        let output = process
            .arg("check")
            .arg(source)
            .arg("-M")
            .arg(format!("{reserved}={}", source.display()))
            .output_timeout_in_session("run reserved module-map root case");
        assert_failure(
            output,
            &[&format!("`{reserved}` is a compiler-reserved module root")],
        );
    }
}

fn run_option_placement(source: &Path, mapped_entry: &Path, mapped: &Path) {
    let before = command(["-O2", "emit", "--llvm"], source);
    assert_success(&before);
    assert_contains(&before.stdout, &["define i32 @"]);

    let mut trailing = support::nia_command();
    let trailing = trailing
        .arg("check")
        .arg(source)
        .arg("-Oz")
        .arg("--opt-report")
        .output_timeout_in_session("run trailing optimization option case");
    assert_success(&trailing);
    assert_contains(
        &trailing.stdout,
        &["policy level=Oz", "prefer_size=true", "llvm_size=tiny"],
    );

    let alias = command(["-O", "check", "--opt-report"], source);
    assert_success(&alias);
    assert_contains(
        &alias.stdout,
        &[
            "policy level=O2",
            "inline=normal",
            "specialize=normal",
            "llvm_codegen=default",
            "llvm_size=default",
        ],
    );

    let mut module_map = support::nia_command();
    let module_map = module_map
        .arg("check")
        .arg(mapped_entry)
        .arg("-M")
        .arg(format!("share={}", mapped.display()))
        .output_timeout_in_session("run trailing module-map option case");
    assert_success(&module_map);
}

fn run_inspection_contracts(source: &Path) {
    for (target, expected) in [
        ("--tokens", &["Fn", "Ident"][..]),
        ("--ast", &["FunctionItem", "name: SymbolId"]),
        ("--checked", &["CheckedProgram", "modules"]),
        ("--backend", &["BackendProgram", "functions", "main"]),
        ("--llvm", &["define i32 @", "ret i32"]),
    ] {
        let output = command(["emit", target], source);
        assert_success(&output);
        assert_contains(&output.stdout, expected);
        if target == "--checked" {
            assert!(!String::from_utf8_lossy(&output.stdout).contains("backend_lowering"));
        }
    }

    for removed in ["lex", "parse"] {
        assert_failure(
            command([removed], source),
            &[&format!("unknown command `{removed}`")],
        );
    }
    assert_failure(
        command(["emit", "obj"], source),
        &["old `nia emit obj` syntax was removed; use `nia emit --obj`"],
    );
    assert_failure(command(["emit"], source), &["missing emit target flag"]);
    assert_failure(
        command(["emit", "--llvm", "--backend"], source),
        &["use exactly one emit target flag"],
    );
}

fn command<const N: usize>(args: [&str; N], source: &Path) -> std::process::Output {
    let mut command = support::nia_command();
    command
        .args(args)
        .arg(source)
        .output_timeout_in_session("run command metadata case")
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: std::process::Output, expected: &[&str]) {
    assert!(!output.status.success());
    assert_contains(&output.stderr, expected);
}

fn assert_contains(actual: &[u8], expected: &[&str]) {
    let actual = String::from_utf8_lossy(actual);
    for expected in expected {
        assert!(
            actual.contains(expected),
            "missing {expected:?} in {actual}"
        );
    }
}
