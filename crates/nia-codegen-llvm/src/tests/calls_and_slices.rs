// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_direct_function_calls() {
    let root = temp_dir("emits_direct_function_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn add(a: i32, b: i32) i32 {
    a + b
}

fn main() i32 {
    add(20, 22)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @nia__m0__d"));
    assert!(ir.contains("call i32 @nia__m0__d"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_extern_function_definitions_with_unmangled_symbols() {
    let root = temp_dir("emits_extern_function_definitions_with_unmangled_symbols");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn add(a: i32, b: i32) i32 {
    a + b
}

fn main() i32 {
    add(40, 2)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("define i32 @add("), "{ir}");
    assert!(!ir.contains("nia__m0__d0__add"), "{ir}");
    assert!(ir.contains("call i32 @add"), "{ir}");
}

#[test]
fn emits_slice_construction_len_ptr_and_indexing() {
    let root = temp_dir("emits_slice_construction_len_ptr_and_indexing");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn first(xs: &const [i32]) i32 {
    xs[0]
}

fn main() i32 {
    var xs: [4]i32 = [1, 2, 3, 4];
    var s = &const xs[1..=2];
    var p = s.get_ptr_const();
    var single = &const p[..];
    first(s) + s.len() as i32 + single.len() as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("insertvalue"));
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_array_to_slice_coercions() {
    let root = temp_dir("emits_array_to_slice_coercions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn first(xs: &const [i32]) i32 {
    xs[0]
}

fn first_byte(xs: &const [u8]) i32 {
    xs[0] as i32
}

fn overwrite(xs: &[i32]) i32 {
    xs[1] = 9;
    xs[1]
}

fn main() i32 {
    var xs: [3]i32 = [1, 2, 3];
    var borrow = &const xs[..];
    var literal: &const [i32] = [4, 5, 6];
    first(borrow) + first(literal) + first([7, 8]) + first_byte(c"hi") + overwrite([6, 7])
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("arraytmp"), "{ir}");
    assert!(ir.contains("insertvalue"), "{ir}");
    assert!(ir.contains("getelementptr"), "{ir}");
}

#[test]
fn emits_global_string_pointer_call() {
    let root = temp_dir("emits_global_string_pointer_call");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &const u8) i32;
const hello = c"hello";

fn main() i32 {
    _ = puts(&const hello[0]);
    0
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
    assert!(ir.contains("c\"hello\\00\""));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("@nia__m0__d"));
}

#[test]
fn emits_address_of_checked_places_from_function_ir() {
    let root = temp_dir("emits_address_of_checked_places_from_function_ir");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn read(ptr: &const i32) i32 {
    ptr.*
}

fn main(i: usize) i32 {
    var pair: Pair = { a: 10, b: 20 };
    var xs: [2]i32 = [30, 40];
    read(&const pair.b) + read(&const xs[i])
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("getelementptr"), "{ir}");
    assert!(ir.contains("call i32 @nia__m0__d3__read"), "{ir}");
}

#[test]
fn emits_c_string_literal_pointer_coercions() {
    let root = temp_dir("emits_c_string_literal_pointer_coercions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn puts(s: &const u8) i32;

fn first(ptr: &u8) i32 {
    ptr.* = b'J';
    ptr.* as i32
}

fn main() i32 {
    var direct: &const u8 = c"hello";
    var writable: &u8 = c"mutable";
    _ = puts(c"world");
    _ = puts(
        c\\multi
        \\line
    );
    first(writable) + direct.* as i32
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
    assert!(ir.contains("arraytmp"));
    assert!(ir.contains("getelementptr"));
    assert!(ir.contains("call i32 @puts"));
    assert!(ir.contains("[6 x i8] c\"hello\\00\""), "{ir}");
    assert!(ir.contains("[8 x i8] c\"mutable\\00\""), "{ir}");
}
