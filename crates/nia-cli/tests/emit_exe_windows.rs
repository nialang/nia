// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(windows)]

use std::{
    path::{Path, PathBuf},
    process::Command,
};
mod support;
use support::{CommandExt, CommandStatusExt, temp_dir};

fn emit(root: &Path, name: &str, source: &str) -> PathBuf {
    let main = root.join(format!("{name}.nia"));
    let exe = root.join(format!("{name}.exe"));
    std::fs::write(&main, source).expect("write Windows regression source");
    let output = support::nia_command()
        .args(["emit", "--exe"])
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("emit Windows regression executable");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    exe
}

fn byte_literal(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| format!("\\x{byte:02x}"))
        .collect()
}

#[test]
fn windows_unicode_paths_and_directory_handles_round_trip() {
    let root = temp_dir("windows_unicode_paths_and_directory_handles");
    let directory = "\u{76ee}\u{5f55}-\u{1f680}";
    let filename = "\u{6587}\u{4ef6}-\u{1f680}.txt";
    let renamed = "\u{91cd}\u{547d}\u{540d}-\u{1f680}.txt";
    let working = root.join(directory);
    std::fs::create_dir(&working).expect("create Unicode working directory");
    let source = r#"
using std::fs;
using std::process;
using std::slice;

extern fn GetCurrentProcess() usize;
extern fn GetProcessHandleCount(process: usize, count: &mut u32) i32;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut pathBuffer: [u8; 4096] = [0; 4096];
    let path = fs::getCwd(&mut pathBuffer[..]).?;
    let suffix = b"$DIRECTORY_BYTES";
    if path.len() < suffix.len() or not (&path[path.len() - suffix.len()..]).equals(&suffix) {
        return process::ExitCode(1)!;
    }
    let mut dir = fs::Dir::cwd().?;
    defer dir.close().?;
    let oldPath = fs::RelativePathView::fromText(&"$FILE").?;
    let newPath = fs::RelativePathView::fromText(&"$RENAMED").?;
    let mut file = dir.createFile(oldPath, fs::CreateOptions::readWrite()).?;
    _ = file.seekTo(7).?;
    file.truncate(17).?;
    if file.seekBy(0).? != 7 { return process::ExitCode(2)!; }
    file.setPermissions(0o444).?;
    file.setPermissions(0o666).?;
    file.close().?;
    dir.rename(oldPath, newPath).?;
    if dir.metadata(newPath, fs::MetadataOptions::init()).?.size() != 17 {
        return process::ExitCode(3)!;
    }

    let mut buffer: [u8; 40] = [0; 40];
    let mut entries = dir.entries(&mut buffer[..]).?;
    defer entries.close().?;
    let mut count: usize = 0;
    loop {
        let result = match entries.next() { ?value => value, null => break };
        let entry = result.?;
        if entry.isDot() or entry.isDotDot() { continue; }
        if not entry.name().equals(&b"$RENAMED_BYTES") { return process::ExitCode(4)!; }
        count += 1;
    }
    if count != 1 { return process::ExitCode(5)!; }
    entries.close().?;

    let process = GetCurrentProcess();
    let mut before: u32 = 0;
    if GetProcessHandleCount(process, &mut before) == 0 { return process::ExitCode(6)!; }
    let mut iteration: usize = 0;
    while iteration < 64 {
        let mut partial = dir.entries(&mut buffer[..]).?;
        if partial.next() is ?result { _ = result.?; } else { return process::ExitCode(7)!; }
        partial.close().?;
        partial.close().?;
        if partial.next() is ?_ { return process::ExitCode(8)!; }
        iteration += 1;
    }
    let mut after: u32 = 0;
    if GetProcessHandleCount(process, &mut after) == 0 or before != after {
        return process::ExitCode(9)!;
    }
    dir.deleteFile(newPath).?;
    !()
}
"#
    .replace("$DIRECTORY_BYTES", &byte_literal(directory))
    .replace("$RENAMED_BYTES", &byte_literal(renamed))
    .replace("$RENAMED", renamed)
    .replace("$FILE", filename);
    let exe = emit(&root, "unicode", &source);
    let status = Command::new(exe)
        .current_dir(&working)
        .status_timeout("run Unicode paths and directory ownership regression");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read_dir(&working)
            .expect("read test directory")
            .count(),
        0
    );
}

#[test]
fn windows_spawn_limits_inheritance_and_releases_failed_attempts() {
    let root = temp_dir("windows_spawn_handle_ownership");
    // A per-run name keeps concurrent test processes from sharing the event.
    let event_name = format!(
        "Local\\nia-inherit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let name_units = event_name.encode_utf16().chain([0]).collect::<Vec<_>>();
    let name_literal = format!(
        "[{}]",
        name_units
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let name_len = name_units.len().to_string();
    let child = emit(
        &root,
        "child",
        &r#"
using std::process;
extern fn GetStdHandle(which: i32) usize;
extern fn ReadFile(handle: usize, buffer: &mut u8, size: u32, read: &mut u32, overlapped: usize) i32;
extern fn WriteFile(handle: usize, buffer: &u8, size: u32, written: &mut u32, overlapped: usize) i32;
extern fn OpenEventW(access: u32, inherit: i32, name: &u16) usize;
extern fn SetEvent(handle: usize) i32;
extern fn WaitForSingleObject(handle: usize, milliseconds: u32) u32;
extern fn GetLastError() u32;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let input = GetStdHandle(-10);
    let mut bytes: [u8; 8] = [0; 8];
    let mut count: u32 = 0;
    if ReadFile(input, &mut bytes[0], 8, &mut count, 0) == 0 or count != 8 {
        return process::ExitCode(1)!;
    }
    let mut handle: usize = 0;
    let mut index: usize = 0;
    while index < 8 {
        handle = handle | ((bytes[index] as usize) << (index * 8));
        index += 1;
    }
    // The parent's event is unsignaled. If the parent's handle value named it
    // here, signaling our own handle would change what that value observes; an
    // unrelated handle of this process cannot pass both checks.
    let name: [u16; $NAME_LEN] = $NAME;
    let own = OpenEventW(0x00100002, 0, &name[0]);
    if own == 0 { return process::ExitCode(2)!; }
    if handle != own and WaitForSingleObject(handle, 0) == 258 {
        if SetEvent(own) == 0 { return process::ExitCode(2)!; }
        if WaitForSingleObject(handle, 0) == 0 { return process::ExitCode(2)!; }
    }
    let mut extra: u8 = 0;
    let result = ReadFile(input, &mut extra, 1, &mut count, 0);
    if result == 0 {
        if GetLastError() != 109 { return process::ExitCode(3)!; }
    } else if count != 0 { return process::ExitCode(4)!; }
    if WriteFile(GetStdHandle(-11), &bytes[0], 8, &mut count, 0) == 0 or count != 8 {
        return process::ExitCode(5)!;
    }
    !()
}
"#
        .replace("$NAME_LEN", &name_len)
        .replace("$NAME", &name_literal),
    );
    let parent_source = r#"
using std;
using std::process;
using std::io;
using std::slice;
extern struct Security { length: u32, descriptor: usize, inherit: i32 }
extern fn CreateEventW(security: &mut Security, manual: i32, initial: i32, name: &u16) usize;
extern fn CloseHandle(handle: usize) i32;
extern fn GetCurrentProcess() usize;
extern fn GetProcessHandleCount(process: usize, count: &mut u32) i32;

pub fn main(init: process::Init) process::ExitCode!() {
    let mut security: Security = .{ length: std::builtin::size[Security]() as u32, descriptor: 0, inherit: 1 };
    // Manual reset and unsignaled, so the child can observe identity.
    let name: [u16; $NAME_LEN] = $NAME;
    let sentinel = CreateEventW(&mut security, 1, 0, &name[0]);
    if sentinel == 0 { return process::ExitCode(1)!; }
    defer _ = CloseHandle(sentinel);
    let process = GetCurrentProcess();
    // The first attempt loads process-lifetime state, and Windows loader
    // worker threads open and close a few handles asynchronously. That drift
    // is small and independent of the attempt count, whereas a leak of even
    // one handle per attempt exceeds the slack over this window.
    let mut before: u32 = 0;
    let mut iteration: usize = 0;
    while iteration < 257 {
        if iteration == 1 and GetProcessHandleCount(process, &mut before) == 0 {
            return process::ExitCode(2)!;
        }
        let command = process::Command::init(std::PathView::init(&"$MISSING"), init.env())
            .withStdin(.Pipe).withStdout(.Pipe).withStderr(.Pipe);
        let mut attempt = command.spawn();
        match attempt.finish() {
            !child => { _ = child; return process::ExitCode(3)!; },
            error! => {
                match error {
                    process::Error::Spawn(process::SpawnError::Exec(_)) => {},
                    _ => return process::ExitCode(4)!,
                }
            },
        }
        iteration += 1;
    }
    let mut after: u32 = 0;
    if GetProcessHandleCount(process, &mut after) == 0 or after > before + 32 {
        return process::ExitCode(5)!;
    }

    // The first successful child also loads one-time state. Sixteen more
    // children must not grow the handle count beyond the same bounded drift.
    let mut round: usize = 0;
    while round < 17 {
        let command = process::Command::init(std::PathView::init(&"$CHILD"), init.env())
            .withStdin(.Pipe).withStdout(.Pipe).withStderr(.Pipe);
        let mut attempt = command.spawn();
        let mut child = attempt.finish().?;
        let mut stdin = match child.takeStdin() { ?value => value, null => return process::ExitCode(6)! };
        let mut stdout = match child.takeStdout() { ?value => value, null => return process::ExitCode(7)! };
        let mut bytes: [u8; 8] = [0; 8];
        let mut index: usize = 0;
        while index < 8 { bytes[index] = (sentinel >> (index * 8)) as u8; index += 1; }
        stdin.writeAll(&bytes).?;
        stdin.close().?;
        let mut echoed: [u8; 8] = [0; 8];
        // A child that rejects its handle set exits before echoing; report
        // that directly instead of as an end-of-stream read failure.
        if stdout.readExact(&mut echoed[..]) is error! {
            _ = error;
            _ = stdout.close();
            _ = child.closeStderr();
            _ = child.wait();
            return process::ExitCode(13)!;
        }
        if not (&echoed[..]).equals(&bytes) { return process::ExitCode(8)!; }
        if stdout.read(&mut echoed[..]).? != 0 { return process::ExitCode(9)!; }
        stdout.close().?;
        child.closeStderr().?;
        if not child.wait().?.succeeded() { return process::ExitCode(10)!; }
        if round == 0 {
            if GetProcessHandleCount(process, &mut before) == 0 { return process::ExitCode(12)!; }
        }
        round += 1;
    }
    if GetProcessHandleCount(process, &mut after) == 0 or after > before + 8 {
        return process::ExitCode(11)!;
    }
    !()
}
"#
        .replace("$CHILD", &child.to_string_lossy().replace('\\', "\\\\"))
        .replace("$MISSING", &root.join("missing.exe").to_string_lossy().replace('\\', "\\\\"))
        .replace("$NAME_LEN", &name_len)
        .replace("$NAME", &name_literal);
    let parent = emit(&root, "parent", &parent_source);
    assert_eq!(
        Command::new(parent)
            .status_timeout("run Windows pipe and handle ownership regression")
            .code(),
        Some(0)
    );
}

#[test]
fn windows_metadata_preserves_pre_unix_epoch_timestamps() {
    use std::time::{Duration, UNIX_EPOCH};
    let root = temp_dir("windows_metadata_before_epoch");
    let file = std::fs::File::create(root.join("old.txt")).expect("create dated file");
    file.set_times(
        std::fs::FileTimes::new().set_modified(UNIX_EPOCH - Duration::from_millis(1250)),
    )
    .expect("set pre-epoch modification time");
    drop(file);
    let exe = emit(
        &root,
        "timestamp",
        r#"
using std::fs;
using std::process;
pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut dir = fs::Dir::cwd().?;
    defer dir.close().?;
    let metadata = dir.metadata(fs::RelativePathView::fromText(&"old.txt").?, fs::MetadataOptions::init()).?;
    let modified = metadata.modified();
    if modified.seconds() != -2 or modified.nanos() != 750000000 { return process::ExitCode(1)!; }
    !()
}
"#,
    );
    assert_eq!(
        Command::new(exe)
            .current_dir(&root)
            .status_timeout("run Windows pre-epoch timestamp regression")
            .code(),
        Some(0)
    );
}

#[test]
fn windows_name_surrogate_reparse_points_are_links_and_directories_are_not() {
    let root = temp_dir("windows_reparse_point_kinds");
    std::fs::create_dir(root.join("target")).expect("create junction target");
    let junction = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J", "link", "target"])
        .current_dir(&root)
        .output_timeout_for_runtime("create directory junction");
    assert!(
        junction.status.success(),
        "{}",
        String::from_utf8_lossy(&junction.stderr)
    );
    let exe = emit(
        &root,
        "kinds",
        r#"
using std::fs;
using std::process;
using std::slice;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut dir = fs::Dir::cwd().?;
    defer dir.close().?;
    let link = fs::RelativePathView::fromText(&"link").?;
    let target = fs::RelativePathView::fromText(&"target").?;
    if dir.metadata(link, fs::MetadataOptions::noFollow()).?.kind() != .Symlink {
        return process::ExitCode(1)!;
    }
    if dir.metadata(link, fs::MetadataOptions::init()).?.kind() != .Directory {
        return process::ExitCode(2)!;
    }
    if dir.metadata(target, fs::MetadataOptions::noFollow()).?.kind() != .Directory {
        return process::ExitCode(3)!;
    }
    let mut buffer: [u8; 256] = [0; 256];
    let mut entries = dir.entries(&mut buffer[..]).?;
    defer entries.close().?;
    let mut seen: usize = 0;
    loop {
        let result = match entries.next() { ?value => value, null => break };
        let entry = result.?;
        if entry.name().equals(&b"link") {
            if entry.kind() != .Symlink { return process::ExitCode(4)!; }
            seen += 1;
        } else if entry.name().equals(&b"target") {
            if entry.kind() != .Directory { return process::ExitCode(5)!; }
            seen += 1;
        }
    }
    if seen != 2 { return process::ExitCode(6)!; }
    !()
}
"#,
    );
    let status = Command::new(exe)
        .current_dir(&root)
        .status_timeout("run reparse point kind regression");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn windows_startup_and_spawn_accept_environments_beyond_fixed_buffers() {
    let root = temp_dir("windows_large_environment");
    let value = "\u{4e2d}".repeat(30000);
    let check = r#"
fn hasLargeEntry(init: process::Init) bool {
    let prefix = b"NIA_TEST_ENV_LARGE_3=";
    let mut iter = init.env().iter();
    loop {
        let entry = match iter.next() { ?value => value, null => break };
        let bytes = entry.bytes();
        if bytes.len() == prefix.len() + 90000 and (&bytes[0..prefix.len()]).equals(&prefix) {
            return bytes[bytes.len() - 3] == 0xe4;
        }
    }
    false
}
"#;
    let child = emit(
        &root,
        "child",
        &format!(
            r#"
using std::process;
using std::slice;
{check}
pub fn main(init: process::Init) process::ExitCode!() {{
    if not hasLargeEntry(init) {{ return process::ExitCode(1)!; }}
    !()
}}
"#
        ),
    );
    let parent = emit(
        &root,
        "parent",
        &format!(
            r#"
using std;
using std::process;
using std::slice;
{check}
pub fn main(init: process::Init) process::ExitCode!() {{
    if not hasLargeEntry(init) {{ return process::ExitCode(2)!; }}
    let command = process::Command::init(std::PathView::init(&"$CHILD"), init.env());
    let mut attempt = command.spawn();
    let mut child = attempt.finish().?;
    if not child.wait().?.succeeded() {{ return process::ExitCode(3)!; }}
    !()
}}
"#
        )
        .replace("$CHILD", &child.to_string_lossy().replace('\\', "\\\\")),
    );
    let mut command = Command::new(parent);
    for index in 0..4 {
        command.env(format!("NIA_TEST_ENV_LARGE_{index}"), &value);
    }
    let status = command.status_timeout("run large environment regression");
    assert_eq!(status.code(), Some(0));
}
