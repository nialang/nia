// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded reads for transport responses, repository files, and SSH configuration.

use std::io::{self, Read};
use std::path::Path;

use crate::invalid_packet;

pub(crate) fn read_limited(
    reader: impl Read,
    limit: usize,
    message: &'static str,
) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    reader
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut contents)?;
    if contents.len() > limit {
        return Err(invalid_packet(message));
    }
    Ok(contents)
}

pub(crate) fn read_limited_file(
    path: &Path,
    limit: usize,
    message: &'static str,
) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > limit as u64 {
        return Err(invalid_packet(message));
    }
    read_limited(file, limit, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_read_accepts_exact_limit_and_rejects_one_extra_byte() {
        assert_eq!(
            read_limited(&b"1234"[..], 4, "input too large").unwrap(),
            b"1234"
        );
        let error = read_limited(&b"12345"[..], 4, "input too large").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "input too large");
    }

    #[test]
    fn bounded_file_read_checks_metadata_and_preserves_io_errors() {
        struct TemporaryFile(std::path::PathBuf);
        impl Drop for TemporaryFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = TemporaryFile(
            std::env::temp_dir().join(format!("nia-git-input-{}-{nonce}", std::process::id())),
        );
        let mut file = std::fs::File::create_new(&path.0).unwrap();
        std::io::Write::write_all(&mut file, b"1234").unwrap();
        assert_eq!(
            read_limited_file(&path.0, 4, "file too large").unwrap(),
            b"1234"
        );
        assert_eq!(
            read_limited_file(&path.0, 3, "file too large")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        drop(file);
        std::fs::remove_file(&path.0).unwrap();
        assert_eq!(
            read_limited_file(&path.0, 4, "file too large")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
