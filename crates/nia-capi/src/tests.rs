// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[test]
fn exposes_version_and_status_names() {
    assert_eq!(nia_capi_abi_version(), NIA_CAPI_ABI_VERSION);

    let version = nia_version();
    let version_text = string_from_capi(version);
    assert_eq!(version_text, env!("CARGO_PKG_VERSION"));

    let status = nia_status_name(NiaStatus::InvalidArtifactRequest);
    let status_text = string_from_capi(status);
    assert_eq!(status_text, "invalid-artifact-request");
}

#[test]
fn check_file_reports_success() {
    let root = temp_dir("check_file_reports_success");
    let main = root.join("main.nia");
    fs::write(&main, "fn main() i32 { 0 }").expect("write source");
    let path = main.to_string_lossy();

    let result = nia_check_file(path.as_ptr(), path.len());

    assert!(!result.is_null());
    assert_eq!(nia_result_status(result), NiaStatus::Ok);
    let message = nia_result_message(result);
    assert!(message.ptr.is_null());
    assert_eq!(message.len, 0);
    nia_result_free(result);
}

#[test]
fn session_check_accepts_request_options_and_module_map() {
    let root = temp_dir("session_check_accepts_request_options_and_module_map");
    let main = root.join("main.nia");
    let dep = root.join("dep.nia");
    fs::write(
        &main,
        r#"
using dep;

fn main() i32 {
    dep::answer
}
"#,
    )
    .expect("write source");
    fs::write(&dep, "pub comptime let answer: i32 = 7;").expect("write dep");
    let main_path = main.to_string_lossy();
    let dep_path = dep.to_string_lossy();

    let session = nia_session_new();
    assert!(!session.is_null());
    let request = nia_check_request_new(main_path.as_ptr(), main_path.len());
    assert!(!request.is_null());
    assert_eq!(
        nia_check_request_add_module(
            request,
            b"dep".as_ptr(),
            3,
            dep_path.as_ptr(),
            dep_path.len()
        ),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_check_request_set_runtime(request, NiaRuntime::Bare),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_check_request_set_optimization(request, NiaOptimizationLevel::O0),
        NiaStatus::Ok
    );

    let result = nia_session_check(session, request);

    assert!(!result.is_null());
    assert_eq!(nia_result_status(result), NiaStatus::Ok);
    nia_result_free(result);
    nia_check_request_free(request);
    nia_session_free(session);
}

#[test]
fn session_emit_object_file_writes_object() {
    let root = temp_dir("session_emit_object_file_writes_object");
    let main = root.join("main.nia");
    let object = root.join("main.o");
    fs::write(&main, "fn main() i32 { 0 }").expect("write source");
    let main_path = main.to_string_lossy();
    let object_path = object.to_string_lossy();

    let session = nia_session_new();
    assert!(!session.is_null());
    let request = nia_check_request_new(main_path.as_ptr(), main_path.len());
    assert!(!request.is_null());
    assert_eq!(
        nia_check_request_set_runtime(request, NiaRuntime::Bare),
        NiaStatus::Ok
    );

    let result =
        nia_session_emit_object_file(session, request, object_path.as_ptr(), object_path.len());

    assert!(!result.is_null());
    assert_eq!(nia_result_status(result), NiaStatus::Ok);
    let metadata = fs::metadata(&object).expect("object metadata");
    assert!(metadata.len() > 0);
    nia_result_free(result);
    nia_check_request_free(request);
    nia_session_free(session);
}

#[test]
fn session_emit_object_directory_writes_object() {
    let root = temp_dir("session_emit_object_directory_writes_object");
    let main = root.join("main.nia");
    let out_dir = root.join("obj");
    fs::write(&main, "fn main() i32 { 0 }").expect("write source");
    let main_path = main.to_string_lossy();
    let out_dir_path = out_dir.to_string_lossy();

    let session = nia_session_new();
    assert!(!session.is_null());
    let request = nia_check_request_new(main_path.as_ptr(), main_path.len());
    assert!(!request.is_null());

    let result = nia_session_emit_object_directory(
        session,
        request,
        out_dir_path.as_ptr(),
        out_dir_path.len(),
    );

    assert!(!result.is_null());
    assert_eq!(nia_result_status(result), NiaStatus::Ok);
    let entries = fs::read_dir(&out_dir)
        .expect("read object dir")
        .collect::<Result<Vec<_>, _>>()
        .expect("read entries");
    assert_eq!(entries.len(), 1);
    assert!(entries[0].metadata().expect("metadata").len() > 0);
    nia_result_free(result);
    nia_check_request_free(request);
    nia_session_free(session);
}

#[test]
fn link_options_accept_structured_linker_options() {
    let options = nia_link_options_new();
    assert!(!options.is_null());

    assert_eq!(
        nia_link_options_add_arg(options, b"-lc".as_ptr(), 3),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_set_linker(options, b"ld".as_ptr(), 2),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_set_dynamic_linker_auto(options),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_set_no_dynamic_linker(options),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_set_dynamic_linker_path(options, b"/loader".as_ptr(), 7),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_add_library_path(options, b"/lib".as_ptr(), 4),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_add_rpath(options, b"$ORIGIN".as_ptr(), 7),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_add_library(options, b"nia_capi".as_ptr(), 8),
        NiaStatus::Ok
    );
    assert_eq!(
        nia_link_options_add_arg(std::ptr::null_mut(), b"-lc".as_ptr(), 3),
        NiaStatus::InvalidInput
    );

    nia_link_options_free(options);
}

#[test]
fn check_file_returns_rendered_diagnostics() {
    let root = temp_dir("check_file_returns_rendered_diagnostics");
    let main = root.join("main.nia");
    fs::write(&main, "fn main() i32 { true }").expect("write source");
    let path = main.to_string_lossy();

    let result = nia_check_file(path.as_ptr(), path.len());

    assert!(!result.is_null());
    assert_eq!(nia_result_status(result), NiaStatus::Diagnostics);
    let message = nia_result_message(result);
    let text = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(message.ptr, message.len))
            .expect("utf8 message")
            .to_string()
    };
    assert!(text.contains("diagnostics:"), "{text}");
    assert!(text.contains("main.nia"), "{text}");
    nia_string_free(message);
    nia_result_free(result);
}

#[test]
fn session_check_rejects_null_handles() {
    let result = nia_session_check(std::ptr::null_mut(), std::ptr::null());

    assert!(!result.is_null());
    assert_eq!(nia_result_status(result), NiaStatus::InvalidInput);
    nia_result_free(result);

    assert_eq!(
        nia_check_request_set_runtime(std::ptr::null_mut(), NiaRuntime::Bare),
        NiaStatus::InvalidInput
    );

    let output = b"out.o";
    let emit = nia_session_emit_object_file(
        std::ptr::null_mut(),
        std::ptr::null(),
        output.as_ptr(),
        output.len(),
    );
    assert!(!emit.is_null());
    assert_eq!(nia_result_status(emit), NiaStatus::InvalidInput);
    nia_result_free(emit);
}

#[test]
fn result_and_string_helpers_tolerate_nulls() {
    assert_eq!(nia_result_status(std::ptr::null()), NiaStatus::InvalidInput);
    let message = nia_result_message(std::ptr::null());
    assert!(message.ptr.is_null());
    assert_eq!(message.len, 0);
    nia_result_free(std::ptr::null_mut());
    nia_string_free(NiaString {
        ptr: std::ptr::null_mut(),
        len: 0,
    });
    nia_session_free(std::ptr::null_mut());
    nia_check_request_free(std::ptr::null_mut());
    nia_link_options_free(std::ptr::null_mut());
}

#[test]
fn check_file_rejects_invalid_utf8() {
    let bytes = [0xffu8];

    let result = nia_check_file(bytes.as_ptr(), bytes.len());

    assert!(!result.is_null());
    assert_eq!(nia_result_status(result), NiaStatus::InvalidInput);
    let message = nia_result_message(result);
    let text = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(message.ptr, message.len))
            .expect("utf8 message")
            .to_string()
    };
    assert!(text.contains("not valid UTF-8"), "{text}");
    nia_string_free(message);
    nia_result_free(result);
}

#[test]
fn c_header_smoke_test_can_call_check_file() {
    let Some(cc) = c_compiler() else {
        eprintln!("skipping C smoke test: no cc/clang/gcc on PATH");
        return;
    };
    let Some(lib) = nia_capi_dylib() else {
        eprintln!("skipping C smoke test: libnia_capi dynamic library not built");
        return;
    };

    let root = temp_dir("c_header_smoke_test_can_call_check_file");
    let source = root.join("smoke.c");
    let exe = root.join("smoke");
    let nia = root.join("main.nia");
    fs::write(&nia, "fn main() i32 { 0 }").expect("write nia source");
    fs::write(
        &source,
        r#"
#include "nia.h"
#include <stdint.h>
#include <stddef.h>

int main(int argc, char **argv) {
    if (argc != 2) {
        return 10;
    }
    if (nia_capi_abi_version() != NIA_CAPI_ABI_VERSION) {
        return 13;
    }
    const uint8_t *path = (const uint8_t *)argv[1];
    size_t len = 0;
    while (path[len] != 0) {
        len += 1;
    }
    NiaResult *result = nia_check_file(path, len);
    if (result == 0) {
        return 11;
    }
    NiaStatus status = nia_result_status(result);
    NiaString message = nia_result_message(result);
    nia_string_free(message);
    nia_result_free(result);
    return status == NIA_STATUS_OK ? 0 : 12;
}
"#,
    )
    .expect("write C source");

    let include = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("include");
    let lib_dir = lib.parent().expect("library parent");
    let output = Command::new(&cc)
        .arg(&source)
        .arg("-I")
        .arg(include)
        .arg("-L")
        .arg(lib_dir)
        .arg("-lnia_capi")
        .arg("-Wl,-rpath")
        .arg(format!("-Wl,{}", lib_dir.display()))
        .arg("-o")
        .arg(&exe)
        .output()
        .unwrap_or_else(|error| panic!("run C compiler `{cc}`: {error}"));
    if !output.status.success() {
        panic!(
            "C smoke compile failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let run = Command::new(&exe)
        .arg(&nia)
        .output()
        .expect("run C smoke executable");
    assert!(
        run.status.success(),
        "C smoke executable failed: {:?}\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

fn temp_dir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nia_capi_{name}_{}_{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    if path.exists() {
        fs::remove_dir_all(&path).expect("remove stale temp dir");
    }
    fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn string_from_capi(value: NiaString) -> String {
    let text = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(value.ptr, value.len))
            .expect("utf8 string")
            .to_string()
    };
    nia_string_free(value);
    text
}

fn c_compiler() -> Option<String> {
    for candidate in ["cc", "clang", "gcc"] {
        if Command::new(candidate).arg("--version").output().is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

fn nia_capi_dylib() -> Option<PathBuf> {
    let deps = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("target/debug/deps");
    let suffix = std::env::consts::DLL_SUFFIX;
    let prefix = std::env::consts::DLL_PREFIX;
    fs::read_dir(deps)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{prefix}nia_capi")) && name.ends_with(suffix)
                })
        })
}
