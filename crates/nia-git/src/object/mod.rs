// SPDX-License-Identifier: GPL-3.0-or-later
//! Git object representations and decoders.

mod delta;
mod loose;
mod pack;
mod refs;

pub(crate) use delta::apply_delta;
pub(crate) use loose::{GitObject, ObjectKind, decode_loose_object};
pub(crate) use pack::{decode_pack_objects, read_packed_object};
pub(crate) use refs::{encode_hex, parse_object_id, read_loose_object, resolve_revision};

#[cfg(feature = "fuzzing")]
pub(super) fn fuzz_loose_object(input: &[u8]) {
    let _ = decode_loose_object(input);
}

#[cfg(feature = "fuzzing")]
pub(super) fn fuzz_pack_object(input: &[u8], wanted: &[u8; 20]) {
    let _ = pack::fuzz_decode_pack_object(input, wanted);
}

#[cfg(feature = "fuzzing")]
pub(super) fn fuzz_delta(input: &[u8]) {
    let split = input.len() / 2;
    let _ = apply_delta(&input[..split], &input[split..], MAX_OBJECT_BYTES);
}

const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMPRESSED_OBJECT_BYTES: usize = 128 * 1024 * 1024;
