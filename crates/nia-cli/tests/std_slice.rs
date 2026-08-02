// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_slice_direct_borrowed_iteration() {
    let root = temp_dir("emit_exe_std_slice_direct_borrowed_iteration");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;
using std::slice;

struct Token {
    value: i32,
}

extend Token : Eq[Token] {
    fn eq(&self, other: &Token) bool {
        self.value == other.value
    }

    fn ne(&self, other: &Token) bool {
        self.value != other.value
    }
}

fn foundAt(result: ?usize, expected: usize) bool {
    if result is ?index {
        index == expected
    } else {
        false
    }
}

fn sum(values: &[i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

fn sumMutView(values: &mut [i32]) i32 {
    let mut total = 0;
    for &value in values {
        total += value;
    }
    total
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;

    let values: [5]i32 = [2, 3, 5, 7, 11];
    if sum(&values) != 28 {
        return process::exit(1)!;
    }

    let mut middle = 0;
    for &value in &values[1..4] {
        middle += value;
    }
    if middle != 15 {
        return process::exit(2)!;
    }

    let mut writable: [3]i32 = [10, 20, 30];
    if sumMutView(&mut writable) != 60 {
        return process::exit(3)!;
    }
    for value in (&mut writable[..]).iterMut().rev() {
        value.* += 1;
    }
    if writable[0] != 11 or writable[1] != 21 or writable[2] != 31 {
        return process::exit(4)!;
    }

    let iter = (&values[..]).iter();
    if iter.len() != 5 or iter.isEmpty() {
        return process::exit(5)!;
    }

    let valueSlice = &values[..];
    if valueSlice.get(2) is ?value {
        if value.* != 5 {
            return process::exit(6)!;
        }
    } else {
        return process::exit(7)!;
    }
    if valueSlice.get(5) is ?unexpected {
        _ = unexpected;
        return process::exit(8)!;
    }
    if valueSlice.first() is ?value {
        if value.* != 2 {
            return process::exit(9)!;
        }
    } else {
        return process::exit(10)!;
    }
    if valueSlice.last() is ?value {
        if value.* != 11 {
            return process::exit(11)!;
        }
    } else {
        return process::exit(12)!;
    }

    switch valueSlice.getRange(1, 4) {
        ?middle => {
            if middle.len() != 3 or sum(middle) != 15 {
                return process::exit(13)!;
            }
        },
        null => { return process::exit(14)!; },
    }
    if valueSlice.getRange(4, 2) is ?unexpected {
        _ = unexpected;
        return process::exit(15)!;
    }
    if valueSlice.getRange(0, 6) is ?unexpected {
        _ = unexpected;
        return process::exit(16)!;
    }

    let empty = &values[values.len()..values.len()];
    if empty.first() is ?unexpected {
        _ = unexpected;
        return process::exit(17)!;
    }
    if empty.last() is ?unexpected {
        _ = unexpected;
        return process::exit(18)!;
    }
    switch empty.getRange(0, 0) {
        ?range => {
            if not range.isEmpty() {
                return process::exit(19)!;
            }
        },
        null => { return process::exit(20)!; },
    }

    let mut checked: [4]i32 = [1, 2, 3, 4];
    let mut checkedSlice = &mut checked[..];
    switch checkedSlice.getMut(1) {
        mut ?value => { value.* = 20; },
        null => { return process::exit(21)!; },
    }
    switch checkedSlice.firstMut() {
        mut ?value => { value.* = 10; },
        null => { return process::exit(22)!; },
    }
    switch checkedSlice.lastMut() {
        mut ?value => { value.* = 40; },
        null => { return process::exit(23)!; },
    }
    switch checkedSlice.getRangeMut(1, 3) {
        mut ?range => {
            for value in range.iterMut() {
                value.* += 1;
            }
        },
        null => { return process::exit(24)!; },
    }
    if checked[0] != 10 or checked[1] != 21 or checked[2] != 4 or checked[3] != 40 {
        return process::exit(25)!;
    }

    let sequence: [5]i32 = [1, 2, 1, 2, 3];
    let sequenceView = &sequence[..];
    if not sequenceView.equals(&[1, 2, 1, 2, 3])
        or sequenceView.equals(&[1, 2, 1, 2])
        or not sequenceView.startsWith(&[1, 2])
        or sequenceView.startsWith(&[2, 1])
        or not sequenceView.endsWith(&[2, 3])
        or sequenceView.endsWith(&[1, 2])
    {
        return process::exit(26)!;
    }
    let emptyStorage: [0]i32 = [];
    let emptyValues = &emptyStorage[..];
    if not foundAt(sequenceView.find(&[1, 2, 3]), 2)
        or not foundAt(sequenceView.find(emptyValues), 0)
        or not sequenceView.contains(&[2, 1])
        or not sequenceView.contains(emptyValues)
        or sequenceView.contains(&[3, 4])
    {
        return process::exit(27)!;
    }
    if sequenceView.find(&[1, 2, 1, 2, 3, 4]) is ?unexpected {
        _ = unexpected;
        return process::exit(28)!;
    }

    let noValues: &[i32] = emptyValues;
    if not noValues.equals(emptyValues)
        or not noValues.startsWith(emptyValues)
        or not noValues.endsWith(emptyValues)
        or noValues.contains(&[1])
    {
        return process::exit(29)!;
    }

    let tokens: [4]Token = [
        Token { value: 4 },
        Token { value: 5 },
        Token { value: 4 },
        Token { value: 6 },
    ];
    let tokenNeedle: [2]Token = [Token { value: 5 }, Token { value: 4 }];
    if not (&tokens[..]).contains(&tokenNeedle[..])
        or not foundAt((&tokens[..]).find(&tokenNeedle[..]), 1)
    {
        return process::exit(30)!;
    }

    !{}
}
"#,
    )
    .expect("write test source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
