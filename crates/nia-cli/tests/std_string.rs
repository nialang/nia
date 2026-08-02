// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_string_compares_and_searches_scalar_text() {
    let root = temp_dir("emit_exe_std_string_compares_and_searches_scalar_text");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std;
using std::mem;
using std::process;
using std::string;

fn foundAt(result: ?usize, expected: usize) bool {
    if result is ?index {
        index == expected
    } else {
        false
    }
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let text: &[char] = &"alpha λ beta λ";
    if not text.equals(&"alpha λ beta λ")
        or text.equals(&"alpha λ beta")
        or not text.startsWith(&"alpha λ")
        or text.startsWith(&"lpha")
        or not text.endsWith(&"beta λ")
        or text.endsWith(&"beta")
    {
        return process::exit(1)!;
    }

    if not foundAt(text.find(&"λ"), 6usize)
        or not foundAt(text.find(&"λ beta"), 6usize)
        or not foundAt(text.find(&""), 0usize)
        or not text.contains(&"beta")
        or not text.contains(&"")
        or text.contains(&"gamma")
    {
        return process::exit(2)!;
    }
    if text.find(&"gamma") is ?unexpected {
        _ = unexpected;
        return process::exit(3)!;
    }

    let overlapping: &[char] = &"ababa";
    if not foundAt(overlapping.find(&"aba"), 0usize)
        or not overlapping.endsWith(&"aba")
        or overlapping.startsWith(&"bab")
    {
        return process::exit(4)!;
    }

    let empty: &[char] = &"";
    if not empty.equals(&"")
        or not empty.startsWith(&"")
        or not empty.endsWith(&"")
        or not foundAt(empty.find(&""), 0usize)
    {
        return process::exit(5)!;
    }
    if empty.find(&"a") is ?unexpected {
        _ = unexpected;
        return process::exit(5)!;
    }

    let mut allocator = mem::PageAllocator::init();
    let mut page = &mut allocator;
    let mut owned = std::StringBuf::from_slice(page, text).exit().?;
    defer owned.deinit(page).exit().?;
    if not owned.equals(text)
        or not owned.startsWith(&"alpha")
        or not owned.endsWith(&"λ")
        or not foundAt(owned.find(&"beta"), 8usize)
        or not owned.contains(&"λ beta")
    {
        return process::exit(6)!;
    }

    owned.append(page, &"!").exit().?;
    if not owned.endsWith(&"λ!") or owned.equals(text) {
        return process::exit(7)!;
    }

    let mut lambdaCount = 0usize;
    for &ch in text.iter() {
        if ch == 'λ' {
            lambdaCount += 1usize;
        }
    }
    if lambdaCount != 2usize {
        return process::exit(8)!;
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
