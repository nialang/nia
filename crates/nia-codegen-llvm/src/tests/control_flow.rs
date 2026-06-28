// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

const COUNTER_ITERATOR: &str = r#"
struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = i32;

    fn next(&mut self) ?i32 {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?value
        }
    }
}
"#;

#[test]
fn emits_short_circuit_logical_operators() {
    let root = temp_dir("emits_short_circuit_logical_operators");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main(a: bool, b: bool) i32 {
    let mut x = a and b;
    let mut y = a or b;
    if x or y { 1 } else { 0 }
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
        format!(
            "{COUNTER_ITERATOR}\n{}",
            r#"
fn main() i32 {
    let mut sum = 0;
    let mut iter = Counter { current: 0, end: 10 };
    for i in iter {
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
        ),
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("fir.bb"));
    assert!(ir.contains("br i1"));
    assert!(ir.contains("br label"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_for_over_range_value() {
    let root = temp_dir("emits_for_over_range_value");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std;

fn main() i32 {
    let mut sum = 0;
    for i in std::range(1usize..4usize) {
        sum += i as i32;
    }
    sum
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("br i1"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_for_over_explicit_iterator_value() {
    let root = temp_dir("emits_for_over_explicit_iterator_value");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        format!(
            "{COUNTER_ITERATOR}\n{}",
            r#"
fn main() i32 {
    let mut iter = Counter { current: 1, end: 4 };
    let mut sum = 0;
    for i in iter {
        sum += i;
    }
    sum
}
"#,
        ),
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("br i1"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_for_over_error_union_iterator_item() {
    let root = temp_dir("emits_for_over_error_union_iterator_item");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Error: i32 {
    Bad = 1,
    _
}

struct Counter {
    current: i32,
    end: i32,
}

extend Counter : Iterator {
    type Item = Error!i32;

    fn next(&mut self) ?(Error!i32) {
        if self.current >= self.end {
            null
        } else {
            let value = self.current;
            self.current += 1;
            ?(!value)
        }
    }
}

fn main() i32 {
    let mut iter = Counter { current: 1, end: 4 };
    let mut sum = 0;
    for result in iter {
        let value = if let !item = result {
            item
        } else error! {
            return 100;
        };
        sum += value;
    }
    sum
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("br i1"));
    assert!(ir.contains("ret i32"));
}

#[test]
fn emits_for_over_cross_module_composite_iterator_items() {
    let root = temp_dir("emits_for_over_cross_module_composite_iterator_items");
    let main = root.join("main.nia");
    let iter = root.join("iter.nia");
    std::fs::write(
        &iter,
        r#"
pub enum Error: i32 {
    Bad = 1,
    _
}

pub struct Payload {
    value: i32,
}

pub struct Box[T] {
    value: T,
}

pub struct StructIter {
    index: i32,
}

extend StructIter : Iterator {
    type Item = Payload;

    pub fn next(&mut self) ?Payload {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ?Payload { value: 2 }
        }
    }
}

pub struct ArrayIter {
    index: i32,
}

extend ArrayIter : Iterator {
    type Item = [2]i32;

    pub fn next(&mut self) ?[2]i32 {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ?[3, 4]
        }
    }
}

pub struct OptionalIter {
    index: i32,
}

extend OptionalIter : Iterator {
    type Item = ?Payload;

    pub fn next(&mut self) ??Payload {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ??Payload { value: 5 }
        }
    }
}

pub struct ErrorIter {
    index: i32,
}

extend ErrorIter : Iterator {
    type Item = Error!Payload;

    pub fn next(&mut self) ?(Error!Payload) {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ?(!Payload { value: 6 })
        }
    }
}

pub struct OptionalErrorIter {
    index: i32,
}

extend OptionalErrorIter : Iterator {
    type Item = ?(Error!Payload);

    pub fn next(&mut self) ??(Error!Payload) {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ??(!Payload { value: 7 })
        }
    }
}

pub struct ErrorOptionalIter {
    index: i32,
}

extend ErrorOptionalIter : Iterator {
    type Item = Error!?Payload;

    pub fn next(&mut self) ?(Error!?Payload) {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ?(!?Payload { value: 8 })
        }
    }
}

pub struct SliceIter {
    index: i32,
    data: [2]i32,
}

extend SliceIter : Iterator {
    type Item = &[i32];

    pub fn next(&mut self) ?&[i32] {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ?&self.data[..]
        }
    }
}

pub struct GenericIter {
    index: i32,
}

extend GenericIter : Iterator {
    type Item = Box[Payload];

    pub fn next(&mut self) ?Box[Payload] {
        if self.index >= 1 {
            null
        } else {
            self.index += 1;
            ?Box[Payload] { value: Payload { value: 11 } }
        }
    }
}
"#,
    )
    .expect("write iter source");
    std::fs::write(
        &main,
        r#"
module iter;
using entry::iter;

fn main() i32 {
    let mut total = 0;

    let mut struct_iter = iter::StructIter { index: 0 };
    for item in struct_iter {
        total += item.value;
    }

    let mut array_iter = iter::ArrayIter { index: 0 };
    for item in array_iter {
        total += item[0] + item[1];
    }

    let mut optional_iter = iter::OptionalIter { index: 0 };
    for item in optional_iter {
        if let ?value = item {
            total += value.value;
        } else null {
            total += 100;
        }
    }

    let mut error_iter = iter::ErrorIter { index: 0 };
    for item in error_iter {
        if let !value = item {
            total += value.value;
        } else error! {
            total += 1000;
        }
    }

    let mut optional_error_iter = iter::OptionalErrorIter { index: 0 };
    for item in optional_error_iter {
        if let ?result = item {
            if let !value = result {
                total += value.value;
            } else error! {
                total += 1000;
            }
        } else null {
            total += 100;
        }
    }

    let mut error_optional_iter = iter::ErrorOptionalIter { index: 0 };
    for item in error_optional_iter {
        if let !maybe = item {
            if let ?value = maybe {
                total += value.value;
            } else null {
                total += 100;
            }
        } else error! {
            total += 1000;
        }
    }

    let mut slice_iter = iter::SliceIter { index: 0, data: [9, 10] };
    for item in slice_iter {
        total += item[0] + item[1];
    }

    let mut generic_iter = iter::GenericIter { index: 0 };
    for item in generic_iter {
        total += item.value.value;
    }

    total
}
"#,
    )
    .expect("write main source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = output
        .modules
        .iter()
        .map(|module| module.ir.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(ir.contains("extractvalue"));
    assert!(ir.contains("br i1"));
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
extern fn fopen(path: & u8, mode: & u8) &void;

fn inspect(path: & u8) i32 {
    let mode = b"rb\0";
    let mut file = fopen(path, &(mode.*[0]));

    if file as usize == 0 {
        return 1;
    }

    defer {
        _ = fclose(file);
    };

    0
}

fn run(argv: & & u8) i32 {
    inspect(argv[0])
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let cleanup = mangled_symbol(ir, '@', 0, "cleanup");
    assert_substrings_in_order(ir, &[&format!("call void {cleanup}()"), "ret i32 0"]);
}

#[test]
fn emits_deferred_return_as_ordinary_delayed_control_flow() {
    let root = temp_dir("emits_deferred_return_as_ordinary_delayed_control_flow");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
fn main() i32 {
    defer {
        return 7;
    };
    return 1;
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("defer.entry"), "{ir}");
    assert!(ir.contains("ret i32 7"), "{ir}");
}

#[test]
fn emits_deferred_break_and_continue_to_outer_loop_targets() {
    let root = temp_dir("emits_deferred_break_and_continue_to_outer_loop_targets");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
extern fn log(x: i32);

fn break_from_defer() i32 {
    loop {
        defer {
            log(1);
            break;
        };
        continue;
    }
    10
}

fn continue_from_defer() i32 {
    let mut i = 0;
    loop {
        i += 1;
        defer {
            log(2);
            continue;
        };
        break;
    }
    i
}

fn main() i32 {
    break_from_defer() + continue_from_defer()
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("call void @log(i32 1)"), "{ir}");
    assert!(ir.contains("call void @log(i32 2)"), "{ir}");
    assert!(ir.contains("defer.entry"), "{ir}");
    assert!(ir.contains("br label"), "{ir}");
}

#[test]
fn emits_deferred_try_propagation() {
    let root = temp_dir("emits_deferred_try_propagation");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
enum Error: i32 {
    Bad = 1,
    _
}

fn cleanup(fail: bool) Error!void {
    if fail {
        Error::Bad!
    } else {
        !{}
    }
}

fn main(fail: bool) Error!i32 {
    defer cleanup(fail).?;
    !1
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert!(ir.contains("defer.try.failure"), "{ir}");
    assert!(ir.contains("defer.try.failed"), "{ir}");
    assert!(
        ir.contains("store { i8, { i32 } } %try.return.value, ptr %0"),
        "{ir}"
    );
    assert!(ir.contains("ret void"), "{ir}");
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);
    let module = &codegen.backend_lowering.program.modules[0];
    assert!(
        module
            .function_instances
            .iter()
            .any(|instance| instance.name == "id"),
        "{:?}",
        module.function_instances
    );

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    let id = mangled_symbol(ir, '@', 0, "id__inst__i32");
    assert!(ir.contains(&format!("define i32 {id}")), "{ir}");
    assert!(ir.contains(&format!("call i32 {id}(i32 7)")), "{ir}");
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
    let mut i = 0;
    loop {
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering.program);
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
