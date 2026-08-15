use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_tuple_values_projections_and_patterns() {
    let root = temp_dir("emit_exe_tuple_values_projections_and_patterns");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

const fn const_select(value: (usize, (bool, usize))) usize {
    switch value {
        (left, (true, right)) => left + right,
        (_, (_, fallback)) => fallback,
    }
}

fn runtime_select(pair: (i32, (bool, i32))) i32 {
    if pair is (40, (true, value)) {
        value
    } else {
        switch pair {
            (left, (false, right)) => left + right,
            (_, (_, fallback)) => fallback,
        }
    }
}

const width: usize = const_select((3usize, (true, 5usize)));

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut pair = (40, (true, 2));
    pair.0 += pair.1.1;
    let (answer, (enabled, tail)) = pair;
    if answer != 42 or not enabled or tail != 2 {
        return process::exit(1)!;
    }
    if runtime_select((40, (true, 7))) != 7 {
        return process::exit(2)!;
    }
    if width != 8 {
        return process::exit(3)!;
    }
    let () = ();
    !()
}
"#,
    )
    .expect("write tuple executable source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit tuple executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run tuple executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_nominal_struct_patterns_match_runtime_and_const_values() {
    let root = temp_dir("emit_exe_nominal_struct_patterns_match_runtime_and_const_values");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

struct Point { x: i32, y: i32 }
struct Box[T] { value: T, tag: i32 }
enum Event { Stop, Resize { wide: bool, height: i32 } }

const fn constSum(point: Point) i32 {
    let mut Point { y, x } = point;
    x += 1;
    x + y
}

fn runtimeSum(point: Point) i32 {
    let Point { y: second, x } = point;
    x + second
}

fn unbox[T](boxed: Box[T]) T {
    let Box { value, .. } = boxed;
    value
}

fn classify(point: Point) i32 {
    switch point {
        Point { x: 0, .. } => 7,
        Point { .. } => 9,
    }
}

fn readOptional(point: ?Point) i32 {
    if point is ?Point { x, .. } {
        x
    } else {
        0
    }
}

fn eventScore(event: Event) i32 {
    switch event {
        Event::Stop => 0,
        Event::Resize { wide: true, .. } => 1,
        Event::Resize { wide: false, .. } => 2,
    }
}

const total: i32 = constSum(Point { x: 19, y: 22 });

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    if total != 42 {
        return process::exit(1)!;
    }
    if runtimeSum(Point { x: 20, y: 22 }) != 42 {
        return process::exit(2)!;
    }
    if unbox[i32](Box[i32] { value: 42, tag: 7 }) != 42 {
        return process::exit(3)!;
    }
    if classify(Point { x: 0, y: 7 }) != 7 {
        return process::exit(4)!;
    }
    if classify(Point { x: 1, y: 7 }) != 9 {
        return process::exit(5)!;
    }
    if readOptional(?Point { x: 42, y: 0 }) != 42 {
        return process::exit(6)!;
    }
    if eventScore(Event::Resize { wide: false, height: 99 }) != 2 {
        return process::exit(7)!;
    }
    !()
}
"#,
    )
    .expect("write nominal struct pattern executable source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit nominal struct pattern executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run nominal struct pattern executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_switch_destructuring_matches_const_semantics() {
    let root = temp_dir("emit_exe_switch_destructuring_matches_const_semantics");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::process;

fn read_optional(value: ?i32) i32 {
    switch value {
        ?payload => payload,
        null => 0,
    }
}

fn read_error(value: i32!i32) i32 {
    switch value {
        !payload => payload,
        error! => error,
    }
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    if read_optional(?4) != 4 {
        return process::exit(1)!;
    }
    if read_optional(null) != 0 {
        return process::exit(2)!;
    }
    if read_error(!7) != 7 {
        return process::exit(3)!;
    }
    if read_error(5!) != 5 {
        return process::exit(4)!;
    }
    !()
}
"#,
    )
    .expect("write switch destructuring source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit switch destructuring executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run switch destructuring executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_error_propagation_converts_with_into_error() {
    let root = temp_dir("emit_exe_error_propagation_converts_with_into_error");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::error;
using std::process;

enum SourceError: i32 {
    Failed = 1,
    _,
}

enum TargetError: i32 {
    Converted = 2,
    Unknown = 3,
    _,
}

struct EmptySourceError {}
struct EmptyTargetError {}

static mut conversionCount: i32 = 0;
static mut sourceCount: i32 = 0;

extend SourceError : error::IntoError[TargetError] {
    fn into_error(self) TargetError {
        conversionCount += 1;
        switch self {
            SourceError::Failed => TargetError::Converted,
            _ => TargetError::Unknown,
        }
    }
}

extend EmptySourceError : error::IntoError[EmptyTargetError] {
    fn into_error(self) EmptyTargetError {
        conversionCount += 1;
        {}
    }
}

fn failVoid() SourceError!() {
    SourceError::Failed!
}

fn failEmpty() EmptySourceError!() {
    EmptySourceError {}!
}

fn propagateWithDefer() TargetError!() {
    defer conversionCount *= 10;
    failVoid().?;
    !()
}

fn propagateEmpty() EmptyTargetError!() {
    failEmpty().?;
    !()
}

fn source(succeed: bool) SourceError!i32 {
    sourceCount += 1;
    if succeed {
        !41
    } else {
        SourceError::Failed!
    }
}

fn propagate[Source, Target](value: Source!i32) Target!i32
where Source: error::IntoError[Target]
{
    !(value.? + 1)
}

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    switch propagate[SourceError, TargetError](source(true)) {
        !value => {
            if value != 42 {
                return process::exit(1)!;
            }
        },
        error! => {
            _ = error;
            return process::exit(2)!;
        },
    }
    if conversionCount != 0 {
        return process::exit(5)!;
    }
    if sourceCount != 1 {
        return process::exit(12)!;
    }
    switch propagate[SourceError, TargetError](source(false)) {
        !value => {
            _ = value;
            return process::exit(3)!;
        },
        TargetError::Converted! => {},
        error! => {
            _ = error;
            return process::exit(4)!;
        },
    }
    if conversionCount != 1 {
        return process::exit(6)!;
    }
    if sourceCount != 2 {
        return process::exit(13)!;
    }
    conversionCount = 0;
    switch propagateWithDefer() {
        !ok => {
            _ = ok;
            return process::exit(7)!;
        },
        TargetError::Converted! => {},
        error! => {
            _ = error;
            return process::exit(8)!;
        },
    }
    if conversionCount != 10 {
        return process::exit(9)!;
    }
    conversionCount = 0;
    switch propagateEmpty() {
        !ok => {
            _ = ok;
            return process::exit(10)!;
        },
        error! => { _ = error; },
    }
    if conversionCount != 1 {
        return process::exit(11)!;
    }
    !()
}
"#,
    )
    .expect("write IntoError propagation source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit IntoError propagation executable");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe).status_timeout("run IntoError propagation executable");
    assert_eq!(status.code(), Some(0));
}
