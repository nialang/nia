// SPDX-License-Identifier: GPL-3.0-or-later
//! Generic const values: the carrier of a value, the binders of a const
//! expression written in a declaration, laziness and failure of terms, and
//! how each is reported.
use super::common::*;

/// Builds `source` as an executable and returns its exit code.
fn run_exit_code(name: &str, source: &str) -> Option<i32> {
    let root = temp_dir(name);
    let path = root.join("main.nia");
    write(&path, source);
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(path.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .unwrap_or_else(|error| panic!("{name} builds: {error:?}"));
    std::process::Command::new(&artifact.path)
        .status()
        .expect("run test executable")
        .code()
}

/// The error summaries `check` reports for `source`, in order.
fn error_summaries(name: &str, source: &str) -> Vec<String> {
    let root = temp_dir(name);
    let path = root.join("main.nia");
    write(&path, source);
    check_program(path.to_string_lossy().into_owned())
        .diagnostics
        .iter()
        .filter(|d| d.diagnostic.severity() == nia_diagnostic::Severity::Error)
        .map(|d| d.diagnostic.summary.clone())
        .collect()
}

/// A literal that does not fit the type its context gives it is an error,
/// in a symbolic branch as anywhere else. Building the branch's term used
/// to wrap `300` into `44` for a `u8` parameter.
#[test]
fn symbolic_const_operands_never_wrap() {
    let errors = error_summaries(
        "symbolic_const_operands_never_wrap",
        "fun g[M: u8]() u8 { M }\n\
         fun f[N: usize]() u8 { g[if N > 5 { 300 } else { 2 }]() }\n\
         fun main() u8 { f[6]() }\n",
    );
    assert_eq!(errors, ["const integer literal 300 is out of range for u8"]);
}

/// Equal values are one instance whatever produced them, also when the
/// const parameter's type is an alias of an integer type.
#[test]
fn const_values_have_one_identity_through_carrier_aliases() {
    let root = temp_dir("const_values_have_one_identity_through_carrier_aliases");
    let path = root.join("main.nia");
    write(
        &path,
        r#"
type Len = usize;
struct Holder[M: Len] { a: u8 }
fun mk[M: Len]() Holder[M] { Holder[M] { a: 1 } }
fun val[M: Len](h: Holder[M]) usize { M + (h.a as usize) }
fun f[T]() usize where T: Sized { val[std::builtin::size[T]()](mk[std::builtin::size[T]()]()) }
fun main() usize { f[u32]() + val[4](mk[4]()) + val[2 + 2](mk[2 + 2]()) }
"#,
    );
    let output = test_driver().emit_llvm_ir(crate::EmitLlvmRequest::new(crate::CheckRequest::new(
        path.to_string_lossy().into_owned(),
    )));
    let ir = output
        .result
        .expect("emit LLVM IR")
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<String>();
    // `main`, `f[u32]`, and one instance each of `val[4]` and `mk[4]`.
    assert_eq!(ir.matches("\ndefine ").count(), 4, "{ir}");
}

/// A const parameter's type holds a const value: an integer type, `bool`,
/// or `char`, possibly through an alias.
#[test]
fn const_parameter_types_are_scalars() {
    let errors = error_summaries(
        "const_parameter_types_are_scalars",
        "struct P { x: i32 }\n\
         type Len = usize;\n\
         struct A[M: f64] { a: u8 }\n\
         struct B[M: P] { a: u8 }\n\
         struct C[M: Len, F: bool, K: char] { a: u8 }\n\
         fun main() usize { 0 }\n",
    );
    assert_eq!(
        errors,
        [
            "a const parameter must have an integer type, `bool`, or `char`",
            "a const parameter must have an integer type, `bool`, or `char`",
        ]
    );
}

/// Const expressions over a parameter of any integer type are terms that
/// each instance evaluates, as `usize` ones are.
#[test]
fn computed_const_arguments_of_every_integer_type() {
    for carrier in ["u8", "i8", "i32", "u64", "isize"] {
        let source = format!(
            r#"
using std::process;
struct H[M: {carrier}] {{ a: u8 }}
fun mk[M: {carrier}]() H[M] {{ H[M] {{ a: M as u8 }} }}
fun next[K: {carrier}]() H[K + 1] {{ mk[K + 1]() }}
fun same[K: {carrier}](h: H[K + 1]) u8 {{ h.a }}
pub fun main(init: process::Init) process::ExitCode!() {{
    _ = init;
    let h: H[4] = next[3]();
    if h.a != 4 or same[3](mk[4]()) != 4 {{
        return process::ExitCode(1)!;
    }}
    !()
}}
"#
        );
        assert_eq!(
            run_exit_code(&format!("computed_const_arguments_{carrier}"), &source),
            Some(0),
            "{carrier}"
        );
    }
}

/// An alias whose target computes a length is expanded with the
/// computation bound to the alias's arguments, so `A2[3]` is `[u8; 4]`.
#[test]
fn alias_targets_bind_their_computations() {
    let source = r#"
using std::process;
const fun inc(n: usize) usize { n + 1 }
struct P1[M: usize] { a: [u8; M + 1] }
type A1[Q: usize] = P1[Q + 1];
type A2[Q: usize] = [u8; Q + 1];
type A6[Q: usize] = [u8; inc(Q)];
fun generic[N: usize]() usize {
    let x: A6[N] = [1; inc(N)];
    x.len()
}
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let a: A2[3] = [1; 4];
    let b: A6[3] = [1; 4];
    let c = A1[3] { a: [1; 5] };
    if a.len() + b.len() + c.a.len() + generic[3]() != 17 {
        return process::ExitCode(1)!;
    }
    !()
}
"#;
    assert_eq!(
        run_exit_code("alias_targets_bind_their_computations", source),
        Some(0)
    );
    assert_eq!(
        error_summaries(
            "alias_targets_bind_their_computations_mismatch",
            "type A2[Q: usize] = [u8; Q + 1];\n\
             fun f() usize { let x: A2[3] = [1; 5]; x.len() }\n\
             fun main() usize { f() }\n",
        ),
        ["array literal length mismatch: expected `3 + 1`, got 5"]
    );
}

/// A declaration of another module is resolved with that module's const
/// facts before its arguments are substituted.
#[test]
fn foreign_declarations_bind_their_computations() {
    let root = temp_dir("foreign_declarations_bind_their_computations");
    write(
        &root.join("t.nia"),
        r#"
pub enum E[K: usize] { Bytes([u8; K + 1]), Empty }
pub struct B2[M: usize] { a: [u8; M + 1] }
pub struct R7[P: usize] { t: (B2[P], [u8; P + 1]) }
"#,
    );
    let path = root.join("main.nia");
    write(
        &path,
        r#"
module t;
using std::process;
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let e = t::E[4]::Bytes([1; 5]);
    let n = match e {
        .Bytes(x) => x.len(),
        .Empty => 0,
    };
    let r = t::R7[3] { t: (t::B2[3] { a: [1; 4] }, [2; 4]) };
    if n != 5 or std::builtin::size[t::R7[3]]() != 8 or r.t.0.a.len() != 4 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(path.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("foreign declarations build");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run executable");
    assert_eq!(status.code(), Some(0));
}

/// A term never executes what its instance does not select: `error`,
/// `trap`, an out-of-bounds index, or a `match` arm. A selected failure is
/// reported at its own span.
#[test]
fn symbolic_terms_are_lazy_and_report_selected_failures() {
    let program = r#"
using std::process;
const ARR: [usize; 3] = [1, 2, 3];
fun pick[N: usize]() usize {
    let a = [0u8; if N > 5 { std::builtin::error("too big") } else { N + 1 }];
    a.len()
}
fun trapping[N: usize]() usize {
    let a = [0u8; if N > 5 { std::builtin::trap() } else { 2 }];
    a.len()
}
fun indexed[N: usize]() usize {
    let a = [0u8; if N > 5 { ARR[7] } else { ARR[N] }];
    a.len()
}
fun classify[N: usize]() usize {
    const K: usize = match N {
        0 => 10,
        3..=5 => 30,
        n => n * 100,
    };
    K
}
fun width[N: u8]() usize {
    const W: usize = if N > 0 { 2 } else { 1 };
    W
}
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if pick[3]() != 4 or trapping[1]() != 2 or indexed[1]() != 2 {
        return process::ExitCode(1)!;
    }
    if classify[0]() != 10 or classify[4]() != 30 or classify[7]() != 700 {
        return process::ExitCode(2)!;
    }
    if width[0]() != 1 or width[9]() != 2 {
        return process::ExitCode(3)!;
    }
    !()
}
"#;
    assert_eq!(run_exit_code("symbolic_terms_are_lazy", program), Some(0));
    for (call, expected) in [
        ("pick[7]()", "too big"),
        (
            "trapping[9]()",
            "builtin `trap` reached during const evaluation",
        ),
        ("indexed[9]()", "const array index 7 is out of bounds"),
    ] {
        let selected = program.replace(
            "if pick[3]() != 4 or",
            &format!("if {call} != 0 or pick[3]() != 4 or"),
        );
        assert_eq!(
            error_summaries("symbolic_terms_report_selected_failures", &selected),
            [expected],
            "{call}"
        );
    }
}

/// A closed subterm is its value, so spellings that differ only in closed
/// operands are one term; `N + 1` and `1 + N` remain different.
#[test]
fn closed_subterms_are_values() {
    assert!(
        error_summaries(
            "closed_subterms_are_values",
            "fun f[N: usize]() usize {\n\
             let x: [u8; N + 3] = [0; N + (1 + 2)];\n\
             let y: [u8; N + 1] = [0; N + (1 as usize)];\n\
             x.len() + y.len()\n\
             }\n\
             fun main() usize { f[1]() }\n",
        )
        .is_empty()
    );
}

/// Non-tail recursion in a `const fun` stays within the host stack and
/// reaches the call-depth limit as a diagnostic.
#[test]
fn const_recursion_never_exhausts_the_host_stack() {
    let finite = "using std::process;\n\
         const fun down(n: u32) u32 {\n\
             if n == 0 {\n\
                 return 0;\n\
             }\n\
             return down(n - 1) + 1;\n\
         }\n\
         const A: u32 = down(255);\n\
         pub fun main(init: process::Init) process::ExitCode!() {\n\
             _ = init;\n\
             if A != 255 {\n\
                 return process::ExitCode(1)!;\n\
             }\n\
             !()\n\
         }\n";
    assert_eq!(run_exit_code("const_recursion_finite", finite), Some(0));
    assert_eq!(
        error_summaries(
            "const_recursion_unbounded",
            "const fun g(n: u32) u32 { return g(n + 1) + 1; }\n\
             const A: u32 = g(0);\n\
             fun main() u32 { A }\n",
        ),
        ["const evaluation exceeded the 256 call depth limit"]
    );
}

/// A value that violates its type is reported once, where it is written,
/// not again at each call boundary and constant it reaches.
#[test]
fn ill_typed_const_values_report_only_their_source() {
    let mut errors = error_summaries(
        "ill_typed_const_values_report_only_their_source",
        "struct Value[T] { value: T }\n\
         const fun take[T](x: Value[T]) T { x.value }\n\
         const fun make() u8 { take(.{ value: 256 }) }\n\
         const fun pass(x: u8) u8 { x }\n\
         const fun other() u8 { pass(300) }\n\
         const A = make();\n\
         const B = other();\n\
         fun main() u8 { A + B }\n",
    );
    errors.sort();
    assert_eq!(
        errors,
        [
            "integer literal 256 is out of range for u8 in struct literal field",
            "integer literal 300 is out of range for u8 in call argument 1",
        ]
    );
}

/// A local constant written in a type, a cast or a qualified path is the
/// constant; one a closure has not captured is reported once per use.
#[test]
fn local_constants_in_types_resolve_lexically() {
    assert!(
        error_summaries(
            "local_constants_in_types_resolve_lexically",
            "struct Exact[M: usize] { a: u8 }\n\
             enum Packet[K: usize] { Bytes([u8; K + 1]), Empty }\n\
             fun f[T]() u8 where T: Sized {\n\
             const S = std::builtin::size[T]();\n\
             let e = Exact[S] { a: 1 };\n\
             let p = &e as &Exact[S];\n\
             let q: Packet[S] = Packet[S]::Empty;\n\
             e.a + p.a\n\
             }\n\
             fun main() u8 { f[u32]() }\n",
        )
        .is_empty()
    );
    assert_eq!(
        error_summaries(
            "uncaptured_local_constants",
            "struct Exact[M: usize] { a: u8 }\n\
             fun mk[M: usize]() Exact[M] { Exact[M] { a: 5 } }\n\
             fun main() u8 {\n\
             const S: usize = 4;\n\
             let c = \\ -> {\n\
             let e: Exact[S] = mk[S]();\n\
             e.a\n\
             };\n\
             c()\n\
             }\n",
        ),
        [
            "local `S` is not captured by this closure; add it to the closure capture list",
            "local `S` is not captured by this closure; add it to the closure capture list",
        ]
    );
}

/// A mismatch names nominal types with their const arguments.
#[test]
fn type_names_show_const_arguments() {
    assert_eq!(
        error_summaries(
            "type_names_show_const_arguments",
            "struct Holder[N: usize] { a: [u8; N] }\n\
             fun mk[N: usize]() Holder[N] { Holder[N] { a: [0u8; N] } }\n\
             fun main() usize { let z: Holder[3] = mk[4](); z.a.len() }\n",
        ),
        ["type mismatch in binding initializer: expected Holder[3], got Holder[4]"]
    );
}

/// A value pattern is a literal or a path; `1 | 2` is not an alternative.
#[test]
fn value_patterns_are_literals_or_paths() {
    assert_eq!(
        error_summaries(
            "value_patterns_are_literals_or_paths",
            "fun f(n: usize) usize {\n\
             match n {\n\
             1 | 2 => 20,\n\
             _ => 0,\n\
             }\n\
             }\n\
             fun main() usize { f(2) }\n",
        ),
        [
            "a value pattern with an operator is written in parentheses, as in `(a + 1)`; there are no alternative patterns, so write each value as its own arm"
        ]
    );
}

/// A computed length in an explicit type argument is the term of each
/// instance, as it is in any other type.
#[test]
fn computed_lengths_in_type_arguments_instantiate() {
    let source = r#"
using std::process;
fun g[T]() usize where T: Sized { std::builtin::size[T]() }
fun f[N: usize]() usize { g[[u8; N + 1]]() + g[(u8, [u8; N])]() }
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if f[3]() != 8 or f[5]() != 12 {
        return process::ExitCode(1)!;
    }
    !()
}
"#;
    assert_eq!(
        run_exit_code("computed_lengths_in_type_arguments_instantiate", source),
        Some(0)
    );
}

/// A type is checked the same way whatever else its module contains.
#[test]
fn type_checking_is_independent_of_unrelated_generic_code() {
    let plain = "type Bytes = [u8; 2];\nfun f() Bytes { let x: Bytes = [1; 3]; x }\nfun main() usize { 0 }\n";
    let with_generic = format!(
        "{plain}fun other[N: usize]() usize {{ let y: [u8; N + 1] = [0; N + 1]; y.len() }}\n"
    );
    assert_eq!(
        error_summaries("type_checking_plain", plain),
        error_summaries("type_checking_with_generic", &with_generic)
    );
}
