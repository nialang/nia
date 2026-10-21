// SPDX-License-Identifier: GPL-3.0-or-later
//! Git object representations and decoders.

mod loose;

#[cfg(test)]
pub(crate) use loose::encode_loose_object;
pub(crate) use loose::{GitObject, ObjectKind, decode_loose_object};
