// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

fn check_and_emit(name: &str, text: &str) {
    let root = temp_dir(name);
    check_and_emit_in(&root, text);
}

fn check_and_emit_in(root: &std::path::Path, text: &str) {
    let source = root.join("main.nia");
    write(&source, text);
    let program = check_program(source.to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:?}", program.diagnostics);
    let output = test_driver().emit_llvm_ir(crate::EmitLlvmRequest::new(crate::CheckRequest::new(
        source.to_string_lossy().into_owned(),
    )));
    assert!(output.result.is_ok(), "{output:?}");
}

#[test]
fn layout_const_return_instance_has_one_identity() {
    check_and_emit(
        "layout_const_return_instance",
        r#"
struct Holder[M: usize] { a: u8 }
fun mk[M: usize]() Holder[M] { Holder[M] { a: 1 } }
fun f[T]() u8 where T: Sized { mk[std::builtin::size[T]()]().a }
fun main() u8 { f[u32]() }
"#,
    );
}

#[test]
fn computed_field_length_accepts_concrete_constructor() {
    check_and_emit(
        "computed_field_concrete",
        r#"
struct S[N: usize] { a: [u8; N + 1] }
fun main() usize { S[3] { a: [0u8; 4] }.a.len() }
"#,
    );
}

#[test]
fn computed_field_length_accepts_symbolic_constructor() {
    check_and_emit(
        "computed_field_symbolic",
        r#"
struct S[N: usize] { a: [u8; N + 1] }
fun f[N: usize]() usize { S[N] { a: [0u8; N + 1] }.a.len() }
fun main() usize { f[3]() }
"#,
    );
}

#[test]
fn local_layout_alias_in_type_and_call() {
    check_and_emit(
        "local_layout_alias_in_type",
        r#"
struct Holder[M: usize] { a: u8 }
fun mk[M: usize]() Holder[M] { Holder[M] { a: 1 } }
fun f[T]() u8 where T: Sized {
    const S = std::builtin::size[T]();
    let x: Holder[S] = mk[S]();
    x.a
}
fun main() u8 { f[u32]() }
"#,
    );
}

#[test]
fn symbolic_if_repeat_contextualizes_integer_branches() {
    check_and_emit(
        "symbolic_if_repeat",
        r#"
fun f[N: usize]() usize { [0u8; if N > 0 { 1 } else { 2 }].len() }
fun main() usize { f[1]() + f[0]() }
"#,
    );
}

#[test]
fn symbolic_comparison_contextualizes_left_literal() {
    check_and_emit(
        "symbolic_left_literal",
        r#"
fun f[N: usize]() usize {
    const K = if N > 0 and 1 < N { 1usize } else { 2usize };
    K
}
fun main() usize { f[2]() }
"#,
    );
}

#[test]
fn symbolic_if_defers_indexing_global_array() {
    check_and_emit(
        "symbolic_global_array_branch",
        r#"
const ARR: [usize; 1] = [0];
fun f[N: usize]() usize {
    const K = if N > 5 { ARR[7] } else { 2usize };
    K
}
fun main() usize { f[1]() }
"#,
    );
}

#[test]
fn nested_const_computation_call_respects_binders() {
    check_and_emit(
        "nested_const_computation_binders",
        r#"
const fun plus[N: usize]() usize { N + 1 }
const fun twice[N: usize]() usize { plus[N + 1]() }
const K = twice[2]();
fun main() usize { K }
"#,
    );
}

#[test]
fn computed_fields_preserve_imported_declaration_binders() {
    let root = temp_dir("computed_imported_fields");
    write(
        &root.join("types.nia"),
        r#"
pub struct Buffer[M: usize] { a: [u8; M + 1] }
pub type Bytes[N: usize] = Buffer[N];
pub union Slot[J: usize] { bytes: [u8; J + 1], tag: u8 }
pub enum Packet[K: usize] { Bytes([u8; K + 1]), Empty }
"#,
    );
    check_and_emit_in(
        &root,
        r#"
module types;
fun f[N: usize]() usize {
    let a = types::Bytes[N] { a: [0u8; N + 1] };
    let b = types::Slot[N] { bytes: [0u8; N + 1] };
    let c = types::Packet[N]::Bytes([0u8; N + 1]);
    a.a.len() + b.bytes.len() + match c { .Bytes(x) => x.len(), .Empty => 0 }
}
fun main() usize { f[3]() }
"#,
    );
}

#[test]
fn terminating_generic_const_recursion_keeps_context() {
    check_and_emit(
        "terminating_generic_const_recursion",
        r#"
const fun countdown[N: usize]() usize {
    if N == 0 { return 0; }
    countdown[N - 1]() + 1
}

const K = countdown[16]();
fun main() usize { K }
"#,
    );
}

#[test]
fn const_followup_results_execute() {
    let root = temp_dir("const_followup_results");
    let source = root.join("main.nia");
    write(
        &root.join("types.nia"),
        r#"
pub struct Buffer[M: usize] { a: [u8; M + 1] }
pub enum Packet[K: usize] { Bytes([u8; K + 1]), Empty }
"#,
    );
    write(
        &source,
        r#"
using std::process;
module types;
fun foreign[N: usize]() usize {
    let b = types::Buffer[N] { a: [0u8; N + 1] };
    let make: &fun([u8; N + 1]) types::Packet[N] = types::Packet[N]::Bytes;
    let p = make([0u8; N + 1]);
    b.a.len() + match p { .Bytes(x) => x.len(), .Empty => 0 }
}
struct Buffer[N: usize] { a: [u8; N + 1] }
struct Holder[N: usize] { a: usize }
fun mk[N: usize]() Holder[N] { Holder[N] { a: N } }
fun aliased[T]() usize where T: Sized {
    const S = std::builtin::size[T]();
    let x: Holder[S] = mk[S]();
    x.a
}
fun field[N: usize]() usize { Buffer[N] { a: [0u8; N + 1] }.a.len() }
const ARR: [usize; 1] = [9];
fun lazy[N: usize]() usize {
    const K = if N > 5 { ARR[7] } else { 2usize };
    K
}
fun contextual[N: usize]() usize {
    [0u8; if N > 0 and 1 < N { 1 } else { 2 }].len()
}
const fun plus[N: usize]() usize { N + 1 }
const fun twice[N: usize]() usize { plus[N + 1]() }
const fun countdown[N: usize]() usize {
    if N == 0 { return 0; }
    countdown[N - 1]() + 1
}
const K = twice[2]();
const R = countdown[16]();
const R2 = countdown[8]();
const J = twice[7]();
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if aliased[u32]() != 4 or aliased[u64]() != 8 or field[3]() != 4
        or field[7]() != 8 or lazy[1]() != 2 or contextual[2]() != 1
        or contextual[0]() != 2 or contextual[1]() != 2 or K != 4 or R != 16
        or R2 != 8 or J != 9 or foreign[3]() != 8 or foreign[7]() != 16 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    );
    let artifact = test_driver()
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("link const follow-up regression");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("execute const follow-up regression");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn selected_symbolic_branch_reports_its_real_failure() {
    let root = temp_dir("selected_symbolic_branch_failure");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
const ARR: [usize; 1] = [0];
fun f[N: usize]() usize {
    const K = if N > 5 { ARR[7] } else { 2usize };
    K
}
fun main() usize { f[6]() }
"#,
    );
    for diagnostics in [
        check_program(source.to_string_lossy().into_owned()).diagnostics,
        codegen_program(source.to_string_lossy().into_owned()).diagnostics,
    ] {
        assert!(
            diagnostics
                .iter()
                .any(|d| d.diagnostic.summary.contains("out of bounds")),
            "{diagnostics:?}"
        );
        assert!(
            diagnostics
                .iter()
                .all(|d| d.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal),
            "{diagnostics:?}"
        );
        assert!(
            !diagnostics.iter().any(|d| d
                .diagnostic
                .summary
                .contains("failed to evaluate const generic parameter")),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn type_argument_candidate_does_not_capture_runtime_namesake() {
    check_and_emit(
        "ambiguous_type_argument_capture",
        r#"
struct Box[T] { a: T }
type X = i32;
fun main() i32 {
    let X = 1;
    let c = \ -> { let b: Box[X] = Box[i32] { a: 1 }; b.a };
    c()
}
"#,
    );
}

#[test]
fn const_expression_roots_preserve_their_own_integer_context() {
    for (name, body) in [
        ("binding", "let x: u8 = { (255 + 1) - 1 }; x as usize"),
        (
            "pattern",
            "let (x, _): (u8, u8) = ({ (255 + 1) - 1 }, 0); x as usize",
        ),
        ("argument", "byte({ (255 + 1) - 1 }) as usize"),
        (
            "constructor",
            "match Event::Data({ (255 + 1) - 1 }) { .Data(x) => x as usize }",
        ),
    ] {
        let root = temp_dir(&format!("const_expression_root_{name}"));
        let source = root.join("main.nia");
        write(
            &source,
            &format!(
                "enum Event {{ Data(u8) }}\n\
                 const fun byte(x: u8) u8 {{ x }}\n\
                 const fun compute() usize {{ {body} }}\n\
                 const K = compute();\nfun main() usize {{ K }}\n"
            ),
        );
        let program = check_program(source.to_string_lossy().into_owned());
        assert!(
            program.diagnostics.iter().any(|diagnostic| diagnostic
                .diagnostic
                .summary
                .contains("integer overflow in const addition")),
            "{name}: {:?}",
            program.diagnostics
        );
    }
    check_and_emit(
        "const_nested_wider_context",
        r#"
const fun byte(x: usize) u8 { if x == 256 { 0 } else { 1 } }
const fun compute() u8 {
    let x: usize = { 255 + 1 };
    let (y, _): (usize, u8) = ({ 255 + 1 }, 0);
    byte({ x + y - 256 })
}

const K = compute();
fun main() u8 { K }
"#,
    );
}

#[test]
fn const_omitted_aggregates_use_instantiated_context() {
    check_and_emit(
        "const_omitted_instantiated_context",
        r#"
struct Value[T] { value: T }
union Bits[T] { value: T, bytes: [u8; 1] }
const fun take[T](x: Value[T]) T { x.value }
const fun take_bits[T](x: Bits[T]) T { x.value }
const fun make() u8 { take(.{ value: 255 }) }
const fun nested() u8 {
    let x: Value[Value[u8]] = .{ value: .{ value: 255 } };
    x.value.value
}
const fun bits() u8 { take_bits(.{ value: 255 }) }
const A = make();
const B = nested();
const C = bits();
const OK: bool = A == 255u8 and B == 255u8 and C == 255u8;
fun main() usize { let x: [u8; if OK { 1 } else { 2 }] = [0u8; 1]; x.len() }
"#,
    );
    let root = temp_dir("const_explicit_aggregate_keeps_its_arguments");
    let source = root.join("main.nia");
    write(
        &source,
        r#"
struct Value[T] { value: T }
const fun take[T](x: Value[T]) T { x.value }
const fun make() u8 { take(Value[u16] { value: 255 }) }
const A = make();
fun main() u8 { A }
"#,
    );
    let program = check_program(source.to_string_lossy().into_owned());
    assert!(
        nia_compiler_query::has_error_diagnostics(&program.diagnostics),
        "{:?}",
        program.diagnostics
    );
    assert!(
        program
            .diagnostics
            .iter()
            .all(|d| d.diagnostic.category() != nia_diagnostic::DiagnosticCategory::Internal),
        "{:?}",
        program.diagnostics
    );
}
