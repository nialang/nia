// SPDX-License-Identifier: GPL-3.0-or-later
use super::common::*;

#[test]
fn inline_closure_pointers_preserve_entries_and_source_effects() {
    let _resources =
        nia_test_support::acquire_test_resources(nia_test_support::TestWorkload::Build);
    let root = temp_dir("inline_closure_pointers");
    let source = root.join("main.nia");
    write(
        &root.join("helper.nia"),
        r#"
pub fun marker() i32 { 10 }
pub fun apply[T](callback: &fun(T) T, value: T) T { callback(value) }
pub fun make() &fun(i32) i32 { \value -> marker() + value }
pub fun makeGeneric[T]() &fun(T) T { \value -> value }
pub type Word = i32;
pub fun makeAlias() &fun(Word) Word { \value: Word -> value + 1 }
"#,
    );
    write(
        &source,
        r#"
module helper;
using root::helper;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let implicit: &fun(i32) i32 = \value -> helper::marker() + value;
    let explicit: &fun(i32) i32 = &\value -> helper::marker() + value;
    let mut effects = 0;
    let block: &fun(i32) i32 = {
        effects += 1;
        \value: i32 -> value + 3
    };
    let explicitBlock: &fun(i32) i32 = &{
        effects += 2;
        \value: i32 -> value + 4
    };
    let stored = \value: i32 -> value + 5;
    let storedPointer: &fun(i32) i32 = &stored;
    if implicit(1) != 11 or explicit(2) != 12 or block(3) != 6
        or explicitBlock(4) != 8 or effects != 3 or storedPointer(5) != 10
        or helper::make()(6) != 16
        or helper::makeGeneric[i32]()(9) != 9
        or helper::makeGeneric[i64]()(10i64) != 10i64
        or helper::makeAlias()(11) != 12
        or helper::apply[i32](\value -> value + 6, 7) != 13
        or helper::apply[i32](&\value -> value + 7, 8) != 15
    {
        return process::ExitCode(10)!;
    }
    !()
}
"#,
    );
    let driver = test_driver();
    let artifact = driver
        .link_executable(crate::LinkExecutableRequest::new(
            crate::CheckRequest::new(source.to_string_lossy().into_owned())
                .with_runtime(test_freestanding_runtime()),
            root.join("runner.exe"),
        ))
        .result
        .expect("inline closure pointers must link");
    let status = std::process::Command::new(&artifact.path)
        .status()
        .expect("run closure pointers");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn imported_aliases_are_transparent_in_synthesized_trait_arguments() {
    let root = temp_dir("imported_alias_trait_arguments");
    write(
        &root.join("helper.nia"),
        r#"
pub type Word = i32;
pub type Words = [Word; 2];
pub fun number() Word { 7 }
pub fun words() Words { [7, 9] }
"#,
    );
    write(
        &root.join("main.nia"),
        r#"
module helper;
using root::helper;
fun same[T](lhs: T, rhs: T) bool where T: Eq[T] { lhs == rhs }
fun main() i32 {
    let number = helper::number();
    let words = helper::words();
    if same(number, number) and words[0] == number { 0 } else { 1 }
}
"#,
    );
    let program = codegen_program(root.join("main.nia").to_string_lossy().into_owned());
    assert!(program.diagnostics.is_empty(), "{:#?}", program.diagnostics);
}
