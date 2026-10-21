// SPDX-License-Identifier: GPL-3.0-or-later
//! Git object representations and decoders.

mod loose;

pub(crate) use loose::{GitObject, ObjectKind, decode_loose_object};
