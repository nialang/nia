// SPDX-License-Identifier: GPL-3.0-or-later
//! Git packfile reading, validation, and object decoding.

use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read},
    path::Path,
};

use flate2::read::ZlibDecoder;
use sha1::{Digest, Sha1};

use super::MAX_OBJECT_BYTES;
use super::{GitObject, ObjectKind, apply_delta};
use crate::{input::read_limited_file, invalid_packet};

const MAX_PACK_BYTES: usize = 128 * 1024 * 1024;
const MAX_PACK_OBJECTS: u32 = 1_000_000;
const MAX_DECODED_PACK_BYTES: usize = 512 * 1024 * 1024;
const MAX_DELTA_DEPTH: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DeltaBase {
    Offset(usize),
    Id([u8; 20]),
}

struct PendingDelta {
    offset: usize,
    instructions: Vec<u8>,
}

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
        let pack = read_limited_file(&path, MAX_PACK_BYTES, "Git pack exceeds size limit")?;
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
    decode_pack_object_with_limit(pack, wanted, MAX_DECODED_PACK_BYTES)
}

#[cfg(feature = "fuzzing")]
pub(super) fn fuzz_decode_pack_object(pack: &[u8], wanted: &[u8; 20]) -> io::Result<GitObject> {
    decode_pack_object(pack, wanted)
}

fn decode_pack_object_with_limit(
    pack: &[u8],
    wanted: &[u8; 20],
    remaining_bytes: usize,
) -> io::Result<GitObject> {
    decode_pack_objects_with_limit(pack, remaining_bytes)?
        .remove(wanted)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Git packed object id was not found",
            )
        })
}

pub(crate) fn decode_pack_objects(pack: &[u8]) -> io::Result<BTreeMap<[u8; 20], GitObject>> {
    decode_pack_objects_with_limit(pack, MAX_DECODED_PACK_BYTES)
}

fn decode_pack_objects_with_limit(
    pack: &[u8],
    mut remaining_bytes: usize,
) -> io::Result<BTreeMap<[u8; 20], GitObject>> {
    if pack.len() > MAX_PACK_BYTES {
        return Err(invalid_packet("Git pack exceeds size limit"));
    }
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
    if count > MAX_PACK_OBJECTS {
        return Err(invalid_packet("Git pack contains too many objects"));
    }
    // Object headers and delta bases must never consume the checksum trailer.
    let pack = &pack[..payload_end];
    let mut cursor = 12usize;
    let mut objects = BTreeMap::<usize, GitObject>::new();
    let mut waiting = BTreeMap::<DeltaBase, Vec<PendingDelta>>::new();
    let mut ready = VecDeque::new();
    for _ in 0..count {
        let offset = cursor;
        let (kind, declared_size) = read_pack_header(pack, &mut cursor)?;
        // Charge before inflation. Delta instruction streams and their later
        // reconstructed results both count, even after instructions are freed.
        remaining_bytes = remaining_bytes
            .checked_sub(declared_size)
            .ok_or_else(|| invalid_packet("Git pack exceeds decoded size limit"))?;
        match kind {
            1..=4 => {
                let (data, consumed) = read_zlib_stream(&pack[cursor..payload_end], declared_size)?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                let object = make_object(pack_kind(kind)?, data)?;
                objects.insert(offset, object);
                ready.push_back((offset, 0));
            }
            6 => {
                let base_offset = read_ofs_delta_base(pack, &mut cursor, offset)?;
                let (delta, consumed) =
                    read_zlib_stream(&pack[cursor..payload_end], declared_size)?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                waiting
                    .entry(DeltaBase::Offset(base_offset))
                    .or_default()
                    .push(PendingDelta {
                        offset,
                        instructions: delta,
                    });
            }
            7 => {
                if pack.len().saturating_sub(cursor) < 20 {
                    return Err(invalid_packet("truncated Git ref-delta base id"));
                }
                let base_id: [u8; 20] = pack[cursor..cursor + 20]
                    .try_into()
                    .expect("length checked");
                cursor += 20;
                let (delta, consumed) =
                    read_zlib_stream(&pack[cursor..payload_end], declared_size)?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                waiting
                    .entry(DeltaBase::Id(base_id))
                    .or_default()
                    .push(PendingDelta {
                        offset,
                        instructions: delta,
                    });
            }
            _ => return Err(invalid_packet("unsupported Git pack object type")),
        }
    }
    if cursor != payload_end {
        return Err(invalid_packet("Git pack has trailing object data"));
    }
    // Each base wakes only its dependents. Forward ref-deltas are supported
    // without repeated full scans or copies of their base object's data.
    while let Some((base_offset, depth)) = ready.pop_front() {
        let base_id = objects[&base_offset].id;
        for key in [DeltaBase::Offset(base_offset), DeltaBase::Id(base_id)] {
            let Some(dependents) = waiting.remove(&key) else {
                continue;
            };
            if depth == MAX_DELTA_DEPTH {
                return Err(invalid_packet("Git pack exceeds delta depth limit"));
            }
            for delta in dependents {
                let base = &objects[&base_offset];
                let data = apply_delta(&base.data, &delta.instructions, remaining_bytes)?;
                remaining_bytes -= data.len();
                let object = make_object(base.kind, data)?;
                objects.insert(delta.offset, object);
                ready.push_back((delta.offset, depth + 1));
            }
        }
    }
    if !waiting.is_empty() {
        return Err(invalid_packet("Git delta base object is missing"));
    }
    Ok(objects
        .into_values()
        .map(|object| (object.id, object))
        .collect())
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
        let digit = usize::from(byte & 0x7f);
        if shift >= usize::BITS || digit > (usize::MAX >> shift) {
            return Err(invalid_packet("Git pack object size overflows"));
        }
        size |= digit << shift;
        shift += 7;
        if size > MAX_OBJECT_BYTES {
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
            .and_then(|value| value.checked_mul(128))
            .and_then(|value| value.checked_add(usize::from(byte & 0x7f)))
            .ok_or_else(|| invalid_packet("Git ofs-delta base offset overflows"))?;
    }
    object_offset
        .checked_sub(value)
        .ok_or_else(|| invalid_packet("Git ofs-delta base offset is invalid"))
}

fn read_zlib_stream(input: &[u8], expected_size: usize) -> io::Result<(Vec<u8>, usize)> {
    let mut decoder = ZlibDecoder::new(input);
    let mut data = Vec::new();
    (&mut decoder)
        .take((expected_size + 1) as u64)
        .read_to_end(&mut data)?;
    if data.len() > MAX_OBJECT_BYTES || data.len() != expected_size || decoder.total_in() == 0 {
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
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use sha1::{Digest, Sha1};
    use std::io::Write;

    fn pack_header(count: u32) -> Vec<u8> {
        let mut pack = b"PACK".to_vec();
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&count.to_be_bytes());
        pack
    }

    fn append_object(pack: &mut Vec<u8>, kind: u8, size: usize, base: &[u8], data: &[u8]) -> usize {
        let offset = pack.len();
        let mut remaining = size >> 4;
        pack.push((kind << 4) | (size & 15) as u8 | if remaining == 0 { 0 } else { 128 });
        while remaining != 0 {
            let digit = (remaining & 127) as u8;
            remaining >>= 7;
            pack.push(digit | if remaining == 0 { 0 } else { 128 });
        }
        pack.extend_from_slice(base);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(data).unwrap();
        pack.extend(encoder.finish().unwrap());
        offset
    }

    fn finish_pack(mut pack: Vec<u8>) -> Vec<u8> {
        pack.extend_from_slice(&Sha1::digest(&pack));
        pack
    }

    fn blob_id(data: &[u8]) -> [u8; 20] {
        let mut hash = Sha1::new();
        hash.update(format!("blob {}\0", data.len()).as_bytes());
        hash.update(data);
        hash.finalize().into()
    }

    #[test]
    fn decoded_budget_counts_all_objects_and_delta_results() {
        let mut pack = pack_header(2);
        append_object(&mut pack, 3, 3, &[], b"abc");
        append_object(&mut pack, 3, 3, &[], b"xyz");
        let pack = finish_pack(pack);
        assert_eq!(
            decode_pack_object_with_limit(&pack, &blob_id(b"abc"), 6)
                .unwrap()
                .data,
            b"abc"
        );
        assert_eq!(
            decode_pack_object_with_limit(&pack, &blob_id(b"abc"), 5)
                .unwrap_err()
                .to_string(),
            "Git pack exceeds decoded size limit"
        );

        let mut pack = pack_header(2);
        let delta = [3, 3, 3, b'x', b'y', b'z'];
        // Ref-delta precedes its base, exercising the forward dependency path.
        append_object(&mut pack, 7, delta.len(), &blob_id(b"abc"), &delta);
        append_object(&mut pack, 3, 3, &[], b"abc");
        let pack = finish_pack(pack);
        assert_eq!(
            decode_pack_object_with_limit(&pack, &blob_id(b"xyz"), 12)
                .unwrap()
                .data,
            b"xyz"
        );
        assert_eq!(
            decode_pack_object_with_limit(&pack, &blob_id(b"xyz"), 11)
                .unwrap_err()
                .to_string(),
            "Git pack exceeds decoded size limit"
        );
    }

    #[test]
    fn delta_instruction_stream_must_match_declared_pack_size() {
        for kind in [6, 7] {
            for declared in [3, 5] {
                let mut pack = pack_header(2);
                let base_offset = append_object(&mut pack, 3, 1, &[], b"a");
                let base = if kind == 6 {
                    vec![(pack.len() - base_offset) as u8]
                } else {
                    blob_id(b"a").to_vec()
                };
                append_object(&mut pack, kind, declared, &base, &[1, 1, 1, b'b']);
                let error = decode_pack_object(&finish_pack(pack), &blob_id(b"b")).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    "Git pack object size does not match its header"
                );
            }
        }
    }

    #[test]
    fn reverse_ordered_ref_delta_chains_resolve_once_and_enforce_depth() {
        for depth in [MAX_DELTA_DEPTH, MAX_DELTA_DEPTH + 1] {
            let mut pack = pack_header(depth as u32 + 1);
            for value in (1..=depth).rev() {
                append_object(
                    &mut pack,
                    7,
                    4,
                    &blob_id(&[(value - 1) as u8]),
                    &[1, 1, 1, value as u8],
                );
            }
            append_object(&mut pack, 3, 1, &[], &[0]);
            let result = decode_pack_object(&finish_pack(pack), &blob_id(&[depth as u8]));
            if depth == MAX_DELTA_DEPTH {
                assert_eq!(result.unwrap().data, [depth as u8]);
            } else {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "Git pack exceeds delta depth limit"
                );
            }
        }
    }

    #[test]
    fn missing_delta_base_is_rejected() {
        let mut pack = pack_header(1);
        append_object(&mut pack, 7, 4, &blob_id(b"a"), &[1, 1, 1, b'b']);
        assert_eq!(
            decode_pack_object(&finish_pack(pack), &blob_id(b"b"))
                .unwrap_err()
                .to_string(),
            "Git delta base object is missing"
        );
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn mixed_delta_chains_obey_the_exact_decoded_budget(
                steps in prop::collection::vec((any::<u8>(), any::<bool>()), 1..32),
            ) {
                let mut pack = pack_header(steps.len() as u32 + 1);
                let mut previous = 0;
                let mut base_offset = append_object(&mut pack, 3, 1, &[], &[previous]);
                for &(value, by_offset) in &steps {
                    let base = if by_offset {
                        let mut distance = pack.len() - base_offset;
                        let mut encoded = vec![(distance & 127) as u8];
                        while distance >= 128 {
                            distance = (distance >> 7) - 1;
                            encoded.push(128 | (distance & 127) as u8);
                        }
                        encoded.reverse();
                        encoded
                    } else {
                        blob_id(&[previous]).to_vec()
                    };
                    base_offset = append_object(&mut pack, if by_offset { 6 } else { 7 }, 4, &base, &[1, 1, 1, value]);
                    previous = value;
                }
                let pack = finish_pack(pack);
                let budget = 1 + steps.len() * 5;
                let wanted = blob_id(&[previous]);
                prop_assert_eq!(decode_pack_object_with_limit(&pack, &wanted, budget).unwrap().data, [previous]);
                prop_assert_eq!(decode_pack_object_with_limit(&pack, &wanted, budget - 1).unwrap_err().to_string(), "Git pack exceeds decoded size limit");
            }
        }
    }

    #[test]
    fn overflowing_ofs_delta_offset_cannot_wrap_to_valid_base() {
        let mut value = (1u128 << usize::BITS) + 1;
        let mut encoded = vec![(value & 0x7f) as u8];
        while value >= 128 {
            value = (value >> 7) - 1;
            encoded.push(0x80 | (value & 0x7f) as u8);
        }
        encoded.reverse();
        assert!(super::read_ofs_delta_base(&encoded, &mut 0, 20).is_err());
    }

    #[test]
    fn truncated_pack_zlib_trailers_are_rejected() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(b"payload").unwrap();
        let encoded = encoder.finish().unwrap();
        for removed in 1..=4 {
            assert!(
                super::read_zlib_stream(&encoded[..encoded.len() - removed], 7).is_err(),
                "removed {removed} trailer bytes"
            );
        }
    }

    #[test]
    fn truncated_object_header_cannot_consume_pack_checksum() {
        let mut pack = b"PACK".to_vec();
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&1u32.to_be_bytes());
        // A blob header whose size continuation is absent from the payload.
        pack.push(0xb0);
        pack.extend_from_slice(&Sha1::digest(&pack));
        assert!(decode_pack_object(&pack, &[0; 20]).is_err());
    }

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

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn ofs_delta_offsets_preserve_integer_value(value in prop_oneof![any::<u128>(), any::<usize>().prop_map(|value| value as u128), Just(usize::MAX as u128), Just(usize::MAX as u128 + 1)]) {
            let mut remaining = value;
            let mut encoded = vec![(remaining & 127) as u8];
            while remaining >= 128 {
                remaining = (remaining >> 7) - 1;
                encoded.push(128 | (remaining & 127) as u8);
            }
            encoded.reverse();
            let decoded = read_ofs_delta_base(&encoded, &mut 0, usize::MAX);
            match usize::try_from(value) {
                Ok(expected) => prop_assert_eq!(decoded.unwrap(), usize::MAX - expected),
                Err(_) => prop_assert!(decoded.is_err()),
            }
        }

        #[test]
        fn pack_sizes_preserve_bounded_integer_value(value in prop_oneof![any::<u128>(), 0u128..=MAX_OBJECT_BYTES as u128, Just(MAX_OBJECT_BYTES as u128 + 1)]) {
            let mut remaining = value >> 4;
            let mut encoded = vec![0x30 | (value & 15) as u8 | if remaining == 0 { 0 } else { 128 }];
            while remaining != 0 {
                let digit = (remaining & 127) as u8;
                remaining >>= 7;
                encoded.push(digit | if remaining == 0 { 0 } else { 128 });
            }
            let decoded = read_pack_header(&encoded, &mut 0);
            if value <= MAX_OBJECT_BYTES as u128 {
                prop_assert_eq!(decoded.unwrap(), (3, value as usize));
            } else {
                prop_assert!(decoded.is_err());
            }
        }

        #[test]
        fn arbitrary_pack_bytes_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let _ = decode_pack_object(&bytes, &[0; 20]);
        }

        #[test]
        fn checksummed_pack_payloads_do_not_panic(
            payload in prop::collection::vec(any::<u8>(), 0..1024),
            count in 0u32..8,
        ) {
            let mut pack = b"PACK".to_vec();
            pack.extend_from_slice(&2u32.to_be_bytes());
            pack.extend_from_slice(&count.to_be_bytes());
            pack.extend(payload);
            pack.extend_from_slice(&Sha1::digest(&pack));
            let _ = decode_pack_object(&pack, &[0; 20]);
        }
    }

    #[test]
    fn all_single_byte_object_headers_stay_inside_payload() {
        for byte in 0..=255 {
            let mut pack = b"PACK".to_vec();
            pack.extend_from_slice(&2u32.to_be_bytes());
            pack.extend_from_slice(&1u32.to_be_bytes());
            pack.push(byte);
            pack.extend_from_slice(&Sha1::digest(&pack));
            assert!(decode_pack_object(&pack, &[0; 20]).is_err());
        }
    }
}

#[cfg(test)]
mod round_trip_properties {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use proptest::prelude::*;
    use std::io::Write;

    proptest! {
        #[test]
        fn blob_pack_round_trip(data in prop::collection::vec(any::<u8>(), 0..4096), version in 2u32..=3) {
            let mut pack = b"PACK".to_vec();
            pack.extend_from_slice(&version.to_be_bytes());
            pack.extend_from_slice(&1u32.to_be_bytes());
            let mut size = data.len();
            let first = 0x30 | (size & 15) as u8;
            size >>= 4;
            pack.push(first | if size == 0 { 0 } else { 0x80 });
            while size != 0 {
                let byte = (size & 127) as u8;
                size >>= 7;
                pack.push(byte | if size == 0 { 0 } else { 0x80 });
            }
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(&data).unwrap();
            pack.extend(encoder.finish().unwrap());
            pack.extend_from_slice(&Sha1::digest(&pack));
            let mut hash = Sha1::new();
            hash.update(format!("blob {}\0", data.len()).as_bytes());
            hash.update(&data);
            let id: [u8; 20] = hash.finalize().into();
            let decoded = decode_pack_object(&pack, &id).unwrap();
            prop_assert_eq!(decoded.kind, ObjectKind::Blob);
            prop_assert_eq!(decoded.data, data);
            prop_assert_eq!(decoded.id, id);
        }
    }
}
