// SPDX-License-Identifier: GPL-3.0-or-later
use nia_test_support::{CommandExt, TestWorkload, test_dir};

mod support;

#[test]
fn generic_const_recursion_reports_budget_exhaustion_instead_of_aborting() {
    let _resources = nia_test_support::acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("generic_const_recursion");
    for (name, functions, invocation) in [
        (
            "self",
            "const fun recurse[N: usize]() usize { recurse[N + 1]() }",
            "recurse[0]()",
        ),
        (
            "mutual",
            "const fun recurse[N: usize]() usize { other[N + 1]() }\n\
             const fun other[N: usize]() usize { recurse[N + 1]() }",
            "recurse[0]()",
        ),
        (
            "plain",
            "const fun recurse() usize { recurse() }",
            "recurse()",
        ),
    ] {
        let source = root.join(format!("{name}.nia"));
        std::fs::write(
            &source,
            format!("{functions}\nconst K = {invocation};\nfun main() usize {{ K }}\n"),
        )
        .expect("write recursion source");
        let output = support::nia_command()
            .arg("check")
            .arg(&source)
            .output_timeout_for_runtime("check recursive generic const call");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{name}: {output:?}");
        assert!(stderr.contains("call depth limit"), "{name}: {stderr}");
        assert!(
            !stderr.contains("internal compiler error"),
            "{name}: {stderr}"
        );
    }
}
