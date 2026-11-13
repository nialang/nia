// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn generic_const_computation_semantic_terms() {
    let root = temp_dir("generic_const_computation_semantic_terms");
    let cases = [
        (
            "local",
            "fun f[N: usize]() usize { const M = N + 1; M } fun main() usize { f[3]() }",
        ),
        (
            "repeat",
            "fun f[N: usize]() usize { let a = [0u8; N + 1]; a.len() } fun main() usize { f[3]() }",
        ),
        (
            "annotation",
            "fun f[N: usize]() usize { let a: [u8; N + 1] = [0; N + 1]; a.len() } fun main() usize { f[3]() }",
        ),
        (
            "argument",
            "fun g[M: usize]() usize { M } fun f[N: usize]() usize { g[N + 1]() } fun main() usize { f[3]() }",
        ),
        (
            "abstraction",
            "const fun plus(n: usize) usize { n + 1 } fun f[N: usize]() usize { const M = plus(N); M } fun main() usize { f[3]() }",
        ),
        (
            "generic_call",
            "const fun plus[M: usize]() usize { M + 1 } fun f[N: usize]() usize { const K = plus[N](); K } fun main() usize { f[3]() }",
        ),
        (
            "signature",
            "fun f[N: usize](a: [u8; N + 1]) usize { a.len() } fun main() usize { f[3]([0; 4]) }",
        ),
        (
            "nominal",
            "struct Buffer[N: usize] { bytes: [u8; N + 1], } fun main(b: Buffer[3]) usize { b.bytes.len() }",
        ),
        (
            "nested_call",
            "const fun plus[M: usize]() usize { M + 1 } fun f[N: usize]() usize { const K = plus[N + 1](); K } fun main() usize { f[3]() }",
        ),
        (
            "lazy_or",
            "fun f[N: usize]() bool { const K = N == 1 or 1usize / 0usize == 0; K } fun main() bool { f[1]() }",
        ),
        (
            "lazy_and",
            "fun f[N: usize]() bool { const K = N == 0 and 1usize / 0usize == 0; K } fun main() bool { f[1]() }",
        ),
        (
            "lazy_if",
            "fun f[N: usize]() usize { const K = if N == 0 { 1usize } else { 2usize / N }; K } fun main() usize { f[0]() }",
        ),
        (
            "loop_abstraction",
            "const fun total(n: usize) usize { let mut i = 0usize; let mut result = 0usize; while i < n { result += i; i += 1; } result } fun f[N: usize]() usize { const K = total(N); K } fun main() usize { f[3]() }",
        ),
        (
            "layout_operand",
            "struct Buffer[N: usize] { bytes: [u8; N + 1], } fun f[N: usize]() usize where Buffer[N]: Sized { const K = std::builtin::size[Buffer[N]](); K } fun main() usize { f[3]() }",
        ),
        (
            "alias",
            "type Bytes[N: usize] = [u8; N + 1]; fun f[N: usize]() usize { let a: Bytes[N] = [0; N + 1]; a.len() } fun main() usize { f[3]() }",
        ),
        (
            "cast",
            "fun f[N: u8]() usize { const K = (N as usize) + 1; K } fun main() usize { f[255]() }",
        ),
        (
            "shift",
            "fun f[N: u8]() u8 { const K = (1u8 << N) | 3u8; K } fun main() u8 { f[3]() }",
        ),
        (
            "negative",
            "fun f[N: i8]() i8 { const K = -N; K } fun main() i8 { f[-127]() }",
        ),
        (
            "unused_generic",
            "fun f[N: usize]() usize { const K = N / 0usize; K } fun main() usize { 0 }",
        ),
    ];
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../formal/NiaFormal/symbolic_const_matrix.json"
    ))
    .expect("const computation manifest");
    assert_eq!(
        cases.iter().map(|case| case.0).collect::<Vec<_>>(),
        manifest["computation_cases"]
            .as_array()
            .expect("computation inventory")
            .iter()
            .map(|name| name.as_str().expect("case name"))
            .collect::<Vec<_>>()
    );
    for (name, source) in cases {
        let path = root.join(format!("{name}.nia"));
        write(&path, source);
        let program = check_program(path.to_string_lossy().into_owned());
        assert!(
            program.diagnostics.is_empty(),
            "{name}: {:?}",
            program.diagnostics
        );
        let output = test_driver().codegen(crate::CheckRequest::new(
            path.to_string_lossy().into_owned(),
        ));
        assert!(output.result.is_ok(), "{name}: {:?}", output);
    }
}

#[test]
fn generic_const_computation_instances_execute() {
    let root = temp_dir("generic_const_computation_instances_execute");
    write(
        &root.join("arithmetic.nia"),
        r#"
pub const fun sum(n: usize) usize {
    let mut i = 0usize;
    let mut result = 0usize;
    while i < n { result += i; i += 1; }
    result
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
using std::process;
module arithmetic;
fun value[M: usize]() usize { M }
fun f[N: usize]() usize {
    const K = arithmetic::sum(N + 1);
    let a: [u8; N + 1] = [0; N + 1];
    K + a.len() + value[N + 1]()
}
fun safe[N: usize]() bool { const K = N == 0 or 4usize / N == 2; K }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if f[1]() != 5 or f[3]() != 14 or f[6]() != 35 or value[4]() != 4 or not safe[0]() or not safe[2]() or safe[4]() {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );
    let source = root.join("main.nia");
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("generic const computations link");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("execute generic const computations");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn generic_const_computation_instance_obligations() {
    let root = temp_dir("generic_const_computation_instance_obligations");
    for (name, source, expected) in [
        (
            "unused_overflow",
            "fun f[N: u8]() u8 { const M = N + 1u8; 0 } fun main() u8 { f[255]() }",
            "overflow",
        ),
        (
            "division",
            "fun f[N: usize]() usize { const M = 3usize / N; M } fun main() usize { f[0]() }",
            "division by zero",
        ),
        (
            "repeat_division",
            "fun f[N: usize]() usize { let a = [0u8; 3usize / N]; a.len() } fun main() usize { f[0]() }",
            "division by zero",
        ),
        (
            "argument_overflow",
            "fun g[M: u8]() u8 { M } fun f[N: u8]() u8 { g[N + 1u8]() } fun main() u8 { f[255]() }",
            "overflow",
        ),
        (
            "call_overflow",
            "const fun plus(n: u8) u8 { n + 1u8 } fun f[N: u8]() u8 { const M = plus(N); M } fun main() u8 { f[255]() }",
            "overflow",
        ),
        (
            "signature_overflow",
            "fun f[N: usize](a: ?&[u8; N + 1]) {} fun main() { f[18446744073709551615](null); }",
            "overflow",
        ),
        (
            "field_overflow",
            "struct Buffer[N: usize] { bytes: [u8; N + 1], } fun main(b: Buffer[18446744073709551615]) usize { 0 }",
            "overflow",
        ),
        (
            "negative_overflow",
            "fun f[N: i8]() i8 { const M = -N; M } fun main() i8 { f[-128]() }",
            "overflow",
        ),
        (
            "shift_overflow",
            "fun f[N: u8]() u8 { const K = 1u8 << N; K } fun main() u8 { f[8]() }",
            "shift",
        ),
    ] {
        let path = root.join(format!("{name}.nia"));
        write(&path, source);
        for diagnostics in [
            check_program(path.to_string_lossy().into_owned()).diagnostics,
            codegen_program(path.to_string_lossy().into_owned()).diagnostics,
        ] {
            let errors = diagnostics
                .iter()
                .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
                .collect::<Vec<_>>();
            assert_eq!(errors.len(), 1, "{name}: {diagnostics:?}");
            assert!(
                errors[0].diagnostic.summary.contains(expected),
                "{name}: {diagnostics:?}"
            );
            assert_ne!(
                errors[0].diagnostic.category(),
                nia_diagnostic::DiagnosticCategory::Internal,
                "{name}: {diagnostics:?}"
            );
        }
    }
}

#[test]
fn generic_const_computation_reports_the_executed_module() {
    let root = temp_dir("generic_const_computation_reports_the_executed_module");
    let arithmetic = "pub const fun plus(n: u8) u8 { n + 1u8 }";
    write(&root.join("arithmetic.nia"), arithmetic);
    let source = "module arithmetic; fun f[N: u8]() u8 { const M = arithmetic::plus(N); M } fun main() u8 { f[255]() }";
    let path = root.join("main.nia");
    write(&path, source);
    for diagnostics in [
        check_program(path.to_string_lossy().into_owned()).diagnostics,
        codegen_program(path.to_string_lossy().into_owned()).diagnostics,
    ] {
        let errors = diagnostics
            .iter()
            .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{diagnostics:?}");
        let error = errors[0];
        assert!(
            error.path.as_str().ends_with("arithmetic.nia"),
            "{diagnostics:?}"
        );
        let span = error.diagnostic.primary_span().expect("operation source");
        assert_eq!(&arithmetic[span.start..span.end], "n + 1u8");
        assert!(
            error.diagnostic.summary.contains("overflow"),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn generic_const_computation_identity_is_structural() {
    let root = temp_dir("generic_const_computation_identity_is_structural");
    for (name, source, accepted) in [
        (
            "alias",
            "fun f[N: usize]() usize { const M = N + 1; let a: [u8; N + 1] = [0; M]; a.len() } fun main() usize { f[3]() }",
            true,
        ),
        (
            "order",
            "fun f[N: usize]() usize { let a: [u8; N + 1] = [0; 1 + N]; a.len() } fun main() usize { f[3]() }",
            false,
        ),
        (
            "concrete",
            "fun f[N: usize]() usize { let a: [u8; N + 1] = [0; 4]; a.len() } fun main() usize { f[3]() }",
            false,
        ),
        (
            "inversion",
            "fun f[N: usize](a: [u8; N + 1]) usize { N } fun main() usize { f([0; 4]) }",
            false,
        ),
        (
            "type_error",
            "fun f[N: usize]() usize { const K = N + true; K } fun main() usize { 0 }",
            false,
        ),
    ] {
        let path = root.join(format!("{name}.nia"));
        write(&path, source);
        let program = check_program(path.to_string_lossy().into_owned());
        assert_eq!(
            !nia_compiler_query::has_error_diagnostics(&program.diagnostics),
            accepted,
            "{name}: {:?}",
            program.diagnostics
        );
        assert!(
            program
                .diagnostics
                .iter()
                .all(|d| d.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal),
            "{name}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn generic_const_computation_uses_artifact_integer_width() {
    let root = temp_dir("generic_const_computation_uses_artifact_integer_width");
    let path = root.join("main.nia");
    write(
        &path,
        "fun f[N: usize]() usize { const K = N + 1; K } fun main() usize { f[4294967295]() }",
    );
    for (target, accepted) in [("x86-unknown-linux", false), ("x86_64-unknown-linux", true)] {
        let driver = crate::Driver::with_config(
            crate::DriverConfig::new(test_toolchain_layout())
                .with_artifact_target(nia_target::TargetConfig::parse(target).expect("target")),
        );
        let program = driver.analyze_all_modules(crate::CheckRequest::new(
            path.to_string_lossy().into_owned(),
        ));
        let errors = program
            .diagnostics
            .iter()
            .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
            .collect::<Vec<_>>();
        assert_eq!(
            errors.is_empty(),
            accepted,
            "{target}: {:?}",
            program.diagnostics
        );
        if !accepted {
            assert_eq!(errors.len(), 1, "{target}: {:?}", program.diagnostics);
            assert!(
                errors[0].diagnostic.summary.contains("overflow"),
                "{target}: {:?}",
                program.diagnostics
            );
        }
    }
}

#[test]
fn generic_const_computation_retains_independent_failure_sites() {
    let root = temp_dir("generic_const_computation_retains_independent_failure_sites");
    let path = root.join("main.nia");
    let source = "fun f[N: usize]() usize { const K = N + 1; K } fun g[N: usize]() usize { const K = N + 1; K } fun main() usize { f[18446744073709551615]() + g[18446744073709551615]() }";
    write(&path, source);
    for diagnostics in [
        check_program(path.to_string_lossy().into_owned()).diagnostics,
        codegen_program(path.to_string_lossy().into_owned()).diagnostics,
    ] {
        let errors = diagnostics
            .iter()
            .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 2, "{diagnostics:?}");
        let mut spans = errors
            .iter()
            .map(|error| error.diagnostic.primary_span().expect("source site"))
            .collect::<Vec<_>>();
        spans.sort_by_key(|span| span.start);
        assert_ne!(spans[0], spans[1]);
        for span in spans {
            assert_eq!(&source[span.start..span.end], "N + 1");
        }
    }
}

#[test]
fn generic_const_computation_retains_independent_array_failure_sites() {
    let root = temp_dir("generic_const_computation_retains_independent_array_failure_sites");
    for (name, source) in [
        (
            "signature",
            "fun f[N: usize](a: ?&[u8; N + 1]) {} fun g[N: usize](a: ?&[u8; N + 1]) {} fun main() { f[18446744073709551615](null); g[18446744073709551615](null); }",
        ),
        (
            "repeat",
            "fun f[N: usize]() { let a = [0u8; N + 1]; } fun g[N: usize]() { let a = [0u8; N + 1]; } fun main() { f[18446744073709551615](); g[18446744073709551615](); }",
        ),
    ] {
        let path = root.join(format!("{name}.nia"));
        write(&path, source);
        let checked = check_program(path.to_string_lossy().into_owned());
        let generated = codegen_program(path.to_string_lossy().into_owned());
        for (phase, diagnostics) in [
            ("check", checked.diagnostics),
            ("codegen", generated.diagnostics),
        ] {
            let errors = diagnostics
                .iter()
                .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
                .collect::<Vec<_>>();
            assert_eq!(errors.len(), 2, "{name}/{phase}: {diagnostics:?}");
            let mut spans = errors
                .iter()
                .map(|error| {
                    assert!(
                        error.diagnostic.summary.contains("overflow"),
                        "{name}: {diagnostics:?}"
                    );
                    error.diagnostic.primary_span().expect("source site")
                })
                .collect::<Vec<_>>();
            spans.sort_by_key(|span| span.start);
            assert_ne!(spans[0], spans[1]);
            for span in spans {
                assert_eq!(&source[span.start..span.end], "N + 1");
            }
        }
    }
}

#[test]
fn generic_const_computation_invalidates_cross_module_execution() {
    let root = temp_dir("generic_const_computation_invalidates_cross_module_execution");
    let path = root.join("main.nia");
    let arithmetic = root.join("arithmetic.nia");
    let main = "module arithmetic; fun f[N: u8]() u8 { const K = arithmetic::plus(N); K } fun main() u8 { f[255]() }";
    write(&path, main);
    write(&arithmetic, "pub const fun plus(n: u8) u8 { n + 0u8 }");
    let driver = test_driver();
    for (source, accepted) in [
        ("pub const fun plus(n: u8) u8 { n + 0u8 }", true),
        ("pub const fun plus(n: u8) u8 { n + 1u8 }", false),
        ("pub const fun plus(n: u8) u8 { n + 0u8 }", true),
    ] {
        driver
            .set_source(arithmetic.to_string_lossy().into_owned(), source)
            .expect("edit const callee");
        let clean = test_driver();
        clean
            .set_source(arithmetic.to_string_lossy().into_owned(), source)
            .expect("clean const callee");
        let incremental = driver.analyze_all_modules(crate::CheckRequest::new(
            path.to_string_lossy().into_owned(),
        ));
        let fresh = clean.analyze_all_modules(crate::CheckRequest::new(
            path.to_string_lossy().into_owned(),
        ));
        let signatures = |diagnostics: &[nia_compiler_query::ProgramDiagnostic]| {
            let mut result = diagnostics
                .iter()
                .map(|d| {
                    (
                        d.path.as_str().to_owned(),
                        d.diagnostic.code.as_str().to_owned(),
                        d.diagnostic
                            .primary_span()
                            .map(|span| (span.start, span.end)),
                        d.diagnostic.summary.clone(),
                    )
                })
                .collect::<Vec<_>>();
            result.sort();
            result
        };
        assert_eq!(
            signatures(&incremental.diagnostics),
            signatures(&fresh.diagnostics)
        );
        assert_eq!(
            !nia_compiler_query::has_error_diagnostics(&incremental.diagnostics),
            accepted,
            "{source}: {:?}",
            incremental.diagnostics
        );
        let output = driver.codegen(crate::CheckRequest::new(
            path.to_string_lossy().into_owned(),
        ));
        assert_eq!(output.result.is_ok(), accepted, "{source}: {output:?}");
    }
}

#[test]
fn typed_symbolic_const_formal_matrix() {
    let cases = [
        ("bool_value", "bool", "true", 64, true),
        ("integer_is_not_bool", "bool", "1", 64, false),
        ("unsigned_negative", "u8", "-1", 64, false),
        ("u8_max", "u8", "255", 64, true),
        ("u8_overflow", "u8", "256", 64, false),
        ("i8_min", "i8", "-128", 64, true),
        ("i8_underflow", "i8", "-129", 64, false),
        ("i8_overflow", "i8", "128", 64, false),
        (
            "u128_max",
            "u128",
            "340282366920938463463374607431768211455",
            64,
            true,
        ),
        (
            "u128_overflow",
            "u128",
            "340282366920938463463374607431768211456",
            64,
            false,
        ),
        (
            "i128_min",
            "i128",
            "-170141183460469231731687303715884105728",
            64,
            true,
        ),
        (
            "i128_underflow",
            "i128",
            "-170141183460469231731687303715884105729",
            64,
            false,
        ),
        (
            "i128_overflow",
            "i128",
            "170141183460469231731687303715884105728",
            64,
            false,
        ),
        ("usize32_max", "usize", "4294967295", 32, true),
        ("usize32_overflow", "usize", "4294967296", 32, false),
        ("usize64_wider", "usize", "4294967296", 64, true),
        ("usize64_max", "usize", "18446744073709551615", 64, true),
        (
            "usize64_overflow",
            "usize",
            "18446744073709551616",
            64,
            false,
        ),
        ("isize32_min", "isize", "-2147483648", 32, true),
        ("isize32_max", "isize", "2147483647", 32, true),
        ("isize32_underflow", "isize", "-2147483649", 32, false),
        ("isize32_overflow", "isize", "2147483648", 32, false),
        ("isize64_wider", "isize", "2147483648", 64, true),
        ("isize64_min", "isize", "-9223372036854775808", 64, true),
        ("isize64_max", "isize", "9223372036854775807", 64, true),
        (
            "isize64_underflow",
            "isize",
            "-9223372036854775809",
            64,
            false,
        ),
        (
            "isize64_overflow",
            "isize",
            "9223372036854775808",
            64,
            false,
        ),
    ];
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../formal/NiaFormal/symbolic_const_matrix.json"
    ))
    .expect("symbolic const manifest");
    assert_eq!(
        cases.iter().map(|case| case.0).collect::<Vec<_>>(),
        manifest["typed_compiler_cases"]
            .as_array()
            .expect("typed case list")
            .iter()
            .map(|name| name.as_str().expect("case name"))
            .collect::<Vec<_>>()
    );
    let root = temp_dir("typed_symbolic_const_formal_matrix");
    for (name, ty, value, bits, accepted) in cases {
        let target_name = if bits == 32 {
            "x86-unknown-linux"
        } else {
            "x86_64-unknown-linux"
        };
        let target = nia_target::TargetConfig::parse(target_name).expect("maintained target");
        let driver = crate::Driver::with_config(
            crate::DriverConfig::new(test_toolchain_layout()).with_artifact_target(target),
        );
        // Test both declaration values and explicit const arguments: they
        // share the scalar contract but reach it through different owners.
        for (form, source) in [
            (
                "binding",
                format!("const K: {ty} = {value}; fun main() {ty} {{ K }}"),
            ),
            (
                "argument",
                format!("fun f[N: {ty}]() {ty} {{ N }} fun main() {ty} {{ f[{value}]() }}"),
            ),
            (
                "const_call",
                format!(
                    "const fun f[N: {ty}]() {ty} {{ N }} const K: {ty} = f[{value}](); fun main() {ty} {{ K }}"
                ),
            ),
        ] {
            let path = root.join(format!("{name}-{form}.nia"));
            write(&path, &source);
            let checked = driver.analyze_all_modules(crate::CheckRequest::new(
                path.to_string_lossy().into_owned(),
            ));
            assert!(
                checked.diagnostics.iter().all(
                    |d| d.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal
                ),
                "{name}-{form}: {:?}",
                checked.diagnostics
            );
            let errors = nia_compiler_query::has_error_diagnostics(&checked.diagnostics);
            assert_eq!(
                !errors, accepted,
                "{name}-{form}: {:?}",
                checked.diagnostics
            );
        }
    }
}

#[test]
fn generic_arithmetic_staging_boundary() {
    let cases = [
        (
            "concrete_arithmetic",
            "const K: usize = 3 + 1; fun main() usize { K }",
            true,
        ),
        (
            "executable_generic_arithmetic",
            "fun f[N: usize]() usize { N + 1 } fun main() usize { f[3]() }",
            true,
        ),
        (
            "concrete_const_function",
            "const fun f[N: usize]() usize { N + 1 } const K: usize = f[3](); fun main() usize { K }",
            true,
        ),
        (
            "symbolic_local_initializer",
            "fun f[N: usize]() usize { const K = N + 1; K } fun main() usize { f[3]() }",
            true,
        ),
        (
            "symbolic_array_length",
            "fun f[N: usize](a: [u8; N + 1]) usize { a.len() } fun main() usize { f[3]([0; 4]) }",
            true,
        ),
        (
            "symbolic_repeat_count",
            "fun f[N: usize]() usize { let a = [0u8; N + 1]; a.len() } fun main() usize { f[3]() }",
            true,
        ),
        (
            "symbolic_argument",
            "fun g[M: usize]() usize { M } fun f[N: usize]() usize { g[N + 1]() } fun main() usize { f[3]() }",
            true,
        ),
        (
            "symbolic_const_call",
            "const fun plus(n: usize) usize { n + 1 } fun f[N: usize]() usize { const K = plus(N); K } fun main() usize { f[3]() }",
            true,
        ),
    ];
    let root = temp_dir("generic_arithmetic_staging_boundary");
    for (name, source, accepted) in cases {
        let path = root.join(format!("{name}.nia"));
        write(&path, source);
        let program = codegen_program(path.to_string_lossy().into_owned());
        assert!(
            program
                .diagnostics
                .iter()
                .all(|d| d.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal),
            "{name}: {:?}",
            program.diagnostics
        );
        assert_eq!(
            !nia_compiler_query::has_error_diagnostics(&program.diagnostics),
            accepted,
            "{name}: {:?}",
            program.diagnostics
        );
    }
}

#[test]
fn signed_const_arguments_and_static_casts_execute_with_canonical_values() {
    let root = temp_dir("signed_const_arguments_and_static_casts_execute_with_canonical_values");
    write(
        &root.join("main.nia"),
        r#"
using std::process;
fun small[N: i8]() i8 { N }
const fun folded[N: i8]() i8 { N }
const FIRST: i8 = folded[-128]();
const SECOND: i8 = folded[-1]();
static mut TRUNCATED: i8 = 255u16 as i8;
fun minimum() i128 { -170141183460469231731687303715884105728 }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if small[-128]() != -128i8 or small[-1]() != -1i8
        or small[127]() != 127i8 or FIRST != -128i8 or SECOND != -1i8
        or TRUNCATED != -1i8
        or minimum() != -170141183460469231731687303715884105728i128 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(root.join("main.nia").to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("signed constants build");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run signed constants");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn symbolic_const_formal_matrix() {
    let cases = [
        (
            "global_const_argument",
            "const W: usize = 4; fun f[N: usize]() usize { N } fun main() usize { f[W]() }",
            None,
        ),
        (
            "local_const_argument",
            "fun f[N: usize]() usize { N } fun main() usize { const W: usize = 4; f[W]() }",
            None,
        ),
        (
            "parameter_alias_chain",
            "fun f[N: usize]() usize { const M = N; const K = M; let a: [u8; K] = [0; M]; a.len() } fun main() usize { f[3]() + f[7]() }",
            None,
        ),
        (
            "layout_alias_chain",
            "fun f[T]() usize where T: Sized { const M = std::builtin::size[T](); const K = M; let a: [u8; K] = [0; M]; a.len() } fun main() usize { f[u16]() + f[u64]() }",
            None,
        ),
        (
            "layout_argument",
            "fun f[N: usize]() usize { N } fun g[T]() usize where T: Sized { f[std::builtin::size[T]()]() } fun main() usize { g[u16]() + g[u64]() }",
            None,
        ),
        (
            "const_function_instances",
            "const fun f[N: usize]() usize { const M = N; M } const A: usize = f[3](); const B: usize = f[7](); fun main() usize { A + B }",
            None,
        ),
        (
            "shadowed_parameter",
            "fun f[N: usize]() usize { const N: usize = 4; N } fun main() usize { f[7]() }",
            None,
        ),
        (
            "wrong_parameter_type",
            "fun f[N: usize]() usize { N } fun g[B: bool]() usize { f[B]() } fun main() usize { g[true]() }",
            Some("const generic argument"),
        ),
        (
            "runtime_argument",
            "fun f[N: usize]() usize { N } fun main() usize { let W: usize = 4; f[W]() }",
            Some("const"),
        ),
        (
            "symbol_literal_mismatch",
            "fun f[N: usize]() usize { const M = N; let a: [u8; M] = [0; 4]; a.len() } fun main() usize { f[4]() }",
            Some("array literal length mismatch"),
        ),
        (
            "generic_arithmetic",
            "fun f[N: usize]() usize { const M = N + 1; M } fun main() usize { f[3]() }",
            None,
        ),
    ];
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../formal/NiaFormal/symbolic_const_matrix.json"
    ))
    .expect("symbolic const manifest");
    let expected_cases = manifest["compiler_cases"]
        .as_array()
        .expect("case list")
        .iter()
        .map(|name| name.as_str().expect("case name"))
        .collect::<Vec<_>>();
    assert_eq!(
        cases.iter().map(|case| case.0).collect::<Vec<_>>(),
        expected_cases
    );
    let root = temp_dir("symbolic_const_formal_matrix");
    for (name, source, expected_error) in cases {
        let path = root.join(format!("{name}.nia"));
        write(&path, source);
        let checked = check_program(path.to_string_lossy().into_owned());
        if expected_error.is_none() {
            assert_no_error_diagnostics(&checked.diagnostics);
        }
        let program = codegen_program(path.to_string_lossy().into_owned());
        if let Some(expected) = expected_error {
            assert!(
                program
                    .diagnostics
                    .iter()
                    .any(|d| d.diagnostic.summary.contains(expected)),
                "{name}: {:?}",
                program.diagnostics
            );
            assert!(
                program.diagnostics.iter().all(
                    |d| d.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal
                ),
                "{name}: {:?}",
                program.diagnostics
            );
        } else {
            assert_no_error_diagnostics(&program.diagnostics);
        }
    }
}

#[test]
fn symbolic_const_aliases_execute_per_instance() {
    let root = temp_dir("symbolic_const_aliases_execute_per_instance");
    write(
        &root.join("main.nia"),
        r#"
module worker;
using std::process;
fun value[N: usize]() usize { N }
fun alias[N: usize]() usize {
    const M = N;
    const K = M;
    let bytes: [u8; K] = [0; M];
    bytes.len() + value[K]()
}
fun measured[T]() usize where T: Sized {
    const M = std::builtin::size[T]();
    const K = M;
    let bytes: [u8; K] = [0; M];
    bytes.len() + value[K]() + value[std::builtin::align[T]()]()
}
const fun folded[N: usize]() usize { const M = N; M }
const A: usize = folded[3]();
const B: usize = folded[7]();
fun shadowed[N: usize]() usize { const N: usize = 4; N }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if alias[3]() != 6 or alias[7]() != 14
        or measured[u16]() != 6 or measured[u64]() != 24
        or A != 3 or B != 7 or shadowed[7]() != 4
        or worker::length[u16]() != 2 or worker::length[u64]() != 8 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );
    write(
        &root.join("worker.nia"),
        r#"
fun value[N: usize]() usize { N }
pub fun length[T]() usize where T: Sized {
    const N = std::builtin::size[T]();
    const M = N;
    let bytes: [u8; M] = [0; N];
    value[M]() + bytes.len() - M
}
"#,
    );
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(root.join("main.nia").to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("symbolic local constants build");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run const instances");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn generic_nominal_extension_methods_materialize_const_instances() {
    let root = temp_dir("generic_nominal_extension_methods_materialize_const_instances");
    write(
        &root.join("main.nia"),
        r#"
module hashing;
using root::hashing;

fun main() u64 {
    let mut one = hashing::One::identity(hashing::One::init());
    one.write(3);
    let mut two = hashing::Two::init();
    two.write(4);
    _ = one.finish() + two.finish();
    hashing::One::hash(3u64) + hashing::Two::hash(4u64)
}
"#,
    );
    write(
        &root.join("hashing.nia"),
        r#"
pub trait Hasher { fun write(&mut self, value: u64) (); fun finish(&self) u64; }
pub trait Hash[H] where H: Hasher { fun hash(&self, hasher: &mut H) (); }
pub struct Sip[C: usize, D: usize] { value: u64 }
pub type One = Sip[1, 3];
pub type Two = Sip[2, 4];

extend[C: usize, D: usize] Sip[C, D] {
    pub fun init() Self { .{ value: 0 } }
    pub fun identity(value: Self) Self { value }
    pub fun hash[T](value: T) u64 where T: Hash[Sip[C, D]] {
        let mut hasher: Self = .init();
        value.hash(hasher);
        hasher.finish()
    }
}
extend[C: usize, D: usize] Sip[C, D] : Hasher {
    pub fun write(&mut self, value: u64) () { self.value += value + C as u64; }
    pub fun finish(&self) u64 { self.value + D as u64 }
}
extend[H] u64 : Hash[H] where H: Hasher {
    pub fun hash(&self, hasher: &mut H) () { hasher.write(self.*); }
}
"#,
    );
    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert_no_error_diagnostics(&program.diagnostics);
    let hashes = program
        .backend_lowering
        .program
        .modules
        .iter()
        .flat_map(|module| &module.function_instances)
        .filter(|instance| instance.name == sym("hash") && instance.const_args.len() == 2)
        .collect::<Vec<_>>();
    assert_eq!(hashes.len(), 2, "{hashes:?}");
    assert_ne!(hashes[0].const_args, hashes[1].const_args);
}

#[test]
fn const_generic_supertrait_projection_substitutes_array_lengths() {
    let root = temp_dir("const_generic_supertrait_projection_substitutes_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait ArraySource[N: usize] {
    type Item;

    fun values(& self) [[Self as ArraySource[N]]::Item; N];
}

trait ArraySummary[N: usize] : ArraySource[N] {
    fun len(& self) usize {
        N
    }

    fun first_or(& self, fallback: [Self as ArraySource[N]]::Item) [Self as ArraySource[N]]::Item {
        if self.len() == 0usize {
            fallback
        } else {
            self.values()[0]
        }
    }
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] : ArraySource[N] {
    type Item = T;

    fun values(& self) [T; N] {
        self.values
    }
}

extend[T, N: usize] Buffer[T, N] : ArraySummary[N] {}

fun read[S, N: usize](source: & S, fallback: [S as ArraySource[N]]::Item) [S as ArraySource[N]]::Item
where S: ArraySummary[N] {
    source.first_or(fallback)
}

fun main() i32 {
    let buffer = Buffer[i32, 3] { values: [7, 8, 9] };
    read[Buffer[i32, 3], 3](& buffer, 99)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_solver_keeps_open_const_bounds_symbolic() {
    let root = temp_dir("trait_solver_keeps_open_const_bounds_symbolic");
    write(
        &root.join("main.nia"),
        r#"
trait Tr[K: usize] {
    fun k(&self) usize;
}

struct X[N: usize] {}

extend[N: usize] X[N] : Tr[N + 1] {
    fun k(&self) usize { N + 1 }
}

fun by_parameter[N: usize](x: X[N]) usize
where X[N]: Tr[N + 1]
{
    x.k()
}

fun by_concrete_type[N: usize](x: X[2]) usize
where X[2]: Tr[N + 1]
{
    x.k()
}

fun main() usize { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_default_trait_method_codegen_keeps_owner_assumption() {
    let root = temp_dir("const_generic_default_trait_method_codegen_keeps_owner_assumption");
    write(
        &root.join("main.nia"),
        r#"
trait Source[N: usize] {
    fun value(& self) usize;
}

trait Summary[N: usize] : Source[N] {
    fun total(& self) usize {
        self.value() + N
    }
}

struct Meter[N: usize] {
    value: usize,
}

extend[N: usize] Meter[N] : Source[N] {
    fun value(& self) usize {
        self.value
    }
}

extend[N: usize] Meter[N] : Summary[N] {}

fun main() usize {
    let meter = Meter[8] { value: 34usize };
    meter.total()
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .backend_lowering
            .program
            .modules
            .iter()
            .any(|module| {
                module.function_instances.iter().any(|instance| {
                    matches!(
                        instance.const_args.as_slice(),
                        [nia_ty::ConstGenericArg {
                            value: nia_ty::ConstGenericValue::Int(value),
                            ..
                        }] if value.bits() == 8
                    )
                })
            })
    );
}

#[test]
fn const_generic_assoc_projection_rejects_unconstrained_literal_fallback() {
    let root = temp_dir("const_generic_assoc_projection_rejects_unconstrained_literal_fallback");
    write(
        &root.join("main.nia"),
        r#"
trait ArraySource[N: usize] {
    type Item;
}

trait ArraySummary[N: usize] : ArraySource[N] {
    fun first_or(& self, fallback: [Self as ArraySource[N]]::Item) [Self as ArraySource[N]]::Item {
        fallback
    }
}

fun read[S, N: usize](source: & S) [S as ArraySource[N]]::Item
where S: ArraySummary[N] {
    source.first_or(99)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_assoc_projection_rejects_mismatched_lengths() {
    let root = temp_dir("const_generic_assoc_projection_rejects_mismatched_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait ArraySource[N: usize] {
    type Item;

    fun values(& self) [[Self as ArraySource[N]]::Item; N];
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] : ArraySource[N] {
    type Item = i32;

    fun values(& self) [i32; N] {
        self.values
    }
}

fun wrong[S, N: usize](source: & S) [[S as ArraySource[N]]::Item; N]
where S: ArraySource[N] {
    let values: [[S as ArraySource[N]]::Item; N] = source.values();
    let bad: [[S as ArraySource[N]]::Item; 4] = values;
    bad
}

fun main(buffer: Buffer[i32, 3]) [i32; 3] {
    wrong[Buffer[i32, 3], 3](& buffer)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_impl_target_pattern_infers_array_lengths() {
    let root = temp_dir("const_generic_impl_target_pattern_infers_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait HasLen {
    fun width(& self) usize;
}

extend[T, N: usize] [T; N] : HasLen {
    fun width(& self) usize {
        N
    }
}

fun read_len[A](value: & A) usize
where A: HasLen {
    value.width()
}

fun main() i32 {
    let values: [i32; 3] = [1, 2, 3];
    read_len(& values) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    // Checking alone exercises trait selection in sema; codegen ensures the
    // selected const-generic trait instance survives backend dispatch too.
    let codegen = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
}

#[test]
fn codegen_materializes_const_generic_trait_method_impl_arguments() {
    let root = temp_dir("codegen_materializes_const_generic_trait_method_impl_arguments");
    write(
        &root.join("main.nia"),
        r#"
trait Make[N: usize] {
    fun make(& self) [u8; N];
}

struct Box {}

extend[N: usize] Box : Make[N] {
    fun make(& self) [u8; N] {
        [0u8; N]
    }
}

fun build[N: usize](value: & Box) [u8; N]
where Box: Make[N] {
    value.make()
}

fun main(value: & Box) [u8; 3] {
    build[3](value)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.function_instances)
            .any(|instance| instance.name == sym("make") && instance.const_args.len() == 1)
    );
}

#[test]
fn codegen_preserves_interleaved_function_generic_order() {
    let root = temp_dir("codegen_preserves_interleaved_function_generic_order");
    write(
        &root.join("main.nia"),
        r#"
fun choose[T, N: usize, U](left: T, right: U) U {
    right
}

fun main() i64 {
    choose[i32, 3, i64](1, 9i64)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    assert!(
        program
            .backend_lowering
            .program
            .modules
            .iter()
            .flat_map(|module| &module.function_instances)
            .any(|instance| {
                instance.name == sym("choose")
                    && instance.args.len() == 2
                    && instance.const_args.len() == 1
                    && matches!(
                        program.type_store.get(instance.return_type),
                        Some(nia_ty::TyKind::Primitive(nia_ty::PrimitiveTy::I64))
                    )
            })
    );
}

#[test]
fn const_generic_const_fn_result_can_drive_nominal_arg_and_extend_value() {
    let root = temp_dir("const_generic_const_fn_result_can_drive_nominal_arg_and_extend_value");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] {
    pub const WIDTH: usize = N;

    pub fun len(& self) usize {
        Buffer[T, N]::WIDTH
    }
}

const WIDTH: usize = plus_one(3usize);

fun main() i32 {
    let buffer = Buffer[i32, WIDTH] { values: [1, 2, 3, 4] };
    buffer.len() as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_extend_const_can_call_const_fn_per_instance() {
    let root = temp_dir("const_generic_extend_const_can_call_const_fn_per_instance");
    write(
        &root.join("main.nia"),
        r#"
const fun double(value: usize) usize {
    value * 2usize
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] {
    pub const DOUBLE_WIDTH: usize = double(N);
}

fun main() usize {
    Buffer[i32, 2]::DOUBLE_WIDTH + Buffer[i32, 5]::DOUBLE_WIDTH
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_instances_keep_assoc_const_values_separate() {
    let root = temp_dir("const_generic_instances_keep_assoc_const_values_separate");
    write(
        &root.join("main.nia"),
        r#"
struct Buffer[T, N: usize] {
    values: [T; N],
}

extend[T, N: usize] Buffer[T, N] {
    pub const WIDTH: usize = N;
}

fun main() usize {
    Buffer[i32, 2]::WIDTH + Buffer[i32, 5]::WIDTH
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_fake_refs_do_not_runtime_materialize_const_values() {
    let root = temp_dir("const_generic_fake_refs_do_not_runtime_materialize_const_values");
    write(
        &root.join("main.nia"),
        r#"
const fun width[N: usize]() usize {
    let value: usize = N;
    value
}

struct Buffer[T, N: usize] {
    values: [T; N],
}

fun make[T](value: T) Buffer[T, 4] {
    Buffer[T, 4] { values: [value; width[4]()] }
}

fun main() i32 {
    let buffer = make[i32](3);
    buffer.values[3]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

/// A const parameter of an `extend` block is in scope in its methods as a
/// value, though the block is not the method's definition parent and `N`
/// appears only in the implemented trait.
#[test]
fn extend_const_params_are_values_in_their_methods() {
    let root = temp_dir("extend_const_params_are_values_in_their_methods");
    write(
        &root.join("main.nia"),
        r#"
trait Make[N: usize] {
    fun make(&self) usize;
}

struct Box {}

extend[N: usize] Box : Make[N] {
    fun make(&self) usize {
        N
    }
}

fun build[N: usize](value: &Box) usize
where Box: Make[N] {
    value.make()
}

fun main(value: &Box) usize {
    build[3](value)
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_supertrait_arguments_remain_distinct_for_projection() {
    let root = temp_dir("const_generic_supertrait_arguments_remain_distinct_for_projection");
    write(
        &root.join("main.nia"),
        r#"
trait Source[N: usize] {
    type Item;
}

trait SizedSource[N: usize] : Source[N] {}

struct Source4 {}

extend Source4 : Source[4] {
    type Item = i32;
}

extend Source4 : SizedSource[4] {}

fun bad[S](value: [S as Source[3]]::Item) [S as Source[3]]::Item
where S: SizedSource[4] {
    value
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_impl_pattern_rejects_conflicting_array_lengths() {
    let root = temp_dir("const_generic_impl_pattern_rejects_conflicting_array_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait HasLength[N: usize] {}

extend[T, N: usize] [T; N] : HasLength[N] {}

fun require_len[A, N: usize](value: & A)
where A: HasLength[N] {
    _ = value;
}

fun main() i32 {
    let values: [i32; 3] = [1, 2, 3];
    require_len[_, 4](& values);
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_multi_param_impl_pattern_keeps_substitutions_independent() {
    let root = temp_dir("const_generic_multi_param_impl_pattern_keeps_substitutions_independent");
    write(
        &root.join("main.nia"),
        r#"
trait MatrixShape {
    fun rows(& self) usize;
    fun cols(& self) usize;
}

struct Matrix[T, R: usize, C: usize] {
    values: [[T; C]; R],
}

extend[T, R: usize, C: usize] Matrix[T, R, C] : MatrixShape {
    fun rows(& self) usize {
        R
    }

    fun cols(& self) usize {
        C
    }
}

fun shape[M](matrix: & M) usize
where M: MatrixShape {
    matrix.rows() * 10usize + matrix.cols()
}

fun main() usize {
    let matrix = Matrix[i32, 2, 3] { values: [[1, 2, 3], [4, 5, 6]] };
    shape(& matrix)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_const_arg_expression_normalizes_for_trait_matching() {
    let root = temp_dir("const_generic_const_arg_expression_normalizes_for_trait_matching");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

trait Width[N: usize] {
    fun width(& self) usize {
        N
    }
}

struct Buffer[N: usize] {
    values: [i32; N],
}

extend[N: usize] Buffer[N] : Width[N] {}

const THREE: usize = plus_one(2usize);

fun read[T](value: & T) usize
where T: Width[THREE] {
    value.width()
}

fun main() usize {
    let buffer = Buffer[3] { values: [1, 2, 3] };
    read(& buffer)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_assoc_type_projection_keeps_each_const_instance_separate() {
    let root = temp_dir("const_generic_assoc_type_projection_keeps_each_const_instance_separate");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[N: usize] {
    type Item;
}

struct Store {}

extend Store : Slot[2] {
    type Item = i32;
}

extend Store : Slot[4] {
    type Item = usize;
}

fun pick2(value: [Store as Slot[2]]::Item) i32 {
    value
}

fun pick4(value: [Store as Slot[4]]::Item) usize {
    value
}

fun main() i32 {
    pick2(7) + pick4(8usize) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_assoc_type_projection_rejects_cross_instance_rewrite() {
    let root = temp_dir("const_generic_assoc_type_projection_rejects_cross_instance_rewrite");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[N: usize] {
    type Item;
}

struct Store {}

extend Store : Slot[2] {
    type Item = i32;
}

extend Store : Slot[4] {
    type Item = usize;
}

fun bad(value: [Store as Slot[2]]::Item) [Store as Slot[4]]::Item {
    value
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_type_position_accepts_const_expression_args() {
    let root = temp_dir("const_generic_type_position_accepts_const_expression_args");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[N: usize] {
    values: [i32; N],
}

fun main() i32 {
    let buffer = Buffer[plus_one(2usize)] { values: [1, 2, 3] };
    buffer.values[2]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_call_position_accepts_const_expression_args() {
    let root = temp_dir("const_generic_call_position_accepts_const_expression_args");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

fun width[N: usize]() usize {
    N
}

fun main() usize {
    width[plus_one(2usize)]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn nested_const_generic_calls_preserve_the_caller_instance() {
    let root = temp_dir("nested_const_generic_calls_preserve_the_caller_instance");
    write(
        &root.join("main.nia"),
        r#"
fun inner[N: usize]() usize {
    N
}

fun outer[N: usize]() usize {
    inner[N]()
}

fun main() usize {
    outer[3]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_type_position_accepts_imported_const_expression_args() {
    let root = temp_dir("const_generic_type_position_accepts_imported_const_expression_args");
    write(
        &root.join("config.nia"),
        r#"
pub const WIDTH: usize = 3usize;

pub const fun plus_one(value: usize) usize {
    value + 1usize
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module config;
using root::config;

struct Buffer[N: usize] {
    values: [i32; N],
}

fun main() i32 {
    let left = Buffer[config::WIDTH] { values: [1, 2, 3] };
    let right = Buffer[config::plus_one(2usize)] { values: [4, 5, 6] };
    left.values[2] + right.values[2]
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_call_position_rejects_non_usize_arg() {
    let root = temp_dir("const_generic_call_position_rejects_non_usize_arg");
    write(
        &root.join("main.nia"),
        r#"
fun width[N: usize]() usize {
    N
}

fun main() usize {
    width[true]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("const value")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_call_position_rejects_out_of_range_usize_arg() {
    let root = temp_dir("const_generic_call_position_rejects_out_of_range_usize_arg");
    let source = r#"
fun width[N: usize]() usize {
    N
}

fun main() usize {
    width[-1]()
}
"#;
    write(&root.join("main.nia"), source);

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    let errors = program
        .diagnostics
        .iter()
        .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1, "{:?}", program.diagnostics);
    let diagnostic = &errors[0].diagnostic;
    assert_eq!(diagnostic.code.as_str(), "E0301");
    assert!(
        diagnostic.summary.contains("out of range for usize"),
        "{:?}",
        program.diagnostics
    );
    let span = diagnostic.primary_span().expect("range error source");
    assert_eq!(&source[span.start..span.end], "-1");
}

#[test]
fn const_generic_trait_impls_accept_bool_args_without_colliding() {
    let root = temp_dir("const_generic_trait_impls_accept_bool_args_without_colliding");
    write(
        &root.join("main.nia"),
        r#"
trait Flagged[FLAG: bool] {
    fun value(& self) usize;
}

struct Token {}

extend Token : Flagged[true] {
    fun value(& self) usize {
        1usize
    }
}

extend Token : Flagged[false] {
    fun value(& self) usize {
        2usize
    }
}

fun read_true[T](value: & T) usize
where T: Flagged[true] {
    value.value()
}

fun read_false[T](value: & T) usize
where T: Flagged[false] {
    value.value()
}

fun main() usize {
    let token = Token {};
    read_true(& token) * 10usize + read_false(& token)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_trait_impl_rejects_bool_arg_mismatch() {
    let root = temp_dir("const_generic_trait_impl_rejects_bool_arg_mismatch");
    write(
        &root.join("main.nia"),
        r#"
trait Flagged[FLAG: bool] {}

struct Token {}

extend Token : Flagged[true] {}

fun require_false[T](value: & T)
where T: Flagged[false] {
    _ = value;
}

fun main() i32 {
    let token = Token {};
    require_false(& token);
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("trait bound not satisfied")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_projection_rejects_cross_bool_instance_rewrite() {
    let root = temp_dir("const_generic_projection_rejects_cross_bool_instance_rewrite");
    write(
        &root.join("main.nia"),
        r#"
trait Slot[FLAG: bool] {
    type Item;
}

struct Store {}

extend Store : Slot[true] {
    type Item = i32;
}

extend Store : Slot[false] {
    type Item = usize;
}

fun bad(value: [Store as Slot[true]]::Item) [Store as Slot[false]]::Item {
    value
}

fun main() i32 { 0 }
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("type mismatch")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_bool_expression_normalizes_for_trait_matching() {
    let root = temp_dir("const_generic_bool_expression_normalizes_for_trait_matching");
    write(
        &root.join("main.nia"),
        r#"
const fun yes() bool {
    true
}

trait Flagged[FLAG: bool] {
    fun value(& self) usize;
}

struct Token {}

extend Token : Flagged[true] {
    fun value(& self) usize {
        7usize
    }
}

const ENABLED: bool = yes();

fun read[T](value: & T) usize
where T: Flagged[ENABLED] {
    value.value()
}

fun main() usize {
    let token = Token {};
    read(& token)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_char_expression_normalizes_for_trait_matching() {
    let root = temp_dir("const_generic_char_expression_normalizes_for_trait_matching");
    write(
        &root.join("main.nia"),
        r#"
const fun marker() char {
    'N'
}

trait Tagged[TAG: char] {
    fun value(& self) usize;
}

struct Token {}

extend Token : Tagged['N'] {
    fun value(& self) usize {
        9usize
    }
}

const TAG: char = marker();

fun read[T](value: & T) usize
where T: Tagged[TAG] {
    value.value()
}

fun main() usize {
    let token = Token {};
    read(& token)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_expression_normalizes_for_extension_method_specificity() {
    let root = temp_dir("const_generic_expression_normalizes_for_extension_method_specificity");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

struct Buffer[N: usize] {
    values: [i32; N],
}

extend[N: usize] Buffer[N] {
    fun rank(& self) i32 {
        1
    }
}

extend Buffer[plus_one(2usize)] {
    fun rank(& self) i32 {
        2
    }
}

fun main() i32 {
    let buffer = Buffer[3] { values: [1, 2, 3] };
    buffer.rank()
}
"#,
    );

    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_expression_normalizes_for_projection_equivalence() {
    let root = temp_dir("const_generic_expression_normalizes_for_projection_equivalence");
    write(
        &root.join("main.nia"),
        r#"
const fun plus_one(value: usize) usize {
    value + 1usize
}

trait Slot[N: usize] {
    type Item;
}

struct Store {}

extend Store : Slot[3] {
    type Item = i32;
}

const WIDTH: usize = plus_one(2usize);

fun read[S](value: [S as Slot[3]]::Item) [S as Slot[WIDTH]]::Item
where S: Slot[3] {
    value
}

fun main() i32 {
    read[Store](7)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn repeated_const_parameter_impl_specializes_the_complete_trait_header() {
    let root = temp_dir("repeated_const_parameter_impl_specializes_the_complete_trait_header");
    write(
        &root.join("main.nia"),
        r#"
trait Select[N: usize] {
    type Item;
}

struct Buffer[N: usize] {}

extend[A: usize, B: usize] Buffer[A] : Select[B] {
    type Item = i32;
}

extend[N: usize] Buffer[N] : Select[N] {
    type Item = i64;
}

fun selected(value: [Buffer[3] as Select[3]]::Item) i64 {
    value
}

fun main() i64 {
    selected(7i64)
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_generic_u8_expression_rejects_out_of_range_value() {
    let root = temp_dir("const_generic_u8_expression_rejects_out_of_range_value");
    write(
        &root.join("main.nia"),
        r#"
const fun too_large() usize {
    256usize
}

fun value[N: u8]() u8 {
    N
}

fun main() u8 {
    value[too_large()]()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("out of range")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn const_generic_rejects_conflicting_repeated_inferred_lengths() {
    let root = temp_dir("const_generic_rejects_conflicting_repeated_inferred_lengths");
    write(
        &root.join("main.nia"),
        r#"
trait SameLen[N: usize] {}

struct Pair[A, B] {
    left: A,
    right: B,
}

extend[T, U, N: usize] Pair[[T; N], [U; N]] : SameLen[N] {}

fun require_same_len[P, N: usize](pair: & P)
where P: SameLen[N] {
    _ = pair;
}

fun main() i32 {
    let pair = Pair[[i32; 2], [i32; 3]] { left: [1, 2], right: [3, 4, 5] };
    require_same_len(& pair);
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("cannot infer const generic parameter")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn ambiguous_generic_arg_reports_value_error_only_when_const_param_requires_it() {
    let root =
        temp_dir("ambiguous_generic_arg_reports_value_error_only_when_const_param_requires_it");
    write(
        &root.join("main.nia"),
        r#"
struct Buffer[N: usize] {
    values: [i32; N],
}

pub fun take(value: Buffer[MISSING]) () {
    _ = value;
}

fun main() i32 {
    0
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(
        program.diagnostics.iter().any(|diagnostic| diagnostic
            .diagnostic
            .summary
            .contains("failed to resolve const name")),
        "{:?}",
        program.diagnostics
    );
    assert!(
        !program
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.diagnostic.summary.contains("unknown type")),
        "{:?}",
        program.diagnostics
    );
}

#[test]
fn imported_const_generic_layout_builtins_resolve_after_instantiation() {
    let root = temp_dir("imported_const_generic_layout_builtins_resolve_after_instantiation");
    write(
        &root.join("types.nia"),
        r#"
pub struct Packet[N: usize] {
    marker: u8,
    values: [u32; N],
}
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module types;
using root::types;

const fun packet_size[N: usize]() usize
where types::Packet[N]: Sized {
    std::builtin::size[types::Packet[N]]()
}

const fun marker_offset[N: usize]() usize {
    std::builtin::offset[types::Packet[N]]("marker")
}

const SIZE: usize = packet_size[3]();
const OFFSET: usize = marker_offset[3]();

fun main() usize {
    SIZE + OFFSET
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn trait_method_prefers_concrete_const_argument_impl() {
    let root = temp_dir("trait_method_prefers_concrete_const_argument_impl");
    write(
        &root.join("main.nia"),
        r#"
trait Rank[N: usize] {
    fun rank(&self) i32;
}

struct Box {}

extend[N: usize] Box : Rank[N] {
    fun rank(&self) i32 {
        1
    }
}

extend Box : Rank[3] {
    fun rank(&self) i32 {
        3
    }
}

fun main(box: &Box) i32 {
    box.rank()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn expected_return_selects_trait_method_const_instance() {
    let root = temp_dir("expected_return_selects_trait_method_const_instance");
    write(
        &root.join("main.nia"),
        r#"
trait Make[N: usize] {
    fun make(&self) [u8; N];
}

struct Box {}

extend Box : Make[2] {
    fun make(&self) [u8; 2] {
        [1u8, 2u8]
    }
}

extend Box : Make[3] {
    fun make(&self) [u8; 3] {
        [1u8, 2u8, 3u8]
    }
}

fun main(box: &Box) [u8; 3] {
    box.make()
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn associated_type_substitutes_const_argument_inferred_from_impl_target() {
    let root = temp_dir("associated_type_substitutes_const_argument_inferred_from_impl_target");
    write(
        &root.join("main.nia"),
        r#"
trait Storage {
    type Bytes;
}

struct Box[N: usize] {
    bytes: [u8; N],
}

extend[N: usize] Box[N] : Storage {
    type Bytes = [u8; N];
}

fun third(bytes: [Box[3] as Storage]::Bytes) u8 {
    bytes[2]
}

fun main() i32 {
    third([1u8, 2u8, 3u8]) as i32
}
"#,
    );

    let program = check_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

/// Array lengths that depend on generic parameters compare symbolically in a
/// generic body: `N` equals `N` and `@size[T]()` equals `@size[T]()`, while
/// a value never equals a symbol, whatever an instance would make them. A
/// repeat count that is a const parameter or a layout query is such a
/// length, so `[0u8; N]` has type `[u8; N]`; a mismatched count was ignored,
/// and these counts failed to evaluate.
#[test]
fn generic_array_lengths_compare_symbolically() {
    let root = temp_dir("generic_array_lengths_compare_symbolically");
    write(
        &root.join("main.nia"),
        r#"
using std::process;
fun zeros[N: usize]() usize {
    let inferred = [0u8; N];
    let annotated: [u8; N] = [0; N];
    inferred.len() + annotated.len()
}

fun measured[T]() usize where T: Sized {
    let a: [u8; std::builtin::size[T]()] = [0; std::builtin::size[T]()];
    a.len()
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if zeros[3]() != 6 or measured[u32]() != 4 or measured[u16]() != 2 {
        return process::ExitCode(1)!;
    }
    !()
}

"#,
    );
    let source = root.join("main.nia");
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("symbolic array lengths build");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run symbolic array length executable");
    assert_eq!(status.code(), Some(0));

    for (name, body, expected) in [
        (
            "count against a const parameter",
            "fun f[N: usize]() usize { let a: [u8; N] = [7; 16]; a.len() }\nfun main() usize { f[16]() }",
            "array literal length mismatch: expected N, got 16",
        ),
        (
            "list against a layout query",
            "fun f[T]() usize where T: Sized { let a: [u8; std::builtin::size[T]()] = [0, 0, 0, 0]; a.len() }\nfun main() usize { f[u32]() }",
            "array literal length mismatch: expected @size[T](), got 4",
        ),
    ] {
        let path = root.join("mismatch.nia");
        write(&path, &format!("{body}\n"));
        let program = check_program(path.to_string_lossy().into_owned());
        let errors = program
            .diagnostics
            .iter()
            .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
            .map(|d| d.diagnostic.summary.as_str())
            .collect::<Vec<_>>();
        assert_eq!(errors, [expected], "{name}");
    }
}

#[test]
fn const_substitution_does_not_capture_same_named_caller_type() {
    let root = temp_dir("const_substitution_does_not_capture_same_named_caller_type");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
const fun h[U, T, N: usize]() usize where T: Sized { std::builtin::size[T]() + N }
fun f[U, T]() usize where T: Sized { h[U, T, std::builtin::size[T]()]() }
fun main() usize { f[u8, u16]() }
"#,
    );
    let program = check_program(source.to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let output = test_driver().codegen(crate::CheckRequest::new(
        source.to_string_lossy().into_owned(),
    ));
    assert!(output.result.is_ok(), "{output:?}");
}

#[test]
fn const_expression_instance_identity_is_canonical_for_emit() {
    let root = temp_dir("const_expression_instance_identity_is_canonical_for_emit");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
struct Holder[N: usize] { a: [u8; N] }
fun make() Holder[3 + 1] { Holder[3 + 1] { a: [1, 2, 3, 4] } }
fun main() usize { make().a.len() }
"#,
    );
    let output = test_driver().emit_llvm_ir(crate::EmitLlvmRequest::new(crate::CheckRequest::new(
        source.to_string_lossy().into_owned(),
    )));
    assert!(output.result.is_ok(), "{output:?}");
}

#[test]
fn symbolic_const_short_circuit_does_not_check_unselected_failure() {
    let root = temp_dir("symbolic_const_short_circuit_does_not_check_unselected_failure");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
fun f[N: usize]() bool {
    const K = if N > 5 { [false; 7][8] } else { true };
    K
}

fun g[N: usize]() bool { N > 0 and 1 < N }
fun main() bool { f[1]() and g[2]() }
"#,
    );
    let program = check_program(source.to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let output = test_driver().emit_llvm_ir(crate::EmitLlvmRequest::new(crate::CheckRequest::new(
        source.to_string_lossy().into_owned(),
    )));
    assert!(output.result.is_ok(), "{output:?}");
}

#[test]
fn symbolic_const_short_circuit_checks_selected_array_index_failure() {
    let root = temp_dir("symbolic_const_short_circuit_checks_selected_array_index_failure");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
fun f[N: usize]() bool {
    const K = if N > 5 { [false; 7][8] } else { true };
    K
}

fun main() bool { f[6]() }
"#,
    );
    let program = check_program(source.to_string_lossy().into_owned());
    let errors = program
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.diagnostic.severity() == nia_diagnostic::Severity::Error)
        .map(|diagnostic| diagnostic.diagnostic.summary.as_str())
        .collect::<Vec<_>>();
    assert_eq!(errors, ["const array index 8 is out of bounds"]);
}

#[test]
fn symbolic_const_array_list_and_repeat_execute_after_substitution() {
    let root = temp_dir("symbolic_const_array_list_and_repeat_execute_after_substitution");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
fun list[N: usize]() bool {
    const K = [N, N + 1][1] == N + 1;
    K
}
fun repeat[N: usize]() bool {
    const K = [N; 2][1] == N;
    K
}
fun main() bool { list[3]() and repeat[3]() }
"#,
    );
    let program = check_program(source.to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let output = test_driver().emit_llvm_ir(crate::EmitLlvmRequest::new(crate::CheckRequest::new(
        source.to_string_lossy().into_owned(),
    )));
    assert!(output.result.is_ok(), "{output:?}");
}

#[test]
fn const_parameter_shadows_module_constant_in_all_value_contexts() {
    let root = temp_dir("const_parameter_shadows_module_constant_in_all_value_contexts");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
const N: usize = 9;
const fun value[N: usize]() usize { N }
fun f[N: usize]() usize {
    let a: [u8; N] = [0; N];
    value[N]() + a.len()
}

fun main() usize { f[2]() }
"#,
    );
    let program = check_program(source.to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}

#[test]
fn const_computation_accepts_layout_operand_without_local_alias() {
    let root = temp_dir("const_computation_accepts_layout_operand_without_local_alias");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
fun f[T]() usize where T: Sized {
    let a: [u8; std::builtin::size[T]() + 1] = [0; std::builtin::size[T]() + 1];
    a.len()
}
fun main() usize { f[u16]() }
"#,
    );
    let program = check_program(source.to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
}
