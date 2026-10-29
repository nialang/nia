// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn generic_methods_infer_and_execute_capturing_callable_arguments() {
    let root = temp_dir("generic_methods_infer_and_execute_capturing_callable_arguments");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct Source { value: i32 }
struct Target { value: i32 }
struct Runner {}
struct Number { value: i32 }

trait Describe {
    fun value(&self) i32;
}

trait Bump {
    fun bump(&mut self) i32;
}

trait Elements {
    fun count(&self) i32;
}

extend Number : Describe {
    fun value(&self) i32 { self.value }
}

extend Number : Bump {
    fun bump(&mut self) i32 {
        self.value += 1;
        self.value
    }
}

extend [Number] : Elements {
    fun count(&self) i32 { 1 }
}

fun makeSource() Source { Source { value: 20 } }

fun readGeneric[T](value: &T) T where T: Sized { value.* }
fun replaceGeneric[T](value: &mut T, replacement: T) () where T: Sized {
    value.* = replacement;
}
fun firstGeneric[T](values: &[T]) T { values[0] }
fun firstReferenceGeneric[T](values: &[&T]) T where T: Sized { values[0].*.* }

fun verifyImplicitAdjustments() i32 {
    let mut value = 7;
    replaceGeneric(value, 8);
    readGeneric(value) + firstGeneric([5, 6]) + firstReferenceGeneric([value, value])
}

fun describe(value: &Describe) i32 { value.value() }
fun describeMany(values: &[&Describe]) i32 { values[0].value() }
fun countElements(values: &Elements) i32 { values.count() }

fun verifyTraitObjectAdjustments() i32 {
    let mut number = Number { value: 7 };
    let numbers: [Number; 1] = [number];
    describe(number) + describeMany([number]) + countElements(numbers) + bump(number)
}

fun bump(value: &mut Bump) i32 { value.bump() }

extend Source {
    fun intoTarget(self) Target { Target { value: self.value * 2 } }
}

extend Runner {
    fun apply[Input, Output, Mapper](self, value: Input, callback: Mapper) Output
    where Mapper: Fun(Input) Output
    {
        callback(value)
    }

    fun applyBorrowed[Input, Output](self, value: Input, callback: &Fun(Input) Output) Output {
        callback(value)
    }
}

fun transform[Context](context: Context, offset: i32) i32 {
    let result = Runner {}.apply(makeSource(), \[context, offset] value -> {
        _ = context;
        Target { value: value.intoTarget().value + offset }
    });
    let borrowed = Runner {}.applyBorrowed(makeSource(), &\[context, offset] value -> {
        _ = context;
        Target { value: value.intoTarget().value + offset }
    });
    let borrowedImplicit = Runner {}.applyBorrowed(makeSource(), \[context, offset] value -> {
        _ = context;
        Target { value: value.intoTarget().value + offset }
    });
    if borrowedImplicit.value != borrowed.value { return -1; }
    result.value + borrowed.value
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    if verifyImplicitAdjustments() != 21 { return process::ExitCode(3)!; }
    if verifyTraitObjectAdjustments() != 23 { return process::ExitCode(4)!; }
    if transform(1i32, 2) != 84 { return process::ExitCode(1)!; }
    if transform(true, 3) != 86 { return process::ExitCode(2)!; }
    !()
}
"#,
    )
    .expect("write callable inference source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit callable inference executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run callable inference executable");
    assert_eq!(run.code(), Some(0));
}
