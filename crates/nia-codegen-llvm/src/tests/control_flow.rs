// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_short_circuit_logical_operators() {
    let root = temp_dir("emits_short_circuit_logical_operators");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main(a: bool, b: bool) i32 {
    var x = a and b;
    var y = a or b;
    if x or y { 1 } else { 0 }
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("logic.rhs"));
    assert!(ir.contains("logic.end"));
    assert!(ir.contains("phi i1"));
    assert!(ir.contains("br i1"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_for_break_and_continue() {
    let root = temp_dir("emits_for_break_and_continue");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    var sum = 0;
    for var i = 0; i < 10; i += 1 {
        if i == 3 {
            continue;
        }
        if i == 8 {
            break;
        }
        sum += i;
    }
    sum
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("fir.bb"));
    assert!(ir.contains("br i1"));
    assert!(ir.contains("br label"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_defer_before_return_and_block_exit() {
    let root = temp_dir("emits_defer_before_return_and_block_exit");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn inner(x: i32) i32 {
    defer log(10);
    if x == 0 {
        defer log(11);
        return 1;
    }
    {
        defer log(12);
        x + 1
    }
}

fn main() i32 {
    inner(0)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_substrings_in_order(ir, &["call void @log(i32 11)", "call void @log(i32 10)"]);
    assert!(ir.contains("call void @log(i32 12)") || ir.contains("call void @log(i32 12,"));
    assert!(ir.contains("ret i32 1"));
}

#[test]
fn emits_defer_registered_after_earlier_return_branch() {
    let root = temp_dir("emits_defer_registered_after_earlier_return_branch");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn fclose(file: &void) i32;
extern fn fopen(path: &const u8, mode: &const u8) &void;

fn inspect(path: &const u8) i32 {
    var file = fopen(path, c"rb");

    if file as usize == 0 {
        return 1;
    }

    defer {
        _ = fclose(file);
    };

    0
}

fn main(argc: i32, argv: &const &const u8) i32 {
    inspect(argv[0])
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call ptr @fopen"));
    assert!(ir.contains("call i32 @fclose"));
    assert!(ir.contains("ret i32 1"));
    assert!(ir.contains("ret i32 0"));
}

#[test]
fn emits_deferred_block_cleanup() {
    let root = temp_dir("emits_deferred_block_cleanup");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn cleanup() void {}

fn main() i32 {
    defer {
        cleanup();
    };
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
    assert_substrings_in_order(ir, &["call void @nia__m0__d0__cleanup()", "ret i32 0"]);
}

#[test]
fn instantiates_generic_calls_from_defer_tail_expr() {
    let root = temp_dir("instantiates_generic_calls_from_defer_tail_expr");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(value: i32);

fn id[T](value: T) T {
    value
}

fn main() i32 {
    defer {
        log(id[i32](7))
    };
    0
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let module = &checked.backend_lowering.program.modules[0];
    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == "id"),
        "{:?}",
        module.function_instances
    );

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(
        ir.contains("define i32 @nia__m0__d1__id__inst__i32"),
        "{ir}"
    );
    assert!(
        ir.contains("call i32 @nia__m0__d1__id__inst__i32(i32 7)"),
        "{ir}"
    );
    assert!(ir.contains("call void @log(i32 %calltmp)"), "{ir}");
    assert!(ir.contains("ret i32 0"), "{ir}");
}

#[test]
fn emits_defer_lifo_for_normal_nested_block_exit() {
    let root = temp_dir("emits_defer_lifo_for_normal_nested_block_exit");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main() i32 {
    defer log(1);
    defer log(2);
    {
        defer log(3);
        defer log(4);
    }
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
    assert_substrings_in_order(
        ir,
        &[
            "call void @log(i32 4)",
            "call void @log(i32 3)",
            "call void @log(i32 2)",
            "call void @log(i32 1)",
        ],
    );
}

#[test]
fn emits_defer_before_loop_break_and_continue() {
    let root = temp_dir("emits_defer_before_loop_break_and_continue");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn main() i32 {
    var i = 0;
    for {
        defer log(20);
        if i == 0 {
            defer log(21);
            i = 1;
            continue;
        }
        defer log(22);
        break;
    }
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
    assert_substrings_in_order(ir, &["call void @log(i32 21)", "call void @log(i32 20)"]);
    assert!(ir.contains("call void @log(i32 22)"));
    assert!(ir.contains("br label"));
}

#[test]
fn emits_switch_statements() {
    let root = temp_dir("emits_switch_statements");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Color: u8 {
    Red,
    Green,
    Blue,
}

fn main(c: Color) i32 {
    switch c {
        Color::Red => return 1,
        Color::Green => {
            return 2;
        }
        _ => return 3,
    }
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
    assert!(ir.contains("switch i8"));
    assert!(ir.contains("fir.bb"));
    assert!(ir.contains("ret i32 1"));
    assert!(ir.contains("ret i32 2"));
    assert!(ir.contains("ret i32 3"));
}

#[test]
fn emits_switch_expressions() {
    let root = temp_dir("emits_switch_expressions");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn pick(x: u32) i32 {
    switch x {
        0 => 10,
        1 => 20,
        _ => 30,
    }
}

fn early(x: u32) i32 {
    switch x {
        0 => return 1,
        _ => 2,
    }
}

fn main() i32 {
    pick(1) + early(2)
}
"#,
    )
    .expect("write test source");

    let checked = nia_driver::check_program(main.to_string_lossy().into_owned());
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);

    let output = emit_llvm_ir(&checked.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("switch i32"));
    assert!(ir.contains("store i32 10"));
    assert!(ir.contains("store i32 20"));
    assert!(ir.contains("store i32 30"));
    assert!(!ir.contains("switchtmp"));
    assert!(!ir.contains("phi i32"));
    assert!(ir.contains("ret i32 1"));
}
