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
using std::cmp;
using std::iter;
using std::process;
using std::slice;

struct Token {
    value: i32,
}

struct Marker {}

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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let values: [i32; 5] = [2, 3, 5, 7, 11];
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

    let mut writable: [i32; 3] = [10, 20, 30];
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

    let mut mixed = (&values[..]).iter();
    if mixed.next() is ?value {
        if value.* != 2 {
            return process::exit(47)!;
        }
    } else {
        return process::exit(47)!;
    }
    if mixed.nextBack() is ?value {
        if value.* != 11 {
            return process::exit(48)!;
        }
    } else {
        return process::exit(48)!;
    }
    if mixed.nextBack() is ?value {
        if value.* != 7 {
            return process::exit(49)!;
        }
    } else {
        return process::exit(49)!;
    }
    if mixed.len() != 2 {
        return process::exit(50)!;
    }

    let mut taken = (0usize..10usize).iter().take(3usize);
    if taken.next() is ?value {
        if value != 0usize {
            return process::exit(51)!;
        }
    } else {
        return process::exit(51)!;
    }
    if taken.next() is ?value {
        if value != 1usize {
            return process::exit(52)!;
        }
    } else {
        return process::exit(52)!;
    }
    if taken.next() is ?value {
        if value != 2usize {
            return process::exit(53)!;
        }
    } else {
        return process::exit(53)!;
    }
    if taken.next() is ?unexpected {
        _ = unexpected;
        return process::exit(54)!;
    }

    let mut rear = (0usize..10usize).iter().rev().take(3usize);
    if rear.next() is ?value {
        if value != 9usize {
            return process::exit(55)!;
        }
    } else {
        return process::exit(55)!;
    }
    if rear.next() is ?value {
        if value != 8usize {
            return process::exit(56)!;
        }
    } else {
        return process::exit(56)!;
    }
    if rear.next() is ?value {
        if value != 7usize {
            return process::exit(57)!;
        }
    } else {
        return process::exit(57)!;
    }

    let mut maximum = (u8::MAX..).iter();
    if maximum.next() is ?value {
        if value != u8::MAX {
            return process::exit(58)!;
        }
    } else {
        return process::exit(58)!;
    }
    if maximum.next() is ?unexpected {
        _ = unexpected;
        return process::exit(59)!;
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

    let mut checked: [i32; 4] = [1, 2, 3, 4];
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

    let mut wideCopy: [i32; 5] = [0, 0, 0, 44, 55];
    let wideSource: [i32; 3] = [11, 22, 33];
    let mut wideCopyView = &mut wideCopy[..];
    if wideCopyView.copyFrom(&wideSource) != 3
        or wideCopy[0] != 11
        or wideCopy[1] != 22
        or wideCopy[2] != 33
        or wideCopy[3] != 44
        or wideCopy[4] != 55
    {
        return process::exit(31)!;
    }

    let mut shortCopy: [i32; 2] = [0, 0];
    let mut shortCopyView = &mut shortCopy[..];
    if shortCopyView.copyFrom(&wideSource) != 2
        or shortCopy[0] != 11
        or shortCopy[1] != 22
    {
        return process::exit(32)!;
    }

    let mut overlapRight: [i32; 5] = [1, 2, 3, 4, 5];
    if (&mut overlapRight[1..]).copyFrom(&overlapRight[0..4]) != 4
        or not (&overlapRight[..]).equals(&[1, 1, 2, 3, 4])
    {
        return process::exit(33)!;
    }
    let mut overlapLeft: [i32; 5] = [1, 2, 3, 4, 5];
    if (&mut overlapLeft[0..4]).copyFrom(&overlapLeft[1..]) != 4
        or not (&overlapLeft[..]).equals(&[2, 3, 4, 5, 5])
    {
        return process::exit(34)!;
    }

    let mut markers: [Marker; 2] = [Marker {}, Marker {}];
    let markerSource: [Marker; 3] = [Marker {}, Marker {}, Marker {}];
    if (&mut markers[..]).copyFrom(&markerSource) != 2 {
        return process::exit(35)!;
    }

    let mut emptyCopy: [i32; 0] = [];
    if (&mut emptyCopy[..]).copyFrom(&wideSource) != 0
        or wideCopyView.copyFrom(&emptyCopy) != 0
    {
        return process::exit(37)!;
    }

    let low: &[i32] = &[1, 2];
    let high: &[i32] = &[1, 3];
    let prefix: &[i32] = &[1];
    if low.compare(high) != cmp::Ordering::Less
        or high.compare(low) != cmp::Ordering::Greater
        or low.compare(low) != cmp::Ordering::Equal
        or prefix.compare(low) != cmp::Ordering::Less
        or (&emptyCopy[..]).compare(prefix) != cmp::Ordering::Less
    {
        return process::exit(36)!;
    }

    let sequence: [i32; 5] = [1, 2, 1, 2, 3];
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
    let emptyStorage: [i32; 0] = [];
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

    let tokens: [Token; 4] = [
        Token { value: 4 },
        Token { value: 5 },
        Token { value: 4 },
        Token { value: 6 },
    ];
    let tokenNeedle: [Token; 2] = [Token { value: 5 }, Token { value: 4 }];
    if not (&tokens[..]).contains(&tokenNeedle[..])
        or not foundAt((&tokens[..]).find(&tokenNeedle[..]), 1)
    {
        return process::exit(30)!;
    }

    let separated: [i32; 8] = [0, 1, 0, 0, 2, 0, 3, 0];
    let separator: [i32; 1] = [0];
    let mut partIndex = 0;
    for part in (&separated[..]).split(&separator) {
        let matches = if partIndex == 0 or partIndex == 2 or partIndex == 5 {
            part.isEmpty()
        } else if partIndex == 1 {
            part.equals(&[1])
        } else if partIndex == 3 {
            part.equals(&[2])
        } else if partIndex == 4 {
            part.equals(&[3])
        } else {
            false
        };
        if not matches {
            return process::exit(38)!;
        }
        partIndex += 1;
    }
    if partIndex != 6 {
        return process::exit(39)!;
    }

    let mut unsplit = (&separated[..]).split(emptyValues);
    if unsplit.next() is ?whole {
        if not whole.equals(&separated) {
            return process::exit(42)!;
        }
    } else {
        return process::exit(42)!;
    }
    if unsplit.next() is ?unexpected {
        _ = unexpected;
        return process::exit(42)!;
    }
    if (&separated[..]).split(&[99]).count() != 1 or emptyValues.split(&separator).count() != 1 {
        return process::exit(42)!;
    }

    let overlappingItems: [i32; 5] = [1, 1, 1, 1, 1];
    let overlappingSeparator: [i32; 2] = [1, 1];
    let mut overlappingIndex = 0;
    for part in (&overlappingItems[..]).split(&overlappingSeparator) {
        if overlappingIndex == 0 or overlappingIndex == 1 {
            if not part.isEmpty() {
                return process::exit(44)!;
            }
        } else if overlappingIndex == 2 {
            if not part.equals(&[1]) {
                return process::exit(43)!;
            }
        } else {
            return process::exit(45)!;
        }
        overlappingIndex += 1;
    }
    if overlappingIndex != 3 {
        return process::exit(46)!;
    }

    !()
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

#[test]
fn check_std_take_does_not_claim_double_ended_iteration() {
    let root = temp_dir("check_std_take_does_not_claim_double_ended_iteration");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::iter;

fn main() () {
    _ = (0usize..10usize).iter().take(3usize).rev();
}
"#,
    )
    .expect("write invalid take reverse source");

    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("check take double-ended boundary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown struct field `rev`"), "{stderr}");
}
