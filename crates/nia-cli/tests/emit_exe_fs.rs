// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

#[test]
fn emit_exe_std_fs_owned_path_builds_char_paths() {
    let root = temp_dir("emit_exe_std_fs_owned_path_builds_char_paths");
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut page = mem::PageAllocator::init();
    let mut path = fs::Path::fromView(&mut page, fs::PathView::init(&"subdir")).?;
    defer path.deinit(&mut page).?;

    path.joinComponent(&mut page, &"/inside.txt").?;
    let expected: &[char] = &"subdir/inside.txt";
    if path.text().len() != expected.len() {
        return process::ExitCode(1)!;
    }

    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    cwd.createDir(fs::RelativePathView::fromText(&"subdir").?, fs::CreateDirOptions::init()).?;
    let mut file = cwd.createFile(path.view().relative().?, fs::CreateOptions::readWrite()).?;
    let mut buffer: [u8; 16] = [0; 16];
    let mut writer = file.writer(&mut buffer[..]).?;
    writer.writeAll(&b"joined").?;
    writer.flush().?;
    file.close().?;
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut buffer: [u8; 4096] = [0; 4096];
    let cwd = match fs::getCwd(&mut buffer[..]) {
        !value => {
            value
        },
        error! => {
            return process::ExitCode(1)!;
        },
    };
    if cwd.len() == 0usize {
        return process::ExitCode(2)!;
    }
    if cwd[cwd.len() - 1usize] == 0u8 {
        return process::ExitCode(3)!;
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let missingTerminator: [u8; 1] = [b'x'];
    match fs::NativePathView::fromBytes(&missingTerminator[..]) {
        !value => { _ = value; return process::ExitCode(1)!; },
        fs::PathError::MissingTerminator! => {},
        error! => { _ = error; return process::ExitCode(2)!; },
    }
    let interiorNul: [u8; 3] = [b'x', 0, 0];
    match fs::NativePathView::fromBytes(&interiorNul[..]) {
        !value => { _ = value; return process::ExitCode(3)!; },
        fs::PathError::ContainsNul! => {},
        error! => { _ = error; return process::ExitCode(4)!; },
    }

    let nativeBytes: [u8; 13] = [b'n', b'a', b't', b'i', b'v', b'e', b'-', 0xffu8, b'.', b'b', b'i', b'n', 0];
    let native = match fs::NativePathView::fromBytes(&nativeBytes[..]) {
        !value => value,
        error! => { _ = error; return process::ExitCode(5)!; },
    };
    if native.len() != 12usize or native.bytes()[7] != 0xffu8 {
        return process::ExitCode(6)!;
    }
    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    let relative = native.relative().?;
    let mut file = cwd.createNativeFile(relative, fs::CreateOptions::init()).?;
    file.close().?;
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    match fs::RelativePathView::fromText(&"/absolute") {
        !path => { _ = path; return process::ExitCode(1)!; },
        fs::PathError::Absolute! => {},
        error! => { _ = error; return process::ExitCode(2)!; },
    }
    match fs::RelativePathView::fromText(&"../outside") {
        !path => { _ = path; return process::ExitCode(3)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::ExitCode(4)!; },
    }
    match fs::PathView::init(&"a/../outside").relative() {
        !path => { _ = path; return process::ExitCode(5)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::ExitCode(6)!; },
    }
    match fs::RelativePathView::fromText(&"a/..") {
        !path => { _ = path; return process::ExitCode(7)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::ExitCode(8)!; },
    }

    let absoluteBytes: [u8; 3] = [b'/', b'x', 0];
    match fs::RelativeNativePathView::fromBytes(&absoluteBytes[..]) {
        !path => { _ = path; return process::ExitCode(9)!; },
        fs::PathError::Absolute! => {},
        error! => { _ = error; return process::ExitCode(10)!; },
    }
    let parentBytes: [u8; 7] = [b'a', b'/', b'.', b'.', b'/', b'x', 0];
    let native = match fs::NativePathView::fromBytes(&parentBytes[..]) {
        !path => path,
        error! => { _ = error; return process::ExitCode(11)!; },
    };
    match native.relative() {
        !path => { _ = path; return process::ExitCode(12)!; },
        fs::PathError::ParentTraversal! => {},
        error! => { _ = error; return process::ExitCode(13)!; },
    }

    let dot = fs::RelativePathView::fromText(&".").?;
    let repeated = fs::RelativePathView::fromText(&"a//b").?;
    let adjacent = fs::RelativePathView::fromText(&"a/..b/.../b").?;
    let empty = fs::RelativePathView::fromText(&"").?;
    if dot.text().len() != 1usize
        or repeated.text().len() != 4usize
        or adjacent.text().len() != 11usize
        or empty.text().len() != 0usize
    {
        return process::ExitCode(14)!;
    }
    let adjacentBytes: [u8; 4] = [b'.', b'.', b'x', 0];
    let adjacentNative = fs::RelativeNativePathView::fromBytes(&adjacentBytes[..]).?;
    if adjacentNative.bytes().len() != 3usize {
        return process::ExitCode(15)!;
    }

    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    let nested = fs::RelativePathView::fromText(&"nested").?;
    cwd.createDir(nested, fs::CreateDirOptions::init()).?;
    let child = fs::RelativePathView::fromText(&"nested/inside.txt").?;
    let mut file = cwd.createFile(child, fs::CreateOptions::init()).?;
    file.close().?;
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
fn emit_exe_std_fs_scalar_paths_encode_into_bounded_storage() {
    let root = temp_dir("emit_exe_std_fs_scalar_paths_encode_into_bounded_storage");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::mem;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();
    let allocator: &mut mem::Allocator = &mut page;
    let mut longPath = fs::Path::init();
    defer longPath.deinit(allocator).?;
    let mut index: usize = 0;
    while index < 4095usize {
        longPath.push(allocator, 'a').?;
        index += 1;
    }

    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    let syscallRelative = longPath.view().relative().?;
    match cwd.createFile(syscallRelative, fs::CreateOptions::init()) {
        !file => { _ = file; return process::ExitCode(1)!; },
        fs::OperationError::System {
            operation: fs::Operation::CreateFile,
            cause: fs::Error::TooLong,
        }! => {},
        error! => { _ = error; return process::ExitCode(2)!; },
    }

    longPath.push(allocator, 'a').?;
    let longRelative = longPath.view().relative().?;
    match cwd.createFile(longRelative, fs::CreateOptions::init()) {
        !file => { _ = file; return process::ExitCode(3)!; },
        fs::OperationError::Path {
            operation: fs::Operation::CreateFile,
            cause: fs::PathError::TooLong,
        }! => {},
        fs::OperationError::System {
            operation: fs::Operation::CreateFile,
            cause: fs::Error::TooLong,
        }! => {},
        error! => { _ = error; return process::ExitCode(4)!; },
    }

    let accepted = fs::RelativePathView::fromText(&"allocated.txt").?;
    let mut file = cwd.createFile(accepted, fs::CreateOptions::init()).?;
    file.close().?;
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
        .output_timeout_for_build("run nia emit --exe fs bounded scalar paths");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted bounded scalar path executable")
            .code(),
        Some(0)
    );
    assert!(root.join("allocated.txt").is_file());
}

#[test]
fn check_std_fs_allocator_path_entry_points_are_absent() {
    let root = temp_dir("check_std_fs_allocator_path_entry_points_are_absent");
    let main = root.join("main.nia");
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::mem;

fn probe(
    dir: &fs::Dir,
    allocator: &mut mem::Allocator,
    path: fs::RelativePathView,
    native: fs::RelativeNativePathView,
) () {
    _ = dir.openFileWithAllocator(allocator, path, fs::OpenOptions::init());
    _ = dir.createDirWithAllocator(allocator, path, fs::CreateDirOptions::init());
    _ = dir.deleteNativeFileWithAllocator(allocator, native);
    _ = dir.renameNativeToWithAllocator(allocator, native, dir, native);
    _ = fs::File::openWithAllocator(allocator, path.path(), fs::OpenOptions::init());
    _ = fs::File::createWithAllocator(allocator, path.path(), fs::CreateOptions::init());
}

fn main() () {}
"#,
    )
    .expect("write obsolete filesystem allocator API source");

    let output = support::nia_command()
        .arg("check")
        .arg(&main)
        .output_timeout_for_compiler("check obsolete filesystem allocator APIs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for name in [
        "openFileWithAllocator",
        "createDirWithAllocator",
        "deleteNativeFileWithAllocator",
        "renameNativeToWithAllocator",
        "openWithAllocator",
        "createWithAllocator",
    ] {
        assert!(
            stderr.contains(name),
            "missing diagnostic for {name}:\n{stderr}"
        );
    }
}

#[cfg(unix)]
#[test]
fn emit_exe_std_fs_dir_rejects_symlink_escape_without_side_effects() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("emit_exe_std_fs_dir_rejects_symlink_escape_without_side_effects");
    let outside = temp_dir("emit_exe_std_fs_dir_symlink_escape_target");
    std::fs::write(outside.join("sentinel.txt"), b"outside").expect("write outside sentinel");
    std::fs::create_dir(outside.join("sentinel-dir")).expect("create outside sentinel directory");
    std::fs::write(root.join("rename-source.txt"), b"source").expect("write rename source");
    symlink(&*outside, root.join("escape")).expect("create escape symlink");

    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    let createPath = fs::RelativePathView::fromText(&"escape/new-dir").?;
    match cwd.createDir(createPath, fs::CreateDirOptions::init()) {
        !ok => { _ = ok; return process::ExitCode(4)!; },
        error! => { _ = error; },
    }
    let deleteFilePath = fs::RelativePathView::fromText(&"escape/sentinel.txt").?;
    match cwd.deleteFile(deleteFilePath) {
        !ok => { _ = ok; return process::ExitCode(5)!; },
        error! => { _ = error; },
    }
    let deleteDirPath = fs::RelativePathView::fromText(&"escape/sentinel-dir").?;
    match cwd.deleteDir(deleteDirPath) {
        !ok => { _ = ok; return process::ExitCode(6)!; },
        error! => { _ = error; },
    }
    let localSource = fs::RelativePathView::fromText(&"rename-source.txt").?;
    let outsideDestination = fs::RelativePathView::fromText(&"escape/renamed.txt").?;
    match cwd.rename(localSource, outsideDestination) {
        !ok => { _ = ok; return process::ExitCode(7)!; },
        error! => { _ = error; },
    }
    let outsideSource = fs::RelativePathView::fromText(&"escape/sentinel.txt").?;
    let localDestination = fs::RelativePathView::fromText(&"stolen.txt").?;
    match cwd.rename(outsideSource, localDestination) {
        !ok => { _ = ok; return process::ExitCode(8)!; },
        error! => { _ = error; },
    }
    let path = fs::RelativePathView::fromText(&"escape/sentinel.txt").?;
    match cwd.metadata(path, fs::MetadataOptions::init()) {
        !metadata => { _ = metadata; return process::ExitCode(3)!; },
        error! => { _ = error; },
    }
    match cwd.openFile(path, fs::OpenOptions::readOnly()) {
        !file => { return process::ExitCode(1)!; },
        fs::OperationError::System {
            operation: fs::Operation::OpenFile,
            cause: fs::Error::CrossDevice,
        }! => {},
        error! => {
            _ = error;
            return process::ExitCode(2)!;
        },
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
    assert!(outside.join("sentinel-dir").is_dir());
    assert_eq!(
        std::fs::read(root.join("rename-source.txt")).expect("read rename source"),
        b"source"
    );
    assert!(!outside.join("renamed.txt").exists());
    assert!(!root.join("stolen.txt").exists());
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    let path = fs::RelativePathView::fromText(&"alias.txt").?;
    let mut file = cwd.openFile(path, fs::OpenOptions::readOnly()).?;
    file.close().?;
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

#[test]
fn emit_exe_std_fs_rename_to_resolves_both_directory_roots() {
    let root = temp_dir("emit_exe_std_fs_rename_to_resolves_both_directory_roots");
    std::fs::create_dir(root.join("source")).expect("create source directory");
    std::fs::create_dir(root.join("destination")).expect("create destination directory");
    std::fs::write(root.join("source/item.txt"), b"item").expect("write source item");

    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    let mut source = cwd.openDir(
        fs::RelativePathView::fromText(&"source").?,
        fs::OpenDirOptions::init(),
    ).?;
    defer source.close().?;
    let mut destination = cwd.openDir(
        fs::RelativePathView::fromText(&"destination").?,
        fs::OpenDirOptions::init(),
    ).?;
    defer destination.close().?;
    source.renameTo(
        fs::RelativePathView::fromText(&"item.txt").?,
        &destination,
        fs::RelativePathView::fromText(&"moved.txt").?,
    ).?;
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
        .output_timeout_for_build("run nia emit --exe fs cross-dir rename");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        Command::new(&exe)
            .current_dir(&root)
            .status_timeout("run emitted cross-dir rename executable")
            .code(),
        Some(0)
    );
    assert!(!root.join("source/item.txt").exists());
    assert_eq!(
        std::fs::read(root.join("destination/moved.txt")).expect("read moved item"),
        b"item"
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd = fs::Dir::cwd().?;
    defer cwd.close().?;
    let path = fs::RelativePathView::fromText(&"alias").?;
    match cwd.openDir(path, fs::OpenDirOptions::noFollow()) {
        !dir => { return process::ExitCode(1)!; },
        error! => { _ = error; },
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut path = fs::RelativePathView::fromText(&"data.txt").?;
    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(90)!;
        },
    }
    defer {
        match cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    let mut file: fs::File;
    match cwd.createFile(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }
    let mut write_buffer: [u8; 64] = [0; 64];
    let mut writer = file.writer(&mut write_buffer[..]).?;
    match writer.writeAll(&b"nia fs") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(4)!;
        },
    }

    let mut opened: fs::File;
    match cwd.openFile(path, fs::OpenOptions::readOnly()) {
        !value => {
            opened = value;
        },
        error! => {
            return process::ExitCode(5)!;
        },
    }
    let mut read_buffer: [u8; 64] = [0; 64];
    let mut reader = opened.reader(&mut read_buffer[..]).?;
    let mut bytes: [u8; 6] = [0, 0, 0, 0, 0, 0];
    match reader.readExact(&mut bytes[..]) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(6)!;
        },
    }
    match opened.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(7)!;
        },
    }
    let mut expected: &[u8] = &b"nia fs";
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return process::ExitCode(8)!;
        }
        index += 1usize;
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut path = fs::PathView::init(&"data.txt");
    let mut file: fs::File;
    match fs::File::create(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }
    let mut write_buffer: [u8; 16] = [0; 16];
    let mut writer = file.writer(&mut write_buffer[..]).?;
    match writer.writeAll(&b"open close") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(4)!;
        },
    }

    let mut opened: fs::File;
    match fs::File::open(path, fs::OpenOptions::readOnly()) {
        !value => {
            opened = value;
        },
        error! => {
            return process::ExitCode(5)!;
        },
    }
    let mut read_buffer: [u8; 16] = [0; 16];
    let mut reader = opened.reader(&mut read_buffer[..]).?;
    let mut bytes: [u8; 10] = [0; 10];
    match reader.readExact(&mut bytes[..]) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(6)!;
        },
    }
    match opened.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(7)!;
        },
    }
    let mut expected: &[u8] = &b"open close";
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return process::ExitCode(8)!;
        }
        index += 1usize;
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut file = match fs::File::create(fs::PathView::init(&"data.txt"), fs::CreateOptions::init()) {
        !value => {
            value
        },
        error! => {
            return process::ExitCode(1)!;
        },
    };
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match file.len() {
        !len => {
            _ = len;
            return process::ExitCode(3)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::ExitCode(4)!;
            }
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
            return process::ExitCode(5)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::ExitCode(6)!;
            }
        },
    }
    let mut buffer: [u8; 8] = [0; 8];
    match file.writer(&mut buffer[..]) {
        !writer => {
            _ = writer;
            return process::ExitCode(7)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::ExitCode(8)!;
            }
        },
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
fn emit_exe_std_fs_owner_adapters_reject_reused_descriptors() {
    let root = temp_dir("emit_exe_std_fs_owner_adapters_reject_reused_descriptors");
    let writer_path = root.join("writer-replacement.txt");
    let reader_path = root.join("reader-replacement.txt");
    std::fs::write(root.join("reader-original.txt"), b"a").expect("write original reader file");
    std::fs::write(&reader_path, b"b").expect("write replacement reader file");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut original = fs::File::create(
        fs::PathView::init(&"writer-original.txt"),
        fs::CreateOptions::init(),
    ).?;
    let mut writeBuffer: [u8; 8] = [0; 8];
    let mut writer = original.writer(&mut writeBuffer[..]).?;
    writer.writeAll(&b"old").?;
    original.close().?;

    let mut replacement = fs::File::create(
        fs::PathView::init(&"writer-replacement.txt"),
        fs::CreateOptions::init(),
    ).?;
    match writer.flush() {
        !ok => {
            _ = ok;
            return process::ExitCode(1)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::ExitCode(2)!;
            }
        },
    }
    if writer.len() != 3usize or not writer.buffered().equals(&b"old") {
        return process::ExitCode(3)!;
    }
    replacement.close().?;

    let mut readOriginal = fs::File::open(
        fs::PathView::init(&"reader-original.txt"),
        fs::OpenOptions::readOnly(),
    ).?;
    let mut readBuffer: [u8; 8] = [0; 8];
    let mut reader = readOriginal.reader(&mut readBuffer[..]).?;
    readOriginal.close().?;

    let mut readReplacement = fs::File::open(
        fs::PathView::init(&"reader-replacement.txt"),
        fs::OpenOptions::readOnly(),
    ).?;
    let mut byte: [u8; 1] = [0];
    match reader.read(&mut byte[..]) {
        !count => {
            _ = count;
            return process::ExitCode(4)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::ExitCode(5)!;
            }
        },
    }
    if reader.len() != 0usize {
        return process::ExitCode(6)!;
    }
    readReplacement.close().?;

    let mut cwd = fs::Dir::cwd().?;
    cwd.createDir(
        fs::RelativePathView::fromText(&"dir-original").?,
        fs::CreateDirOptions::init(),
    ).?;
    cwd.createDir(
        fs::RelativePathView::fromText(&"dir-replacement").?,
        fs::CreateDirOptions::init(),
    ).?;
    let mut dirOriginal = cwd.openDir(
        fs::RelativePathView::fromText(&"dir-original").?,
        fs::OpenDirOptions::init(),
    ).?;
    let mut entryBuffer: [u8; 256] = [0; 256];
    let mut entries = dirOriginal.entries(&mut entryBuffer[..]).?;
    dirOriginal.close().?;
    let mut dirReplacement = cwd.openDir(
        fs::RelativePathView::fromText(&"dir-replacement").?,
        fs::OpenDirOptions::init(),
    ).?;
    match entries.next() {
        ?result => {
            match result {
                !entry => {
                    _ = entry;
                    return process::ExitCode(7)!;
                },
                error! => {
                    if error != fs::Error::BadFd {
                        return process::ExitCode(8)!;
                    }
                },
            }
        },
        null => {
            return process::ExitCode(9)!;
        },
    }
    dirReplacement.close().?;
    cwd.deleteDir(fs::RelativePathView::fromText(&"dir-original").?).?;
    cwd.deleteDir(fs::RelativePathView::fromText(&"dir-replacement").?).?;
    cwd.close().?;
    !()
}
"#,
    )
    .expect("write file adapter descriptor identity source");

    let output = support::nia_command()
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout_for_build("run nia emit --exe file adapter descriptor identity");
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run file adapter descriptor identity executable");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        std::fs::read(&writer_path).expect("read writer replacement"),
        b""
    );
    assert_eq!(
        std::fs::read(&reader_path).expect("read reader replacement"),
        b"b"
    );
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd = match fs::Dir::cwd() {
        !value => {
            value
        },
        error! => {
            return process::ExitCode(1)!;
        },
    };
    match cwd.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    let mut entryBuffer: [u8; 1] = [0];
    match cwd.entries(&mut entryBuffer[..]) {
        !entries => {
            _ = entries;
            return process::ExitCode(3)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::ExitCode(4)!;
            }
        },
    }
    match cwd.close() {
        !ok => {
            _ = ok;
            return process::ExitCode(5)!;
        },
        error! => {
            if error != fs::Error::BadFd {
                return process::ExitCode(6)!;
            }
        },
    }
    match cwd.createFile(fs::RelativePathView::fromText(&"bad.txt").?, fs::CreateOptions::init()) {
        !file => {
            _ = file;
            return process::ExitCode(7)!;
        },
        fs::OperationError::System {
            operation: fs::Operation::CreateFile,
            cause: fs::Error::BadFd,
        }! => {},
        error! => { _ = error; return process::ExitCode(8)!; },
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut path = fs::PathView::init(&"data.txt");
    let mut file: fs::File;
    match fs::File::create(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }

    let mut write_buffer: [u8; 16] = [0; 16];
    let mut writer = file.writer(&mut write_buffer[..]).?;
    match writer.writeAll(&b"abcdef") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }

    match file.len() {
        !value => {
            if value != 6u64 {
                return process::ExitCode(4)!;
            }
        },
        error! => {
            return process::ExitCode(5)!;
        },
    }
    match file.seekBy(0) {
        !value => {
            if value != 6u64 {
                return process::ExitCode(6)!;
            }
        },
        error! => {
            return process::ExitCode(7)!;
        },
    }
    match file.seekTo(2u64) {
        !value => {
            if value != 2u64 {
                return process::ExitCode(8)!;
            }
        },
        error! => {
            return process::ExitCode(9)!;
        },
    }
    match file.seekBy(1i64) {
        !value => {
            if value != 3u64 {
                return process::ExitCode(10)!;
            }
        },
        error! => {
            return process::ExitCode(11)!;
        },
    }
    match file.seekFromEnd(-2i64) {
        !value => {
            if value != 4u64 {
                return process::ExitCode(12)!;
            }
        },
        error! => {
            return process::ExitCode(13)!;
        },
    }

    match file.truncate(4u64) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(14)!;
        },
    }
    match file.seekTo(9223372036854775808u64) {
        !value => {
            _ = value;
            return process::ExitCode(20)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return process::ExitCode(21)!;
            }
        },
    }
    match file.truncate(9223372036854775808u64) {
        !ok => {
            _ = ok;
            return process::ExitCode(22)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return process::ExitCode(23)!;
            }
        },
    }
    match file.len() {
        !value => {
            if value != 4u64 {
                return process::ExitCode(15)!;
            }
        },
        error! => {
            return process::ExitCode(16)!;
        },
    }
    match file.syncData() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(17)!;
        },
    }
    match file.sync() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(18)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(19)!;
        },
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut path = fs::PathView::init(&"data.txt");
    let mut file: fs::File;
    match fs::File::create(path, fs::CreateOptions::readWrite()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }

    let mut write_buffer: [u8; 16] = [0; 16];
    let mut writer = file.writer(&mut write_buffer[..]).?;
    match writer.writeAll(&b"metadata") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }

    match file.metadata() {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return process::ExitCode(4)!;
            }
            if metadata.size() != 8u64 {
                return process::ExitCode(5)!;
            }
            match metadata.linkCount() {
                ?value => {
                    if value == 0u32 {
                        return process::ExitCode(6)!;
                    }
                },
                null => {},
            }
            if metadata.preferredBlockSize() == 0u32 {
                return process::ExitCode(7)!;
            }
        },
        error! => {
            return process::ExitCode(8)!;
        },
    }

    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(9)!;
        },
    }
    match cwd.metadata(path.relative().?, fs::MetadataOptions::init()) {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return process::ExitCode(10)!;
            }
            if metadata.size() != 8u64 {
                return process::ExitCode(11)!;
            }
            match metadata.accessed() {
                ?time => {
                    _ = time.seconds();
                    _ = time.nanos();
                },
                null => {},
            }
            _ = metadata.modified().seconds();
            match metadata.statusChanged() {
                ?time => {
                    _ = time.nanos();
                },
                null => {},
            }
        },
        error! => {
            return process::ExitCode(12)!;
        },
    }

    match cwd.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(13)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(14)!;
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn emit_exe_std_fs_file_set_permissions() {
    let root = temp_dir("emit_exe_std_fs_file_set_permissions");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut path = fs::PathView::init(&"data.txt");
    let mut file = fs::File::create(path, fs::CreateOptions::init()).?;
    defer file.close().?;
    file.setPermissions(0o755).?;
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

pub fn main(init: process::Init) process::ExitCode!() {
    let mut path = fs::RelativePathView::fromText(&"nia-λ.txt").?;
    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(90)!;
        },
    }
    defer {
        match cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    let mut file: fs::File;
    match cwd.createFile(path, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }
    let mut buffer: [u8; 64] = [0; 64];
    let mut writer = file.writer(&mut buffer[..]).?;
    match writer.writeAll(&b"ok") {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match writer.flush() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(4)!;
        },
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut page = mem::PageAllocator::init();

    let valid: [u8; 3] = [b'A', 0xceu8, 0xbbu8];
    let mut path = match fs::Path::fromUtf8(&mut page, &valid) {
        !value => value,
        error! => { _ = error; return process::ExitCode(1)!; },
    };
    defer path.deinit(&mut page).?;
    if path.text().len() != 2
        or path.text()[0] != 'A'
        or path.text()[1].codepoint() != 0x03bbu32
    {
        return process::ExitCode(2)!;
    }

    let mut encodedStorage: [u8; 4] = [0; 4];
    let encoded = match path.encode(&mut encodedStorage) {
        !value => value,
        error! => { _ = error; return process::ExitCode(3)!; },
    };
    if encoded.len() != 3
        or encoded.bytes()[0] != b'A'
        or encoded.nulTerminatedBytes().len() != 4
        or encoded.nulTerminatedBytes()[3] != 0
    {
        return process::ExitCode(4)!;
    }

    let invalid: [u8; 5] = [b'o', b'k', 0xe2u8, 0x28u8, 0xa1u8];
    match fs::Path::fromUtf8(&mut page, &invalid) {
        !value => { _ = value; return process::ExitCode(5)!; },
        string::TextError::InvalidUtf8(unicode::Utf8DecodeError::InvalidContinuation)! => {},
        error! => { _ = error; return process::ExitCode(6)!; },
    }

    let mut invalidStorage: [u8; 16] = [0xa5u8; 16];
    match fs::PathView::init(&"bad\0path").encode(&mut invalidStorage) {
        !value => { _ = value; return process::ExitCode(7)!; },
        fs::PathError::ContainsNul! => {},
        error! => { _ = error; return process::ExitCode(8)!; },
    }
    let mut invalidIndex = 0usize;
    while invalidIndex < invalidStorage.len() {
        if invalidStorage[invalidIndex] != 0xa5u8 {
            return process::ExitCode(16)!;
        }
        invalidIndex += 1usize;
    }

    let mut shortStorage: [u8; 3] = [0x5au8; 3];
    match path.encode(&mut shortStorage) {
        !value => { _ = value; return process::ExitCode(9)!; },
        fs::PathError::TooLong! => {},
        error! => { _ = error; return process::ExitCode(10)!; },
    }
    let mut shortIndex = 0usize;
    while shortIndex < shortStorage.len() {
        if shortStorage[shortIndex] != 0x5au8 {
            return process::ExitCode(17)!;
        }
        shortIndex += 1usize;
    }

    let mut fixedStorage: [u8; 96] = [0; 96];
    let mut fixed = mem::FixedBufferAllocator::init(&mut fixedStorage);
    let mut bounded = fs::Path::init();
    bounded.append(&mut fixed, &"base").?;
    defer bounded.deinit(&mut fixed).?;
    match bounded.joinComponent(&mut fixed, &"component-that-requires-growth") {
        !ok => { _ = ok; return process::ExitCode(11)!; },
        mem::Error::OutOfMemory! => {},
        error! => { _ = error; return process::ExitCode(12)!; },
    }
    if bounded.text().len() != 4
        or bounded.text()[0] != 'b'
        or bounded.text()[3] != 'e'
    {
        return process::ExitCode(13)!;
    }

    let mut aliased = fs::Path::fromView(&mut page, fs::PathView::init(&"base")).?;
    defer aliased.deinit(&mut page).?;
    let aliasedComponent = aliased.text();
    aliased.joinComponent(&mut page, aliasedComponent).?;
    let aliasedExpected: &[char] = &"base/base";
    if aliased.text().len() != aliasedExpected.len() {
        return process::ExitCode(14)!;
    }
    let mut aliasIndex = 0usize;
    while aliasIndex < aliasedExpected.len() {
        if aliased.text()[aliasIndex] != aliasedExpected[aliasIndex] {
            return process::ExitCode(15)!;
        }
        aliasIndex += 1usize;
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut path = fs::RelativePathView::fromText(&"bad\0path").?;
    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(90)!;
        },
    }
    defer {
        match cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    match cwd.openFile(path, fs::OpenOptions::readOnly()) {
        !file => {
            _ = file;
            return process::ExitCode(1)!;
        },
        fs::OperationError::Path {
            operation: fs::Operation::OpenFile,
            cause: fs::PathError::ContainsNul,
        }! => {},
        err! => { _ = err; return process::ExitCode(2)!; },
    }
    let missing = fs::RelativePathView::fromText(&"definitely-missing.nia-test-file").?;
    match cwd.openFile(missing, fs::OpenOptions::readOnly()) {
        !value => {
            let mut file = value;
            file.close().?;
            return process::ExitCode(3)!;
        },
        fs::OperationError::System {
            operation: fs::Operation::OpenFile,
            cause: fs::Error::NotFound,
        }! => {},
        err! => { _ = err; return process::ExitCode(4)!; },
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(90)!;
        },
    }
    defer {
        match cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    let mut file: fs::File;
    match cwd.createFile(fs::RelativePathView::fromText(&"delete-me.txt").?, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match cwd.deleteFile(fs::RelativePathView::fromText(&"delete-me.txt").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }
    match cwd.openFile(fs::RelativePathView::fromText(&"delete-me.txt").?, fs::OpenOptions::readOnly()) {
        !file => {
            _ = file;
            return process::ExitCode(4)!;
        },
        error! => {
        },
    }

    match cwd.deleteFile(fs::RelativePathView::fromText(&"bad\0path").?) {
        !ok => {
            _ = ok;
            return process::ExitCode(5)!;
        },
        error! => {
        },
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(90)!;
        },
    }
    defer {
        match cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };

    match cwd.createDir(fs::RelativePathView::fromText(&"subdir").?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }

    let mut file: fs::File;
    match cwd.createFile(fs::RelativePathView::fromText(&"old-name.txt").?, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }

    match cwd.rename(
        fs::RelativePathView::fromText(&"old-name.txt").?,
        fs::RelativePathView::fromText(&"subdir/new-name.txt").?,
    ) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(4)!;
        },
    }

    match cwd.openFile(fs::RelativePathView::fromText(&"old-name.txt").?, fs::OpenOptions::readOnly()) {
        !value => {
            _ = value;
            return process::ExitCode(5)!;
        },
        error! => {
        },
    }

    match cwd.openFile(fs::RelativePathView::fromText(&"subdir/new-name.txt").?, fs::OpenOptions::readOnly()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(6)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(7)!;
        },
    }

    match cwd.deleteDir(fs::RelativePathView::fromText(&"subdir").?) {
        !ok => {
            _ = ok;
            return process::ExitCode(8)!;
        },
        error! => {
        },
    }

    match cwd.deleteFile(fs::RelativePathView::fromText(&"subdir/new-name.txt").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(9)!;
        },
    }
    match cwd.deleteDir(fs::RelativePathView::fromText(&"subdir").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(10)!;
        },
    }

    match cwd.createDir(fs::RelativePathView::fromText(&"bad\0path").?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
            return process::ExitCode(11)!;
        },
        error! => {
        },
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(90)!;
        },
    }
    defer {
        match cwd.close() {
            !ok => {
                _ = ok;
            },
            error! => {},
        }
    };
    match cwd.createDir(fs::RelativePathView::fromText(&"subdir").?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }

    let mut subdir: fs::Dir;
    match cwd.openDir(fs::RelativePathView::fromText(&"subdir").?, fs::OpenDirOptions::init()) {
        !value => {
            subdir = value;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }

    let mut file: fs::File;
    match subdir.createFile(fs::RelativePathView::fromText(&"inside.txt").?, fs::CreateOptions::init()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(4)!;
        },
    }

    match subdir.openFile(fs::RelativePathView::fromText(&"inside.txt").?, fs::OpenOptions::readOnly()) {
        !value => {
            file = value;
        },
        error! => {
            return process::ExitCode(5)!;
        },
    }
    match file.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(6)!;
        },
    }

    match subdir.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(7)!;
        },
    }

    match cwd.openDir(fs::RelativePathView::fromText(&"subdir/inside.txt").?, fs::OpenDirOptions::init()) {
        !value => {
            _ = value;
            return process::ExitCode(8)!;
        },
        error! => {
        },
    }

    match cwd.deleteFile(fs::RelativePathView::fromText(&"subdir/inside.txt").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(9)!;
        },
    }
    match cwd.deleteDir(fs::RelativePathView::fromText(&"subdir").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(10)!;
        },
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

pub fn main(init: process::Init) process::ExitCode!() {
    _ = init;
    let mut cwd: fs::Dir;
    match fs::Dir::cwd() {
        !value => {
            cwd = value;
        },
        error! => {
            return process::ExitCode(1)!;
        },
    }

    match cwd.createDir(fs::RelativePathView::fromText(&"entries").?, fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(2)!;
        },
    }

    let mut first: fs::File;
    match cwd.createFile(fs::RelativePathView::fromText(&"entries/alpha.txt").?, fs::CreateOptions::init()) {
        !value => {
            first = value;
        },
        error! => {
            return process::ExitCode(3)!;
        },
    }
    match first.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(4)!;
        },
    }

    let mut second: fs::File;
    match cwd.createFile(fs::RelativePathView::fromText(&"entries/beta.txt").?, fs::CreateOptions::init()) {
        !value => {
            second = value;
        },
        error! => {
            return process::ExitCode(5)!;
        },
    }
    match second.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(6)!;
        },
    }

    let mut dir: fs::Dir;
    match cwd.openDir(fs::RelativePathView::fromText(&"entries").?, fs::OpenDirOptions::init()) {
        !value => {
            dir = value;
        },
        error! => {
            return process::ExitCode(7)!;
        },
    }

    let mut buffer: [u8; 1024] = [0; 1024];
    let mut iter: fs::DirIterator;
    match dir.entries(&mut buffer[..]) {
        !value => {
            iter = value;
        },
        error! => {
            return process::ExitCode(8)!;
        },
    }

    let mut saw_alpha = false;
    let mut saw_beta = false;
    let mut count = 0usize;
    for result in iter {
        let value = match result {
            !entry => {
                entry
            },
            error! => {
                return process::ExitCode(10)!;
            },
        };
        if not value.isDot() and not value.isDotDot() {
            count += 1usize;
            if value.kind() != fs::FileKind::File and value.kind() != fs::FileKind::Unknown {
                return process::ExitCode(9)!;
            }
            if bytes_equal(value.name(), &b"alpha.txt") {
                saw_alpha = true;
            } else if bytes_equal(value.name(), &b"beta.txt") {
                saw_beta = true;
            }
        }
    }

    if count != 2usize {
        return process::ExitCode(11)!;
    }
    if not saw_alpha or not saw_beta {
        return process::ExitCode(12)!;
    }

    match dir.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(13)!;
        },
    }
    match cwd.deleteFile(fs::RelativePathView::fromText(&"entries/alpha.txt").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(14)!;
        },
    }
    match cwd.deleteFile(fs::RelativePathView::fromText(&"entries/beta.txt").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(15)!;
        },
    }
    match cwd.deleteDir(fs::RelativePathView::fromText(&"entries").?) {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(16)!;
        },
    }
    match cwd.close() {
        !ok => {
            _ = ok;
        },
        error! => {
            return process::ExitCode(17)!;
        },
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

    let status = Command::new(&exe)
        .current_dir(&root)
        .status_timeout("run emitted executable");
    assert_eq!(status.code(), Some(0));
}
