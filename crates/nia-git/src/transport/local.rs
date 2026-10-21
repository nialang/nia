// SPDX-License-Identifier: GPL-3.0-or-later
//! Local repository checkout transport.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Component, Path};

#[cfg(test)]
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};

use crate::invalid_packet;
#[cfg(all(test, unix))]
use crate::object::read_loose_object;
use crate::object::{
    GitObject, ObjectKind, decode_pack_objects, encode_hex, open_directory_path, open_subdirectory,
    parse_object_id, read_loose_object_in, resolve_revision_in,
};

const MAX_TREE_DEPTH: usize = 64;
const MAX_CHECKOUT_ENTRIES: usize = 1_000_000;
const MAX_CHECKOUT_BYTES: usize = 512 * 1024 * 1024;

pub(crate) fn checkout_local_into(
    repository: &Path,
    revision: &str,
    destination_dir: &Dir,
) -> io::Result<String> {
    if destination_dir.entries()?.next().transpose()?.is_some() {
        return Err(invalid_packet("Git checkout destination must be empty"));
    }
    checkout_repository(repository, revision, destination_dir)
}

fn checkout_repository(
    repository: &Path,
    revision: &str,
    destination_dir: &Dir,
) -> io::Result<String> {
    let repository_dir = open_directory_path(repository)?;
    let git_dir = match open_subdirectory(&repository_dir, OsStr::new(".git")) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => repository_dir,
        Err(error) => return Err(error),
    };
    let commit_id = resolve_revision_in(&git_dir, revision)?;
    checkout_commit_with(&commit_id, destination_dir, |id| {
        read_loose_object_in(&git_dir, id)
    })?;
    Ok(encode_hex(&commit_id))
}

fn checkout_decoded_pack(
    objects: &std::collections::BTreeMap<[u8; 20], GitObject>,
    commit_id: &[u8; 20],
    destination_dir: &Dir,
) -> io::Result<String> {
    checkout_commit_with(commit_id, destination_dir, |id| {
        objects.get(id).cloned().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Git packed object id was not found",
            )
        })
    })?;
    Ok(encode_hex(commit_id))
}

fn checkout_commit_with(
    commit_id: &[u8; 20],
    destination_dir: &Dir,
    mut read_object: impl FnMut(&[u8; 20]) -> io::Result<GitObject>,
) -> io::Result<()> {
    let commit = read_object(commit_id)?;
    if commit.kind != ObjectKind::Commit {
        return Err(invalid_packet("selected Git revision is not a commit"));
    }
    let tree_line = commit
        .data
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(b"tree "))
        .ok_or_else(|| invalid_packet("Git commit has no tree"))?;
    let tree_id = parse_object_id(tree_line)?;
    checkout_tree_with(&tree_id, destination_dir, &mut read_object)
}

#[cfg(test)]
fn prepare_destination(path: &Path) -> io::Result<()> {
    // Check every existing ancestor, including parents of an existing staging
    // directory. Tree output is only written into this private, empty root.
    let absolute = std::path::absolute(path)?;
    for ancestor in absolute.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata)
                if nia_compat::is_link_or_reparse_point(&metadata) || !metadata.is_dir() =>
            {
                return Err(invalid_packet(
                    "Git checkout destination must use real directories",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    nia_compat::create_real_directory(&absolute)?;
    if std::fs::read_dir(&absolute)?.next().transpose()?.is_some() {
        return Err(invalid_packet("Git checkout destination must be empty"));
    }
    Ok(())
}

#[cfg(all(test, unix))]
fn checkout_tree(git_dir: &Path, tree_id: &[u8; 20], directory: &Dir) -> io::Result<()> {
    checkout_tree_with(tree_id, directory, &mut |id| read_loose_object(git_dir, id))
}

fn checkout_tree_with(
    tree_id: &[u8; 20],
    directory: &Dir,
    read_object: &mut impl FnMut(&[u8; 20]) -> io::Result<GitObject>,
) -> io::Result<()> {
    struct Frame {
        directory: Dir,
        data: Vec<u8>,
        offset: usize,
    }
    let mut bytes = 0usize;
    let mut entries = 0usize;
    let mut stack = vec![Frame {
        directory: directory.try_clone()?,
        data: load_tree(read_object, tree_id, &mut bytes)?,
        offset: 0,
    }];
    while !stack.is_empty() {
        let depth = stack.len();
        let frame = stack.last_mut().expect("stack is not empty");
        if frame.offset == frame.data.len() {
            stack.pop();
            continue;
        }
        if entries == MAX_CHECKOUT_ENTRIES {
            return Err(invalid_packet("Git checkout exceeds entry limit"));
        }
        entries += 1;
        let mut input = &frame.data[frame.offset..];
        let mode_end = input
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| invalid_packet("Git tree entry has no mode separator"))?;
        let mode = std::str::from_utf8(&input[..mode_end])
            .map_err(|_| invalid_packet("Git tree mode is not ASCII"))?;
        input = &input[mode_end + 1..];
        let name_end = input
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| invalid_packet("Git tree entry has no name terminator"))?;
        let name = std::str::from_utf8(&input[..name_end])
            .map_err(|_| invalid_packet("Git tree path is not UTF-8"))?;
        validate_tree_name(name)?;
        input = &input[name_end + 1..];
        if input.len() < 20 {
            return Err(invalid_packet("truncated Git tree object id"));
        }
        let id: [u8; 20] = input[..20].try_into().expect("length checked");
        input = &input[20..];
        frame.offset = frame.data.len() - input.len();
        match mode {
            "40000" | "040000" => {
                if depth > MAX_TREE_DEPTH {
                    return Err(invalid_packet("Git checkout exceeds tree depth limit"));
                }
                let data = load_tree(read_object, &id, &mut bytes)?;
                frame.directory.create_dir(name)?;
                let child = frame.directory.open_dir(name)?;
                stack.push(Frame {
                    directory: child,
                    data,
                    offset: 0,
                });
            }
            "100644" | "100755" | "100664" => {
                let blob = read_object(&id)?;
                if blob.kind != ObjectKind::Blob {
                    return Err(invalid_packet("Git file entry does not name a blob"));
                }
                charge_checkout_bytes(&mut bytes, blob.data.len())?;
                write_new_file(&frame.directory, Path::new(name), &blob.data)?;
            }
            _ => return Err(invalid_packet("unsupported Git tree entry mode")),
        }
    }
    Ok(())
}

fn load_tree(
    read_object: &mut impl FnMut(&[u8; 20]) -> io::Result<GitObject>,
    id: &[u8; 20],
    bytes: &mut usize,
) -> io::Result<Vec<u8>> {
    let tree = read_object(id)?;
    if tree.kind != ObjectKind::Tree {
        return Err(invalid_packet("Git tree entry does not name a tree"));
    }
    charge_checkout_bytes(bytes, tree.data.len())?;
    Ok(tree.data)
}

fn charge_checkout_bytes(used: &mut usize, amount: usize) -> io::Result<()> {
    if amount > MAX_CHECKOUT_BYTES.saturating_sub(*used) {
        return Err(invalid_packet("Git checkout exceeds decoded size limit"));
    }
    *used += amount;
    Ok(())
}

fn validate_tree_name(name: &str) -> io::Result<()> {
    let path = Path::new(name);
    let folded = name.to_ascii_lowercase();
    let stem = folded.split('.').next().unwrap_or_default();
    let device = matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
        || ["com", "lpt"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
    if name.is_empty()
        || name.len() > 255
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        || name.chars().any(char::is_control)
        || name.ends_with(['.', ' '])
        || folded == ".git"
        || folded == "git~1"
        || device
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
        || name == "."
        || name == ".."
    {
        return Err(invalid_packet("Git tree contains an unsafe path"));
    }
    Ok(())
}

pub(crate) fn write_pack_checkout_into(
    pack: &[u8],
    commit: &[u8; 20],
    destination_dir: &Dir,
) -> io::Result<String> {
    if destination_dir.entries()?.next().transpose()?.is_some() {
        return Err(invalid_packet("Git checkout destination must be empty"));
    }
    let objects = decode_pack_objects(pack)?;
    checkout_decoded_pack(&objects, commit, destination_dir)
}

fn write_new_file(directory: &Dir, path: &Path, data: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = directory.open_with(path, &options)?;
    file.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout_local(repository: &Path, revision: &str, destination: &Path) -> io::Result<String> {
        std::fs::create_dir_all(destination)?;
        let directory = Dir::open_ambient_dir(destination, ambient_authority())?;
        checkout_local_into(repository, revision, &directory)
    }

    fn open_destination(path: &Path) -> Dir {
        Dir::open_ambient_dir(path, ambient_authority()).unwrap()
    }
    use flate2::{Compression, write::ZlibEncoder};
    use sha1::{Digest, Sha1};

    fn store_object(repository: &Path, kind: &str, data: &[u8]) -> [u8; 20] {
        let mut bytes = format!("{kind} {}\0", data.len()).into_bytes();
        bytes.extend_from_slice(data);
        let id: [u8; 20] = Sha1::digest(&bytes).into();
        let hex = encode_hex(&id);
        let object_dir = repository.join("objects").join(&hex[..2]);
        std::fs::create_dir_all(&object_dir).unwrap();
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&bytes).unwrap();
        std::fs::write(object_dir.join(&hex[2..]), encoder.finish().unwrap()).unwrap();
        id
    }

    fn tree_entry(mode: &str, name: &str, id: &[u8; 20]) -> Vec<u8> {
        let mut entry = format!("{mode} {name}\0").into_bytes();
        entry.extend_from_slice(id);
        entry
    }

    fn set_head(repository: &Path, tree: &[u8; 20]) {
        let commit = store_object(
            repository,
            "commit",
            format!("tree {}\n", encode_hex(tree)).as_bytes(),
        );
        std::fs::write(repository.join("HEAD"), encode_hex(&commit)).unwrap();
    }

    #[test]
    fn existing_destination_content_is_preserved_for_every_checkout_path() {
        let root = nia_test_support::test_dir("git-checkout-existing");
        let destination = root.join("destination");
        std::fs::create_dir_all(destination.join(".git")).unwrap();
        std::fs::write(destination.join("sentinel"), b"keep").unwrap();
        std::fs::write(destination.join(".git/HEAD"), b"keep head").unwrap();
        assert!(checkout_local(&root.join("absent"), "HEAD", &destination).is_err());
        let destination_dir = open_destination(&destination);
        assert!(write_pack_checkout_into(&[0; 32], &[0; 20], &destination_dir).is_err());
        assert_eq!(
            std::fs::read(destination.join("sentinel")).unwrap(),
            b"keep"
        );
        assert_eq!(
            std::fs::read(destination.join(".git/HEAD")).unwrap(),
            b"keep head"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn linked_destination_and_linked_parent_are_rejected() {
        let root = nia_test_support::test_dir("git-checkout-linked-root");
        let outside = root.join("outside");
        let link = root.join("link");
        std::fs::create_dir(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        {
            use nia_test_support::CommandExt;
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .output_timeout_for_runtime("create checkout junction fixture");
            assert!(output.status.success(), "{output:?}");
        }
        for destination in [&link, &link.join("nested")] {
            assert_eq!(
                prepare_destination(destination).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        // Remove the link itself before the scratch directory guard runs.
        #[cfg(windows)]
        std::fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(&link).unwrap();
    }

    #[test]
    fn checkout_rejects_metadata_entries_and_duplicate_files() {
        let root = nia_test_support::test_dir("git-checkout-malicious-tree");
        let repository = root.join("repository");
        let blob = store_object(&repository, "blob", b"first");
        for name in [".git", "a\\b", "file:stream", "NUL"] {
            let tree = store_object(&repository, "tree", &tree_entry("100644", name, &blob));
            set_head(&repository, &tree);
            let destination = root.join(format!("checkout-{}", encode_hex(&tree)));
            assert_eq!(
                checkout_local(&repository, "HEAD", &destination)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
            assert!(std::fs::read_dir(destination).unwrap().next().is_none());
        }
        let other = store_object(&repository, "blob", b"overwrite");
        let mut entries = tree_entry("100644", "file", &blob);
        entries.extend(tree_entry("100644", "file", &other));
        let tree = store_object(&repository, "tree", &entries);
        set_head(&repository, &tree);
        let destination = root.join("duplicates");
        assert_eq!(
            checkout_local(&repository, "HEAD", &destination)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(destination.join("file")).unwrap(), b"first");
    }

    #[test]
    fn checkout_accepts_empty_staging_and_bounds_tree_depth() {
        let root = nia_test_support::test_dir("git-checkout-depth");
        let repository = root.join("repository");
        let mut tree = store_object(&repository, "tree", &[]);
        for _ in 0..MAX_TREE_DEPTH {
            tree = store_object(&repository, "tree", &tree_entry("40000", "d", &tree));
        }
        set_head(&repository, &tree);
        let destination = root.join("at-limit");
        std::fs::create_dir(&destination).unwrap();
        checkout_local(&repository, "HEAD", &destination).unwrap();
        tree = store_object(&repository, "tree", &tree_entry("40000", "d", &tree));
        set_head(&repository, &tree);
        let error = checkout_local(&repository, "HEAD", &root.join("over-limit")).unwrap_err();
        assert_eq!(error.to_string(), "Git checkout exceeds tree depth limit");
        let mut used = MAX_CHECKOUT_BYTES - 1;
        charge_checkout_bytes(&mut used, 1).unwrap();
        assert!(charge_checkout_bytes(&mut used, 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn checkout_writes_through_open_directory_after_path_replacement() {
        let root = nia_test_support::test_dir("git-checkout-root-replacement");
        let repository = root.join("repository");
        let blob = store_object(&repository, "blob", b"inside pinned root");
        let tree = store_object(&repository, "tree", &tree_entry("100644", "file", &blob));
        let destination = root.join("destination");
        let moved = root.join("moved-destination");
        let outside = root.join("outside");
        std::fs::create_dir(&destination).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let destination_dir = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
        std::fs::rename(&destination, &moved).unwrap();
        std::os::unix::fs::symlink(&outside, &destination).unwrap();

        checkout_tree(&repository, &tree, &destination_dir).unwrap();

        assert_eq!(
            std::fs::read(moved.join("file")).unwrap(),
            b"inside pinned root"
        );
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        std::fs::remove_file(&destination).unwrap();
    }

    #[test]
    fn invalid_pack_leaves_checkout_destination_empty() {
        let root = nia_test_support::test_dir("git-checkout-pack-cleanup");
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        let destination_dir = open_destination(&destination);
        assert!(write_pack_checkout_into(&[0; 32], &[0; 20], &destination_dir).is_err());
        assert!(std::fs::read_dir(destination).unwrap().next().is_none());
    }

    #[test]
    fn tree_names_reject_metadata_and_cross_platform_aliases() {
        for name in [
            ".git",
            ".GiT",
            "git~1",
            ".git.",
            "file.",
            "file ",
            "file:stream",
            "a\\b",
            "a/b",
            "NUL",
            "con.txt",
            "LPT1",
            "COM9.log",
            "..",
            ".",
            "",
        ] {
            assert!(validate_tree_name(name).is_err(), "accepted {name:?}");
        }
        for name in [
            "nia.toml",
            "src",
            ".github",
            "console.nia",
            "hello world.nia",
            "模块.nia",
        ] {
            validate_tree_name(name).unwrap();
        }
    }
}
