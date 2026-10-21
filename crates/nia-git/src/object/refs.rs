// SPDX-License-Identifier: GPL-3.0-or-later
//! Git object lookup and revision/ref resolution.

use std::{io, path::Path};

use super::read_packed_object;
use super::{GitObject, ObjectKind, decode_loose_object};
use crate::{input::read_limited_file, invalid_packet};

const MAX_REF_BYTES: usize = 4096;
const MAX_PACKED_REFS_BYTES: usize = 16 * 1024 * 1024;
const MAX_SYMBOLIC_REF_DEPTH: usize = 8;
const MAX_TAG_DEPTH: usize = 8;

fn validate_ref_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name.len() > 1024
        || name == "@"
        || name.contains([':', '\\', '?', '*', '[', '^'])
        || name.contains("..")
        || name.contains("@{")
        || name.bytes().any(|byte| byte <= b' ' || byte == 0x7f)
        || name.split('/').any(|part| {
            part.is_empty()
                || part.starts_with('.')
                || part.ends_with('.')
                || part.to_ascii_lowercase().ends_with(".lock")
        })
    {
        return Err(invalid_packet("Git reference name is invalid"));
    }
    Ok(())
}

fn read_ref_text(git_dir: &Path, path: &Path, limit: usize) -> io::Result<String> {
    let relative = path
        .strip_prefix(git_dir)
        .map_err(|_| invalid_packet("Git reference path escapes repository"))?;
    let mut current = git_dir.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(invalid_packet("Git reference path escapes repository"));
        }
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current)?;
        if nia_compat::is_link_or_reparse_point(&metadata) {
            return Err(invalid_packet("Git reference path contains a link"));
        }
    }
    let bytes = read_limited_file(path, limit, "Git reference file exceeds size limit")?;
    String::from_utf8(bytes).map_err(|_| invalid_packet("Git reference file is not UTF-8"))
}

pub(crate) fn resolve_revision(git_dir: &Path, revision: &str) -> io::Result<[u8; 20]> {
    if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let id = parse_object_id(revision.as_bytes())?;
        return peel_commit(git_dir, id);
    }
    if revision == "HEAD" {
        return peel_commit(
            git_dir,
            resolve_ref_file(git_dir, &git_dir.join("HEAD"), 0)?,
        );
    }
    validate_ref_name(revision)?;
    let candidates = if revision.starts_with("refs/") {
        vec![revision.to_owned()]
    } else {
        vec![
            format!("refs/heads/{revision}"),
            format!("refs/tags/{revision}"),
            format!("refs/{revision}"),
        ]
    };
    for target in candidates {
        match resolve_symbolic_ref(git_dir, &target, 0) {
            Ok(id) => return peel_commit(git_dir, id),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git revision was not found",
    ))
}

fn peel_commit(git_dir: &Path, mut id: [u8; 20]) -> io::Result<[u8; 20]> {
    let mut depth = 0;
    loop {
        let object = read_loose_object(git_dir, &id)?;
        match object.kind {
            ObjectKind::Commit => return Ok(id),
            ObjectKind::Tag if depth < MAX_TAG_DEPTH => {
                let target = object
                    .data
                    .split(|byte| *byte == b'\n')
                    .find_map(|line| line.strip_prefix(b"object "))
                    .ok_or_else(|| invalid_packet("Git tag has no target object"))?;
                id = parse_object_id(target)?;
                depth += 1;
            }
            ObjectKind::Tag => {
                return Err(invalid_packet("Git tag exceeds dereference depth limit"));
            }
            _ => return Err(invalid_packet("Git reference does not name a commit")),
        }
    }
}

fn resolve_symbolic_ref(git_dir: &Path, target: &str, depth: usize) -> io::Result<[u8; 20]> {
    validate_ref_name(target)?;
    if depth > MAX_SYMBOLIC_REF_DEPTH || !target.starts_with("refs/") {
        return Err(invalid_packet("Git symbolic reference is invalid"));
    }
    match read_ref_text(git_dir, &git_dir.join(target), MAX_REF_BYTES) {
        Ok(text) => resolve_ref_text(git_dir, &text, depth),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            resolve_packed_ref(git_dir, target)
        }
        Err(error) => Err(error),
    }
}

fn resolve_packed_ref(git_dir: &Path, target: &str) -> io::Result<[u8; 20]> {
    let packed = read_ref_text(git_dir, &git_dir.join("packed-refs"), MAX_PACKED_REFS_BYTES)?;
    for line in packed.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((id, name)) = line.split_once(' ')
            && name == target
        {
            return parse_object_id(id.as_bytes());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git reference was not found",
    ))
}

fn resolve_ref_file(git_dir: &Path, path: &Path, depth: usize) -> io::Result<[u8; 20]> {
    let text = read_ref_text(git_dir, path, MAX_REF_BYTES)?;
    resolve_ref_text(git_dir, &text, depth)
}

fn resolve_ref_text(git_dir: &Path, text: &str, depth: usize) -> io::Result<[u8; 20]> {
    let line = text.trim();
    if let Some(target) = line.strip_prefix("ref: ") {
        resolve_symbolic_ref(git_dir, target, depth + 1)
    } else {
        parse_object_id(line.as_bytes())
    }
}

pub(crate) fn read_loose_object(git_dir: &Path, id: &[u8; 20]) -> io::Result<GitObject> {
    let hex = encode_hex(id);
    let path = git_dir.join("objects").join(&hex[..2]).join(&hex[2..]);
    match read_limited_file(
        &path,
        super::MAX_COMPRESSED_OBJECT_BYTES,
        "Git compressed object exceeds size limit",
    ) {
        Ok(compressed) => {
            let object = decode_loose_object(&compressed)?;
            if object.id != *id {
                return Err(invalid_packet("Git object id does not match its contents"));
            }
            Ok(object)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => read_packed_object(git_dir, id),
        Err(error) => Err(error),
    }
}

pub(crate) fn parse_object_id(bytes: &[u8]) -> io::Result<[u8; 20]> {
    if bytes.len() != 40 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(invalid_packet(
            "Git object id must be 40 hexadecimal digits",
        ));
    }
    let mut id = [0; 20];
    for (index, pair) in bytes.chunks(2).enumerate() {
        id[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Ok(id)
}

fn hex_value(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(invalid_packet("invalid hexadecimal digit")),
    }
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_object(repository: &Path, kind: ObjectKind, data: &[u8]) -> [u8; 20] {
        let encoded = super::super::loose::encode_loose_object(kind, data).unwrap();
        let id = decode_loose_object(&encoded).unwrap().id;
        let hex = encode_hex(&id);
        let directory = repository.join("objects").join(&hex[..2]);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(&hex[2..]), encoded).unwrap();
        id
    }

    #[test]
    fn revision_and_symbolic_ref_names_cannot_escape_refs() {
        let root = nia_test_support::test_dir("git-ref-paths");
        for name in [
            "../outside",
            "refs/../outside",
            "refs//heads/main",
            "/absolute",
            "C:/outside",
            "a\\b",
            "branch.lock",
            "branch name",
            "a/.hidden",
            "a@{b",
            "a^{}",
        ] {
            assert_eq!(
                resolve_revision(&root, name).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{name}"
            );
            assert_eq!(
                resolve_symbolic_ref(&root, name, 0).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{name}"
            );
        }
        for name in ["main", "feature/topic", "refs/heads/main", "release-1.2"] {
            validate_ref_name(name).unwrap();
        }
        std::fs::write(root.join("HEAD"), b"ref: refs/../outside\n").unwrap();
        assert_eq!(
            resolve_revision(&root, "HEAD").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn loose_and_packed_refs_resolve_the_same_commit_and_tags() {
        let root = nia_test_support::test_dir("git-ref-storage");
        let commit = store_object(
            &root,
            ObjectKind::Commit,
            b"tree 0000000000000000000000000000000000000000\n",
        );
        let tag = store_object(
            &root,
            ObjectKind::Tag,
            format!("object {}\ntype commit\ntag version\n", encode_hex(&commit)).as_bytes(),
        );
        std::fs::create_dir_all(root.join("refs/heads")).unwrap();
        std::fs::create_dir_all(root.join("refs/tags")).unwrap();
        std::fs::write(root.join("refs/heads/main"), encode_hex(&commit)).unwrap();
        std::fs::write(root.join("refs/tags/version"), encode_hex(&tag)).unwrap();
        std::fs::write(root.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        for packed in [false, true] {
            if packed {
                std::fs::write(
                    root.join("packed-refs"),
                    format!(
                        "{} refs/heads/main\n{} refs/tags/version\n^{}\n",
                        encode_hex(&commit),
                        encode_hex(&tag),
                        encode_hex(&commit)
                    ),
                )
                .unwrap();
                std::fs::remove_file(root.join("refs/heads/main")).unwrap();
                std::fs::remove_file(root.join("refs/tags/version")).unwrap();
            }
            for name in [
                "main",
                "refs/heads/main",
                "HEAD",
                "version",
                "refs/tags/version",
                &encode_hex(&tag),
            ] {
                assert_eq!(
                    resolve_revision(&root, name).unwrap(),
                    commit,
                    "{name}, packed={packed}"
                );
            }
        }
    }

    #[test]
    fn symbolic_ref_cycles_are_bounded_and_broken_loose_refs_do_not_fall_back() {
        let root = nia_test_support::test_dir("git-ref-cycles");
        std::fs::create_dir_all(root.join("refs/heads")).unwrap();
        std::fs::write(root.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        std::fs::write(root.join("refs/heads/main"), b"ref: refs/heads/main\n").unwrap();
        assert_eq!(
            resolve_revision(&root, "HEAD").unwrap_err().to_string(),
            "Git symbolic reference is invalid"
        );
        std::fs::write(root.join("refs/heads/main"), b"ref: refs/heads/missing\n").unwrap();
        std::fs::write(
            root.join("packed-refs"),
            format!("{} refs/heads/main\n", "00".repeat(20)),
        )
        .unwrap();
        assert_eq!(
            resolve_symbolic_ref(&root, "refs/heads/main", 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn symbolic_ref_chain_accepts_exact_depth_limit() {
        let root = nia_test_support::test_dir("git-ref-depth");
        let commit = store_object(&root, ObjectKind::Commit, b"commit");
        std::fs::create_dir_all(root.join("refs/heads")).unwrap();
        for depth in 0..MAX_SYMBOLIC_REF_DEPTH {
            std::fs::write(
                root.join(format!("refs/heads/chain-{depth}")),
                format!("ref: refs/heads/chain-{}\n", depth + 1),
            )
            .unwrap();
        }
        std::fs::write(
            root.join(format!("refs/heads/chain-{MAX_SYMBOLIC_REF_DEPTH}")),
            encode_hex(&commit),
        )
        .unwrap();
        assert_eq!(resolve_revision(&root, "chain-0").unwrap(), commit);
        std::fs::write(root.join("HEAD"), b"ref: refs/heads/chain-0\n").unwrap();
        assert_eq!(
            resolve_revision(&root, "HEAD").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn nested_tags_must_resolve_to_a_commit_within_depth_limit() {
        let root = nia_test_support::test_dir("git-tag-depth");
        let commit = store_object(&root, ObjectKind::Commit, b"commit");
        let mut id = commit;
        for _ in 0..MAX_TAG_DEPTH {
            id = store_object(
                &root,
                ObjectKind::Tag,
                format!("object {}\n", encode_hex(&id)).as_bytes(),
            );
        }
        assert_eq!(resolve_revision(&root, &encode_hex(&id)).unwrap(), commit);
        id = store_object(
            &root,
            ObjectKind::Tag,
            format!("object {}\n", encode_hex(&id)).as_bytes(),
        );
        assert_eq!(
            resolve_revision(&root, &encode_hex(&id))
                .unwrap_err()
                .to_string(),
            "Git tag exceeds dereference depth limit"
        );
        let blob = store_object(&root, ObjectKind::Blob, b"blob");
        let tag = store_object(
            &root,
            ObjectKind::Tag,
            format!("object {}\n", encode_hex(&blob)).as_bytes(),
        );
        assert_eq!(
            resolve_revision(&root, &encode_hex(&tag))
                .unwrap_err()
                .to_string(),
            "Git reference does not name a commit"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn linked_ref_directories_are_rejected() {
        let root = nia_test_support::test_dir("git-ref-links");
        let repository = root.join("repository");
        let outside = root.join("outside");
        std::fs::create_dir(&repository).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let link = repository.join("refs");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        {
            use nia_test_support::CommandExt;
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(&outside)
                .output_timeout_for_runtime("create reference junction fixture");
            assert!(output.status.success(), "{output:?}");
        }
        assert_eq!(
            resolve_revision(&repository, "main")
                .unwrap_err()
                .to_string(),
            "Git reference path contains a link"
        );
        #[cfg(unix)]
        std::fs::remove_file(link).unwrap();
        #[cfg(windows)]
        std::fs::remove_dir(link).unwrap();
    }
}
