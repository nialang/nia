// SPDX-License-Identifier: GPL-3.0-or-later
//! Filesystem publication primitives for driver-produced artifacts.

use std::{
    env, fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{atomic::AtomicU64, atomic::Ordering},
};

const FILE_STREAM_BYTES: usize = 64 * 1024;
static OUTPUT_STAGE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn write_output_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

/// Copies one tool-produced file into a sibling staging file before replacing
/// the destination. The opened source length is enforced across the stream, so
/// arbitrarily large archives do not require a coordinator-sized allocation
/// and a failed or truncated copy cannot damage an existing output.
pub(super) fn install_streamed_output(source: &Path, output: &Path) -> io::Result<()> {
    let mut source_file = fs::File::open(source)?;
    let metadata = source_file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "tool output must be a regular file",
        ));
    }
    let length = metadata.len();
    let parent = output
        .parent()
        .ok_or_else(|| io::Error::other("invalid driver output path"))?;
    if !parent.as_os_str().is_empty() {
        fs::create_dir_all(parent)?;
    }
    let staged = output.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        OUTPUT_STAGE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut staged_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        let mut buffer = [0; FILE_STREAM_BYTES];
        let mut remaining = length;
        while remaining != 0 {
            let chunk_len = crate::stream_chunk_len(remaining, buffer.len());
            source_file.read_exact(&mut buffer[..chunk_len])?;
            staged_file.write_all(&buffer[..chunk_len])?;
            remaining -= chunk_len as u64;
        }
        let mut trailing = [0; 1];
        if source_file.read(&mut trailing)? != 0 || source_file.metadata()?.len() != length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "tool output changed length while it was installed",
            ));
        }
        staged_file.sync_all()?;
        drop(staged_file);
        nia_compat::replace_path(&staged, output)
    })();
    if result.is_err() || staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

pub(super) fn object_file_name(index: usize, module_name: &str) -> String {
    let stem = Path::new(module_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module");
    let clean = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{index:04}_{clean}.o")
}

pub(super) fn archive_member_file_name(
    index: usize,
    key: &nia_codegen_llvm::CodegenUnitKey,
) -> String {
    let stable_name = match key {
        nia_codegen_llvm::CodegenUnitKey::SourceModule {
            source_identity, ..
        } => source_identity.normalized_path(),
        nia_codegen_llvm::CodegenUnitKey::CompilerBuiltins => "nia_compiler_builtins",
        nia_codegen_llvm::CodegenUnitKey::LinkageUnit {
            source_identity, ..
        } => source_identity.normalized_path(),
    };
    object_file_name(index, stable_name)
}

pub(super) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(super) fn new(prefix: &str) -> Self {
        let mut path = env::temp_dir();
        path.push(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_tool_output_is_streamed_and_atomically_installed() {
        let root = TempDir::new("nia_driver_streamed_output");
        fs::create_dir_all(root.path()).expect("create test root");
        let source = root.path().join("source.a");
        let output = root.path().join("nested/output.a");
        let payload = vec![0x6b; FILE_STREAM_BYTES * 5 + 29];
        fs::write(&source, &payload).expect("write source");

        install_streamed_output(&source, &output).expect("install output");

        assert_eq!(fs::read(output).expect("read output"), payload);
    }

    #[test]
    fn missing_tool_output_preserves_existing_destination() {
        let root = TempDir::new("nia_driver_missing_output");
        fs::create_dir_all(root.path()).expect("create test root");
        let output = root.path().join("output.a");
        fs::write(&output, b"existing").expect("write existing output");

        let error = install_streamed_output(&root.path().join("missing.a"), &output)
            .expect_err("missing source must fail");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(fs::read(output).expect("read existing output"), b"existing");
    }
}
