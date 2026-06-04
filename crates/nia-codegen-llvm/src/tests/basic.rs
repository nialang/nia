// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn codegen_ice_boundary_converts_panic_to_diagnostic() {
    let output = catch_llvm_codegen_ice(|| panic!("Nia ICE (LLVM): invalid value kind"));

    assert!(output.modules.is_empty());
    assert_eq!(output.diagnostics.len(), 1);
    assert!(
        output.diagnostics[0]
            .message
            .contains("internal compiler error: invalid value kind"),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn emits_declarations_for_checked_program() {
    let root = temp_dir("emits_declarations_for_checked_program");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &u8) i32;
let hello = c"hello";

extern struct Point {
    x: i32,
    y: i32,
}

extern fn use_point(p: Point) i32;

fn main() i32 {
    var x = 40;
    var y = 2;
    x + y
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("declare i32 @puts"));
    assert!(ir.contains("declare i32 @use_point"));
    assert!(ir.contains("@nia__m0__d"));
    assert!(ir.contains("%nia__m0__d"));
    assert!(ir.contains("define i32 @"));
    assert!(ir.contains("alloca i32"));
    assert!(ir.contains("store i32 40"));
    assert!(ir.contains("add i32"));
    assert!(ir.contains("ret i32"));
}
