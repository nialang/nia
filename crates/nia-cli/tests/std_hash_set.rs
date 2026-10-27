// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use nia_test_support::{CommandExt, CommandStatusExt, test_dir as temp_dir};

#[test]
fn emit_exe_std_hash_set_supports_membership_and_cleanup() {
    let root = temp_dir("emit_exe_std_hash_set_supports_membership_and_cleanup");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::collections;
using std::mem;
using std::process;

pub fun main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut allocator = mem::PageAllocator::init();
    let mut set: collections::HashSet[i32] = .initSeed(19);
    let mut reserved: collections::HashSet[i32] = collections::HashSet[i32]::initSeedCapacity(
        &mut allocator,
        23,
        4,
    ).?;
    _ = reserved.insert(&mut allocator, 99).?;
    let mut cloned = reserved.clone(&mut allocator).?;
    if not cloned.contains(&99) {
        return process::ExitCode(7)!;
    }
    cloned.deinit(&mut allocator).?;
    reserved.deinit(&mut allocator).?;
    if not set.isEmpty() or set.len() != 0 {
        return process::ExitCode(1)!;
    }
    if not set.insert(&mut allocator, 4).? or set.insert(&mut allocator, 4).? {
        return process::ExitCode(2)!;
    }
    if not set.insert(&mut allocator, 8).? or not set.contains(&4) or set.contains(&9) {
        return process::ExitCode(3)!;
    }
    if set.len() != 2 or not set.remove(&4) or set.remove(&4) or set.contains(&4) {
        return process::ExitCode(4)!;
    }
    _ = set.insert(&mut allocator, 12).?;
    _ = set.insert(&mut allocator, 16).?;
    let mut count = 0;
    for value in set.iter() {
        if value.* == 12 or value.* == 16 {
            count += 1;
        }
    }
    if count != 2 or set.len() != 3 {
        return process::ExitCode(5)!;
    }
    set.clear();
    if not set.isEmpty() or set.len() != 0 {
        return process::ExitCode(6)!;
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
        .output_timeout_for_build("run nia emit --exe std hash set");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = Command::new(&exe).status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
