// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_exe_auto_addresses_function_items_for_thin_pointer_contexts() {
    let root = temp_dir("auto_addresses_function_items_for_thin_pointer_contexts");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

fun addsNumbers(left: i32, right: i32) i32 { left + right }
fun invoke(callback: &fun(i32, i32) i32) i32 { callback(2, 3) }
fun invokeView(callback: &Fun(i32, i32) i32) i32 { callback(2, 3) }

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let pointer: &fun(i32, i32) i32 = addsNumbers;
    let view: &Fun(i32, i32) i32 = addsNumbers;
    let callback = \left: i32, right: i32 -> left + right;
    let closure_pointer: &fun(i32, i32) i32 = callback;
    if pointer(4, 5) != 9
        or invoke(addsNumbers) != 5
        or view(6, 7) != 13
        or invokeView(addsNumbers) != 5
        or closure_pointer(8, 9) != 17
    {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    )
    .expect("write function-pointer source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit function-item pointer conversion executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run function-item pointer conversion executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_readonly_methods_accept_writable_call_receivers() {
    let root = temp_dir("readonly_methods_accept_writable_call_receivers");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct Holder {
    items: [u32; 4],
    count: u32,
}

extend Holder {
    fun asMutSlice(&mut self) &mut [u32] { &mut self.items[..] }
    fun me(&mut self) &mut Holder { self }
    fun total(&self) u32 { self.count + self.items[0] }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut holder = Holder { items: [1, 2, 3, 4], count: 10 };
    // `len(&self)` and `total(&self)` take the readonly view of a `&mut`
    // that a call produces.
    if holder.asMutSlice().len() != 4 or holder.me().total() != 11 {
        return process::ExitCode(1)!;
    }
    holder.asMutSlice()[0] = 5;
    if (&mut holder).me().me().total() != 15 {
        return process::ExitCode(2)!;
    }
    !()
}
"#,
    )
    .expect("write writable call receiver source");
    for optimization in ["-O0", "-O2"] {
        let output = support::nia_command()
            .arg("emit")
            .arg("--exe")
            .arg(optimization)
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output_timeout_for_build("emit writable call receiver executable");
        assert!(
            output.status.success(),
            "{optimization} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let run = Command::new(&exe).status_timeout("run writable call receiver executable");
        assert_eq!(run.code(), Some(0), "{optimization}");
    }
}

#[test]
fn emit_exe_operator_results_receive_implicit_addresses() {
    let root = temp_dir("operator_results_receive_implicit_addresses");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fmt;
using std::process;

fun unsigned(value: &usize) usize { value.* }
fun signed(value: &isize) isize { value.* }
fun flag(value: &bool) bool { value.* }
fun formatted(value: &fmt::Format) bool { _ = value; true }

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let x: usize = 3;
    // Operator results are temporaries and take a read-only address where
    // a pointer or trait object is expected.
    let sum: &usize = x + 1;
    if sum.* != 4 or unsigned(x * 7) != 21 or unsigned(x << 1) != 6 {
        return process::ExitCode(1)!;
    }
    if signed(-(x as isize)) != -3 or unsigned(~x) != ~3usize or flag(not (x == 3)) {
        return process::ExitCode(2)!;
    }
    if not formatted(x + 1) or not formatted(-(x as isize)) or not formatted(x == 3) {
        return process::ExitCode(3)!;
    }
    !()
}
"#,
    )
    .expect("write operator implicit address source");
    for optimization in ["-O0", "-O2"] {
        let output = support::nia_command()
            .arg("emit")
            .arg("--exe")
            .arg(optimization)
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output_timeout_for_build("emit operator implicit address executable");
        assert!(
            output.status.success(),
            "{optimization} stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let run = Command::new(&exe).status_timeout("run operator implicit address executable");
        assert_eq!(run.code(), Some(0), "{optimization}");
    }

    // A mismatched operator result is reported by its context, and a
    // temporary never receives a writable address.
    let rejected = root.join("rejected.nia");
    std::fs::write(
        &rejected,
        r#"
fun flag(value: &bool) bool { value.* }

fun main() i32 {
    let x: usize = 3;
    _ = flag(x + 1);
    let writable: &mut usize = x + 1;
    _ = writable;
    0
}
"#,
    )
    .expect("write rejected operator address source");
    let output = support::nia_command()
        .arg("check")
        .arg(&rejected)
        .output_timeout_for_build("check rejected operator addresses");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr:\n{stderr}");
    assert!(
        stderr.contains("type mismatch in call argument 1: expected &bool, got usize"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("type mismatch in binding initializer: expected &mut usize, got usize"),
        "stderr:\n{stderr}"
    );
    assert!(!stderr.contains("internal"), "stderr:\n{stderr}");
}

#[test]
fn emit_exe_qualified_method_items_preserve_identity_and_instantiation() {
    let root = temp_dir("qualified_method_items_preserve_identity_and_instantiation");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        root.join("values.nia"),
        r#"
pub struct Box[T, N: usize] { value: T }
extend[T, N: usize] Box[T, N] {
    pub fun make(value: T) Self { Self { value } }
    pub fun get(&self) T { self.value }
    pub fun set(&mut self, value: T) () { self.value = value; }
    pub fun replace[U](&self, value: U) U { value }
    pub fun count(&self) usize { N }
    pub fun methodCount[M: usize](&self) usize { M }
}
pub fun identity[T](value: T) T { value }
pub fun increment(value: i32) i32 { value + 1 }
"#,
    )
    .expect("write imported method items");
    std::fs::write(
        &main,
        r#"
using std::process;
module values;
type IntBox = values::Box[i32, 3];
extend[T] &T { fun isNull(self) bool { self as usize == 0 } }
fun apply[F](callback: F, value: &IntBox) i32 where F: Fun(&IntBox) i32 {
    callback(value)
}
fun invoke(callback: &fun(&IntBox) i32, value: &IntBox) i32 { callback(value) }
fun makeGetter() &fun(&IntBox) i32 { IntBox::get }
fun generic[T](value: T) T {
    let make = values::Box[T, 5]::make;
    let get = values::Box[T, 5]::get;
    get(make(value))
}
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let make = IntBox::make;
    let get = values::Box[i32, 3]::get;
    let pointer: &fun(&IntBox) i32 = IntBox::get;
    let explicit = &IntBox::get;
    let view: &Fun(&IntBox) i32 = IntBox::get;
    let set: &fun(&mut IntBox, i32) () = IntBox::set;
    let replace = IntBox::replace[bool];
    let replaceView: &Fun(&IntBox, bool) bool = IntBox::replace[bool];
    let count = IntBox::count;
    let methodCount = IntBox::methodCount[7];
    let methodCountPointer: &fun(&IntBox) usize = IntBox::methodCount[11];
    let methodCountExplicit = &IntBox::methodCount[13];
    let callbacks: [&fun(&IntBox) i32; 2] = [IntBox::get, IntBox::get];
    let increment = values::increment;
    let identity = values::identity[i32];
    let identityPointer: &fun(i32) i32 = values::identity[i32];
    let isNull = [&u8]::isNull;
    let mut value = make(7);
    set(value, 9);
    if get(value) != 9 or pointer(value) != 9 or explicit(value) != 9
        or view(value) != 9 or apply(IntBox::get, value) != 9
        or invoke(IntBox::get, value) != 9 or makeGetter()(value) != 9
        or callbacks[0](value) != 9 or callbacks[1](value) != 9
        or not replace(value, true) or not replaceView(value, true)
        or count(value) != 3 or increment(2) != 3 or identity(4) != 4
        or methodCount(value) != 7 or methodCountPointer(value) != 11
        or methodCountExplicit(value) != 13
        or identityPointer(5) != 5 or not isNull(0usize as &u8)
        or generic(11i32) != 11 or not generic(true)
    { return process::ExitCode(1)!; }
    !()
}
"#,
    )
    .expect("write qualified method-item source");
    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit qualified method-item executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(&exe).status_timeout("run qualified method-item executable");
    assert_eq!(run.code(), Some(0));
}

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
