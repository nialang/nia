// SPDX-License-Identifier: GPL-3.0-or-later
//! Git packfile reading, validation, and object decoding.

use std::{
    collections::BTreeMap,
    io::{self, Read},
    path::Path,
};

use flate2::read::ZlibDecoder;
use sha1::{Digest, Sha1};

use super::MAX_OBJECT_BYTES;
use super::{GitObject, ObjectKind, apply_delta};
use crate::invalid_packet;

pub(crate) fn read_packed_object(git_dir: &Path, wanted: &[u8; 20]) -> io::Result<GitObject> {
    let pack_dir = git_dir.join("objects").join("pack");
    let entries = match std::fs::read_dir(&pack_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Git object was not found in loose or packed storage",
            ));
        }
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("pack") {
            continue;
        }
        let pack = std::fs::read(&path)?;
        match decode_pack_object(&pack, wanted) {
            Ok(object) => return Ok(object),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git object was not found in packed storage",
    ))
}

fn decode_pack_object(pack: &[u8], wanted: &[u8; 20]) -> io::Result<GitObject> {
    if pack.len() < 12 || &pack[..4] != b"PACK" {
        return Err(invalid_packet("Git pack header is malformed"));
    }
    if pack.len() < 32 {
        return Err(invalid_packet("Git pack is missing its checksum"));
    }
    let payload_end = pack.len() - 20;
    let mut checksum = Sha1::new();
    checksum.update(&pack[..payload_end]);
    let checksum: [u8; 20] = checksum.finalize().into();
    if checksum != pack[payload_end..] {
        return Err(invalid_packet("Git pack checksum does not match"));
    }
    let version = u32::from_be_bytes(pack[4..8].try_into().expect("length checked"));
    if !matches!(version, 2 | 3) {
        return Err(invalid_packet("unsupported Git pack version"));
    }
    let count = u32::from_be_bytes(pack[8..12].try_into().expect("length checked"));
    if count > 1_000_000 {
        return Err(invalid_packet("Git pack contains too many objects"));
    }
    let mut cursor = 12usize;
    let mut objects = BTreeMap::<usize, GitObject>::new();
    let mut unresolved = Vec::new();
    for _ in 0..count {
        let offset = cursor;
        let (kind, declared_size) = read_pack_header(pack, &mut cursor)?;
        match kind {
            1..=4 => {
                let (data, consumed) =
                    read_zlib_stream(&pack[cursor..payload_end], Some(declared_size))?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                let object = make_object(pack_kind(kind)?, data)?;
                objects.insert(offset, object);
            }
            6 => {
                let base_offset = read_ofs_delta_base(pack, &mut cursor, offset)?;
                let (delta, consumed) = read_zlib_stream(&pack[cursor..payload_end], None)?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                unresolved.push((offset, base_offset, None, delta));
            }
            7 => {
                if pack.len().saturating_sub(cursor) < 20 {
                    return Err(invalid_packet("truncated Git ref-delta base id"));
                }
                let base_id: [u8; 20] = pack[cursor..cursor + 20]
                    .try_into()
                    .expect("length checked");
                cursor += 20;
                let (delta, consumed) = read_zlib_stream(&pack[cursor..payload_end], None)?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                unresolved.push((offset, 0, Some(base_id), delta));
            }
            _ => return Err(invalid_packet("unsupported Git pack object type")),
        }
    }
    while !unresolved.is_empty() {
        let mut progress = false;
        let mut remaining = Vec::with_capacity(unresolved.len());
        for (offset, base_offset, base_id, delta) in unresolved {
            let base = if let Some(base_id) = base_id {
                objects
                    .values()
                    .find(|object| object.id == base_id)
                    .cloned()
            } else {
                objects.get(&base_offset).cloned()
            };
            let Some(base) = base else {
                remaining.push((offset, base_offset, base_id, delta));
                continue;
            };
            let data = apply_delta(&base.data, &delta)?;
            let object = make_object(base.kind, data)?;
            objects.insert(offset, object);
            progress = true;
        }
        if !progress {
            return Err(invalid_packet("Git delta base object is missing"));
        }
        unresolved = remaining;
    }
    if cursor != payload_end {
        return Err(invalid_packet("Git pack has trailing object data"));
    }
    objects
        .into_values()
        .find(|object| &object.id == wanted)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Git packed object id was not found",
            )
        })
}

fn read_pack_header(pack: &[u8], cursor: &mut usize) -> io::Result<(u8, usize)> {
    let first = *pack
        .get(*cursor)
        .ok_or_else(|| invalid_packet("truncated Git pack object header"))?;
    *cursor += 1;
    let kind = (first >> 4) & 7;
    let mut size = usize::from(first & 0x0f);
    let mut shift = 4;
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = *pack
            .get(*cursor)
            .ok_or_else(|| invalid_packet("truncated Git pack object size"))?;
        *cursor += 1;
        size |= usize::from(byte & 0x7f)
            .checked_shl(shift)
            .ok_or_else(|| invalid_packet("Git pack object size overflows"))?;
        shift += 7;
        if shift > 63 || size > MAX_OBJECT_BYTES {
            return Err(invalid_packet("Git pack object exceeds size limit"));
        }
    }
    Ok((kind, size))
}

fn read_ofs_delta_base(pack: &[u8], cursor: &mut usize, object_offset: usize) -> io::Result<usize> {
    let first = *pack
        .get(*cursor)
        .ok_or_else(|| invalid_packet("truncated Git ofs-delta base offset"))?;
    *cursor += 1;
    let mut value = usize::from(first & 0x7f);
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = *pack
            .get(*cursor)
            .ok_or_else(|| invalid_packet("truncated Git ofs-delta base offset"))?;
        *cursor += 1;
        value = value
            .checked_add(1)
            .and_then(|value| value.checked_shl(7))
            .and_then(|value| value.checked_add(usize::from(byte & 0x7f)))
            .ok_or_else(|| invalid_packet("Git ofs-delta base offset overflows"))?;
    }
    object_offset
        .checked_sub(value)
        .ok_or_else(|| invalid_packet("Git ofs-delta base offset is invalid"))
}

fn read_zlib_stream(input: &[u8], expected_size: Option<usize>) -> io::Result<(Vec<u8>, usize)> {
    let mut decoder = ZlibDecoder::new(input);
    let mut data = Vec::new();
    (&mut decoder)
        .take((expected_size.unwrap_or(MAX_OBJECT_BYTES) + 1) as u64)
        .read_to_end(&mut data)?;
    if data.len() > MAX_OBJECT_BYTES
        || expected_size.is_some_and(|expected| data.len() != expected)
        || decoder.total_in() == 0
    {
        return Err(invalid_packet(
            "Git pack object size does not match its header",
        ));
    }
    Ok((data, decoder.total_in() as usize))
}

fn pack_kind(kind: u8) -> io::Result<ObjectKind> {
    match kind {
        1 => Ok(ObjectKind::Commit),
        2 => Ok(ObjectKind::Tree),
        3 => Ok(ObjectKind::Blob),
        4 => Ok(ObjectKind::Tag),
        _ => Err(invalid_packet("unsupported Git pack object type")),
    }
}

fn make_object(kind: ObjectKind, data: Vec<u8>) -> io::Result<GitObject> {
    let name = match kind {
        ObjectKind::Commit => "commit",
        ObjectKind::Tree => "tree",
        ObjectKind::Blob => "blob",
        ObjectKind::Tag => "tag",
    };
    let header = format!("{name} {}\0", data.len());
    let mut hash = Sha1::new();
    hash.update(header.as_bytes());
    hash.update(&data);
    Ok(GitObject {
        kind,
        data,
        id: hash.finalize().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::decode_pack_object;
    use sha1::{Digest, Sha1};

    #[test]
    fn packed_object_checksums_are_part_of_the_trust_boundary() {
        let mut pack = b"PACK".to_vec();
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&0u32.to_be_bytes());
        let checksum = Sha1::digest(&pack);
        pack.extend_from_slice(&checksum);
        let wanted = [0; 20];
        assert!(decode_pack_object(&pack, &wanted).is_err());
        *pack.last_mut().unwrap() ^= 1;
        assert_eq!(
            decode_pack_object(&pack, &wanted).unwrap_err().to_string(),
            "Git pack checksum does not match"
        );
    }
}
