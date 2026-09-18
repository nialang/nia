// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_iterator_for_each_accepts_borrowed_closure() {
    let root = temp_dir("emit_exe_std_iterator_for_each_accepts_borrowed_closure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

static mut invocationCount: i32 = 0;
static mut total: i32 = 0;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    (1..5).iter().forEach(&\[offset] value: i32 -> {
        invocationCount += 1;
        total += value + offset;
    });
    (1..3).iter().forEach(&\value: i32 -> {
        invocationCount += 1;
        total += value;
    });
    let mut values: [i32; 3] = [1, 2, 3];
    (&mut values).iterMut().forEach(&\value: &mut i32 -> {
        value.* += 1;
    });
    if invocationCount != 6 or total != 17
        or values[0] != 2 or values[1] != 3 or values[2] != 4
    {
        return process::ExitCode(1)!;
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
        .output_timeout_for_build("run nia emit --exe std iterator forEach");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_iterator_fold_accepts_capturing_borrowed_closure() {
    let root = temp_dir("emit_exe_std_iterator_fold_accepts_capturing_borrowed_closure");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    let total = (1..5).iter().fold(0, &\[offset] acc: i32, value: i32 -> {
        acc + value + offset
    });
    let summary = (1..5).iter().fold((0, 0), &\state: (i32, i32), value: i32 -> {
        (state.0 + value, state.1 + 1)
    });
    let prefix = (1..6).iter().take(3).fold(0, &\acc: i32, value: i32 -> {
        acc + value
    });
    if total != 14 or summary.0 != 10 or summary.1 != 4 or prefix != 6 {
        return process::ExitCode(1)!;
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
        .output_timeout_for_build("run nia emit --exe std iterator fold");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_iterator_try_fold_preserves_error_and_stops_consuming() {
    let root = temp_dir("emit_exe_std_iterator_try_fold_preserves_error_and_stops_consuming");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::builtin;
using std::process;

enum FoldError: i32 {
    Rejected = 7,
    _,
}

extend FoldError : builtin::IntoError[process::ExitCode] {
    const fn intoError(self) process::ExitCode {
        match self {
            FoldError::Rejected => {},
            _ => {},
        }
        process::ExitCode(9)
    }
}

static mut invocationCount: i32 = 0;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let offset = 1;
    let folded: FoldError!(i32, i32) = (1..5).iter().tryFold(
        (0, 0),
        &\[offset] state: (i32, i32), value: i32 -> {
            invocationCount += 1;
            !(state.0 + value + offset, state.1 + 1)
        },
    );
    let summary = folded.?;
    if summary.0 != 14 or summary.1 != 4 or invocationCount != 4 {
        return process::ExitCode(1)!;
    }

    invocationCount = 0;
    let failed = (1..6).iter().tryFold(
        0,
        &\sum: i32, value: i32 -> {
            invocationCount += 1;
            if value == 3 {
                FoldError::Rejected!
            } else {
                !(sum + value)
            }
        },
    );
    match failed {
        !value => {
            _ = value;
            return process::ExitCode(2)!;
        },
        FoldError::Rejected! => {},
        error! => {
            _ = error;
            return process::ExitCode(3)!;
        },
    }
    if invocationCount != 3 {
        return process::ExitCode(4)!;
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
        .output_timeout_for_build("run nia emit --exe std iterator tryFold");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_iterator_position_consumes_items_with_borrowed_predicate() {
    let root = temp_dir("emit_exe_std_iterator_position_consumes_items_with_borrowed_predicate");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

static mut predicateCount: i32 = 0;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let target = 4;
    let index = (1..6).iter().position(&\[target] value: i32 -> {
        predicateCount += 1;
        value == target
    });
    match index {
        ?found => if found != 3 {
            return process::ExitCode(1)!;
        },
        null => return process::ExitCode(2)!,
    }
    let missing = (1..4).iter().position(&\value: i32 -> {
        predicateCount += 1;
        value == 9
    });
    match missing {
        ?_ => return process::ExitCode(3)!,
        null => {},
    }
    if predicateCount != 7 {
        return process::ExitCode(4)!;
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
        .output_timeout_for_build("run nia emit --exe std iterator position");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_range_types_expose_canonical_constructors() {
    let root = temp_dir("emit_exe_std_range_types_expose_canonical_constructors");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::iter;
using std::process;

struct Counter {
    value: i32,
}

extend Counter : iter::Step {
    fn forwardChecked(self) ?Counter {
        if self.value == 5 { null } else { ?Counter { value: self.value + 1 } }
    }
}

extend Counter : iter::StepBack {
    fn backwardChecked(self) ?Counter {
        if self.value == 0 { null } else { ?Counter { value: self.value - 1 } }
    }
}

extend Counter : Eq[Counter] {
    fn eq(&self, other: &Counter) bool { self.value == other.value }
    fn ne(&self, other: &Counter) bool { self.value != other.value }
}

extend Counter : Ord[Counter] {
    fn lt(&self, other: &Counter) bool { self.value < other.value }
    fn le(&self, other: &Counter) bool { self.value <= other.value }
    fn gt(&self, other: &Counter) bool { self.value > other.value }
    fn ge(&self, other: &Counter) bool { self.value >= other.value }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut half = iter::Range[Counter]::init(
        Counter { value: 1 },
        Counter { value: 4 },
    );
    match half.next() {
        ?value => if value.value != 1 { return process::ExitCode(1)!; },
        null => return process::ExitCode(2)!,
    }
    match half.nextBack() {
        ?value => if value.value != 3 { return process::ExitCode(3)!; },
        null => return process::ExitCode(4)!,
    }
    match half.next() {
        ?value => if value.value != 2 { return process::ExitCode(5)!; },
        null => return process::ExitCode(6)!,
    }
    if half.next() is ?_ { return process::ExitCode(7)!; }

    let mut inclusive = iter::RangeInclusive[Counter]::init(
        Counter { value: 2 },
        Counter { value: 3 },
    );
    match inclusive.nextBack() {
        ?value => if value.value != 3 { return process::ExitCode(8)!; },
        null => return process::ExitCode(9)!,
    }
    match inclusive.next() {
        ?value => if value.value != 2 { return process::ExitCode(10)!; },
        null => return process::ExitCode(11)!,
    }
    if inclusive.next() is ?_ { return process::ExitCode(12)!; }

    let mut empty = iter::RangeInclusive[Counter]::init(
        Counter { value: 4 },
        Counter { value: 3 },
    );
    if empty.next() is ?_ { return process::ExitCode(13)!; }

    let mut from = iter::RangeFrom[Counter]::init(Counter { value: 5 });
    match from.next() {
        ?value => if value.value != 5 { return process::ExitCode(14)!; },
        null => return process::ExitCode(15)!,
    }
    if from.next() is ?_ { return process::ExitCode(16)!; }
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
        .output_timeout_for_build("run nia emit --exe std range constructors");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_ranges_stop_when_custom_steps_cross_bounds() {
    let root = temp_dir("emit_exe_std_ranges_stop_when_custom_steps_cross_bounds");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::iter;
using std::process;

struct Stride {
    value: i32,
}

extend Stride : iter::Step {
    fn forwardChecked(self) ?Stride {
        if self.value == 8 {
            ?self
        } else if self.value >= 9 {
            null
        } else {
            ?Stride { value: self.value + 2 }
        }
    }
}

extend Stride : iter::StepBack {
    fn backwardChecked(self) ?Stride {
        if self.value == 7 {
            ?self
        } else if self.value <= -3 {
            null
        } else {
            ?Stride { value: self.value - 2 }
        }
    }
}

extend Stride : Eq[Stride] {
    fn eq(&self, other: &Stride) bool { self.value == other.value }
    fn ne(&self, other: &Stride) bool { self.value != other.value }
}

extend Stride : Ord[Stride] {
    fn lt(&self, other: &Stride) bool { self.value < other.value }
    fn le(&self, other: &Stride) bool { self.value <= other.value }
    fn gt(&self, other: &Stride) bool { self.value > other.value }
    fn ge(&self, other: &Stride) bool { self.value >= other.value }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut forward = iter::Range[Stride]::init(
        Stride { value: 0 },
        Stride { value: 5 },
    );
    match forward.next() {
        ?value => if value.value != 0 { return process::ExitCode(1)!; },
        null => return process::ExitCode(2)!,
    }
    match forward.next() {
        ?value => if value.value != 2 { return process::ExitCode(3)!; },
        null => return process::ExitCode(4)!,
    }
    match forward.next() {
        ?value => if value.value != 4 { return process::ExitCode(5)!; },
        null => return process::ExitCode(6)!,
    }
    if forward.next() is ?_ { return process::ExitCode(7)!; }
    if forward.start().value != 5 or forward.end().value != 5 {
        return process::ExitCode(8)!;
    }

    let mut backward = iter::Range[Stride]::init(
        Stride { value: 0 },
        Stride { value: 5 },
    );
    match backward.nextBack() {
        ?value => if value.value != 3 { return process::ExitCode(9)!; },
        null => return process::ExitCode(10)!,
    }
    match backward.nextBack() {
        ?value => if value.value != 1 { return process::ExitCode(11)!; },
        null => return process::ExitCode(12)!,
    }
    if backward.nextBack() is ?_ { return process::ExitCode(13)!; }
    if backward.start().value != 0 or backward.end().value != 0 {
        return process::ExitCode(14)!;
    }

    let mut inclusive = iter::RangeInclusive[Stride]::init(
        Stride { value: 0 },
        Stride { value: 5 },
    );
    match inclusive.next() {
        ?value => if value.value != 0 { return process::ExitCode(15)!; },
        null => return process::ExitCode(16)!,
    }
    match inclusive.next() {
        ?value => if value.value != 2 { return process::ExitCode(17)!; },
        null => return process::ExitCode(18)!,
    }
    match inclusive.next() {
        ?value => if value.value != 4 { return process::ExitCode(19)!; },
        null => return process::ExitCode(20)!,
    }
    if inclusive.next() is ?_ { return process::ExitCode(21)!; }

    let mut inclusiveBack = iter::RangeInclusive[Stride]::init(
        Stride { value: 0 },
        Stride { value: 5 },
    );
    match inclusiveBack.nextBack() {
        ?value => if value.value != 5 { return process::ExitCode(22)!; },
        null => return process::ExitCode(23)!,
    }
    match inclusiveBack.nextBack() {
        ?value => if value.value != 3 { return process::ExitCode(24)!; },
        null => return process::ExitCode(25)!,
    }
    match inclusiveBack.nextBack() {
        ?value => if value.value != 1 { return process::ExitCode(26)!; },
        null => return process::ExitCode(27)!,
    }
    if inclusiveBack.nextBack() is ?_ { return process::ExitCode(28)!; }

    let mut mixed = iter::RangeInclusive[Stride]::init(
        Stride { value: 0 },
        Stride { value: 5 },
    );
    match mixed.next() {
        ?value => if value.value != 0 { return process::ExitCode(29)!; },
        null => return process::ExitCode(30)!,
    }
    match mixed.nextBack() {
        ?value => if value.value != 5 { return process::ExitCode(31)!; },
        null => return process::ExitCode(32)!,
    }
    match mixed.next() {
        ?value => if value.value != 2 { return process::ExitCode(33)!; },
        null => return process::ExitCode(34)!,
    }
    if mixed.nextBack() is ?_ { return process::ExitCode(35)!; }

    let mut stalled = iter::RangeInclusive[Stride]::init(
        Stride { value: 8 },
        Stride { value: 9 },
    );
    match stalled.next() {
        ?value => if value.value != 8 { return process::ExitCode(36)!; },
        null => return process::ExitCode(37)!,
    }
    if stalled.next() is ?_ { return process::ExitCode(38)!; }

    let mut stalledBack = iter::Range[Stride]::init(
        Stride { value: 0 },
        Stride { value: 7 },
    );
    if stalledBack.nextBack() is ?_ { return process::ExitCode(39)!; }

    let mut absentBack = iter::Range[Stride]::init(
        Stride { value: -5 },
        Stride { value: -3 },
    );
    if absentBack.nextBack() is ?_ { return process::ExitCode(40)!; }
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
        .output_timeout_for_build("run nia emit --exe custom range steps");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_u128_ranges_cover_integer_endpoints() {
    let root = temp_dir("emit_exe_std_u128_ranges_cover_integer_endpoints");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::iter;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let first = u128::MAX - 2u128;

    let mut half = iter::Range[u128]::init(first, u128::MAX);
    match half.next() {
        ?value => if value != first { return process::ExitCode(1)!; },
        null => return process::ExitCode(2)!,
    }
    match half.nextBack() {
        ?value => if value != u128::MAX - 1u128 { return process::ExitCode(3)!; },
        null => return process::ExitCode(4)!,
    }
    if half.next() is ?_ { return process::ExitCode(5)!; }

    let mut inclusive = (first..=u128::MAX).iter();
    match inclusive.nextBack() {
        ?value => if value != u128::MAX { return process::ExitCode(6)!; },
        null => return process::ExitCode(7)!,
    }
    match inclusive.next() {
        ?value => if value != first { return process::ExitCode(8)!; },
        null => return process::ExitCode(9)!,
    }
    match inclusive.next() {
        ?value => if value != u128::MAX - 1u128 { return process::ExitCode(10)!; },
        null => return process::ExitCode(11)!,
    }
    if inclusive.next() is ?_ { return process::ExitCode(12)!; }

    let mut from = iter::RangeFrom[u128]::init(u128::MAX);
    match from.next() {
        ?value => if value != u128::MAX { return process::ExitCode(13)!; },
        null => return process::ExitCode(14)!,
    }
    if from.next() is ?_ { return process::ExitCode(15)!; }
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
        .output_timeout_for_build("run nia emit --exe u128 ranges");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}

#[test]
fn emit_exe_std_owned_iterator_adapters_expose_constructors() {
    let root = temp_dir("emit_exe_std_owned_iterator_adapters_expose_constructors");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::iter;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;

    let mut take = iter::Take[iter::Range[i32]]::init(
        iter::Range[i32]::init(1, 5),
        2,
    );
    match take.next() {
        ?value => if value != 1 { return process::ExitCode(1)!; },
        null => return process::ExitCode(2)!,
    }
    match take.next() {
        ?value => if value != 2 { return process::ExitCode(3)!; },
        null => return process::ExitCode(4)!,
    }
    if take.next() is ?_ { return process::ExitCode(5)!; }

    let mut rev = iter::Rev[iter::RangeInclusive[i32]]::init(
        iter::RangeInclusive[i32]::init(1, 3),
    );
    match rev.next() {
        ?value => if value != 3 { return process::ExitCode(6)!; },
        null => return process::ExitCode(7)!,
    }
    match rev.nextBack() {
        ?value => if value != 1 { return process::ExitCode(8)!; },
        null => return process::ExitCode(9)!,
    }
    match rev.next() {
        ?value => if value != 2 { return process::ExitCode(10)!; },
        null => return process::ExitCode(11)!,
    }
    if rev.next() is ?_ { return process::ExitCode(12)!; }
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
        .output_timeout_for_build("run nia emit --exe owned iterator constructors");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(run.code(), Some(0));
}
