// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_exe_std_slice_sorting_and_binary_search() {
    let root = temp_dir("emit_exe_std_slice_sorting_and_binary_search");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::cmp;
using std::process;
using std::slice;

struct Record {
    key: i32,
    sequence: i32,
}

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut empty: [i32; 0] = [];
    empty.sort();
    if not empty.isSorted() or empty.lowerBound(&4) != 0 or empty.upperBound(&4) != 0 {
        return process::ExitCode(1)!;
    }

    let mut values: [i32; 9] = [7, 4, -1, 4, 9, 0, 4, 2, -5];
    values.sort();
    if not values.isSorted()
        or values[0] != -5 or values[1] != -1 or values[2] != 0 or values[3] != 2
        or values[4] != 4 or values[5] != 4 or values[6] != 4 or values[7] != 7 or values[8] != 9
    {
        return process::ExitCode(2)!;
    }

    let equal = values.equalRange(&4);
    if equal.start() != 4 or equal.end() != 7
        or values.lowerBound(&3) != 4 or values.upperBound(&3) != 4
        or values.lowerBound(&12) != values.len()
        or values.upperBound(&(-10)) != 0
    {
        return process::ExitCode(3)!;
    }
    if values.binarySearch(&4) is ?found {
        if found < 4 or found >= 7 {
            return process::ExitCode(4)!;
        }
    } else {
        return process::ExitCode(4)!;
    }
    if values.binarySearch(&3) is ?unexpected {
        _ = unexpected;
        return process::ExitCode(5)!;
    }
    if values.partitionPoint(&\value: &i32 -> value.* < 4) != 4 {
        return process::ExitCode(6)!;
    }

    let descending = \left: &i32, right: &i32 -> {
        if left.* > right.* { cmp::Ordering::Less }
        else if left.* < right.* { cmp::Ordering::Greater }
        else { cmp::Ordering::Equal }
    };
    values.sortUnstableBy(&descending);
    if not values.isSortedBy(&descending) or values[0] != 9 or values[8] != -5 {
        return process::ExitCode(7)!;
    }
    let findFourDescending = \value: &i32 -> {
        if value.* > 4 { cmp::Ordering::Less }
        else if value.* < 4 { cmp::Ordering::Greater }
        else { cmp::Ordering::Equal }
    };
    let descendingEqual = values.equalRangeBy(&findFourDescending);
    if descendingEqual.start() != 2 or descendingEqual.end() != 5 {
        return process::ExitCode(8)!;
    }
    if values.binarySearchBy(&findFourDescending) is ?foundDescending {
        _ = foundDescending;
    } else {
        return process::ExitCode(8)!;
    }
    if values.lowerBoundBy(&\value: &i32 -> {
        if value.* > 5 { cmp::Ordering::Less }
        else if value.* < 5 { cmp::Ordering::Greater }
        else { cmp::Ordering::Equal }
    }) != 2
    {
        return process::ExitCode(9)!;
    }

    let mut records: [Record; 5] = [
        Record { key: 2, sequence: 0 },
        Record { key: 1, sequence: 1 },
        Record { key: 2, sequence: 2 },
        Record { key: 1, sequence: 3 },
        Record { key: 2, sequence: 4 },
    ];
    records.sortBy(&\left: &Record, right: &Record -> {
        if left.key < right.key { cmp::Ordering::Less }
        else if left.key > right.key { cmp::Ordering::Greater }
        else { cmp::Ordering::Equal }
    });
    if records[0].sequence != 1 or records[1].sequence != 3
        or records[2].sequence != 0 or records[3].sequence != 2 or records[4].sequence != 4
    {
        return process::ExitCode(10)!;
    }

    let mut heapValues: [i32; 10] = [5, 1, 5, -4, 9, 1, 0, 8, 3, 2];
    heapValues.sortUnstable();
    if not heapValues.isSorted()
        or heapValues[0] != -4 or heapValues[1] != 0 or heapValues[2] != 1
        or heapValues[3] != 1 or heapValues[4] != 2 or heapValues[5] != 3
        or heapValues[6] != 5 or heapValues[7] != 5 or heapValues[8] != 8 or heapValues[9] != 9
    {
        return process::ExitCode(11)!;
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
        .output_timeout_for_build("run nia emit --exe std slice sort and search");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
