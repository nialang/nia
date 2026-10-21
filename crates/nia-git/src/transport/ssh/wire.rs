// SPDX-License-Identifier: GPL-3.0-or-later
//! SSH binary strings, integers, and bounded field reader.

use crate::invalid_packet;
use ring::{rand, rand::SecureRandom};
use std::io::{self, Read, Write};

pub(super) fn encode_ssh_string(value: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| invalid_packet("SSH string exceeds uint32 length"))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

pub(super) fn encode_ssh_mpint(value: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
    let first = value.iter().position(|byte| *byte != 0);
    let value = first.map_or(&[][..], |index| &value[index..]);
    let mut encoded = Vec::with_capacity(value.len() + 1);
    if value.first().is_some_and(|byte| *byte & 0x80 != 0) {
        encoded.push(0);
    }
    encoded.extend_from_slice(value);
    encode_ssh_string(&encoded, output)
}

pub(super) struct SshReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SshReader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(super) fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| invalid_packet("SSH field length overflows"))?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| invalid_packet("truncated SSH payload"))?;
        self.offset = end;
        Ok(value)
    }

    pub(super) fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("length is four"),
        ))
    }

    pub(super) fn boolean(&mut self) -> io::Result<bool> {
        Ok(self.take(1)?[0] != 0)
    }

    pub(super) fn string(&mut self) -> io::Result<&'a [u8]> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| invalid_packet("SSH string length overflows usize"))?;
        self.take(length)
    }

    pub(super) fn name_list(&mut self) -> io::Result<Vec<String>> {
        let value = self.string()?;
        if value.is_empty() {
            return Ok(Vec::new());
        }
        value
            .split(|byte| *byte == b',')
            .map(|name| {
                if name.is_empty()
                    || !name
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-@_".contains(byte))
                {
                    return Err(invalid_packet("SSH name-list contains an invalid name"));
                }
                String::from_utf8(name.to_vec())
                    .map_err(|_| invalid_packet("SSH name-list is not UTF-8"))
            })
            .collect()
    }

    pub(super) fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    pub(super) fn finish(&self) -> io::Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid_packet("SSH payload has trailing bytes"))
        }
    }
}

pub(super) fn decode_base64(input: &str) -> io::Result<Vec<u8>> {
    let padding_start = input.find('=').unwrap_or(input.len());
    let data = &input[..padding_start];
    let padding = &input[padding_start..];
    let expected_padding = match data.len() % 4 {
        0 => 0,
        2 => 2,
        3 => 1,
        _ => return Err(invalid_packet("SSH known_hosts base64 is malformed")),
    };
    if data.is_empty()
        || padding.len() > 2
        || padding.bytes().any(|byte| byte != b'=')
        || (!padding.is_empty()
            && (!input.len().is_multiple_of(4) || padding.len() != expected_padding))
    {
        return Err(invalid_packet("SSH known_hosts base64 is malformed"));
    }
    let mut output = Vec::with_capacity(data.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in data.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(invalid_packet("SSH known_hosts base64 is malformed")),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    if bits >= 6 || (bits > 0 && buffer != 0) {
        return Err(invalid_packet("SSH known_hosts base64 has invalid padding"));
    }
    Ok(output)
}

pub(super) const MAX_SSH_IDENTIFICATION_LINE: usize = 255;
pub(super) const MAX_SSH_PRE_IDENTIFICATION_LINES: usize = 50;
pub(super) const MAX_SSH_PACKET_BYTES: usize = 256 * 1024;

pub(super) fn write_ssh_identification(mut stream: impl Write) -> io::Result<()> {
    stream.write_all(b"SSH-2.0-NiaGit_0.3\r\n")?;
    stream.flush()
}

pub(super) fn read_ssh_identification(mut stream: impl Read) -> io::Result<Vec<u8>> {
    let mut line = Vec::with_capacity(MAX_SSH_IDENTIFICATION_LINE);
    for _ in 0..MAX_SSH_PRE_IDENTIFICATION_LINES {
        line.clear();
        loop {
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte)?;
            line.push(byte[0]);
            if line.len() > MAX_SSH_IDENTIFICATION_LINE {
                return Err(invalid_packet("SSH identification line is too long"));
            }
            if byte[0] == b'\n' {
                break;
            }
        }
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        if text.starts_with(b"SSH-") {
            if !(text.starts_with(b"SSH-2.0-") || text.starts_with(b"SSH-1.99-")) {
                return Err(invalid_packet("SSH server protocol version is unsupported"));
            }
            if text.len() < 8 || text.iter().any(|byte| *byte < 0x20 || *byte == 0x7f) {
                return Err(invalid_packet("SSH server identification is invalid"));
            }
            return Ok(text.to_vec());
        }
    }
    Err(invalid_packet(
        "SSH server sent too many pre-identification lines",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SshPacket {
    pub(super) payload: Vec<u8>,
}

pub(super) fn encode_ssh_packet(
    payload: &[u8],
    block_size: usize,
    padding: &[u8],
) -> io::Result<Vec<u8>> {
    if !(8..=255).contains(&block_size) || !block_size.is_power_of_two() {
        return Err(invalid_packet("SSH block size is invalid"));
    }
    let padding_len = padding.len();
    if !(4..=255).contains(&padding_len)
        || !(payload.len() + padding_len + 5).is_multiple_of(block_size)
        || payload.len() > MAX_SSH_PACKET_BYTES
    {
        return Err(invalid_packet("SSH packet padding is invalid"));
    }
    let packet_len = payload.len() + padding_len + 1;
    let packet_len = u32::try_from(packet_len)
        .map_err(|_| invalid_packet("SSH packet length overflows uint32"))?;
    let mut output = Vec::with_capacity(packet_len as usize + 4);
    output.extend_from_slice(&packet_len.to_be_bytes());
    output.push(u8::try_from(padding_len).expect("padding length is bounded"));
    output.extend_from_slice(payload);
    output.extend_from_slice(padding);
    Ok(output)
}

pub(super) fn read_ssh_packet(mut stream: impl Read, block_size: usize) -> io::Result<SshPacket> {
    if !(8..=255).contains(&block_size) || !block_size.is_power_of_two() {
        return Err(invalid_packet("SSH block size is invalid"));
    }
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let packet_len = u32::from_be_bytes(length) as usize;
    if !(1 + 4..=MAX_SSH_PACKET_BYTES + 255).contains(&packet_len) {
        return Err(invalid_packet("SSH packet length is invalid"));
    }
    if !(packet_len + 4).is_multiple_of(block_size) {
        return Err(invalid_packet("SSH packet is not block aligned"));
    }
    let mut packet = vec![0u8; packet_len];
    stream.read_exact(&mut packet)?;
    let padding_len = usize::from(packet[0]);
    if padding_len < 4 || padding_len + 1 > packet.len() {
        return Err(invalid_packet("SSH packet padding is invalid"));
    }
    let payload_end = packet.len() - padding_len;
    Ok(SshPacket {
        payload: packet[1..payload_end].to_vec(),
    })
}

pub(super) fn write_plain_ssh_packet(stream: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    let mut padding_len = 8 - ((payload.len() + 5) % 8);
    if padding_len < 4 {
        padding_len += 8;
    }
    let mut padding = vec![0u8; padding_len];
    rand::SystemRandom::new()
        .fill(&mut padding)
        .map_err(|_| io::Error::other("SSH packet padding generation failed"))?;
    let packet = encode_ssh_packet(payload, 8, &padding)?;
    stream.write_all(&packet)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_identification_accepts_banners_and_rejects_bad_versions() {
        let mut input = &b"Authorized users only\r\nSSH-2.0-OpenSSH_9.8\r\n"[..];
        assert_eq!(
            read_ssh_identification(&mut input).unwrap(),
            b"SSH-2.0-OpenSSH_9.8"
        );
        let mut unsupported = &b"SSH-1.5-legacy\r\n"[..];
        assert_eq!(
            read_ssh_identification(&mut unsupported)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let long_line = vec![b'a'; MAX_SSH_IDENTIFICATION_LINE + 1];
        assert_eq!(
            read_ssh_identification(&long_line[..]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn ssh_binary_packet_framing_checks_padding_lengths_and_alignment() {
        let payload = [20, 1, 2, 3];
        let padding = [0u8; 7];
        let encoded = encode_ssh_packet(&payload, 8, &padding).unwrap();
        let decoded = read_ssh_packet(&encoded[..], 8).unwrap();
        assert_eq!(decoded.payload, payload);

        assert!(encode_ssh_packet(&payload, 8, &[0; 4]).is_err());
        let mut malformed = encoded;
        malformed[4] = 3;
        assert!(read_ssh_packet(&malformed[..], 8).is_err());
    }
}
