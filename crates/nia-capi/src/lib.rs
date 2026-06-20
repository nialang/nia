// SPDX-License-Identifier: GPL-3.0-or-later
use std::{ptr, slice};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NiaString {
    pub ptr: *mut u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NiaStatus {
    Ok = 0,
    Diagnostics = 1,
    InvalidInput = 2,
    InternalError = 3,
    IoError = 4,
    LinkerError = 5,
    InvalidArtifactRequest = 6,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NiaRuntime {
    Bare = 0,
    Freestanding = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NiaOptimizationLevel {
    O0 = 0,
    O1 = 1,
    O2 = 2,
    O3 = 3,
    Os = 4,
    Oz = 5,
}

#[repr(C)]
pub struct NiaSession {
    driver: nia_driver::Driver,
}

#[repr(C)]
pub struct NiaCheckRequest {
    request: nia_driver::CheckRequest,
}

#[repr(C)]
pub struct NiaLinkOptions {
    link_args: Vec<String>,
    linker_program: Option<String>,
    dynamic_linker: Option<nia_linker::DynamicLinker>,
    library_paths: Vec<String>,
    rpaths: Vec<String>,
    libraries: Vec<String>,
}

#[repr(C)]
pub struct NiaResult {
    status: NiaStatus,
    message: Vec<u8>,
}

pub const NIA_CAPI_ABI_VERSION: u32 = 2;

#[unsafe(no_mangle)]
pub extern "C" fn nia_capi_abi_version() -> u32 {
    NIA_CAPI_ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_version() -> NiaString {
    NiaString::from_bytes(env!("CARGO_PKG_VERSION").as_bytes().to_vec())
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_status_name(status: NiaStatus) -> NiaString {
    NiaString::from_bytes(status.name().as_bytes().to_vec())
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_new() -> *mut NiaSession {
    catch_ptr(|| {
        Box::into_raw(Box::new(NiaSession {
            driver: nia_driver::Driver::new(),
        }))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_free(session: *mut NiaSession) {
    if session.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(session));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_set_source(
    session: *mut NiaSession,
    path_ptr: *const u8,
    path_len: usize,
    text_ptr: *const u8,
    text_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return NiaStatus::InvalidInput;
        };
        let path = match string_from_abi(path_ptr, path_len) {
            Ok(path) => path,
            Err(_) => return NiaStatus::InvalidInput,
        };
        let text = match string_from_abi(text_ptr, text_len) {
            Ok(text) => text,
            Err(_) => return NiaStatus::InvalidInput,
        };
        session.driver.set_source(path, text);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_check_request_new(
    path_ptr: *const u8,
    path_len: usize,
) -> *mut NiaCheckRequest {
    catch_ptr(|| match string_from_abi(path_ptr, path_len) {
        Ok(path) => Box::into_raw(Box::new(NiaCheckRequest {
            request: nia_driver::CheckRequest::new(path),
        })),
        Err(_) => ptr::null_mut(),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_check_request_free(request: *mut NiaCheckRequest) {
    if request.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(request));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_check_request_add_module(
    request: *mut NiaCheckRequest,
    name_ptr: *const u8,
    name_len: usize,
    path_ptr: *const u8,
    path_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(request) = (unsafe { request.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        let name = match string_from_abi(name_ptr, name_len) {
            Ok(name) => name,
            Err(_) => return NiaStatus::InvalidInput,
        };
        let path = match string_from_abi(path_ptr, path_len) {
            Ok(path) => path,
            Err(_) => return NiaStatus::InvalidInput,
        };
        request
            .request
            .module_map
            .insert(name, nia_driver::SourcePath::new(path));
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_check_request_set_runtime(
    request: *mut NiaCheckRequest,
    runtime: NiaRuntime,
) -> NiaStatus {
    catch_status(|| {
        let Some(request) = (unsafe { request.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        request.request.runtime = runtime.into();
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_check_request_set_optimization(
    request: *mut NiaCheckRequest,
    level: NiaOptimizationLevel,
) -> NiaStatus {
    catch_status(|| {
        let Some(request) = (unsafe { request.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        request.request.optimization = level.into();
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_new() -> *mut NiaLinkOptions {
    catch_ptr(|| {
        Box::into_raw(Box::new(NiaLinkOptions {
            link_args: Vec::new(),
            linker_program: None,
            dynamic_linker: None,
            library_paths: Vec::new(),
            rpaths: Vec::new(),
            libraries: Vec::new(),
        }))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_free(options: *mut NiaLinkOptions) {
    if options.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(options));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_add_arg(
    options: *mut NiaLinkOptions,
    arg_ptr: *const u8,
    arg_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        let arg = match string_from_abi(arg_ptr, arg_len) {
            Ok(arg) => arg,
            Err(_) => return NiaStatus::InvalidInput,
        };
        options.link_args.push(arg);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_set_linker(
    options: *mut NiaLinkOptions,
    program_ptr: *const u8,
    program_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        let program = match string_from_abi(program_ptr, program_len) {
            Ok(program) => program,
            Err(_) => return NiaStatus::InvalidInput,
        };
        options.linker_program = Some(program);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_set_dynamic_linker_auto(
    options: *mut NiaLinkOptions,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        options.dynamic_linker = Some(nia_linker::DynamicLinker::Auto);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_set_no_dynamic_linker(
    options: *mut NiaLinkOptions,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        options.dynamic_linker = Some(nia_linker::DynamicLinker::None);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_set_dynamic_linker_path(
    options: *mut NiaLinkOptions,
    path_ptr: *const u8,
    path_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        let path = match string_from_abi(path_ptr, path_len) {
            Ok(path) => path,
            Err(_) => return NiaStatus::InvalidInput,
        };
        options.dynamic_linker = Some(nia_linker::DynamicLinker::Path(path));
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_add_library_path(
    options: *mut NiaLinkOptions,
    path_ptr: *const u8,
    path_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        let path = match string_from_abi(path_ptr, path_len) {
            Ok(path) => path,
            Err(_) => return NiaStatus::InvalidInput,
        };
        options.library_paths.push(path);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_add_rpath(
    options: *mut NiaLinkOptions,
    path_ptr: *const u8,
    path_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        let path = match string_from_abi(path_ptr, path_len) {
            Ok(path) => path,
            Err(_) => return NiaStatus::InvalidInput,
        };
        options.rpaths.push(path);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_link_options_add_library(
    options: *mut NiaLinkOptions,
    name_ptr: *const u8,
    name_len: usize,
) -> NiaStatus {
    catch_status(|| {
        let Some(options) = (unsafe { options.as_mut() }) else {
            return NiaStatus::InvalidInput;
        };
        let name = match string_from_abi(name_ptr, name_len) {
            Ok(name) => name,
            Err(_) => return NiaStatus::InvalidInput,
        };
        options.libraries.push(name);
        NiaStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_check(
    session: *mut NiaSession,
    request: *const NiaCheckRequest,
) -> *mut NiaResult {
    match catch_abi(|| {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null session".to_string());
        };
        let Some(request) = (unsafe { request.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null check request".to_string());
        };
        check_with_driver(&session.driver, request.request.clone())
    }) {
        Ok(result) => Box::into_raw(Box::new(result)),
        Err(message) => Box::into_raw(Box::new(NiaResult::new(NiaStatus::InternalError, message))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_emit_object_file(
    session: *mut NiaSession,
    request: *const NiaCheckRequest,
    output_ptr: *const u8,
    output_len: usize,
) -> *mut NiaResult {
    match catch_abi(|| {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null session".to_string());
        };
        let Some(request) = (unsafe { request.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null check request".to_string());
        };
        let output = match string_from_abi(output_ptr, output_len) {
            Ok(output) => output,
            Err(message) => return NiaResult::new(NiaStatus::InvalidInput, message),
        };
        let driver_output =
            session
                .driver
                .write_native_objects(nia_driver::WriteObjectRequest::new(
                    request.request.clone(),
                    nia_driver::ObjectOutput::Single(output.into()),
                ));
        result_from_driver_output(driver_output, Some(&request.request.root_path))
    }) {
        Ok(result) => Box::into_raw(Box::new(result)),
        Err(message) => Box::into_raw(Box::new(NiaResult::new(NiaStatus::InternalError, message))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_emit_object_directory(
    session: *mut NiaSession,
    request: *const NiaCheckRequest,
    output_ptr: *const u8,
    output_len: usize,
) -> *mut NiaResult {
    match catch_abi(|| {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null session".to_string());
        };
        let Some(request) = (unsafe { request.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null check request".to_string());
        };
        let output = match string_from_abi(output_ptr, output_len) {
            Ok(output) => output,
            Err(message) => return NiaResult::new(NiaStatus::InvalidInput, message),
        };
        let driver_output =
            session
                .driver
                .write_native_objects(nia_driver::WriteObjectRequest::new(
                    request.request.clone(),
                    nia_driver::ObjectOutput::Directory(output.into()),
                ));
        result_from_driver_output(driver_output, Some(&request.request.root_path))
    }) {
        Ok(result) => Box::into_raw(Box::new(result)),
        Err(message) => Box::into_raw(Box::new(NiaResult::new(NiaStatus::InternalError, message))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_emit_executable(
    session: *mut NiaSession,
    request: *const NiaCheckRequest,
    output_ptr: *const u8,
    output_len: usize,
) -> *mut NiaResult {
    nia_session_emit_executable_with_options(session, request, output_ptr, output_len, ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_session_emit_executable_with_options(
    session: *mut NiaSession,
    request: *const NiaCheckRequest,
    output_ptr: *const u8,
    output_len: usize,
    options: *const NiaLinkOptions,
) -> *mut NiaResult {
    match catch_abi(|| {
        let Some(session) = (unsafe { session.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null session".to_string());
        };
        let Some(request) = (unsafe { request.as_ref() }) else {
            return NiaResult::new(NiaStatus::InvalidInput, "null check request".to_string());
        };
        let output = match string_from_abi(output_ptr, output_len) {
            Ok(output) => output,
            Err(message) => return NiaResult::new(NiaStatus::InvalidInput, message),
        };
        let link_options = unsafe { options.as_ref() };
        let mut link_request =
            nia_driver::LinkExecutableRequest::new(request.request.clone(), output);
        if let Some(options) = link_options {
            let mut link_options =
                nia_linker::LinkOptions::default().with_raw_args(options.link_args.clone());
            if let Some(program) = &options.linker_program {
                link_options = link_options
                    .with_linker(nia_linker::ExecutableLinker::with_program(program.clone()));
            }
            if let Some(dynamic_linker) = &options.dynamic_linker {
                link_options = link_options
                    .with_dynamic_mode()
                    .with_dynamic_linker(dynamic_linker.clone());
            }
            for path in &options.library_paths {
                link_options = link_options.add_library_path(path.clone());
            }
            for path in &options.rpaths {
                link_options = link_options.add_rpath(path.clone());
            }
            for library in &options.libraries {
                link_options = link_options.add_library(library.clone());
            }
            link_request.link_options = link_options;
        }
        let driver_output = session.driver.link_executable(link_request);
        result_from_driver_output(driver_output, Some(&request.request.root_path))
    }) {
        Ok(result) => Box::into_raw(Box::new(result)),
        Err(message) => Box::into_raw(Box::new(NiaResult::new(NiaStatus::InternalError, message))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_check_file(path_ptr: *const u8, path_len: usize) -> *mut NiaResult {
    match catch_abi(|| check_file(path_ptr, path_len)) {
        Ok(result) => Box::into_raw(Box::new(result)),
        Err(message) => Box::into_raw(Box::new(NiaResult::new(NiaStatus::InternalError, message))),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_result_status(result: *const NiaResult) -> NiaStatus {
    let Some(result) = (unsafe { result.as_ref() }) else {
        return NiaStatus::InvalidInput;
    };
    result.status
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_result_message(result: *const NiaResult) -> NiaString {
    let Some(result) = (unsafe { result.as_ref() }) else {
        return NiaString::empty();
    };
    NiaString::from_bytes(result.message.clone())
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_result_free(result: *mut NiaResult) {
    if result.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(result));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nia_string_free(value: NiaString) {
    if value.ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(slice::from_raw_parts_mut(
            value.ptr, value.len,
        )));
    }
}

fn check_file(path_ptr: *const u8, path_len: usize) -> NiaResult {
    let path = match string_from_abi(path_ptr, path_len) {
        Ok(path) => path,
        Err(message) => return NiaResult::new(NiaStatus::InvalidInput, message),
    };
    check_with_driver(
        &nia_driver::Driver::new(),
        nia_driver::CheckRequest::new(path),
    )
}

fn check_with_driver(driver: &nia_driver::Driver, request: nia_driver::CheckRequest) -> NiaResult {
    let path = request.root_path.clone();
    let program = driver.check(request);
    if program.diagnostics.is_empty() {
        return NiaResult::new(NiaStatus::Ok, String::new());
    }
    NiaResult::new(
        NiaStatus::Diagnostics,
        nia_driver::render_program_diagnostics(&program, Some(&path), None),
    )
}

fn result_from_driver_output<T>(
    output: nia_driver::DriverOutput<T>,
    primary_path: Option<&str>,
) -> NiaResult {
    match output.result {
        Ok(_) => NiaResult::new(NiaStatus::Ok, String::new()),
        Err(error) => NiaResult::new(
            status_from_driver_error(&error),
            nia_driver::render_driver_error(&error, primary_path, None),
        ),
    }
}

fn status_from_driver_error(error: &nia_driver::DriverError) -> NiaStatus {
    match error {
        nia_driver::DriverError::CheckDiagnostics(_) => NiaStatus::Diagnostics,
        nia_driver::DriverError::CodegenDiagnostics(_) => NiaStatus::Diagnostics,
        nia_driver::DriverError::InvalidArtifactRequest(_) => NiaStatus::InvalidArtifactRequest,
        nia_driver::DriverError::Io { .. } => NiaStatus::IoError,
        nia_driver::DriverError::LinkerStatus { .. }
        | nia_driver::DriverError::LinkerIo { .. }
        | nia_driver::DriverError::LinkerConfig(_) => NiaStatus::LinkerError,
    }
}

fn string_from_abi(ptr: *const u8, len: usize) -> Result<String, String> {
    if ptr.is_null() {
        if len == 0 {
            return Ok(String::new());
        }
        return Err("null pointer with non-zero length".to_string());
    }
    let bytes = unsafe { slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| "input string is not valid UTF-8".to_string())
}

fn catch_abi(f: impl FnOnce() -> NiaResult) -> Result<NiaResult, String> {
    match nia_ice::catch_ice(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(payload)) => Err(panic_message(payload)),
        Err(ice) => Err(ice.render_message()),
    }
}

fn catch_status(f: impl FnOnce() -> NiaStatus) -> NiaStatus {
    match nia_ice::catch_ice(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))) {
        Ok(Ok(status)) => status,
        Ok(Err(_)) | Err(_) => NiaStatus::InternalError,
    }
}

fn catch_ptr<T>(f: impl FnOnce() -> *mut T) -> *mut T {
    match nia_ice::catch_ice(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))) {
        Ok(Ok(ptr)) => ptr,
        Ok(Err(_)) | Err(_) => ptr::null_mut(),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("internal panic: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("internal panic: {message}")
    } else {
        "internal panic".to_string()
    }
}

impl NiaResult {
    fn new(status: NiaStatus, message: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl NiaString {
    fn empty() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
        }
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::empty();
        }
        let mut bytes = bytes.into_boxed_slice();
        let value = Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
        };
        let _ = Box::into_raw(bytes);
        value
    }
}

impl NiaStatus {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Diagnostics => "diagnostics",
            Self::InvalidInput => "invalid-input",
            Self::InternalError => "internal-error",
            Self::IoError => "io-error",
            Self::LinkerError => "linker-error",
            Self::InvalidArtifactRequest => "invalid-artifact-request",
        }
    }
}

impl From<NiaRuntime> for nia_driver::Runtime {
    fn from(value: NiaRuntime) -> Self {
        match value {
            NiaRuntime::Bare => Self::Bare,
            NiaRuntime::Freestanding => Self::Freestanding,
        }
    }
}

impl From<NiaOptimizationLevel> for nia_driver::NiaOptimizationLevel {
    fn from(value: NiaOptimizationLevel) -> Self {
        match value {
            NiaOptimizationLevel::O0 => Self::O0,
            NiaOptimizationLevel::O1 => Self::O1,
            NiaOptimizationLevel::O2 => Self::O2,
            NiaOptimizationLevel::O3 => Self::O3,
            NiaOptimizationLevel::Os => Self::Os,
            NiaOptimizationLevel::Oz => Self::Oz,
        }
    }
}

#[cfg(test)]
mod tests;
