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
