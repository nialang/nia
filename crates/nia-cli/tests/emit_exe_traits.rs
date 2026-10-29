// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_exe_const_generic_alias_extensions_keep_trait_instances_distinct() {
    let root = temp_dir("emit_exe_const_generic_alias_extensions");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        root.join("hashing.nia"),
        r#"
using root::traits::{Hasher, Hash};
pub struct Sip[C: usize, D: usize] { value: u64 }
pub type One = Sip[1, 3];
pub type Two = Sip[2, 4];
extend[C: usize, D: usize] Sip[C, D] {
    pub fun init() Self { .{ value: 0 } }
    pub fun identity(value: Self) Self { value }
    pub fun hash[T](value: T) u64 where T: Hash[Sip[C, D]] {
        let mut hasher: Self = .init();
        value.hash(hasher);
        hasher.finish()
    }
}
extend[C: usize, D: usize] Sip[C, D] : Hasher {
    pub fun write(&mut self, value: u64) () { self.value += value + C as u64; }
    pub fun finish(&self) u64 { self.value + D as u64 }
    pub fun tagged[T, N: usize](&self, value: &T) u64 where T: Sized {
        _ = value;
        self.value + N as u64 + C as u64 + D as u64
    }
}
"#,
    )
    .expect("write hashing module");
    std::fs::write(
        root.join("traits.nia"),
        r#"
pub trait Hasher {
    fun write(&mut self, value: u64) ();
    fun finish(&self) u64;
    fun tagged[U, M: usize](&self, value: &U) u64 where U: Sized;
}
pub trait Hash[H] where H: Hasher { fun hash(&self, hasher: &mut H) (); }
"#,
    )
    .expect("write traits module");
    std::fs::write(
        root.join("impls.nia"),
        r#"
using root::traits::{Hasher, Hash};
extend[H] u64 : Hash[H] where H: Hasher {
    pub fun hash(&self, hasher: &mut H) () { hasher.write(self.*); }
}
pub fun tag[H, T, N: usize](hasher: &H, value: &T) u64 where H: Hasher {
    hasher.tagged[T, N](value)
}
"#,
    )
    .expect("write impls module");
    std::fs::write(
        &main,
        r#"
module hashing;
module facade;
module traits;
module impls;
using root::facade as hashing;
using std::process;
pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut one = hashing::One::identity(hashing::One::init());
    one.write(3);
    let mut two = hashing::Two::init();
    two.write(4);
    if one.finish() != 7 or two.finish() != 10 { return process::ExitCode(2)!; }
    if impls::tag[hashing::One, i32, 5](one, 0) != 13
        or impls::tag[hashing::Two, i32, 5](two, 0) != 17
    {
        return process::ExitCode(3)!;
    }
    if hashing::One::hash(3u64) != 7 or hashing::Two::hash(4u64) != 10 {
        return process::ExitCode(1)!;
    }
    !()
}
"#,
    )
    .expect("write entry");
    std::fs::write(
        root.join("facade.nia"),
        "pub using root::hashing::{One, Two};",
    )
    .expect("write facade");
    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit const generic alias extensions");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .status_timeout("run const generic alias extensions")
            .code(),
        Some(0)
    );
}

#[test]
fn emit_exe_specializes_user_trait_impls_independent_of_declaration_order() {
    let root = temp_dir("emit_exe_specializes_user_trait_impls");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    let general = r#"
extend[T] [T] : Classify
where T: Sized
{
    type Result = i32;
    fun classify(&self) i32 { 11 }
}
"#;
    let specific = r#"
extend [char] : Classify {
    type Result = bool;
    fun classify(&self) bool { true }
}
"#;
    for (first, second) in [(general, specific), (specific, general)] {
        std::fs::write(
            &main,
            format!(
                r#"
using std::process;
trait Classify {{
    type Result;
    fun classify(&self) [Self as Classify]::Result;
}}
{first}
{second}
pub fun main(init: process::Init) process::ExitCode!() {{
    _ = init;
    let text: &[char] = &"nia";
    let numbers: &[i32] = &[1, 2, 3];
    let specialized: bool = text.classify();
    let fallback: i32 = numbers.classify();
    if not specialized or fallback != 11 {{ return process::ExitCode(1)!; }}
    !()
}}
"#
            ),
        )
        .expect("write specialization source");

        let output = support::nia_command()
            .arg("emit")
            .arg("--exe")
            .arg(&main)
            .arg("-o")
            .arg(&exe)
            .output_timeout_for_build("emit specialized trait executable");
        assert!(
            output.status.success(),
            "stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let run = Command::new(&exe).status_timeout("run specialized trait executable");
        assert_eq!(run.code(), Some(0));
    }
}

#[test]
fn emit_exe_retains_cross_module_generic_trait_witnesses() {
    let root = temp_dir("emit_exe_retains_cross_module_generic_trait_witnesses");
    let main = root.join("main.nia");
    let helper = root.join("iter.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &helper,
        r#"
pub trait Step {
    fun next(self) ?Self;
}

pub trait Iterator {
    type Item;

    fun advance(&mut self) ?[Self as Iterator]::Item;
}

pub struct Cursor[T] {
    value: T,
}

extend[T] Cursor[T] {
    pub fun init(value: T) Cursor[T] {
        Self { value }
    }
}

extend[T] Cursor[T] : Iterator
where T: Step + Ord[T]
{
    type Item = T;

    fun advance(&mut self) ?T {
        let value = self.value;
        self.value = match value.next() {
            ?next => next,
            null => return null,
        };
        if value < self.value {
            _ = value;
        }
        ?value
    }
}
"#,
    )
    .expect("write helper source");
    std::fs::write(
        &main,
        r#"
module iter;
using root::iter;
using std::process;

struct Counter {
    value: i32,
}

extend Counter : iter::Step {
    fun next(self) ?Counter {
        ?Counter { value: self.value + 1 }
    }
}

extend Counter : Ord[Counter] {
    fun lt(&self, other: &Counter) bool { self.value < other.value }
    fun le(&self, other: &Counter) bool { self.value <= other.value }
    fun gt(&self, other: &Counter) bool { self.value > other.value }
    fun ge(&self, other: &Counter) bool { self.value >= other.value }
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cursor = iter::Cursor[Counter]::init(Counter { value: 7 });
    match cursor.advance() {
        ?value => if value.value != 7 { return process::ExitCode(1)!; },
        null => return process::ExitCode(2)!,
    }
    !()
}
"#,
    )
    .expect("write main source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit cross-module generic trait executable");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}
