// SPDX-License-Identifier: GPL-3.0-or-later
//! Git pack delta application.

use std::io;

use super::MAX_OBJECT_BYTES;
use crate::invalid_packet;

fn read_delta_varint(delta: &[u8], cursor: &mut usize) -> io::Result<usize> {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = *delta
            .get(*cursor)
            .ok_or_else(|| invalid_packet("truncated Git delta size"))?;
        *cursor += 1;
        let digit = usize::from(byte & 0x7f);
        if shift >= usize::BITS || digit > (usize::MAX >> shift) {
            return Err(invalid_packet("Git delta size overflows"));
        }
        value |= digit << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= usize::BITS {
            return Err(invalid_packet("Git delta size overflows"));
        }
    }
}

pub(crate) fn apply_delta(base: &[u8], delta: &[u8], result_limit: usize) -> io::Result<Vec<u8>> {
    let mut cursor = 0;
    let source_size = read_delta_varint(delta, &mut cursor)?;
    let result_size = read_delta_varint(delta, &mut cursor)?;
    if source_size != base.len() || result_size > MAX_OBJECT_BYTES {
        return Err(invalid_packet("Git delta base or result size is invalid"));
    }
    if result_size > result_limit {
        return Err(invalid_packet("Git pack exceeds decoded size limit"));
    }
    let mut result = Vec::with_capacity(result_size);
    while cursor < delta.len() {
        let instruction = delta[cursor];
        cursor += 1;
        if instruction & 0x80 == 0 {
            let length = usize::from(instruction);
            if length == 0 || delta.len().saturating_sub(cursor) < length {
                return Err(invalid_packet("Git delta insert instruction is invalid"));
            }
            if length > result_size.saturating_sub(result.len()) {
                return Err(invalid_packet("Git delta result exceeds its declared size"));
            }
            result.extend_from_slice(&delta[cursor..cursor + length]);
            cursor += length;
            continue;
        }
        let mut offset = 0usize;
        let mut length = 0usize;
        for bit in 0..4 {
            if instruction & (1 << bit) != 0 {
                offset |= usize::from(
                    *delta
                        .get(cursor)
                        .ok_or_else(|| invalid_packet("truncated Git delta copy offset"))?,
                ) << (bit * 8);
                cursor += 1;
            }
        }
        for bit in 0..3 {
            if instruction & (1 << (bit + 4)) != 0 {
                length |= usize::from(
                    *delta
                        .get(cursor)
                        .ok_or_else(|| invalid_packet("truncated Git delta copy length"))?,
                ) << (bit * 8);
                cursor += 1;
            }
        }
        if length == 0 {
            length = 0x10000;
        }
        if offset > base.len() || length > base.len() - offset {
            return Err(invalid_packet("Git delta copy exceeds its base"));
        }
        if length > result_size.saturating_sub(result.len()) {
            return Err(invalid_packet("Git delta result exceeds its declared size"));
        }
        result.extend_from_slice(&base[offset..offset + length]);
    }
    if result.len() != result_size {
        return Err(invalid_packet("Git delta result size is invalid"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn overflowing_delta_size_cannot_wrap_to_empty_base() {
        let groups = (usize::BITS - 1) / 7;
        let mut delta = vec![0x80; groups as usize];
        delta.push(1 << (usize::BITS - groups * 7));
        delta.push(0);
        assert!(apply_delta(&[], &delta, MAX_OBJECT_BYTES).is_err());
    }

    #[test]
    fn deltas_copy_and_insert_with_checked_sizes() {
        let delta = [5, 10, 0x90, 5, 5, b'!', b' ', b'n', b'i', b'a'];
        assert_eq!(
            apply_delta(b"hello", &delta, MAX_OBJECT_BYTES).unwrap(),
            b"hello! nia"
        );

        let invalid = [1, 1, 0x80];
        assert_eq!(
            apply_delta(b"x", &invalid, MAX_OBJECT_BYTES)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn delta_output_budget_is_checked_before_allocation() {
        let delta = [0, 3, 3, b'a', b'b', b'c'];
        assert_eq!(apply_delta(&[], &delta, 3).unwrap(), b"abc");
        assert_eq!(
            apply_delta(&[], &delta, 2).unwrap_err().to_string(),
            "Git pack exceeds decoded size limit"
        );
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn delta_sizes_preserve_integer_value(value in prop_oneof![any::<u128>(), any::<usize>().prop_map(|value| value as u128), Just(usize::MAX as u128), Just(usize::MAX as u128 + 1)]) {
            let mut remaining = value;
            let mut encoded = Vec::new();
            loop {
                let digit = (remaining & 127) as u8;
                remaining >>= 7;
                encoded.push(digit | if remaining == 0 { 0 } else { 128 });
                if remaining == 0 { break; }
            }
            let decoded = read_delta_varint(&encoded, &mut 0);
            match usize::try_from(value) {
                Ok(expected) => prop_assert_eq!(decoded.unwrap(), expected),
                Err(_) => prop_assert!(decoded.is_err()),
            }
        }

        #[test]
        fn arbitrary_delta_instructions_do_not_panic(
            base in prop::collection::vec(any::<u8>(), 0..128),
            instructions in prop::collection::vec(any::<u8>(), 0..1024),
            result_len in 0u8..128,
        ) {
            // Supply valid, small declared sizes so arbitrary instructions reach the decoder.
            let mut delta = vec![base.len() as u8, result_len];
            delta.extend(instructions);
            if let Ok(result) = apply_delta(&base, &delta, MAX_OBJECT_BYTES) {
                prop_assert_eq!(result.len(), usize::from(result_len));
            }
        }

        #[test]
        fn insert_delta_reconstructs_payload(payload in prop::collection::vec(any::<u8>(), 1..128)) {
            let mut delta = vec![0, payload.len() as u8, payload.len() as u8];
            delta.extend_from_slice(&payload);
            prop_assert_eq!(apply_delta(&[], &delta, MAX_OBJECT_BYTES).unwrap(), payload);
        }
    }
}
