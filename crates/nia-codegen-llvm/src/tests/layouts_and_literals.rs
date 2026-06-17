// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_nia_structs_in_physical_layout_order() {
    let root = temp_dir("emits_nia_structs_in_physical_layout_order");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Mixed {
    a: u8,
    b: i64,
    c: u8,
}

let mixed: Mixed = { a: 1, b: 2, c: 3 };

fn main() i32 {
    var local: Mixed = { a: 4, b: 5, c: 6 };
    mixed.a as i32 + mixed.c as i32 + local.a as i32 + local.c as i32 + local.b as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("%nia__m0__d0__Mixed = type { i64, i8, i8 }"),
        "{ir}"
    );
    assert!(
        ir.contains("@nia__m0__d4__mixed = constant %nia__m0__d0__Mixed { i64 2, i8 1, i8 3 }"),
        "{ir}"
    );
    assert!(ir.contains("ptr @nia__m0__d4__mixed, i32 0, i32 1"), "{ir}");
    assert!(ir.contains("ptr %local, i32 0, i32 2"), "{ir}");
    assert!(ir.contains("ptr %local, i32 0, i32 0"), "{ir}");
}

#[test]
fn emits_nia_function_aggregate_parameter_and_return_abi() {
    let root = temp_dir("emits_nia_function_aggregate_parameter_and_return_abi");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn id(pair: Pair) Pair {
    pair
}

fn sum(pair: Pair) i32 {
    pair.a + pair.b
}

fn main() i32 {
    var pair: Pair = { a: 10, b: 20 };
    var copied = id(pair);
    sum(copied)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("define void @nia__m0__d3__id(ptr %0, ptr %1)"),
        "{ir}"
    );
    assert!(ir.contains("define i32 @nia__m0__d4__sum(ptr %0)"), "{ir}");
    assert!(
        ir.contains("call void @nia__m0__d3__id(ptr %copied, ptr %arg.copy"),
        "{ir}"
    );
    assert!(
        ir.contains("call i32 @nia__m0__d4__sum(ptr %arg.copy"),
        "{ir}"
    );
}

#[test]
fn emits_aggregate_literal_indirect_args_without_extra_literal_temps() {
    let root = temp_dir("emits_aggregate_literal_indirect_args_without_extra_literal_temps");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn sum_pair(pair: Pair) i32 {
    pair.a + pair.b
}

fn sum_array(values: [2]i32) i32 {
    values[0] + values[1]
}

fn main() i32 {
    sum_pair({ a: 10, b: 20 }) + sum_array([30, 40])
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("call i32 @nia__m0__d3__sum_pair(ptr %arg.copy"),
        "{ir}"
    );
    assert!(
        ir.contains("call i32 @nia__m0__d4__sum_array(ptr %arg.copy"),
        "{ir}"
    );
    assert!(!ir.contains("structtmp"), "{ir}");
    assert!(!ir.contains("arraytmp"), "{ir}");
}

#[test]
fn emits_aggregate_literal_local_stores_without_extra_literal_temps() {
    let root = temp_dir("emits_aggregate_literal_local_stores_without_extra_literal_temps");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn sum_pair(pair: Pair) i32 {
    pair.a + pair.b
}

fn sum_array(values: [2]i32) i32 {
    values[0] + values[1]
}

fn main() i32 {
    var pair: Pair = { a: 10, b: 20 };
    pair = { a: 30, b: 40 };
    var values: [2]i32 = [50, 60];
    values = [70, 80];
    sum_pair(pair) + sum_array(values)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("%pair = alloca"), "{ir}");
    assert!(ir.contains("%values = alloca"), "{ir}");
    assert!(!ir.contains("structtmp"), "{ir}");
    assert!(!ir.contains("arraytmp"), "{ir}");
}

#[test]
fn emits_aggregate_call_results_directly_into_local_stores() {
    let root = temp_dir("emits_aggregate_call_results_directly_into_local_stores");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn make_pair(a: i32, b: i32) Pair {
    { a: a, b: b }
}

fn make_array(a: i32, b: i32) [2]i32 {
    [a, b]
}

fn sum_pair(pair: Pair) i32 {
    pair.a + pair.b
}

fn sum_array(values: [2]i32) i32 {
    values[0] + values[1]
}

fn main() i32 {
    var pair: Pair = make_pair(10, 20);
    pair = make_pair(30, 40);
    var values: [2]i32 = make_array(50, 60);
    values = make_array(70, 80);
    sum_pair(pair) + sum_array(values)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(!ir.contains("call.out"), "{ir}");
    assert_substrings_in_order(ir, &["%pair = alloca", "make_pair(ptr %pair"]);
    assert_substrings_in_order(ir, &["%values = alloca", "make_array(ptr %values"]);
}

#[test]
fn emits_aggregate_literal_returns_without_extra_literal_temps() {
    let root = temp_dir("emits_aggregate_literal_returns_without_extra_literal_temps");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn make_pair() Pair {
    { a: 10, b: 20 }
}

fn make_array() [2]i32 {
    [30, 40]
}

fn sum_pair(pair: Pair) i32 {
    pair.a + pair.b
}

fn sum_array(values: [2]i32) i32 {
    values[0] + values[1]
}

fn main() i32 {
    sum_pair(make_pair()) + sum_array(make_array())
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("return.copy"), "{ir}");
    assert!(!ir.contains("call.out"), "{ir}");
    assert_substrings_in_order(ir, &["%arg.copy = alloca", "make_pair(ptr %arg.copy)"]);
    assert_substrings_in_order(ir, &["%arg.copy1 = alloca", "make_array(ptr %arg.copy1)"]);
    assert!(!ir.contains("structtmp"), "{ir}");
    assert!(!ir.contains("arraytmp"), "{ir}");
}

#[test]
fn aggregate_literal_return_preserves_initializer_and_defer_order() {
    let root = temp_dir("aggregate_literal_return_preserves_initializer_and_defer_order");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(value: i32) i32;

struct Pair {
    a: i32,
    b: i32,
}

fn cleanup(value: i32) void {
    _ = log(value);
}

fn make_pair() Pair {
    defer cleanup(2);
    { a: log(1), b: 3 }
}

fn sum_pair(pair: Pair) i32 {
    pair.a + pair.b
}

fn main() i32 {
    sum_pair(make_pair())
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_substrings_in_order(
        ir,
        &[
            "call i32 @log(i32 1)",
            "cleanup(i32 2)",
            "store %nia__m0__d1__Pair %return.value",
        ],
    );
}

#[test]
fn emits_aggregate_call_returns_directly_without_defers() {
    let root = temp_dir("emits_aggregate_call_returns_directly_without_defers");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Pair {
    a: i32,
    b: i32,
}

fn make_pair(a: i32, b: i32) Pair {
    { a: a, b: b }
}

fn forward_pair() Pair {
    make_pair(10, 20)
}

fn sum_pair(pair: Pair) i32 {
    pair.a + pair.b
}

fn main() i32 {
    sum_pair(forward_pair())
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(!ir.contains("call.out"), "{ir}");
    assert_substrings_in_order(
        ir,
        &[
            "define void @nia__m0__d4__forward_pair(ptr %0)",
            "call void @nia__m0__d3__make_pair(ptr %0, i32 10, i32 20)",
            "ret void",
        ],
    );
}

#[test]
fn aggregate_call_return_with_defer_keeps_return_store_after_defer() {
    let root = temp_dir("aggregate_call_return_with_defer_keeps_return_store_after_defer");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(value: i32) i32;

struct Pair {
    a: i32,
    b: i32,
}

fn cleanup(value: i32) void {
    _ = log(value);
}

fn make_pair(a: i32, b: i32) Pair {
    { a: a, b: b }
}

fn forward_pair() Pair {
    defer cleanup(1);
    make_pair(10, 20)
}

fn sum_pair(pair: Pair) i32 {
    pair.a + pair.b
}

fn main() i32 {
    sum_pair(forward_pair())
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call.out"), "{ir}");
    assert_substrings_in_order(
        ir,
        &[
            "define void @nia__m0__d6__forward_pair(ptr %0)",
            "call void @nia__m0__d5__make_pair(ptr %call.out, i32 10, i32 20)",
            "cleanup(i32 1)",
            "store %nia__m0__d1__Pair %call.result, ptr %0",
        ],
    );
}

#[test]
fn emits_unary_cast_float_and_enum_values() {
    let root = temp_dir("emits_unary_cast_float_and_enum_values");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Color: u8 {
    Red,
    Blue = 3,
}

fn main(ptr: & i32) i32 {
    var x = -1;
    var y = not false;
    var n = 1;
    var z: f64 = n as f64;
    var w: i32 = z as i32;
    var addr = ptr as usize;
    var again = addr as &i32;
    x + w + again.* + Color::Blue as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("sitofp"));
    assert!(ir.contains("fptosi"));
    assert!(ir.contains("ptrtoint"));
    assert!(ir.contains("inttoptr"));
    assert!(ir.contains("ret i32"));
    assert!(ir.contains("store i1 true") || ir.contains("store i1 -2"));
}

#[test]
fn emits_local_string_char_and_byte_char_literals() {
    let root = temp_dir("emits_local_string_char_and_byte_char_literals");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var text = "A\0";
    var bytes = b"A\0";
    var nul_bytes = b"A\0";
    var ch = 'A';
    var byte = b'A';
    _ = bytes;
    _ = nul_bytes;
    text.*[0] as u32 as i32 + ch as u32 as i32 + byte as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("[2 x i32]"));
    assert!(ir.contains("[2 x i8] c\"A\\00\"") || ir.contains("[2 x i8] [i8 65, i8 0]"));
    assert!(ir.contains("store i32 65"));
    assert!(ir.contains("store i8 65"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn size_levels_emit_repeated_byte_static_initializers_as_strings() {
    let root = temp_dir("size_levels_emit_repeated_byte_static_initializers_as_strings");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
let bytes: [4]u8 = b"aaaa".*;

fn main() i32 {
    bytes[0] as i32
}
"#,
    )
    .expect("write test source");

    for level in [
        nia_driver::NiaOptimizationLevel::Os,
        nia_driver::NiaOptimizationLevel::Oz,
    ] {
        let checked =
            nia_driver::check_program_with_options(main.to_string_lossy().into_owned(), level);
        assert!(
            checked.diagnostics.is_empty(),
            "{level:?}: {:?}",
            checked.diagnostics
        );
        let output = emit_llvm_ir_with_options(
            &checked.backend_lowering.program,
            LlvmCodegenOptions {
                optimization: checked.optimization,
            },
        );
        assert!(
            output.diagnostics.is_empty(),
            "{level:?}: {:?}",
            output.diagnostics
        );
        let ir = &output.modules[0].ir;

        assert!(
            ir.contains("@nia__m0__d0__bytes = constant [4 x i8] c\"aaaa\""),
            "{level:?}\n{ir}"
        );
    }
}

#[test]
fn emits_inferred_global_string_family_arrays() {
    let root = temp_dir("emits_inferred_global_string_family_arrays");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
let bytes = b"nia";
let text = "ok";

fn main() i32 {
    bytes.*[0] as i32
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;

    assert!(ir.contains("@nia__m0__d0__bytes = constant ptr"), "{ir}");
    assert!(ir.contains("@nia__m0__d1__text = constant ptr"), "{ir}");
    assert!(ir.contains("@.nia.static.array"), "{ir}");
}

#[test]
fn emits_adjacent_string_literal_concatenation() {
    let root = temp_dir("emits_adjacent_string_literal_concatenation");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn printf(fmt: & u8, ...);

let fmt: [104]u8 = (
    b""
    b"  #  Type      Offset             VirtAddr           FileSiz"
    b""
    b"            MemSiz"
    b""
    b"             Flags Align\n\0"
).*;

fn main() i32 {
    var text = "中" "" "a" "" "b" "" "c" "";
    var bytes = b"" b"n" b"" b"i" b"" b"a" b"" b"\0";
    let inline_fmt = (
        b""
        b"  #  Type      Offset             VirtAddr           FileSiz"
        b""
        b"            MemSiz"
        b""
        b"             Flags Align\n\0"
    );
    _ = text;
    _ = bytes;
    printf(&(inline_fmt.*[0]));
    printf(&fmt[0]);
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
    assert!(ir.contains("[4 x i32]"), "{ir}");
    assert!(
        ir.contains("[4 x i8] c\"nia\\00\"")
            || ir.contains("[4 x i8] [i8 110, i8 105, i8 97, i8 0]"),
        "{ir}"
    );
    assert!(ir.contains("MemSiz"));
    assert!(ir.contains("Flags Align\\0A\\00"));
    assert!(ir.contains("@printf"));
}
