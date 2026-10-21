// SPDX-License-Identifier: GPL-3.0-or-later
//! Git packet-line framing.

use std::io::{self, Read};

use crate::invalid_packet;

pub(crate) const MAX_PACKET_PAYLOAD: usize = 65_516;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PacketLine {
    Data(Vec<u8>),
    Flush,
    Delimiter,
    ResponseEnd,
}

pub(super) fn decode_packet_lines(mut input: &[u8]) -> io::Result<Vec<PacketLine>> {
    let mut packets = Vec::new();
    while !input.is_empty() {
        if input.len() < 4 {
            return Err(invalid_packet("truncated Git packet-line header"));
        }
        let length = parse_hex_usize(&input[..4])?;
        let consumed = length.max(4);
        match length {
            0 => packets.push(PacketLine::Flush),
            1 => packets.push(PacketLine::Delimiter),
            2 => packets.push(PacketLine::ResponseEnd),
            3 => return Err(invalid_packet("reserved Git packet-line length")),
            length if length > MAX_PACKET_PAYLOAD + 4 => {
                return Err(invalid_packet("Git packet-line exceeds protocol limit"));
            }
            length => {
                if length < 4 || input.len() < length {
                    return Err(invalid_packet("truncated Git packet-line payload"));
                }
                packets.push(PacketLine::Data(input[4..length].to_vec()));
            }
        }
        input = &input[consumed..];
    }
    Ok(packets)
}

pub(crate) fn encode_packet_line(payload: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
    if payload.len() > MAX_PACKET_PAYLOAD {
        return Err(invalid_packet("Git packet-line exceeds protocol limit"));
    }
    let length = payload.len() + 4;
    output.extend_from_slice(format!("{length:04x}").as_bytes());
    output.extend_from_slice(payload);
    Ok(())
}

fn parse_hex_usize(bytes: &[u8]) -> io::Result<usize> {
    bytes.iter().try_fold(0usize, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => usize::from(byte - b'0'),
            b'a'..=b'f' => usize::from(byte - b'a' + 10),
            b'A'..=b'F' => usize::from(byte - b'A' + 10),
            _ => return Err(invalid_packet("invalid Git packet-line header")),
        };
        Ok((value << 4) | digit)
    })
}

pub(crate) fn read_packet_section(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut section = Vec::new();
    loop {
        let mut header = [0; 4];
        stream.read_exact(&mut header)?;
        let length = parse_hex_usize(&header)?;
        section.extend_from_slice(&header);
        if length == 0 {
            return Ok(section);
        }
        if !(4..=MAX_PACKET_PAYLOAD + 4).contains(&length) {
            return Err(invalid_packet("Git packet-line exceeds protocol limit"));
        }
        let offset = section.len();
        section.resize(offset + length - 4, 0);
        stream.read_exact(&mut section[offset..])?;
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn arbitrary_packet_lines_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let _ = decode_packet_lines(&bytes);
            let _ = read_packet_section(&mut bytes.as_slice());
        }

        #[test]
        fn packet_lines_round_trip(payloads in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..1024), 0..16)) {
            let mut wire = Vec::new();
            let mut expected = Vec::new();
            for payload in payloads {
                encode_packet_line(&payload, &mut wire).unwrap();
                expected.push(PacketLine::Data(payload));
            }
            wire.extend_from_slice(b"000000010002");
            expected.extend([PacketLine::Flush, PacketLine::Delimiter, PacketLine::ResponseEnd]);
            prop_assert_eq!(decode_packet_lines(&wire).unwrap(), expected);
        }
    }
}
