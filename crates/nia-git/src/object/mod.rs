// SPDX-License-Identifier: GPL-3.0-or-later
//! Git object representations and decoders.

mod delta;
mod loose;
mod pack;
mod refs;

pub(crate) use delta::apply_delta;
pub(crate) use loose::{GitObject, ObjectKind, decode_loose_object};
pub(crate) use pack::read_packed_object;
pub(crate) use refs::{encode_hex, parse_object_id, read_loose_object, resolve_revision};

const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMPRESSED_OBJECT_BYTES: usize = 128 * 1024 * 1024;
