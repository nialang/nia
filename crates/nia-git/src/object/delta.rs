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
        value |= usize::from(byte & 0x7f)
            .checked_shl(shift)
            .ok_or_else(|| invalid_packet("Git delta size overflows"))?;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err(invalid_packet("Git delta size overflows"));
        }
    }
}

pub(crate) fn apply_delta(base: &[u8], delta: &[u8]) -> io::Result<Vec<u8>> {
    let mut cursor = 0;
    let source_size = read_delta_varint(delta, &mut cursor)?;
    let result_size = read_delta_varint(delta, &mut cursor)?;
    if source_size != base.len() || result_size > MAX_OBJECT_BYTES {
        return Err(invalid_packet("Git delta base or result size is invalid"));
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
    use super::apply_delta;
    use std::io;

    #[test]
    fn deltas_copy_and_insert_with_checked_sizes() {
        let delta = [5, 10, 0x90, 5, 5, b'!', b' ', b'n', b'i', b'a'];
        assert_eq!(apply_delta(b"hello", &delta).unwrap(), b"hello! nia");

        let invalid = [1, 1, 0x80];
        assert_eq!(
            apply_delta(b"x", &invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
