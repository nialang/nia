// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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
    var page = mem::PageAllocator::init();
    var path = fs::PathBuf::from_path(&mut page, fs::PathView::init("subdir")).exit().?;
    defer path.deinit(&mut page).exit().?;

    path.join_component(&mut page, "/inside.txt").exit().?;
    let expected: &[char] = "subdir/inside.txt";
    if path.text().len() != expected.len() {
        return process::exit(1)!;
    }

    var cwd = fs::Dir::cwd().exit().?;
    defer cwd.close().exit().?;
    cwd.create_dir(fs::PathView::init("subdir"), fs::CreateDirOptions::init()).exit().?;
    var file = cwd.create_file(path.view(), fs::CreateOptions::read_write()).exit().?;
    var buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut buffer[..]).exit().?;
    writer.write_all(b"joined").exit().?;
    writer.flush().exit().?;
    file.close().exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe fs path join");

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
    var buffer: [4096]u8 = [0; 4096];
    let cwd = if let !value = fs::getcwd(&mut buffer[..]) {
        value
    } else error! {
        return (1 as process::ExitCode)!;
    };
    if cwd.len() == 0usize {
        return (2 as process::ExitCode)!;
    }
    if cwd[0] != b'/' {
        return (3 as process::ExitCode)!;
    }
    if cwd[cwd.len() - 1usize] == 0u8 {
        return (4 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe fs getcwd");

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
    var path = fs::PathView::init("data.txt");
    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (90 as process::ExitCode)!;
    }
    defer {
        if let !ok = cwd.close() {
            _ = ok;
        } else error! {}
    };
    var file: fs::File;
    if let !value = cwd.create_file(path, fs::CreateOptions::read_write()) {
        file = value;
    } else error! {
        return (1 as process::ExitCode)!;
    }
    var write_buffer: [64]u8 = [0; 64];
    var writer = file.writer(init.io(), &mut write_buffer[..]).exit().?;
    if let !ok = writer.write_all(b"nia fs") {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !ok = writer.flush() {
        _ = ok;
    } else error! {
        return (3 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (4 as process::ExitCode)!;
    }

    var opened: fs::File;
    if let !value = cwd.open_file(path, fs::OpenOptions::read_only()) {
        opened = value;
    } else error! {
        return (5 as process::ExitCode)!;
    }
    var read_buffer: [64]u8 = [0; 64];
    var reader = opened.reader(init.io(), &mut read_buffer[..]).exit().?;
    var bytes: [6]u8 = [0, 0, 0, 0, 0, 0];
    if let !ok = reader.read_exact(&mut bytes[..]) {
        _ = ok;
    } else error! {
        return (6 as process::ExitCode)!;
    }
    if let !ok = opened.close() {
        _ = ok;
    } else error! {
        return (7 as process::ExitCode)!;
    }
    var expected: &[u8] = b"nia fs";
    var index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return (8 as process::ExitCode)!;
        }
        index += 1usize;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
    var path = fs::PathView::init("data.txt");
    var file: fs::File;
    if let !value = fs::File::create(path, fs::CreateOptions::read_write()) {
        file = value;
    } else error! {
        return (1 as process::ExitCode)!;
    }
    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]).exit().?;
    if let !ok = writer.write_all(b"open close") {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !ok = writer.flush() {
        _ = ok;
    } else error! {
        return (3 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (4 as process::ExitCode)!;
    }

    var opened: fs::File;
    if let !value = fs::File::open(path, fs::OpenOptions::read_only()) {
        opened = value;
    } else error! {
        return (5 as process::ExitCode)!;
    }
    var read_buffer: [16]u8 = [0; 16];
    var reader = opened.reader(init.io(), &mut read_buffer[..]).exit().?;
    var bytes: [10]u8 = [0; 10];
    if let !ok = reader.read_exact(&mut bytes[..]) {
        _ = ok;
    } else error! {
        return (6 as process::ExitCode)!;
    }
    if let !ok = opened.close() {
        _ = ok;
    } else error! {
        return (7 as process::ExitCode)!;
    }
    var expected: &[u8] = b"open close";
    var index = 0usize;
    while index < bytes.len() {
        if bytes[index] != expected[index] {
            return (8 as process::ExitCode)!;
        }
        index += 1usize;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    var file = if let !value = fs::File::create(fs::PathView::init("data.txt"), fs::CreateOptions::init()) {
        value
    } else error! {
        return (1 as process::ExitCode)!;
    };
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !handle = file.borrow_handle() {
        _ = handle;
        return (3 as process::ExitCode)!;
    } else error! {
        if error != fs::Error::BadFd {
            return (4 as process::ExitCode)!;
        }
    }
    if let !ok = file.close() {
        _ = ok;
        return (5 as process::ExitCode)!;
    } else error! {
        if error != fs::Error::BadFd {
            return (6 as process::ExitCode)!;
        }
    }
    var buffer: [8]u8 = [0; 8];
    if let !writer = file.writer(init.io(), &mut buffer[..]) {
        _ = writer;
        return (7 as process::ExitCode)!;
    } else error! {
        if error != fs::Error::BadFd {
            return (8 as process::ExitCode)!;
        }
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe fs file closed state");
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
    var cwd = if let !value = fs::Dir::cwd() {
        value
    } else error! {
        return (1 as process::ExitCode)!;
    };
    if let !ok = cwd.close() {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !handle = cwd.borrow_handle() {
        _ = handle;
        return (3 as process::ExitCode)!;
    } else error! {
        if error != fs::Error::BadFd {
            return (4 as process::ExitCode)!;
        }
    }
    if let !ok = cwd.close() {
        _ = ok;
        return (5 as process::ExitCode)!;
    } else error! {
        if error != fs::Error::BadFd {
            return (6 as process::ExitCode)!;
        }
    }
    if let !file = cwd.create_file(fs::PathView::init("bad.txt"), fs::CreateOptions::init()) {
        _ = file;
        return (7 as process::ExitCode)!;
    } else error! {
        if error != fs::Error::BadFd {
            return (8 as process::ExitCode)!;
        }
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe fs dir closed state");
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
    var path = fs::PathView::init("data.txt");
    var file: fs::File;
    if let !value = fs::File::create(path, fs::CreateOptions::read_write()) {
        file = value;
    } else error! {
        return (1 as process::ExitCode)!;
    }

    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]).exit().?;
    if let !ok = writer.write_all(b"abcdef") {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !ok = writer.flush() {
        _ = ok;
    } else error! {
        return (3 as process::ExitCode)!;
    }

    if let !value = file.len() {
        if value != 6u64 {
            return (4 as process::ExitCode)!;
        }
    } else error! {
        return (5 as process::ExitCode)!;
    }
    if let !value = file.seek_by(0) {
        if value != 6u64 {
            return (6 as process::ExitCode)!;
        }
    } else error! {
        return (7 as process::ExitCode)!;
    }
    if let !value = file.seek_to(2u64) {
        if value != 2u64 {
            return (8 as process::ExitCode)!;
        }
    } else error! {
        return (9 as process::ExitCode)!;
    }
    if let !value = file.seek_by(1i64) {
        if value != 3u64 {
            return (10 as process::ExitCode)!;
        }
    } else error! {
        return (11 as process::ExitCode)!;
    }
    if let !value = file.seek_from_end(-2i64) {
        if value != 4u64 {
            return (12 as process::ExitCode)!;
        }
    } else error! {
        return (13 as process::ExitCode)!;
    }

    if let !ok = file.truncate(4u64) {
        _ = ok;
    } else error! {
        return (14 as process::ExitCode)!;
    }
    if let !value = file.seek_to(9223372036854775808u64) {
        _ = value;
        return (20 as process::ExitCode)!;
    } else err! {
        if err != fs::Error::OutOfRange {
            return (21 as process::ExitCode)!;
        }
    }
    if let !ok = file.truncate(9223372036854775808u64) {
        _ = ok;
        return (22 as process::ExitCode)!;
    } else err! {
        if err != fs::Error::OutOfRange {
            return (23 as process::ExitCode)!;
        }
    }
    if let !value = file.len() {
        if value != 4u64 {
            return (15 as process::ExitCode)!;
        }
    } else error! {
        return (16 as process::ExitCode)!;
    }
    if let !ok = file.sync_data() {
        _ = ok;
    } else error! {
        return (17 as process::ExitCode)!;
    }
    if let !ok = file.sync() {
        _ = ok;
    } else error! {
        return (18 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (19 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
    var path = fs::PathView::init("data.txt");
    var file: fs::File;
    if let !value = fs::File::create(path, fs::CreateOptions::read_write()) {
        file = value;
    } else error! {
        return (1 as process::ExitCode)!;
    }

    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]).exit().?;
    if let !ok = writer.write_all(b"metadata") {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !ok = writer.flush() {
        _ = ok;
    } else error! {
        return (3 as process::ExitCode)!;
    }

    if let !metadata = file.metadata() {
        if metadata.kind() != fs::FileKind::File {
            return (4 as process::ExitCode)!;
        }
        if metadata.size() != 8u64 {
            return (5 as process::ExitCode)!;
        }
        if let ?value = metadata.link_count() {
            if value == 0u32 {
                return (6 as process::ExitCode)!;
            }
        } else null {}
        if metadata.preferred_block_size() == 0u32 {
            return (7 as process::ExitCode)!;
        }
    } else error! {
        return (8 as process::ExitCode)!;
    }

    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (9 as process::ExitCode)!;
    }
    if let !metadata = cwd.metadata(path, fs::MetadataOptions::init()) {
        if metadata.kind() != fs::FileKind::File {
            return (10 as process::ExitCode)!;
        }
        if metadata.size() != 8u64 {
            return (11 as process::ExitCode)!;
        }
        if let ?time = metadata.accessed() {
            _ = time.seconds();
            _ = time.nanos();
        } else null {}
        _ = metadata.modified().seconds();
        if let ?time = metadata.status_changed() {
            _ = time.nanos();
        } else null {}
    } else error! {
        return (12 as process::ExitCode)!;
    }

    if let !ok = cwd.close() {
        _ = ok;
    } else error! {
        return (13 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (14 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
    var path = fs::PathView::init("data.txt");
    var file = fs::File::create(path, fs::CreateOptions::init()).exit().?;
    defer file.close().exit().?;
    file.set_permissions(0o755).exit().?;
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe fs set permissions");

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
    var path = fs::PathView::init("nia-λ.txt");
    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (90 as process::ExitCode)!;
    }
    defer {
        if let !ok = cwd.close() {
            _ = ok;
        } else error! {}
    };
    var file: fs::File;
    if let !value = cwd.create_file(path, fs::CreateOptions::init()) {
        file = value;
    } else error! {
        return (1 as process::ExitCode)!;
    }
    var buffer: [64]u8 = [0; 64];
    var writer = file.writer(init.io(), &mut buffer[..]).exit().?;
    if let !ok = writer.write_all(b"ok") {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !ok = writer.flush() {
        _ = ok;
    } else error! {
        return (3 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (4 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
fn emit_exe_std_fs_rejects_nul_in_text_path() {
    let root = temp_dir("emit_exe_std_fs_rejects_nul_in_text_path");
    let main = root.join("main.nia");
    let exe = root.join(format!("main{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(
        &main,
        r#"
using std::fs;
using std::process;

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var path = fs::PathView::init("bad\0path");
    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (90 as process::ExitCode)!;
    }
    defer {
        if let !ok = cwd.close() {
            _ = ok;
        } else error! {}
    };
    if let !file = cwd.open_file(path, fs::OpenOptions::read_only()) {
        _ = file;
        return (1 as process::ExitCode)!;
    } else err! {
        if err == fs::Error::Invalid {
            !{}
        } else {
            return (2 as process::ExitCode)!;
        }
    }
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (90 as process::ExitCode)!;
    }
    defer {
        if let !ok = cwd.close() {
            _ = ok;
        } else error! {}
    };
    var file: fs::File;
    if let !value = cwd.create_file(fs::PathView::init("delete-me.txt"), fs::CreateOptions::init()) {
        file = value;
    } else error! {
        return (1 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !ok = cwd.delete_file(fs::PathView::init("delete-me.txt")) {
        _ = ok;
    } else error! {
        return (3 as process::ExitCode)!;
    }
    if let !file = cwd.open_file(fs::PathView::init("delete-me.txt"), fs::OpenOptions::read_only()) {
        _ = file;
        return (4 as process::ExitCode)!;
    } else error! {
    }

    if let !ok = cwd.delete_file(fs::PathView::init("bad\0path")) {
        _ = ok;
        return (5 as process::ExitCode)!;
    } else error! {
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (90 as process::ExitCode)!;
    }
    defer {
        if let !ok = cwd.close() {
            _ = ok;
        } else error! {}
    };

    if let !ok = cwd.create_dir(fs::PathView::init("subdir"), fs::CreateDirOptions::init()) {
        _ = ok;
    } else error! {
        return (1 as process::ExitCode)!;
    }

    var file: fs::File;
    if let !value = cwd.create_file(fs::PathView::init("old-name.txt"), fs::CreateOptions::init()) {
        file = value;
    } else error! {
        return (2 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (3 as process::ExitCode)!;
    }

    if let !ok = cwd.rename(fs::PathView::init("old-name.txt"), fs::PathView::init("subdir/new-name.txt")) {
        _ = ok;
    } else error! {
        return (4 as process::ExitCode)!;
    }

    if let !value = cwd.open_file(fs::PathView::init("old-name.txt"), fs::OpenOptions::read_only()) {
        _ = value;
        return (5 as process::ExitCode)!;
    } else error! {
    }

    if let !value = cwd.open_file(fs::PathView::init("subdir/new-name.txt"), fs::OpenOptions::read_only()) {
        file = value;
    } else error! {
        return (6 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (7 as process::ExitCode)!;
    }

    if let !ok = cwd.delete_dir(fs::PathView::init("subdir")) {
        _ = ok;
        return (8 as process::ExitCode)!;
    } else error! {
    }

    if let !ok = cwd.delete_file(fs::PathView::init("subdir/new-name.txt")) {
        _ = ok;
    } else error! {
        return (9 as process::ExitCode)!;
    }
    if let !ok = cwd.delete_dir(fs::PathView::init("subdir")) {
        _ = ok;
    } else error! {
        return (10 as process::ExitCode)!;
    }

    if let !ok = cwd.create_dir(fs::PathView::init("bad\0path"), fs::CreateDirOptions::init()) {
        _ = ok;
        return (11 as process::ExitCode)!;
    } else error! {
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (90 as process::ExitCode)!;
    }
    defer {
        if let !ok = cwd.close() {
            _ = ok;
        } else error! {}
    };
    if let !ok = cwd.create_dir(fs::PathView::init("subdir"), fs::CreateDirOptions::init()) {
        _ = ok;
    } else error! {
        return (1 as process::ExitCode)!;
    }

    var subdir: fs::Dir;
    if let !value = cwd.open_dir(fs::PathView::init("subdir"), fs::OpenDirOptions::init()) {
        subdir = value;
    } else error! {
        return (2 as process::ExitCode)!;
    }

    var file: fs::File;
    if let !value = subdir.create_file(fs::PathView::init("inside.txt"), fs::CreateOptions::init()) {
        file = value;
    } else error! {
        return (3 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (4 as process::ExitCode)!;
    }

    if let !value = subdir.open_file(fs::PathView::init("inside.txt"), fs::OpenOptions::read_only()) {
        file = value;
    } else error! {
        return (5 as process::ExitCode)!;
    }
    if let !ok = file.close() {
        _ = ok;
    } else error! {
        return (6 as process::ExitCode)!;
    }

    if let !ok = subdir.close() {
        _ = ok;
    } else error! {
        return (7 as process::ExitCode)!;
    }

    if let !value = cwd.open_dir(fs::PathView::init("subdir/inside.txt"), fs::OpenDirOptions::init()) {
        _ = value;
        return (8 as process::ExitCode)!;
    } else error! {
    }

    if let !ok = cwd.delete_file(fs::PathView::init("subdir/inside.txt")) {
        _ = ok;
    } else error! {
        return (9 as process::ExitCode)!;
    }
    if let !ok = cwd.delete_dir(fs::PathView::init("subdir")) {
        _ = ok;
    } else error! {
        return (10 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
using std::mem;
using std::process;

fn bytes_equal(left: &[u8], right: &[u8]) bool {
    mem::equal[u8](left, right)
}

pub fn main(init: process::Init) process::ExitCode!void {
    _ = init;
    var cwd: fs::Dir;
    if let !value = fs::Dir::cwd() {
        cwd = value;
    } else error! {
        return (1 as process::ExitCode)!;
    }

    if let !ok = cwd.create_dir(fs::PathView::init("entries"), fs::CreateDirOptions::init()) {
        _ = ok;
    } else error! {
        return (2 as process::ExitCode)!;
    }

    var first: fs::File;
    if let !value = cwd.create_file(fs::PathView::init("entries/alpha.txt"), fs::CreateOptions::init()) {
        first = value;
    } else error! {
        return (3 as process::ExitCode)!;
    }
    if let !ok = first.close() {
        _ = ok;
    } else error! {
        return (4 as process::ExitCode)!;
    }

    var second: fs::File;
    if let !value = cwd.create_file(fs::PathView::init("entries/beta.txt"), fs::CreateOptions::init()) {
        second = value;
    } else error! {
        return (5 as process::ExitCode)!;
    }
    if let !ok = second.close() {
        _ = ok;
    } else error! {
        return (6 as process::ExitCode)!;
    }

    var dir: fs::Dir;
    if let !value = cwd.open_dir(fs::PathView::init("entries"), fs::OpenDirOptions::init()) {
        dir = value;
    } else error! {
        return (7 as process::ExitCode)!;
    }

    var buffer: [1024]u8 = [0; 1024];
    var iter: fs::DirIterator;
    if let !value = dir.entries(&mut buffer[..]) {
        iter = value;
    } else error! {
        return (8 as process::ExitCode)!;
    }

    var saw_alpha = false;
    var saw_beta = false;
    var count = 0usize;
    for result in iter {
        let value = if let !entry = result {
            entry
        } else error! {
            return (10 as process::ExitCode)!;
        };
        if not value.is_dot() and not value.is_dot_dot() {
            count += 1usize;
            if value.kind() != fs::FileKind::File and value.kind() != fs::FileKind::Unknown {
                return (9 as process::ExitCode)!;
            }
            if bytes_equal(value.name(), b"alpha.txt") {
                saw_alpha = true;
            } else if bytes_equal(value.name(), b"beta.txt") {
                saw_beta = true;
            }
        }
    }

    if count != 2usize {
        return (11 as process::ExitCode)!;
    }
    if not saw_alpha or not saw_beta {
        return (12 as process::ExitCode)!;
    }

    if let !ok = dir.close() {
        _ = ok;
    } else error! {
        return (13 as process::ExitCode)!;
    }
    if let !ok = cwd.delete_file(fs::PathView::init("entries/alpha.txt")) {
        _ = ok;
    } else error! {
        return (14 as process::ExitCode)!;
    }
    if let !ok = cwd.delete_file(fs::PathView::init("entries/beta.txt")) {
        _ = ok;
    } else error! {
        return (15 as process::ExitCode)!;
    }
    if let !ok = cwd.delete_dir(fs::PathView::init("entries")) {
        _ = ok;
    } else error! {
        return (16 as process::ExitCode)!;
    }
    if let !ok = cwd.close() {
        _ = ok;
    } else error! {
        return (17 as process::ExitCode)!;
    }
    !{}
}
"#,
    )
    .expect("write test source");

    let output = Command::new(env!("CARGO_BIN_EXE_nia"))
        .arg("emit")
        .arg("--exe")
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output_timeout("run nia emit --exe");

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
