// SPDX-License-Identifier: GPL-3.0-or-later
//! AES-128-CTR packet protection and sequence authentication.

use super::wire::{MAX_SSH_PACKET_BYTES, SshPacket, encode_ssh_packet};
use crate::invalid_packet;
use aes::Aes128;
use ctr::cipher::{KeyIvInit, StreamCipher};
use ring::{hmac, rand, rand::SecureRandom};
use std::io::{self, Read, Write};

type Aes128Ctr = ctr::Ctr128BE<Aes128>;

pub(super) struct SshAes128CtrDirection {
    cipher: Aes128Ctr,
    mac_key: hmac::Key,
    sequence: u32,
}

impl std::fmt::Debug for SshAes128CtrDirection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshAes128CtrDirection")
            .field("cipher", &"<hidden>")
            .field("mac_key", &"<hidden>")
            .field("sequence", &self.sequence)
            .finish()
    }
}

impl SshAes128CtrDirection {
    pub(super) fn new(encryption_key: &[u8], iv: &[u8], mac_key: &[u8]) -> io::Result<Self> {
        let key: &[u8; 16] = encryption_key
            .try_into()
            .map_err(|_| invalid_packet("SSH AES-128 key has invalid length"))?;
        let iv: &[u8; 16] = iv
            .try_into()
            .map_err(|_| invalid_packet("SSH AES-CTR IV has invalid length"))?;
        if mac_key.len() != 32 {
            return Err(invalid_packet("SSH HMAC-SHA2-256 key has invalid length"));
        }
        Ok(Self {
            cipher: Aes128Ctr::new(key.into(), iv.into()),
            mac_key: hmac::Key::new(hmac::HMAC_SHA256, mac_key),
            sequence: 0,
        })
    }

    #[cfg(test)]
    pub(super) fn sequence(&self) -> u32 {
        self.sequence
    }

    pub(super) fn encode(&mut self, payload: &[u8], padding: &[u8]) -> io::Result<Vec<u8>> {
        let mut packet = encode_ssh_packet(payload, 16, padding)?;
        let mac = ssh_packet_mac(&self.mac_key, self.sequence, &packet);
        self.cipher.apply_keystream(&mut packet);
        packet.extend_from_slice(mac.as_ref());
        self.sequence = self.sequence.wrapping_add(1);
        Ok(packet)
    }

    pub(super) fn read_packet(&mut self, mut stream: impl Read) -> io::Result<SshPacket> {
        let mut first_block = [0u8; 16];
        stream.read_exact(&mut first_block)?;
        self.cipher.apply_keystream(&mut first_block);
        let packet_len = u32::from_be_bytes(first_block[..4].try_into().unwrap()) as usize;
        if !(5..=MAX_SSH_PACKET_BYTES + 255).contains(&packet_len) {
            return Err(invalid_packet("SSH encrypted packet length is invalid"));
        }
        let total_len = packet_len
            .checked_add(4)
            .ok_or_else(|| invalid_packet("SSH encrypted packet length overflows"))?;
        if !total_len.is_multiple_of(16) || total_len < 16 {
            return Err(invalid_packet("SSH encrypted packet is not block aligned"));
        }
        let mut packet = Vec::with_capacity(total_len);
        packet.extend_from_slice(&first_block);
        let remaining = total_len - 16;
        if remaining != 0 {
            let offset = packet.len();
            packet.resize(total_len, 0);
            stream.read_exact(&mut packet[offset..])?;
            self.cipher.apply_keystream(&mut packet[offset..]);
        }
        let mut received_mac = [0u8; 32];
        stream.read_exact(&mut received_mac)?;
        let expected_mac = ssh_packet_mac(&self.mac_key, self.sequence, &packet);
        hmac::verify(
            &self.mac_key,
            &mac_input(self.sequence, &packet),
            &received_mac,
        )
        .map_err(|_| invalid_packet("SSH packet MAC verification failed"))?;
        debug_assert_eq!(expected_mac.as_ref(), received_mac);
        let padding_len = usize::from(packet[4]);
        if padding_len < 4 || padding_len + 1 > packet_len {
            return Err(invalid_packet("SSH encrypted packet padding is invalid"));
        }
        let payload_end = packet.len() - padding_len;
        let payload = packet[5..payload_end].to_vec();
        self.sequence = self.sequence.wrapping_add(1);
        Ok(SshPacket { payload })
    }
}

fn ssh_packet_mac(key: &hmac::Key, sequence: u32, packet: &[u8]) -> hmac::Tag {
    let input = mac_input(sequence, packet);
    hmac::sign(key, &input)
}

fn mac_input(sequence: u32, packet: &[u8]) -> Vec<u8> {
    let mut input = Vec::with_capacity(4 + packet.len());
    input.extend_from_slice(&sequence.to_be_bytes());
    input.extend_from_slice(packet);
    input
}

pub(super) fn write_encrypted_ssh_packet(
    stream: &mut impl Write,
    direction: &mut SshAes128CtrDirection,
    payload: &[u8],
) -> io::Result<()> {
    let mut padding_len = 16 - ((payload.len() + 5) % 16);
    if padding_len < 4 {
        padding_len += 16;
    }
    let mut padding = vec![0u8; padding_len];
    rand::SystemRandom::new()
        .fill(&mut padding)
        .map_err(|_| io::Error::other("SSH packet padding generation failed"))?;
    let packet = direction.encode(payload, &padding)?;
    stream.write_all(&packet)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_aes_ctr_packets_roundtrip_and_authenticate_sequence() {
        let key = [1u8; 16];
        let iv = [2u8; 16];
        let mac_key = [3u8; 32];
        let mut sender = SshAes128CtrDirection::new(&key, &iv, &mac_key).unwrap();
        let mut receiver = SshAes128CtrDirection::new(&key, &iv, &mac_key).unwrap();
        let first = sender.encode(&[21, 1, 2, 3, 4], &[5; 6]).unwrap();
        let second = sender.encode(&[21, 9, 8, 7], &[6; 7]).unwrap();
        let mut wire = first;
        wire.extend_from_slice(&second);
        assert_eq!(
            receiver.read_packet(&wire[..]).unwrap().payload,
            [21, 1, 2, 3, 4]
        );
        assert_eq!(
            receiver.read_packet(&wire[48..]).unwrap().payload,
            [21, 9, 8, 7]
        );
        assert_eq!(sender.sequence(), 2);
        assert_eq!(receiver.sequence(), 2);

        let mut fresh_sender = SshAes128CtrDirection::new(&key, &iv, &mac_key).unwrap();
        let mut corrupted = fresh_sender.encode(&[21, 0], &[7; 9]).unwrap();
        *corrupted.last_mut().unwrap() ^= 1;
        let mut fresh_receiver = SshAes128CtrDirection::new(&key, &iv, &mac_key).unwrap();
        assert!(fresh_receiver.read_packet(&corrupted[..]).is_err());
        assert_eq!(fresh_receiver.sequence(), 0);
    }
}
