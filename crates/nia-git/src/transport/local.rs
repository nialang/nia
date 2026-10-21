// SPDX-License-Identifier: GPL-3.0-or-later
//! Local repository checkout transport.

use std::io;
use std::path::{Component, Path};

use crate::invalid_packet;
use crate::object::{ObjectKind, encode_hex, parse_object_id, read_loose_object, resolve_revision};

pub(crate) fn checkout_local(
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
    fs_create_dir(destination)?;
    checkout_tree(&git_dir, &tree_id, destination)?;
    Ok(encode_hex(&commit_id))
}

pub(crate) fn fs_create_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

fn checkout_tree(git_dir: &Path, tree_id: &[u8; 20], directory: &Path) -> io::Result<()> {
    let tree = read_loose_object(git_dir, tree_id)?;
    if tree.kind != ObjectKind::Tree {
        return Err(invalid_packet("Git tree entry does not name a tree"));
    }
    let mut input = tree.data.as_slice();
    while !input.is_empty() {
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
        let path = directory.join(name);
        match mode {
            "40000" | "040000" => {
                fs_create_dir(&path)?;
                checkout_tree(git_dir, &id, &path)?;
            }
            "100644" | "100755" | "100664" => {
                let blob = read_loose_object(git_dir, &id)?;
                if blob.kind != ObjectKind::Blob {
                    return Err(invalid_packet("Git file entry does not name a blob"));
                }
                if let Some(parent) = path.parent() {
                    fs_create_dir(parent)?;
                }
                std::fs::write(path, blob.data)?;
            }
            _ => return Err(invalid_packet("unsupported Git tree entry mode")),
        }
    }
    Ok(())
}

fn validate_tree_name(name: &str) -> io::Result<()> {
    let path = Path::new(name);
    if name.is_empty()
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
    let git_dir = destination.join(".git");
    let pack_dir = git_dir.join("objects/pack");
    fs_create_dir(&pack_dir)?;
    std::fs::write(git_dir.join("HEAD"), encode_hex(commit))?;
    let pack_id: [u8; 20] = pack
        .get(pack.len().saturating_sub(20)..)
        .filter(|bytes| bytes.len() == 20)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid_packet("Git upload-pack response has no pack checksum"))?;
    std::fs::write(
        pack_dir.join(format!("pack-{}.pack", encode_hex(&pack_id))),
        pack,
    )?;
    let result = checkout_local(destination, "HEAD", destination);
    let _ = std::fs::remove_dir_all(&git_dir);
    result
}
