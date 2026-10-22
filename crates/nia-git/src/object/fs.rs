// SPDX-License-Identifier: GPL-3.0-or-later
//! Handle-relative Git repository filesystem access.

use std::{
    ffi::OsStr,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};

use crate::invalid_packet;

pub(crate) fn open_directory_path(path: &Path) -> io::Result<Dir> {
    let absolute = std::path::absolute(path)?;
    let mut root_path = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => root_path.push(prefix.as_os_str()),
            Component::RootDir => {
                root_path.push(component.as_os_str());
                break;
            }
            _ => {}
        }
    }
    let mut directory = Dir::open_ambient_dir(&root_path, ambient_authority())?;
    for component in absolute.components() {
        match component {
            Component::Normal(name) => directory = open_subdirectory(&directory, name)?,
            Component::ParentDir => directory = directory.open_parent_dir(ambient_authority())?,
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
        }
    }
    Ok(directory)
}

pub(crate) fn open_subdirectory(parent: &Dir, name: &OsStr) -> io::Result<Dir> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let file = parent.open_with(name, &options)?.into_std();
    let metadata = file.metadata()?;
    if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(invalid_packet(
            "Git repository path component must be a real directory",
        ));
    }
    Ok(Dir::from_std_file(file))
}

pub(crate) fn read_bounded_file(
    parent: &Dir,
    name: &OsStr,
    limit: usize,
    message: &'static str,
) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = parent.open_with(name, &options)?.into_std();
    let metadata = file.metadata()?;
    if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(invalid_packet("Git repository file must be a real file"));
    }
    if metadata.len() > limit as u64 {
        return Err(invalid_packet(message));
    }
    let mut contents = Vec::new();
    file.take((limit as u64).saturating_add(1))
        .read_to_end(&mut contents)?;
    if contents.len() > limit {
        return Err(invalid_packet(message));
    }
    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn opening_a_directory_path_rejects_linked_components() {
        let root = nia_test_support::test_dir("git-open-directory-link");
        let real = root.join("real");
        let link = root.join("link");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(open_directory_path(&link).is_err());
        assert!(open_directory_path(&link.join("child")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn opening_a_directory_path_rejects_reparse_components() {
        let root = nia_test_support::test_dir("git-open-directory-junction");
        let real = root.join("real");
        let link = root.join("link");
        std::fs::create_dir(&real).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&real)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        assert!(open_directory_path(&link).is_err());
        assert!(open_directory_path(&link.join("child")).is_err());
        std::fs::remove_dir(&link).unwrap();
    }
}
