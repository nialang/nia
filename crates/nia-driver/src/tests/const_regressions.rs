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
