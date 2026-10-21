// SPDX-License-Identifier: GPL-3.0-or-later
//! Git object lookup and revision/ref resolution.

use std::{io, path::Path};

use super::read_packed_object;
use super::{GitObject, ObjectKind, decode_loose_object};
use crate::invalid_packet;

pub(crate) fn resolve_revision(git_dir: &Path, revision: &str) -> io::Result<[u8; 20]> {
    if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let id = parse_object_id(revision.as_bytes())?;
        let object = read_loose_object(git_dir, &id)?;
        if object.kind == ObjectKind::Commit {
            return Ok(id);
        }
    }
    let candidates = [
        git_dir.join("refs").join("heads").join(revision),
        git_dir.join("refs").join("tags").join(revision),
        git_dir.join("refs").join(revision),
    ];
    for path in candidates {
        if path.is_file() {
            let text = std::fs::read_to_string(path)?;
            let line = text.trim();
            let id = if let Some(target) = line.strip_prefix("ref: ") {
                resolve_symbolic_ref(git_dir, target, 0)?
            } else {
                parse_object_id(line.as_bytes())?
            };
            let object = read_loose_object(git_dir, &id)?;
            if object.kind == ObjectKind::Commit {
                return Ok(id);
            }
            if object.kind == ObjectKind::Tag {
                let target = object
                    .data
                    .split(|byte| *byte == b'\n')
                    .find_map(|line| line.strip_prefix(b"object "))
                    .ok_or_else(|| invalid_packet("Git tag has no target object"))?;
                return parse_object_id(target);
            }
            return Err(invalid_packet("Git reference does not name a commit"));
        }
    }
    if revision == "HEAD" {
        return resolve_symbolic_ref_file(git_dir, &git_dir.join("HEAD"), 0);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git revision was not found",
    ))
}

fn resolve_symbolic_ref(git_dir: &Path, target: &str, depth: usize) -> io::Result<[u8; 20]> {
    if depth > 8 || !target.starts_with("refs/") {
        return Err(invalid_packet("Git symbolic reference is invalid"));
    }
    let path = git_dir.join(target);
    if path.is_file() {
        resolve_symbolic_ref_file(git_dir, &path, depth)
    } else {
        resolve_packed_ref(git_dir, target)
    }
}

fn resolve_packed_ref(git_dir: &Path, target: &str) -> io::Result<[u8; 20]> {
    let packed = std::fs::read_to_string(git_dir.join("packed-refs"))?;
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

fn resolve_symbolic_ref_file(git_dir: &Path, path: &Path, depth: usize) -> io::Result<[u8; 20]> {
    let text = std::fs::read_to_string(path)?;
    let line = text.trim();
    let id = if let Some(target) = line.strip_prefix("ref: ") {
        resolve_symbolic_ref(git_dir, target, depth + 1)?
    } else {
        parse_object_id(line.as_bytes())?
    };
    let object = read_loose_object(git_dir, &id)?;
    if object.kind == ObjectKind::Commit {
        Ok(id)
    } else {
        Err(invalid_packet("Git HEAD does not resolve to a commit"))
    }
}

pub(crate) fn read_loose_object(git_dir: &Path, id: &[u8; 20]) -> io::Result<GitObject> {
    let hex = encode_hex(id);
    let path = git_dir.join("objects").join(&hex[..2]).join(&hex[2..]);
    match std::fs::read(path) {
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
