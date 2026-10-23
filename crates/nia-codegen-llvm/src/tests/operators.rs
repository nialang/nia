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

    fun add(self, rhs: Number) Number {
        Self { value: self.value + rhs.value }
    }
}

fun main() i32 {
    let mut one = Number { value: 1 };
    let mut two = Number { value: 2 };
    let mut three = one + two;
    let mut seven = three.add(Number { value: 4 });
    let mut nine = [Number]::add(seven, Number { value: 2 });
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
    assert_contains_mangled_symbol(ir, '@', "add");
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

    fun add(self, rhs: Box[T]) Box[T] {
        rhs
    }
}

fun combine[T](lhs: Box[T], rhs: Box[T]) Box[T] where Box[T]: Add[Box[T], Output = Box[T]] {
    lhs + rhs
}

fun main() i32 {
    let mut one = Box[i32] { value: 1 };
    let mut two = Box[i32] { value: 2 };
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
    assert_contains_mangled_symbol(ir, '@', "add__inst__t_i32");
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
    fun eq(& self, other: & Number) bool {
        self.value == other.value
    }

    fun ne(& self, other: & Number) bool {
        self.value != other.value
    }
}

extend Number : Ord[Number] {
    fun lt(& self, other: & Number) bool {
        self.value < other.value
    }

    fun le(& self, other: & Number) bool {
        self.value <= other.value
    }

    fun gt(& self, other: & Number) bool {
        self.value > other.value
    }

    fun ge(& self, other: & Number) bool {
        self.value >= other.value
    }
}

fun main() bool {
    let mut one = Number { value: 1 };
    let mut two = Number { value: 2 };
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
    assert_contains_mangled_symbol(ir, '@', "ne");
    assert_contains_mangled_symbol(ir, '@', "lt");
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

    fun neg(self) Number {
        Self { value: -self.value }
    }
}

extend Number : BitNot {
    type Output = Number;

    fun bitNot(self) Number {
        Self { value: ~self.value }
    }
}

fun main() i32 {
    let mut one = Number { value: 1 };
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
    assert_contains_mangled_symbol(ir, '@', "neg");
    assert_contains_mangled_symbol(ir, '@', "bitNot");
    assert!(ir.contains("call void @"), "{ir}");
    assert!(ir.contains("ret i32"), "{ir}");
}
