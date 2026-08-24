// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

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
    fn next(self) ?Self;
}

pub trait Iterator {
    type Item;

    fn advance(&mut self) ?[Self as Iterator]::Item;
}

pub struct Cursor[T] {
    value: T,
}

extend[T] Cursor[T] {
    pub fn init(value: T) Cursor[T] {
        Self { value }
    }
}

extend[T] Cursor[T] : Iterator
where T: Step + Ord[T]
{
    type Item = T;

    fn advance(&mut self) ?T {
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
using entry::iter;
using std::process;

struct Counter {
    value: i32,
}

extend Counter : iter::Step {
    fn next(self) ?Counter {
        ?Counter { value: self.value + 1 }
    }
}

extend Counter : Ord[Counter] {
    fn lt(&self, other: &Counter) bool { self.value < other.value }
    fn le(&self, other: &Counter) bool { self.value <= other.value }
    fn gt(&self, other: &Counter) bool { self.value > other.value }
    fn ge(&self, other: &Counter) bool { self.value >= other.value }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cursor = iter::Cursor[Counter]::init(Counter { value: 7 });
    match cursor.advance() {
        ?value => if value.value != 7 { return process::exit(1)!; },
        null => return process::exit(2)!,
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
