// SPDX-License-Identifier: GPL-3.0-or-later
use std::process::Command;

mod support;

use support::{CommandExt, CommandStatusExt, temp_dir};

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
    var path = fs::Path::init("data.txt");
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    var file: fs::File;
    switch cwd.create_file(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    var write_buffer: [64]u8 = [0; 64];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"nia fs") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    var opened: fs::File;
    switch cwd.open_file(path, fs::OpenOptions::read_only()) {
        !value => opened = value,
        error! => return (5 as process::ExitCode)!,
    }
    var read_buffer: [64]u8 = [0; 64];
    var reader = opened.reader(init.io(), &mut read_buffer[..]);
    var bytes: [6]u8 = [0, 0, 0, 0, 0, 0];
    switch reader.read_exact(&mut bytes[..]) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    switch opened.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }
    var expected = b"nia fs";
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
    var path = fs::Path::init("data.txt");
    var file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"open close") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    var opened: fs::File;
    switch fs::File::open(path, fs::OpenOptions::read_only()) {
        !value => opened = value,
        error! => return (5 as process::ExitCode)!,
    }
    var read_buffer: [16]u8 = [0; 16];
    var reader = opened.reader(init.io(), &mut read_buffer[..]);
    var bytes: [10]u8 = [0; 10];
    switch reader.read_exact(&mut bytes[..]) {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }
    switch opened.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }
    var expected = b"open close";
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
    var path = fs::Path::init("data.txt");
    var file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }

    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"abcdef") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    switch file.len() {
        !value => {
            if value != 6u64 {
                return (4 as process::ExitCode)!;
            }
        },
        error! => return (5 as process::ExitCode)!,
    }
    switch file.seek_by(0) {
        !value => {
            if value != 6u64 {
                return (6 as process::ExitCode)!;
            }
        },
        error! => return (7 as process::ExitCode)!,
    }
    switch file.seek_to(2u64) {
        !value => {
            if value != 2u64 {
                return (8 as process::ExitCode)!;
            }
        },
        error! => return (9 as process::ExitCode)!,
    }
    switch file.seek_by(1i64) {
        !value => {
            if value != 3u64 {
                return (10 as process::ExitCode)!;
            }
        },
        error! => return (11 as process::ExitCode)!,
    }
    switch file.seek_from_end(-2i64) {
        !value => {
            if value != 4u64 {
                return (12 as process::ExitCode)!;
            }
        },
        error! => return (13 as process::ExitCode)!,
    }

    switch file.truncate(4u64) {
        !ok => _ = ok,
        error! => return (14 as process::ExitCode)!,
    }
    switch file.seek_to(9223372036854775808u64) {
        !value => {
            _ = value;
            return (20 as process::ExitCode)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return (21 as process::ExitCode)!;
            }
        },
    }
    switch file.truncate(9223372036854775808u64) {
        !ok => {
            _ = ok;
            return (22 as process::ExitCode)!;
        },
        err! => {
            if err != fs::Error::OutOfRange {
                return (23 as process::ExitCode)!;
            }
        },
    }
    switch file.len() {
        !value => {
            if value != 4u64 {
                return (15 as process::ExitCode)!;
            }
        },
        error! => return (16 as process::ExitCode)!,
    }
    switch file.sync_data() {
        !ok => _ = ok,
        error! => return (17 as process::ExitCode)!,
    }
    switch file.sync() {
        !ok => _ = ok,
        error! => return (18 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (19 as process::ExitCode)!,
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
    var path = fs::Path::init("data.txt");
    var file: fs::File;
    switch fs::File::create(path, fs::CreateOptions::read_write()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }

    var write_buffer: [16]u8 = [0; 16];
    var writer = file.writer(init.io(), &mut write_buffer[..]);
    switch writer.write_all(b"metadata") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    switch file.metadata() {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return (4 as process::ExitCode)!;
            }
            if metadata.size() != 8u64 {
                return (5 as process::ExitCode)!;
            }
            switch metadata.link_count() {
                ?value => {
                    if value == 0u32 {
                        return (6 as process::ExitCode)!;
                    }
                },
                null => {},
            }
            if metadata.preferred_block_size() == 0u32 {
                return (7 as process::ExitCode)!;
            }
        },
        error! => return (8 as process::ExitCode)!,
    }

    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (9 as process::ExitCode)!,
    }
    switch cwd.metadata(path, fs::MetadataOptions::init()) {
        !metadata => {
            if metadata.kind() != fs::FileKind::File {
                return (10 as process::ExitCode)!;
            }
            if metadata.size() != 8u64 {
                return (11 as process::ExitCode)!;
            }
            switch metadata.accessed() {
                ?time => {
                    _ = time.seconds();
                    _ = time.nanos();
                },
                null => {},
            }
            _ = metadata.modified().seconds();
            switch metadata.status_changed() {
                ?time => _ = time.nanos(),
                null => {},
            }
        },
        error! => return (12 as process::ExitCode)!,
    }

    switch cwd.close() {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (14 as process::ExitCode)!,
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
    var path = fs::Path::init("nia-λ.txt");
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    var file: fs::File;
    switch cwd.create_file(path, fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    var buffer: [64]u8 = [0; 64];
    var writer = file.writer(init.io(), &mut buffer[..]);
    switch writer.write_all(b"ok") {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch writer.flush() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
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
    var path = fs::Path::init("bad\0path");
    var cwd: fs::Dir;
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    switch cwd.open_file(path, fs::OpenOptions::read_only()) {
        !file => {
            _ = file;
            return (1 as process::ExitCode)!;
        },
        err! => {
            if err == fs::Error::Invalid {
                !{}
            } else {
                return (2 as process::ExitCode)!;
            }
        },
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
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    var file: fs::File;
    switch cwd.create_file(fs::Path::init("delete-me.txt"), fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (1 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }
    switch cwd.delete_file(fs::Path::init("delete-me.txt")) {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }
    switch cwd.open_file(fs::Path::init("delete-me.txt"), fs::OpenOptions::read_only()) {
        !file => {
            _ = file;
            return (4 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.delete_file(fs::Path::init("bad\0path")) {
        !ok => {
            _ = ok;
            return (5 as process::ExitCode)!;
        },
        error! => {},
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
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };

    switch cwd.create_dir(fs::Path::init("subdir"), fs::CreateDirOptions::init()) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }

    var file: fs::File;
    switch cwd.create_file(fs::Path::init("old-name.txt"), fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (2 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (3 as process::ExitCode)!,
    }

    switch cwd.rename(fs::Path::init("old-name.txt"), fs::Path::init("subdir/new-name.txt")) {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    switch cwd.open_file(fs::Path::init("old-name.txt"), fs::OpenOptions::read_only()) {
        !value => {
            _ = value;
            return (5 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.open_file(fs::Path::init("subdir/new-name.txt"), fs::OpenOptions::read_only()) {
        !value => file = value,
        error! => return (6 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }

    switch cwd.delete_dir(fs::Path::init("subdir")) {
        !ok => {
            _ = ok;
            return (8 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.delete_file(fs::Path::init("subdir/new-name.txt")) {
        !ok => _ = ok,
        error! => return (9 as process::ExitCode)!,
    }
    switch cwd.delete_dir(fs::Path::init("subdir")) {
        !ok => _ = ok,
        error! => return (10 as process::ExitCode)!,
    }

    switch cwd.create_dir(fs::Path::init("bad\0path"), fs::CreateDirOptions::init()) {
        !ok => {
            _ = ok;
            return (11 as process::ExitCode)!;
        },
        error! => {},
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
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (90 as process::ExitCode)!,
    }
    defer {
        switch cwd.close() {
            !ok => _ = ok,
            error! => {},
        };
    };
    switch cwd.create_dir(fs::Path::init("subdir"), fs::CreateDirOptions::init()) {
        !ok => _ = ok,
        error! => return (1 as process::ExitCode)!,
    }

    var subdir: fs::Dir;
    switch cwd.open_dir(fs::Path::init("subdir"), fs::OpenDirOptions::init()) {
        !value => subdir = value,
        error! => return (2 as process::ExitCode)!,
    }

    var file: fs::File;
    switch subdir.create_file(fs::Path::init("inside.txt"), fs::CreateOptions::init()) {
        !value => file = value,
        error! => return (3 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    switch subdir.open_file(fs::Path::init("inside.txt"), fs::OpenOptions::read_only()) {
        !value => file = value,
        error! => return (5 as process::ExitCode)!,
    }
    switch file.close() {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }

    switch subdir.close() {
        !ok => _ = ok,
        error! => return (7 as process::ExitCode)!,
    }

    switch cwd.open_dir(fs::Path::init("subdir/inside.txt"), fs::OpenDirOptions::init()) {
        !value => {
            _ = value;
            return (8 as process::ExitCode)!;
        },
        error! => {},
    }

    switch cwd.delete_file(fs::Path::init("subdir/inside.txt")) {
        !ok => _ = ok,
        error! => return (9 as process::ExitCode)!,
    }
    switch cwd.delete_dir(fs::Path::init("subdir")) {
        !ok => _ = ok,
        error! => return (10 as process::ExitCode)!,
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
    switch fs::Dir::cwd() {
        !value => cwd = value,
        error! => return (1 as process::ExitCode)!,
    }

    switch cwd.create_dir(fs::Path::init("entries"), fs::CreateDirOptions::init()) {
        !ok => _ = ok,
        error! => return (2 as process::ExitCode)!,
    }

    var first: fs::File;
    switch cwd.create_file(fs::Path::init("entries/alpha.txt"), fs::CreateOptions::init()) {
        !value => first = value,
        error! => return (3 as process::ExitCode)!,
    }
    switch first.close() {
        !ok => _ = ok,
        error! => return (4 as process::ExitCode)!,
    }

    var second: fs::File;
    switch cwd.create_file(fs::Path::init("entries/beta.txt"), fs::CreateOptions::init()) {
        !value => second = value,
        error! => return (5 as process::ExitCode)!,
    }
    switch second.close() {
        !ok => _ = ok,
        error! => return (6 as process::ExitCode)!,
    }

    var dir: fs::Dir;
    switch cwd.open_dir(fs::Path::init("entries"), fs::OpenDirOptions::init()) {
        !value => dir = value,
        error! => return (7 as process::ExitCode)!,
    }

    var buffer: [1024]u8 = [0; 1024];
    var iter: fs::DirIterator;
    switch dir.entries(&mut buffer[..]) {
        !value => iter = value,
        error! => return (8 as process::ExitCode)!,
    }

    var saw_alpha = false;
    var saw_beta = false;
    var count = 0usize;
    for result in iter {
        let value = switch result {
            !entry => entry,
            error! => return (10 as process::ExitCode)!,
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

    switch dir.close() {
        !ok => _ = ok,
        error! => return (13 as process::ExitCode)!,
    }
    switch cwd.delete_file(fs::Path::init("entries/alpha.txt")) {
        !ok => _ = ok,
        error! => return (14 as process::ExitCode)!,
    }
    switch cwd.delete_file(fs::Path::init("entries/beta.txt")) {
        !ok => _ = ok,
        error! => return (15 as process::ExitCode)!,
    }
    switch cwd.delete_dir(fs::Path::init("entries")) {
        !ok => _ = ok,
        error! => return (16 as process::ExitCode)!,
    }
    switch cwd.close() {
        !ok => _ = ok,
        error! => return (17 as process::ExitCode)!,
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
