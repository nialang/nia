// SPDX-License-Identifier: GPL-3.0-or-later
//! Git packet-line framing.

use std::io;

use crate::{MAX_PACKET_PAYLOAD, invalid_packet, parse_hex_usize};

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
