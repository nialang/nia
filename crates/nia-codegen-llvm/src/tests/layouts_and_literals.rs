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

static mixed: Mixed = { a: 1, b: 2, c: 3 };

fn main() i32 {
    let mut local: Mixed = { a: 4, b: 5, c: 6 };
    mixed.a as i32 + mixed.c as i32 + local.a as i32 + local.c as i32 + local.b as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let mixed_ty = mangled_symbol(ir, '%', 0, "Mixed");
    let mixed_global = mangled_symbol(ir, '@', 0, "mixed");
    assert!(
        ir.contains(&format!("{mixed_ty} = type {{ i64, i8, i8 }}")),
        "{ir}"
    );
    assert!(
        ir.contains(&format!(
            "{mixed_global} = constant {mixed_ty} {{ i64 2, i8 1, i8 3 }}"
        )),
        "{ir}"
    );
    assert!(
        ir.contains(&format!("ptr {mixed_global}, i32 0, i32 1")),
        "{ir}"
    );
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
    let mut pair: Pair = { a: 10, b: 20 };
    let mut copied = id(pair);
    sum(copied)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let id = mangled_symbol(ir, '@', 0, "id");
    let sum = mangled_symbol(ir, '@', 0, "sum");
    assert!(
        ir.contains(&format!("define void {id}(ptr %0, ptr %1)")),
        "{ir}"
    );
    assert!(ir.contains(&format!("define i32 {sum}(ptr %0)")), "{ir}");
    assert!(
        ir.contains(&format!("call void {id}(ptr %copied, ptr %arg.copy")),
        "{ir}"
    );
    assert!(
        ir.contains(&format!("call i32 {sum}(ptr %arg.copy")),
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let sum_pair = mangled_symbol(ir, '@', 0, "sum_pair");
    let sum_array = mangled_symbol(ir, '@', 0, "sum_array");
    assert!(
        ir.contains(&format!("call i32 {sum_pair}(ptr %arg.copy")),
        "{ir}"
    );
    assert!(
        ir.contains(&format!("call i32 {sum_array}(ptr %arg.copy")),
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
    let mut pair: Pair = { a: 10, b: 20 };
    pair = { a: 30, b: 40 };
    let mut values: [2]i32 = [50, 60];
    values = [70, 80];
    sum_pair(pair) + sum_array(values)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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
    let mut pair: Pair = make_pair(10, 20);
    pair = make_pair(30, 40);
    let mut values: [2]i32 = make_array(50, 60);
    values = make_array(70, 80);
    sum_pair(pair) + sum_array(values)
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(!ir.contains("call.out"), "{ir}");
    let make_pair = mangled_symbol(ir, '@', 0, "make_pair");
    let make_array = mangled_symbol(ir, '@', 0, "make_array");
    assert_substrings_in_order(ir, &["%pair = alloca", &format!("{make_pair}(ptr %pair")]);
    assert_substrings_in_order(
        ir,
        &["%values = alloca", &format!("{make_array}(ptr %values")],
    );
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("return.copy"), "{ir}");
    assert!(!ir.contains("call.out"), "{ir}");
    let make_pair = mangled_symbol(ir, '@', 0, "make_pair");
    let make_array = mangled_symbol(ir, '@', 0, "make_array");
    assert_substrings_in_order(
        ir,
        &["%arg.copy = alloca", &format!("{make_pair}(ptr %arg.copy)")],
    );
    assert_substrings_in_order(
        ir,
        &[
            "%arg.copy1 = alloca",
            &format!("{make_array}(ptr %arg.copy1)"),
        ],
    );
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let pair = mangled_symbol(ir, '%', 0, "Pair");
    let cleanup = mangled_symbol(ir, '@', 0, "cleanup");
    assert_substrings_in_order(
        ir,
        &[
            "call i32 @log(i32 1)",
            &format!("{cleanup}(i32 2)"),
            &format!("store {pair} %return.value"),
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(!ir.contains("call.out"), "{ir}");
    let forward_pair = mangled_symbol(ir, '@', 0, "forward_pair");
    let make_pair = mangled_symbol(ir, '@', 0, "make_pair");
    assert_substrings_in_order(
        ir,
        &[
            &format!("define void {forward_pair}(ptr %0)"),
            &format!("call void {make_pair}(ptr %0, i32 10, i32 20)"),
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call.out"), "{ir}");
    let forward_pair = mangled_symbol(ir, '@', 0, "forward_pair");
    let make_pair = mangled_symbol(ir, '@', 0, "make_pair");
    let pair = mangled_symbol(ir, '%', 0, "Pair");
    let cleanup = mangled_symbol(ir, '@', 0, "cleanup");
    assert_substrings_in_order(
        ir,
        &[
            &format!("define void {forward_pair}(ptr %0)"),
            &format!("call void {make_pair}(ptr %call.out, i32 10, i32 20)"),
            &format!("{cleanup}(i32 1)"),
            &format!("store {pair} %call.result, ptr %0"),
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
    let mut x = -1;
    let mut y = not false;
    let mut n = 1;
    let mut z: f64 = n as f64;
    let mut w: i32 = z as i32;
    let mut addr = ptr as usize;
    let mut again = addr as &i32;
    x + w + again.* + Color::Blue as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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
    let mut text = "A\0";
    let mut bytes = b"A\0";
    let mut nul_bytes = b"A\0";
    let mut ch = 'A';
    let mut byte = b'A';
    _ = bytes;
    _ = nul_bytes;
    text[0] as u32 as i32 + ch as u32 as i32 + byte as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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
static bytes: [4]u8 = b"aaaa";

fn main() i32 {
    bytes[0] as i32
}
"#,
    )
    .expect("write test source");

    for level in [NiaOptimizationLevel::Os, NiaOptimizationLevel::Oz] {
        let codegen = codegen_program_with_options(main.to_string_lossy().into_owned(), level);
        assert!(
            codegen.diagnostics.is_empty(),
            "{level:?}: {:?}",
            codegen.diagnostics
        );
        let output = emit_llvm_ir_with_options(
            &codegen.backend_lowering,
            &codegen.type_store,
            LlvmCodegenOptions {
                optimization: codegen.optimization,
                ..LlvmCodegenOptions::default()
            },
        );
        assert!(
            output.diagnostics.is_empty(),
            "{level:?}: {:?}",
            output.diagnostics
        );
        let ir = &output.modules[0].ir;

        let bytes = mangled_symbol(ir, '@', 0, "bytes");
        assert!(
            ir.contains(&format!("{bytes} = constant [4 x i8] c\"aaaa\"")),
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
static bytes = b"nia";
static text = "ok";

fn main() i32 {
    bytes[0] as i32
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;

    let bytes = mangled_symbol(ir, '@', 0, "bytes");
    let text = mangled_symbol(ir, '@', 0, "text");
    assert!(ir.contains(&format!("{bytes} = constant [3 x i8]")), "{ir}");
    assert!(ir.contains(&format!("{text} = constant [2 x i32]")), "{ir}");
}

#[test]
fn emits_adjacent_string_literal_concatenation() {
    let root = temp_dir("emits_adjacent_string_literal_concatenation");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn printf(fmt: & u8, ...);

static fmt: [104]u8 = (
    b""
    b"  #  Type      Offset             VirtAddr           FileSiz"
    b""
    b"            MemSiz"
    b""
    b"             Flags Align\n\0"
    );

fn main() i32 {
    let mut text = "中" "" "a" "" "b" "" "c" "";
    let mut bytes = b"" b"n" b"" b"i" b"" b"a" b"" b"\0";
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
    printf(&inline_fmt[0]);
    printf(&fmt[0]);
    0
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
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
