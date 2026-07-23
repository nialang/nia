// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn emits_struct_operator_overload_calls() {
    let root = temp_dir("emits_struct_operator_overload_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Number {
    value: i32,
}

extend Number : Add[Number] {
    type Output = Number;

    fn add(self, rhs: Number) Number {
        { value: self.value + rhs.value }
    }
}

fn main() i32 {
    let mut one: Number = { value: 1 };
    let mut two: Number = { value: 2 };
    let mut three = one + two;
    let mut seven = three.add({ value: 4 });
    let mut nine = [Number]::add(seven, { value: 2 });
    nine.value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '@', 0, "add");
    assert!(ir.contains("call"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_generic_struct_operator_overload_calls() {
    let root = temp_dir("emits_generic_struct_operator_overload_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Box[T] {
    value: T,
}

extend[T] Box[T] : Add[Box[T]] {
    type Output = Box[T];

    fn add(self, rhs: Box[T]) Box[T] {
        rhs
    }
}

fn combine[T](lhs: Box[T], rhs: Box[T]) Box[T] where Box[T]: Add[Box[T], Output = Box[T]] {
    lhs + rhs
}

fn main() i32 {
    let mut one: Box[i32] = { value: 1 };
    let mut two: Box[i32] = { value: 2 };
    let mut three = combine[i32](one, two);
    three.value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '@', 0, "add__inst__t_i32");
    assert!(ir.contains("call"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}

#[test]
fn emits_struct_comparison_operator_overload_calls() {
    let root = temp_dir("emits_struct_comparison_operator_overload_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Number {
    value: i32,
}

extend Number : Eq[Number] {
    fn eq(& self, other: & Number) bool {
        self.value == other.value
    }

    fn ne(& self, other: & Number) bool {
        self.value != other.value
    }
}

extend Number : Ord[Number] {
    fn lt(& self, other: & Number) bool {
        self.value < other.value
    }

    fn le(& self, other: & Number) bool {
        self.value <= other.value
    }

    fn gt(& self, other: & Number) bool {
        self.value > other.value
    }

    fn ge(& self, other: & Number) bool {
        self.value >= other.value
    }
}

fn main() bool {
    let mut one: Number = { value: 1 };
    let mut two: Number = { value: 2 };
    one != two and one < two
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '@', 0, "ne");
    assert_contains_mangled_symbol(ir, '@', 0, "lt");
    assert!(ir.contains("call i1 @"), "{ir}");
    assert!(ir.contains("ret i1"), "{ir}");
}

#[test]
fn emits_struct_unary_operator_overload_calls() {
    let root = temp_dir("emits_struct_unary_operator_overload_calls");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
struct Number {
    value: i32,
}

extend Number : Neg {
    type Output = Number;

    fn neg(self) Number {
        { value: -self.value }
    }
}

extend Number : BitNot {
    type Output = Number;

    fn bit_not(self) Number {
        { value: ~self.value }
    }
}

fn main() i32 {
    let mut one: Number = { value: 1 };
    let mut neg = -one;
    let mut bits = ~neg;
    bits.value
}
"#,
    )
    .expect("write test source");

    let codegen = codegen_program(main.to_string_lossy().into_owned());
    assert!(codegen.diagnostics.is_empty(), "{:?}", codegen.diagnostics);

    let output = emit_llvm_ir(&codegen.backend_lowering, &codegen.type_store);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let ir = &output.modules[0].ir;
    assert_contains_mangled_symbol(ir, '@', 0, "neg");
    assert_contains_mangled_symbol(ir, '@', 0, "bit_not");
    assert!(ir.contains("call void @"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}
