// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_fs_path_buf_builds_char_paths() {
    let root = temp_dir("emit_exe_std_fs_path_buf_builds_char_paths");
    let data_path = root.join("subdir").join("inside.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::io;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut page = mem::PageAllocator::init();
    let mut path = fs::PathBuf::fromView(&mut page, fs::PathView::init(&"subdir")).exit().?;
    defer path.deinit(&mut page).exit().?;

    path.joinComponent(&mut page, &"/inside.txt").exit().?;
    let expected: &[char] = &"subdir/inside.txt";
    if path.text().len() != expected.len() {
        return process::exit(1)!;
    }

    let mut cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    cwd.createDir(fs::RelativePathView::fromText(&"subdir").exit().?, fs::CreateDirOptions::init()).exit().?;
    let mut file = cwd.createFile(path.view().relative().exit().?, fs::CreateOptions::readWrite()).exit().?;
    let mut buffer: [16]u8 = [0; 16];
    let mut writer = file.writer(&mut buffer[..]).exit().?;
    writer.writeAll(&b"joined").exit().?;
    writer.flush().exit().?;
    file.close().exit().?;
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
        .output_timeout_for_build("run nia emit --exe fs path join");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted path join executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&data_path).expect("read joined file"),
        b"joined"
    );
}

#[test]
fn emit_exe_std_fs_getcwd_returns_path_slice() {
    let root = temp_dir("emit_exe_std_fs_getcwd_returns_path_slice");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut buffer: [4096]u8 = [0; 4096];
    let cwd = switch fs::getCwd(&mut buffer[..]) {
        !value => {
            value
        },
        error! => {
            return process::exit(1)!;
        },
    };
    if cwd.len() == 0usize {
        return process::exit(2)!;
    }
    if cwd[0] != b'/' {
        return process::exit(3)!;
    }
    if cwd[cwd.len() - 1usize] == 0u8 {
        return process::exit(4)!;
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
        .output_timeout_for_build("run nia emit --exe fs getcwd");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[cfg(unix)]
#[test]
fn emit_exe_std_fs_native_paths_preserve_non_utf8_bytes() {
    let root = temp_dir("emit_exe_std_fs_native_paths_preserve_non_utf8_bytes");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let missingTerminator: [1]u8 = [b'x'];
    switch fs::NativePathView::fromBytes(&missingTerminator[..]) {
        !value => { _ = value; return process::exit(1)!; },
        fs::PathError::MissingTerminator! => {},
        error! => { _ = error; return process::exit(2)!; },
    }
    let interiorNul: [3]u8 = [b'x', 0, 0];
    switch fs::NativePathView::fromBytes(&interiorNul[..]) {
        !value => { _ = value; return process::exit(3)!; },
        fs::PathError::ContainsNul! => {},
        error! => { _ = error; return process::exit(4)!; },
    }

    let nativeBytes: [13]u8 = [b'n', b'a', b't', b'i', b'v', b'e', b'-', 0xffu8, b'.', b'b', b'i', b'n', 0];
    let native = switch fs::NativePathView::fromBytes(&nativeBytes[..]) {
        !value => value,
        error! => { _ = error; return process::exit(5)!; },
    };
    if native.len() != 12usize or native.bytes()[7] != 0xffu8 {
        return process::exit(6)!;
    }
    let mut cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    let relative = native.relative().exit().?;
    let mut file = cwd.createNativeFile(relative, fs::CreateOptions::init()).exit().?;
    file.close().exit().?;
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
        .output_timeout_for_build("run nia emit --exe fs native path");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted native path executable")
            .code(),
        Some(0)
    );

    let native_name = std::ffi::OsStr::from_bytes(b"native-\xff.bin");
    assert!(root.join(native_name).is_file());
}

#[test]
fn emit_exe_std_fs_relative_paths_enforce_lexical_roots() {
    let root = temp_dir("emit_exe_std_fs_relative_paths_enforce_lexical_roots");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    switch fs::RelativePathView::fromText(&"/absolute") {
        !path => { _ = path; return process::exit(1)!; },
        fs::PathError::Absolute! => {},
        error! => { _ = error; return process::exit(2)!; },
    }
    switch fs::RelativePathView::fromText(&"../outside") {
        !path => { _ = path; return process::exit(3)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::exit(4)!; },
    }
    switch fs::PathView::init(&"a/../outside").relative() {
        !path => { _ = path; return process::exit(5)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::exit(6)!; },
    }
    switch fs::RelativePathView::fromText(&"a/..") {
        !path => { _ = path; return process::exit(7)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::exit(8)!; },
    }

    let absoluteBytes: [3]u8 = [b'/', b'x', 0];
    switch fs::RelativeNativePathView::fromBytes(&absoluteBytes[..]) {
        !path => { _ = path; return process::exit(9)!; },
        fs::PathError::Absolute! => {},
        error! => { _ = error; return process::exit(10)!; },
    }
    let parentBytes: [7]u8 = [b'a', b'/', b'.', b'.', b'/', b'x', 0];
    let native = switch fs::NativePathView::fromBytes(&parentBytes[..]) {
        !path => path,
        error! => { _ = error; return process::exit(11)!; },
    };
    switch native.relative() {
        !path => { _ = path; return process::exit(12)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::exit(13)!; },
    }

    let dot = fs::RelativePathView::fromText(&".").exit().?;
    let repeated = fs::RelativePathView::fromText(&"a//b").exit().?;
    let adjacent = fs::RelativePathView::fromText(&"a/..b/.../b").exit().?;
    let empty = fs::RelativePathView::fromText(&"").exit().?;
    if dot.text().len() != 1usize
        or repeated.text().len() != 4usize
        or adjacent.text().len() != 11usize
        or empty.text().len() != 0usize
    {
        return process::exit(14)!;
    }
    let adjacentBytes: [4]u8 = [b'.', b'.', b'x', 0];
    let adjacentNative = fs::RelativeNativePathView::fromBytes(&adjacentBytes[..]).exit().?;
    if adjacentNative.bytes().len() != 3usize {
        return process::exit(15)!;
    }

    let mut cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    let nested = fs::RelativePathView::fromText(&"nested").exit().?;
    cwd.createDir(nested, fs::CreateDirOptions::init()).exit().?;
    let child = fs::RelativePathView::fromText(&"nested/inside.txt").exit().?;
    let mut file = cwd.createFile(child, fs::CreateOptions::init()).exit().?;
    file.close().exit().?;
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
        .output_timeout_for_build("run nia emit --exe fs relative path roots");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted relative path roots executable")
            .code(),
        Some(0)
    );
    assert!(root.join("nested").join("inside.txt").is_file());
    assert!(!root.join("outside").exists());
}

#[test]
fn emit_exe_std_fs_scalar_paths_lower_with_dynamic_storage() {
    let root = temp_dir("emit_exe_std_fs_scalar_paths_lower_with_dynamic_storage");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let allocator: &mut mem::Allocator = &mut page;
    let mut longPath = fs::PathBuf::init();
    defer longPath.deinit(allocator).exit().?;
    let mut index: usize = 0;
    while index < 5000usize {
        longPath.push(allocator, 'a').exit().?;
        index += 1;
    }

    let mut cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    let longRelative = longPath.view().relative().exit().?;
    switch cwd.createFileWithAllocator(allocator, longRelative, fs::CreateOptions::init()) {
        !file => { _ = file; return process::exit(1)!; },
        fs::OperationError::System {
            operation: fs::Operation::CreateFile,
            cause: fs::Error::TooLong,
        }! => {},
        error! => { _ = error; return process::exit(2)!; },
    }

    let mut tinyStorage: [1]u8 = [0];
    let mut tiny = mem::FixedBufferAllocator::init(&mut tinyStorage[..]);
    let rejected = fs::RelativePathView::fromText(&"must-not-exist.txt").exit().?;
    switch cwd.createFileWithAllocator(&mut tiny, rejected, fs::CreateOptions::init()) {
        !file => { _ = file; return process::exit(3)!; },
        fs::OperationError::Allocation {
            operation: fs::Operation::CreateFile,
            cause: mem::Error::OutOfMemory,
        }! => {},
        error! => { _ = error; return process::exit(4)!; },
    }

    let mut fixedStorage: [128]u8 = [0; 128];
    let mut fixed = mem::FixedBufferAllocator::init(&mut fixedStorage[..]);
    let accepted = fs::RelativePathView::fromText(&"allocated.txt").exit().?;
    let mut file = cwd.createFileWithAllocator(
        &mut fixed,
        accepted,
        fs::CreateOptions::init(),
    ).exit().?;
    file.close().exit().?;
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
        .output_timeout_for_build("run nia emit --exe fs dynamic scalar paths");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted dynamic scalar path executable")
            .code(),
        Some(0)
    );
    assert!(root.join("allocated.txt").is_file());
    assert!(!root.join("must-not-exist.txt").exists());
}

#[cfg(unix)]
#[test]
fn emit_exe_std_fs_dir_rejects_symlink_escape_without_side_effects() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("emit_exe_std_fs_dir_rejects_symlink_escape_without_side_effects");
    let outside = temp_dir("emit_exe_std_fs_dir_symlink_escape_target");
    std::fs::write(outside.join("sentinel.txt"), b"outside").expect("write outside sentinel");
    symlink(&*outside, root.join("escape")).expect("create escape symlink");

    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    let createPath = fs::RelativePathView::fromText(&"escape/new-dir").exit().?;
    switch cwd.createDir(createPath, fs::CreateDirOptions::init()) {
        !ok => { _ = ok; return process::exit(4)!; },
        error! => { _ = error; },
    }
    let path = fs::RelativePathView::fromText(&"escape/sentinel.txt").exit().?;
    switch cwd.metadata(path, fs::MetadataOptions::init()) {
        !metadata => { _ = metadata; return process::exit(3)!; },
        error! => { _ = error; },
    }
    switch cwd.openFile(path, fs::OpenOptions::readOnly()) {
        !file => { return process::exit(1)!; },
        fs::OperationError::System {
            operation: fs::Operation::OpenFile,
            cause: fs::Error::CrossDevice,
        }! => {},
        error! => {
            _ = error;
            return process::exit(2)!;
        },
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
        .output_timeout_for_build("run nia emit --exe fs symlink containment");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted symlink containment executable")
            .code(),
        Some(0)
    );
    assert_eq!(
        std::fs::read(outside.join("sentinel.txt")).expect("read outside sentinel"),
        b"outside"
    );
    assert!(!outside.join("new-dir").exists());
}

#[cfg(unix)]
#[test]
fn emit_exe_std_fs_dir_allows_symlink_that_resolves_inside_root() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("emit_exe_std_fs_dir_allows_internal_symlink");
    std::fs::write(root.join("real.txt"), b"inside").expect("write target file");
    symlink("real.txt", root.join("alias.txt")).expect("create internal symlink");

    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    let path = fs::RelativePathView::fromText(&"alias.txt").exit().?;
    let mut file = cwd.openFile(path, fs::OpenOptions::readOnly()).exit().?;
    file.close().exit().?;
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
        .output_timeout_for_build("run nia emit --exe fs internal symlink");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted internal symlink executable")
            .code(),
        Some(0)
    );
}

#[cfg(unix)]
#[test]
fn emit_exe_std_fs_open_dir_no_follow_rejects_final_symlink() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("emit_exe_std_fs_open_dir_no_follow_symlink");
    std::fs::create_dir(root.join("real")).expect("create real directory");
    symlink("real", root.join("alias")).expect("create directory symlink");

    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    let path = fs::RelativePathView::fromText(&"alias").exit().?;
    switch cwd.openDir(path, fs::OpenDirOptions::noFollow()) {
        !dir => { return process::exit(1)!; },
        error! => { _ = error; },
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
        .output_timeout_for_build("run nia emit --exe fs no-follow symlink");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted no-follow symlink executable")
            .code(),
        Some(0)
    );
}

#[test]
fn emit_exe_can_create_open_read_and_write_std_fs_files() {
    let root = temp_dir("emit_exe_can_create_open_read_and_write_std_fs_files");
    let data_path = root.join("data.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut path = fs::RelativePathView::fromText(&"data.txt").exit().?;
    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(90)!;
        },
    }
    defer {
        switch cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    let mut file: fs::File;
    switch cwd.createFile(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(1)!;
        },
    }
    let mut write_buffer: [64]u8 = [0; 64];
    let mut writer = file.writer(&mut write_buffer[..]).exit().?;
    switch writer.writeAll(&b"nia fs") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(3)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(4)!;
        },
    }

    let mut opened: fs::File;
    switch cwd.openFile(path, fs::OpenOptions::readOnly()) {
        !value => {
            opened = value;
        },
        error! => {
            return process::exit(5)!;
        },
    }
    let mut read_buffer: [64]u8 = [0; 64];
    let mut reader = opened.reader(&mut read_buffer[..]).exit().?;
    let mut bytes: [6]u8 = [0, 0, 0, 0, 0, 0];
    switch reader.readExact(&mut bytes[..]) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(6)!;
        },
    }
    switch opened.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(7)!;
        },
    }
    let mut expected: &[u8] = &b"nia fs";
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return process::exit(8)!;
        }
        index += 1usize;
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&data_path).expect("read data file"),
        b"nia fs"
    );
}

#[test]
fn emit_exe_std_fs_file_open_create_and_close() {
    let root = temp_dir("emit_exe_std_fs_file_open_create_and_close");
    let data_path = root.join("data.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut path = fs::PathView::init(&"data.txt");
    let mut file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(1)!;
        },
    }
    let mut write_buffer: [16]u8 = [0; 16];
    let mut writer = file.writer(&mut write_buffer[..]).exit().?;
    switch writer.writeAll(&b"open close") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(3)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(4)!;
        },
    }

    let mut opened: fs::File;
    switch fs::File::open(path, fs::OpenOptions::readOnly()) {
        !value => {
            opened = value;
        },
        error! => {
            return process::exit(5)!;
        },
    }
    let mut read_buffer: [16]u8 = [0; 16];
    let mut reader = opened.reader(&mut read_buffer[..]).exit().?;
    let mut bytes: [10]u8 = [0; 10];
    switch reader.readExact(&mut bytes[..]) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(6)!;
        },
    }
    switch opened.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(7)!;
        },
    }
    let mut expected: &[u8] = &b"open close";
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return process::exit(8)!;
        }
        index += 1usize;
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&data_path).expect("read data file"),
        b"open close"
    );
}

#[test]
fn emit_exe_std_fs_file_close_marks_handle_closed() {
    let root = temp_dir("emit_exe_std_fs_file_close_marks_handle_closed");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut file = switch fs::File::create(fs::PathView::init(&"data.txt"), fs::CreateOptions::init()) {
        !value => {
            value
        },
        error! => {
            return process::exit(1)!;
        },
    };
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch file.len() {
        !len => {
            _ = len;
            return process::exit(3)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::exit(4)!;
            }
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
            return process::exit(5)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::exit(6)!;
            }
        },
    }
    let mut buffer: [8]u8 = [0; 8];
    switch file.writer(&mut buffer[..]) {
        !writer => {
            _ = writer;
            return process::exit(7)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::exit(8)!;
            }
        },
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
        .output_timeout_for_build("run nia emit --exe fs file closed state");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fs_dir_close_marks_handle_closed() {
    let root = temp_dir("emit_exe_std_fs_dir_close_marks_handle_closed");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd = switch fs::Dir::cwd() {
        !value => {
            value
        },
        error! => {
            return process::exit(1)!;
        },
    };
    switch cwd.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    let mut entryBuffer: [1]u8 = [0];
    switch cwd.entries(&mut entryBuffer[..]) {
        !entries => {
            _ = entries;
            return process::exit(3)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::exit(4)!;
            }
        },
    }
    switch cwd.close() {
        !ok => {
            _ = ok;
            return process::exit(5)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::exit(6)!;
            }
        },
    }
    switch cwd.createFile(fs::RelativePathView::fromText(&"bad.txt").exit().?, fs::CreateOptions::init()) {
        !file => {
            _ = file;
            return process::exit(7)!;
        },
        fs::OperationError::System {
            operation: fs::Operation::CreateFile,
            cause: fs::Error::BadFd,
        }! => {},
        error! => { _ = error; return process::exit(8)!; },
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
        .output_timeout_for_build("run nia emit --exe fs dir closed state");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fs_file_seek_len_truncate_and_sync() {
    let root = temp_dir("emit_exe_std_fs_file_seek_len_truncate_and_sync");
    let data_path = root.join("data.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut path = fs::PathView::init(&"data.txt");
    let mut file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(1)!;
        },
    }

    let mut write_buffer: [16]u8 = [0; 16];
    let mut writer = file.writer(&mut write_buffer[..]).exit().?;
    switch writer.writeAll(&b"abcdef") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(3)!;
        },
    }

    switch file.len() {
        !value => {
            if value != 6u64 {
                return process::exit(4)!;
            }
        },
        error! => {
            return process::exit(5)!;
        },
    }
    switch file.seekBy(0) {
        !value => {
            if value != 6u64 {
                return process::exit(6)!;
            }
        },
        error! => {
            return process::exit(7)!;
        },
    }
    switch file.seekTo(2u64) {
        !value => {
            if value != 2u64 {
                return process::exit(8)!;
            }
        },
        error! => {
            return process::exit(9)!;
        },
    }
    switch file.seekBy(1i64) {
        !value => {
            if value != 3u64 {
                return process::exit(10)!;
            }
        },
        error! => {
            return process::exit(11)!;
        },
    }
    switch file.seekFromEnd(-2i64) {
        !value => {
            if value != 4u64 {
                return process::exit(12)!;
            }
        },
        error! => {
            return process::exit(13)!;
        },
    }

    switch file.truncate(4u64) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(14)!;
        },
    }
    switch file.seekTo(9223372036854775808u64) {
        !value => {
            _ = value;
            return process::exit(20)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return process::exit(21)!;
            }
        },
    }
    switch file.truncate(9223372036854775808u64) {
        !ok => {
            _ = ok;
            return process::exit(22)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return process::exit(23)!;
            }
        },
    }
    switch file.len() {
        !value => {
            if value != 4u64 {
                return process::exit(15)!;
            }
        },
        error! => {
            return process::exit(16)!;
        },
    }
    switch file.syncData() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(17)!;
        },
    }
    switch file.sync() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(18)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(19)!;
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(std::fs::read(&data_path).expect("read data file"), b"abcd");
}

#[test]
fn emit_exe_std_fs_file_metadata() {
    let root = temp_dir("emit_exe_std_fs_file_metadata");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut path = fs::PathView::init(&"data.txt");
    let mut file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(1)!;
        },
    }

    let mut write_buffer: [16]u8 = [0; 16];
    let mut writer = file.writer(&mut write_buffer[..]).exit().?;
    switch writer.writeAll(&b"metadata") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(3)!;
        },
    }

    switch file.metadata() {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return process::exit(4)!;
            }
            if metadata.size() != 8u64 {
                return process::exit(5)!;
            }
            switch metadata.linkCount() {
                ?value => {
                    if value == 0u32 {
                        return process::exit(6)!;
                    }
                },
                null => {},
            }
            if metadata.preferredBlockSize() == 0u32 {
                return process::exit(7)!;
            }
        },
        error! => {
            return process::exit(8)!;
        },
    }

    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(9)!;
        },
    }
    switch cwd.metadata(path.relative().exit().?, fs::MetadataOptions::init()) {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return process::exit(10)!;
            }
            if metadata.size() != 8u64 {
                return process::exit(11)!;
            }
            switch metadata.accessed() {
                ?time => {
                    _ = time.seconds();
                    _ = time.nanos();
                },
                null => {},
            }
            _ = metadata.modified().seconds();
            switch metadata.statusChanged() {
                ?time => {
                    _ = time.nanos();
                },
                null => {},
            }
        },
        error! => {
            return process::exit(12)!;
        },
    }

    switch cwd.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(13)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(14)!;
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fs_file_set_permissions() {
    let root = temp_dir("emit_exe_std_fs_file_set_permissions");
    let data_path = root.join("data.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut path = fs::PathView::init(&"data.txt");
    let mut file = fs::File::create(path, fs::CreateOptions::init()).exit().?;
    defer file.close().exit().?;
    file.setPermissions(0o755).exit().?;
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
        .output_timeout_for_build("run nia emit --exe fs set permissions");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));

    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&data_path)
            .expect("data metadata")
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[test]
fn emit_exe_can_open_std_fs_paths_from_text() {
    let root = temp_dir("emit_exe_can_open_std_fs_paths_from_text");
    let data_path = root.join("nia-λ.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::io;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    let mut path = fs::RelativePathView::fromText(&"nia-λ.txt").exit().?;
    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(90)!;
        },
    }
    defer {
        switch cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    let mut file: fs::File;
    switch cwd.createFile(path, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(1)!;
        },
    }
    let mut buffer: [64]u8 = [0; 64];
    let mut writer = file.writer(&mut buffer[..]).exit().?;
    switch writer.writeAll(&b"ok") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(3)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(4)!;
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(std::fs::read(&data_path).expect("read data file"), b"ok");
}

#[test]
fn emit_exe_std_fs_path_from_utf8_preserves_decode_errors() {
    let root = temp_dir("emit_exe_std_fs_path_from_utf8_preserves_decode_errors");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::mem;
using std::process;
using std::string;
using std::unicode;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut page = mem::PageAllocator::init();

    let valid: [3]u8 = [b'A', 0xceu8, 0xbbu8];
    let mut path = switch fs::PathBuf::fromUtf8(&mut page, &valid) {
        !value => value,
        error! => { _ = error; return process::exit(1)!; },
    };
    defer path.deinit(&mut page).exit().?;
    if path.text().len() != 2
        or path.text()[0] != 'A'
        or path.text()[1].codepoint() != 0x03bbu32
    {
        return process::exit(2)!;
    }

    let mut encodedStorage: [4]u8 = [0; 4];
    let encoded = switch path.encode(&mut encodedStorage) {
        !value => value,
        error! => { _ = error; return process::exit(3)!; },
    };
    if encoded.len() != 3
        or encoded.bytes()[0] != b'A'
        or encoded.nulTerminatedBytes().len() != 4
        or encoded.nulTerminatedBytes()[3] != 0
    {
        return process::exit(4)!;
    }

    let invalid: [5]u8 = [b'o', b'k', 0xe2u8, 0x28u8, 0xa1u8];
    switch fs::PathBuf::fromUtf8(&mut page, &invalid) {
        !value => { _ = value; return process::exit(5)!; },
        string::TextError::InvalidUtf8(unicode::Utf8DecodeError::InvalidContinuation)! => {},
        error! => { _ = error; return process::exit(6)!; },
    }

    let mut invalidStorage: [16]u8 = [0; 16];
    switch fs::PathView::init(&"bad\0path").encode(&mut invalidStorage) {
        !value => { _ = value; return process::exit(7)!; },
        fs::PathError::ContainsNul! => {},
        error! => { _ = error; return process::exit(8)!; },
    }

    let mut shortStorage: [3]u8 = [0; 3];
    switch path.encode(&mut shortStorage) {
        !value => { _ = value; return process::exit(9)!; },
        fs::PathError::TooLong! => {},
        error! => { _ = error; return process::exit(10)!; },
    }

    let mut fixedStorage: [96]u8 = [0; 96];
    let mut fixed = mem::FixedBufferAllocator::init(&mut fixedStorage);
    let mut bounded = fs::PathBuf::init();
    bounded.append(&mut fixed, &"base").exit().?;
    defer bounded.deinit(&mut fixed).exit().?;
    switch bounded.joinComponent(&mut fixed, &"component-that-requires-growth") {
        !ok => { _ = ok; return process::exit(11)!; },
        mem::Error::OutOfMemory! => {},
        error! => { _ = error; return process::exit(12)!; },
    }
    if bounded.text().len() != 4
        or bounded.text()[0] != 'b'
        or bounded.text()[3] != 'e'
    {
        return process::exit(13)!;
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
        .output_timeout_for_build("run nia emit --exe fs UTF-8 path construction");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .status_timeout("run emitted UTF-8 path construction executable")
            .code(),
        Some(0)
    );
}

#[test]
fn emit_exe_std_fs_reports_invalid_and_missing_paths() {
    let root = temp_dir("emit_exe_std_fs_reports_invalid_and_missing_paths");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut path = fs::RelativePathView::fromText(&"bad\0path").exit().?;
    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(90)!;
        },
    }
    defer {
        switch cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    switch cwd.openFile(path, fs::OpenOptions::readOnly()) {
        !file => {
            _ = file;
            return process::exit(1)!;
        },
        fs::OperationError::Path {
            operation: fs::Operation::OpenFile,
            cause: fs::PathError::ContainsNul,
        }! => {},
        err! => { _ = err; return process::exit(2)!; },
    }
    let missing = fs::RelativePathView::fromText(&"definitely-missing.nia-test-file").exit().?;
    switch cwd.openFile(missing, fs::OpenOptions::readOnly()) {
        !value => {
            let mut file = value;
            file.close().exit().?;
            return process::exit(3)!;
        },
        fs::OperationError::System {
            operation: fs::Operation::OpenFile,
            cause: fs::Error::NotFound,
        }! => {},
        err! => { _ = err; return process::exit(4)!; },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fs_can_delete_files() {
    let root = temp_dir("emit_exe_std_fs_can_delete_files");
    let data_path = root.join("delete-me.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(90)!;
        },
    }
    defer {
        switch cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    let mut file: fs::File;
    switch cwd.createFile(fs::RelativePathView::fromText(&"delete-me.txt").exit().?, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(1)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch cwd.deleteFile(fs::RelativePathView::fromText(&"delete-me.txt").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(3)!;
        },
    }
    switch cwd.openFile(fs::RelativePathView::fromText(&"delete-me.txt").exit().?, fs::OpenOptions::readOnly()) {
        !file => {
            _ = file;
            return process::exit(4)!;
        },
        error! => {
        },
    }

    switch cwd.deleteFile(fs::RelativePathView::fromText(&"bad\0path").exit().?) {
        !ok => {
            _ = ok;
            return process::exit(5)!;
        },
        error! => {
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert!(!data_path.exists());
}

#[test]
fn emit_exe_std_fs_can_create_rename_and_delete_dirs() {
    let root = temp_dir("emit_exe_std_fs_can_create_rename_and_delete_dirs");
    let old_path = root.join("old-name.txt");
    let new_path = root.join("subdir").join("new-name.txt");
    let dir_path = root.join("subdir");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(90)!;
        },
    }
    defer {
        switch cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };

    switch cwd.createDir(fs::RelativePathView::fromText(&"subdir").exit().?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(1)!;
        },
    }

    let mut file: fs::File;
    switch cwd.createFile(fs::RelativePathView::fromText(&"old-name.txt").exit().?, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(2)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(3)!;
        },
    }

    switch cwd.rename(
        fs::RelativePathView::fromText(&"old-name.txt").exit().?,
        fs::RelativePathView::fromText(&"subdir/new-name.txt").exit().?,
    ) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(4)!;
        },
    }

    switch cwd.openFile(fs::RelativePathView::fromText(&"old-name.txt").exit().?, fs::OpenOptions::readOnly()) {
        !value => {
            _ = value;
            return process::exit(5)!;
        },
        error! => {
        },
    }

    switch cwd.openFile(fs::RelativePathView::fromText(&"subdir/new-name.txt").exit().?, fs::OpenOptions::readOnly()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(6)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(7)!;
        },
    }

    switch cwd.deleteDir(fs::RelativePathView::fromText(&"subdir").exit().?) {
        !ok => {
            _ = ok;
            return process::exit(8)!;
        },
        error! => {
        },
    }

    switch cwd.deleteFile(fs::RelativePathView::fromText(&"subdir/new-name.txt").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(9)!;
        },
    }
    switch cwd.deleteDir(fs::RelativePathView::fromText(&"subdir").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(10)!;
        },
    }

    switch cwd.createDir(fs::RelativePathView::fromText(&"bad\0path").exit().?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
            return process::exit(11)!;
        },
        error! => {
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert!(!old_path.exists());
    assert!(!new_path.exists());
    assert!(!dir_path.exists());
}

#[test]
fn emit_exe_std_fs_can_open_dirs_as_capabilities() {
    let root = temp_dir("emit_exe_std_fs_can_open_dirs_as_capabilities");
    let data_path = root.join("subdir").join("inside.txt");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(90)!;
        },
    }
    defer {
        switch cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    switch cwd.createDir(fs::RelativePathView::fromText(&"subdir").exit().?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(1)!;
        },
    }

    let mut subdir: fs::Dir;
    switch cwd.openDir(fs::RelativePathView::fromText(&"subdir").exit().?, fs::OpenDirOptions::init()) {
        !value => {
            subdir = value;
        },
        error! => {
            return process::exit(2)!;
        },
    }

    let mut file: fs::File;
    switch subdir.createFile(fs::RelativePathView::fromText(&"inside.txt").exit().?, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(3)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(4)!;
        },
    }

    switch subdir.openFile(fs::RelativePathView::fromText(&"inside.txt").exit().?, fs::OpenOptions::readOnly()) {
        !value => {
            file = value;
        },
        error! => {
            return process::exit(5)!;
        },
    }
    switch file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(6)!;
        },
    }

    switch subdir.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(7)!;
        },
    }

    switch cwd.openDir(fs::RelativePathView::fromText(&"subdir/inside.txt").exit().?, fs::OpenDirOptions::init()) {
        !value => {
            _ = value;
            return process::exit(8)!;
        },
        error! => {
        },
    }

    switch cwd.deleteFile(fs::RelativePathView::fromText(&"subdir/inside.txt").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(9)!;
        },
    }
    switch cwd.deleteDir(fs::RelativePathView::fromText(&"subdir").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(10)!;
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
    assert!(!data_path.exists());
}

#[test]
fn emit_exe_std_fs_can_iterate_dir_entries() {
    let root = temp_dir("emit_exe_std_fs_can_iterate_dir_entries");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;
using std::slice;

fn bytes_equal(left: &[u8], right: &[u8]) bool {
    left.equals(right)
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    let mut cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::exit(1)!;
        },
    }

    switch cwd.createDir(fs::RelativePathView::fromText(&"entries").exit().?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(2)!;
        },
    }

    let mut first: fs::File;
    switch cwd.createFile(fs::RelativePathView::fromText(&"entries/alpha.txt").exit().?, fs::CreateOptions::init()) {
        !value => {
            first = value;
        },
        error! => {
            return process::exit(3)!;
        },
    }
    switch first.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(4)!;
        },
    }

    let mut second: fs::File;
    switch cwd.createFile(fs::RelativePathView::fromText(&"entries/beta.txt").exit().?, fs::CreateOptions::init()) {
        !value => {
            second = value;
        },
        error! => {
            return process::exit(5)!;
        },
    }
    switch second.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(6)!;
        },
    }

    let mut dir: fs::Dir;
    switch cwd.openDir(fs::RelativePathView::fromText(&"entries").exit().?, fs::OpenDirOptions::init()) {
        !value => {
            dir = value;
        },
        error! => {
            return process::exit(7)!;
        },
    }

    let mut buffer: [1024]u8 = [0; 1024];
    let mut iter: fs::DirIterator;
    switch dir.entries(&mut buffer[..]) {
        !value => {
            iter = value;
        },
        error! => {
            return process::exit(8)!;
        },
    }

    let mut saw_alpha = false;
    let mut saw_beta = false;
    let mut count = 0usize;
    for result in iter {
        let value = switch result {
            !entry => {
                entry
            },
            error! => {
                return process::exit(10)!;
            },
        };
        if not value.isDot() and not value.isDotDot() {
            count += 1usize;
            if value.kind() != fs::FileKind::File and value.kind() != fs::FileKind::Unknown {
                return process::exit(9)!;
            }
            if bytes_equal(value.name(), &b"alpha.txt") {
                saw_alpha = true;
            } else if bytes_equal(value.name(), &b"beta.txt") {
                saw_beta = true;
            }
        }
    }

    if count != 2usize {
        return process::exit(11)!;
    }
    if not saw_alpha or not saw_beta {
        return process::exit(12)!;
    }

    switch dir.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(13)!;
        },
    }
    switch cwd.deleteFile(fs::RelativePathView::fromText(&"entries/alpha.txt").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(14)!;
        },
    }
    switch cwd.deleteFile(fs::RelativePathView::fromText(&"entries/beta.txt").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(15)!;
        },
    }
    switch cwd.deleteDir(fs::RelativePathView::fromText(&"entries").exit().?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(16)!;
        },
    }
    switch cwd.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::exit(17)!;
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
