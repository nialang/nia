// SPDX-License-Identifier: GPL-3.0-or-later
//! Local repository checkout transport.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use crate::invalid_packet;
use crate::object::{ObjectKind, encode_hex, parse_object_id, read_loose_object, resolve_revision};

const MAX_TREE_DEPTH: usize = 64;
const MAX_CHECKOUT_ENTRIES: usize = 1_000_000;
const MAX_CHECKOUT_BYTES: usize = 512 * 1024 * 1024;

pub(crate) fn checkout_local(
    repository: &Path,
    revision: &str,
    destination: &Path,
) -> io::Result<String> {
    prepare_destination(destination)?;
    checkout_repository(repository, revision, destination)
}

fn checkout_repository(
    repository: &Path,
    revision: &str,
    destination: &Path,
) -> io::Result<String> {
    let git_dir = repository.join(".git");
    let git_dir = if git_dir.is_dir() {
        git_dir
    } else {
        repository.to_path_buf()
    };
    let commit_id = resolve_revision(&git_dir, revision)?;
    let commit = read_loose_object(&git_dir, &commit_id)?;
    if commit.kind != ObjectKind::Commit {
        return Err(invalid_packet("selected Git revision is not a commit"));
    }
    let tree_line = commit
        .data
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(b"tree "))
        .ok_or_else(|| invalid_packet("Git commit has no tree"))?;
    let tree_id = parse_object_id(tree_line)?;
    checkout_tree(&git_dir, &tree_id, destination)?;
    Ok(encode_hex(&commit_id))
}

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

fn checkout_tree(git_dir: &Path, tree_id: &[u8; 20], directory: &Path) -> io::Result<()> {
    struct Frame {
        directory: PathBuf,
        data: Vec<u8>,
        offset: usize,
    }
    let mut bytes = 0usize;
    let mut entries = 0usize;
    let load_tree = |id: &[u8; 20], bytes: &mut usize| -> io::Result<Vec<u8>> {
        let tree = read_loose_object(git_dir, id)?;
        if tree.kind != ObjectKind::Tree {
            return Err(invalid_packet("Git tree entry does not name a tree"));
        }
        charge_checkout_bytes(bytes, tree.data.len())?;
        Ok(tree.data)
    };
    let mut stack = vec![Frame {
        directory: directory.to_path_buf(),
        data: load_tree(tree_id, &mut bytes)?,
        offset: 0,
    }];
    while let Some(frame) = stack.last_mut() {
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
        let path = frame.directory.join(name);
        frame.offset = frame.data.len() - input.len();
        match mode {
            "40000" | "040000" => {
                if stack.len() > MAX_TREE_DEPTH {
                    return Err(invalid_packet("Git checkout exceeds tree depth limit"));
                }
                let data = load_tree(&id, &mut bytes)?;
                std::fs::create_dir(&path)?;
                stack.push(Frame {
                    directory: path,
                    data,
                    offset: 0,
                });
            }
            "100644" | "100755" | "100664" => {
                let blob = read_loose_object(git_dir, &id)?;
                if blob.kind != ObjectKind::Blob {
                    return Err(invalid_packet("Git file entry does not name a blob"));
                }
                charge_checkout_bytes(&mut bytes, blob.data.len())?;
                write_new_file(&path, &blob.data)?;
            }
            _ => return Err(invalid_packet("unsupported Git tree entry mode")),
        }
    }
    Ok(())
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

pub(crate) fn write_pack_checkout(
    pack: &[u8],
    commit: &[u8; 20],
    destination: &Path,
) -> io::Result<String> {
    let pack_id: [u8; 20] = pack
        .get(pack.len().saturating_sub(20)..)
        .filter(|bytes| bytes.len() == 20)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid_packet("Git upload-pack response has no pack checksum"))?;
    prepare_destination(destination)?;
    let git_dir = destination.join(".git");
    std::fs::create_dir(&git_dir)?;
    let result = (|| {
        let objects = git_dir.join("objects");
        let pack_dir = objects.join("pack");
        std::fs::create_dir(&objects)?;
        std::fs::create_dir(&pack_dir)?;
        write_new_file(&git_dir.join("HEAD"), encode_hex(commit).as_bytes())?;
        write_new_file(
            &pack_dir.join(format!("pack-{}.pack", encode_hex(&pack_id))),
            pack,
        )?;
        checkout_repository(destination, "HEAD", destination)
    })();
    let cleanup = std::fs::remove_dir_all(&git_dir);
    match result {
        Ok(revision) => {
            cleanup?;
            Ok(revision)
        }
        Err(error) => Err(error),
    }
}

fn write_new_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(write_pack_checkout(&[0; 32], &[0; 20], &destination).is_err());
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

    #[test]
    fn invalid_pack_cleans_up_only_owned_metadata() {
        let root = nia_test_support::test_dir("git-checkout-pack-cleanup");
        let destination = root.join("destination");
        assert!(write_pack_checkout(&[0; 32], &[0; 20], &destination).is_err());
        assert!(!destination.join(".git").exists());
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
