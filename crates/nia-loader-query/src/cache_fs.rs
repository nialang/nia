// SPDX-License-Identifier: GPL-3.0-or-later
//! Filesystem boundaries shared by persistent frontend caches.

use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt as _;

pub(crate) fn ensure_cache_directory(root: &Path, directory: &Path) -> io::Result<()> {
    match fs::symlink_metadata(root) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(root)?,
        Err(error) => return Err(error),
    }
    validate_cache_directory(root)?;
    let relative = directory.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cache directory is outside its invocation root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache directory contains a non-normal component",
            ));
        }
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        validate_cache_directory(&current)?;
    }
    Ok(())
}

pub(crate) fn validate_cache_namespace(root: &Path, directory: &Path) -> io::Result<()> {
    validate_cache_directory(root)?;
    let relative = directory.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cache directory is outside its invocation root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cache directory contains a non-normal component",
            ));
        }
        current.push(component);
        validate_cache_directory(&current)?;
    }
    Ok(())
}

fn validate_cache_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let is_reparse = metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0;
    #[cfg(not(windows))]
    let is_reparse = false;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache namespace must contain real directories",
        ));
    }
    Ok(())
}

pub(crate) fn open_cache_file(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt as _;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        options.share_mode(
            windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
        );
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    let is_reparse = metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0;
    #[cfg(not(windows))]
    let is_reparse = false;
    if !metadata.is_file() || is_reparse {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache entry must be a regular, non-link file",
        ));
    }
    Ok(file)
}

pub(crate) fn open_cache_lock(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt as _;
    #[cfg(windows)]
    use std::os::windows::fs::OpenOptionsExt as _;

    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        options.share_mode(
            windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
        );
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    let is_reparse = metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0;
    #[cfg(not(windows))]
    let is_reparse = false;
    if !metadata.is_file() || is_reparse {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cache lock must be a regular, non-link file",
        ));
    }
    Ok(file)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "nia-loader-cache-fs-{name}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn namespace_rejects_intermediate_symlink() {
        let root = test_root("namespace");
        let outside = test_root("namespace-outside");
        fs::create_dir_all(&root).expect("create cache root");
        fs::create_dir_all(&outside).expect("create outside root");
        symlink(&outside, root.join("artifacts")).expect("create namespace symlink");

        let error = ensure_cache_directory(&root, &root.join("artifacts/frontend"))
            .expect_err("namespace symlink must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!outside.join("frontend").exists());

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn cache_file_open_rejects_symlink() {
        let root = test_root("record");
        fs::create_dir_all(&root).expect("create cache root");
        let target = root.join("target");
        let link = root.join("link");
        fs::write(&target, b"cache record").expect("write cache record");
        symlink(&target, &link).expect("create cache symlink");

        assert!(open_cache_file(&link).is_err());

        let _ = fs::remove_dir_all(root);
    }
}
