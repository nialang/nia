// SPDX-License-Identifier: GPL-3.0-or-later
//! COFF import libraries generated from module definitions.

use std::path::Path;

use super::base::{LlvmError, LlvmResult};

/// COFF machine an import library describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoffMachine {
    /// `IMAGE_FILE_MACHINE_AMD64`.
    Amd64,
}

impl CoffMachine {
    const fn image_file_machine(self) -> u16 {
        match self {
            Self::Amd64 => 0x8664,
        }
    }
}

enum FfiImportLibraryError {}

unsafe extern "C" {
    fn nia_llvm_write_coff_import_library(
        definition: *const u8,
        definition_len: usize,
        machine: u16,
        path: *const u8,
        path_len: usize,
    ) -> *mut FfiImportLibraryError;
    fn nia_llvm_import_library_error_data(error: *const FfiImportLibraryError) -> *const u8;
    fn nia_llvm_import_library_error_len(error: *const FfiImportLibraryError) -> usize;
    fn nia_llvm_import_library_error_free(error: *mut FfiImportLibraryError);
}

/// Writes the short-import library that a module definition (`.def`)
/// describes, as `lib.exe /def` would, to `output`.
pub fn write_coff_import_library(
    definition: &[u8],
    machine: CoffMachine,
    output: &Path,
) -> LlvmResult<()> {
    let output = output.to_str().ok_or_else(|| {
        LlvmError::error(format!(
            "import library path `{}` is not UTF-8",
            output.display()
        ))
    })?;
    // SAFETY: both buffers outlive the call, and a returned error is read and
    // freed exactly once below.
    unsafe {
        let error = nia_llvm_write_coff_import_library(
            definition.as_ptr(),
            definition.len(),
            machine.image_file_machine(),
            output.as_ptr(),
            output.len(),
        );
        if error.is_null() {
            return Ok(());
        }
        let message = std::slice::from_raw_parts(
            nia_llvm_import_library_error_data(error),
            nia_llvm_import_library_error_len(error),
        );
        let message = String::from_utf8_lossy(message).into_owned();
        nia_llvm_import_library_error_free(error);
        Err(LlvmError::error(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn module_definition_becomes_a_short_import_archive() {
        let path = std::env::temp_dir().join(format!(
            "nia-llvm-import-library-{}.lib",
            std::process::id()
        ));
        write_coff_import_library(
            b"LIBRARY kernel32.dll\nEXPORTS\n    ExitProcess\n",
            CoffMachine::Amd64,
            &path,
        )
        .expect("write import library");
        let bytes = std::fs::read(&path).expect("read import library");
        std::fs::remove_file(&path).expect("remove import library");

        assert!(bytes.starts_with(b"!<arch>\n"));
        assert!(contains(&bytes, b"kernel32.dll"));
        assert!(contains(&bytes, b"__imp_ExitProcess"));
    }

    #[test]
    fn malformed_module_definitions_are_errors() {
        let path = std::env::temp_dir().join(format!(
            "nia-llvm-import-library-invalid-{}.lib",
            std::process::id()
        ));
        let error =
            write_coff_import_library(b"EXPORTS\n    ExitProcess\n", CoffMachine::Amd64, &path)
                .expect_err("a definition without LIBRARY names no DLL");
        assert!(
            matches!(&error, LlvmError::Error(message) if message.contains("LIBRARY")),
            "{error:?}"
        );
        assert!(!path.exists());
    }
}
