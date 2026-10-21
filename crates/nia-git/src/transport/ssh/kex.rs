// SPDX-License-Identifier: GPL-3.0-or-later
//! SSH algorithm negotiation, X25519 exchange, and transport key derivation.

use super::cipher::SshAes128CtrDirection;
use super::known_hosts::KnownHosts;
use super::wire::{
    SshReader, encode_ssh_mpint, encode_ssh_string, read_ssh_identification, read_ssh_packet,
    write_plain_ssh_packet, write_ssh_identification,
};
use super::{SshTransportKeys, SshUrl};
use crate::invalid_packet;
use ring::{agreement, digest, rand, rand::SecureRandom, signature};
use std::io::{self, Read, Write};
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SshKexInit {
    pub(super) kex_algorithms: Vec<String>,
    pub(super) host_key_algorithms: Vec<String>,
    pub(super) encryption_client_to_server: Vec<String>,
    pub(super) encryption_server_to_client: Vec<String>,
    pub(super) mac_client_to_server: Vec<String>,
    pub(super) mac_server_to_client: Vec<String>,
    pub(super) compression_client_to_server: Vec<String>,
    pub(super) compression_server_to_client: Vec<String>,
}

pub(super) fn parse_ssh_kexinit(payload: &[u8]) -> io::Result<SshKexInit> {
    if payload.first() != Some(&20) || payload.len() < 17 {
        return Err(invalid_packet("SSH payload is not KEXINIT"));
    }
    let mut reader = SshReader::new(&payload[17..]);
    let kex_algorithms = reader.name_list()?;
    let host_key_algorithms = reader.name_list()?;
    let encryption_client_to_server = reader.name_list()?;
    let encryption_server_to_client = reader.name_list()?;
    let mac_client_to_server = reader.name_list()?;
    let mac_server_to_client = reader.name_list()?;
    let compression_client_to_server = reader.name_list()?;
    let compression_server_to_client = reader.name_list()?;
    let _languages_client_to_server = reader.name_list()?;
    let _languages_server_to_client = reader.name_list()?;
    let _first_kex_packet_follows = reader.boolean()?;
    let _reserved = reader.u32()?;
    reader.finish()?;
    Ok(SshKexInit {
        kex_algorithms,
        host_key_algorithms,
        encryption_client_to_server,
        encryption_server_to_client,
        mac_client_to_server,
        mac_server_to_client,
        compression_client_to_server,
        compression_server_to_client,
    })
}

fn choose_ssh_algorithm<'a>(client: &'a [String], server: &[String]) -> io::Result<&'a str> {
    client
        .iter()
        .find(|candidate| server.iter().any(|peer| peer == *candidate))
        .map(String::as_str)
        .ok_or_else(|| invalid_packet("SSH peers have no compatible algorithm"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SshNegotiatedAlgorithms {
    pub(super) kex: String,
    pub(super) host_key: String,
    pub(super) cipher_client_to_server: String,
    pub(super) cipher_server_to_client: String,
    pub(super) mac_client_to_server: String,
    pub(super) mac_server_to_client: String,
    pub(super) compression_client_to_server: String,
    pub(super) compression_server_to_client: String,
}

pub(super) fn negotiate_ssh_algorithms(
    client: &SshKexInit,
    server: &SshKexInit,
) -> io::Result<SshNegotiatedAlgorithms> {
    Ok(SshNegotiatedAlgorithms {
        kex: choose_ssh_algorithm(&client.kex_algorithms, &server.kex_algorithms)?.to_string(),
        host_key: choose_ssh_algorithm(&client.host_key_algorithms, &server.host_key_algorithms)?
            .to_string(),
        cipher_client_to_server: choose_ssh_algorithm(
            &client.encryption_client_to_server,
            &server.encryption_client_to_server,
        )?
        .to_string(),
        cipher_server_to_client: choose_ssh_algorithm(
            &client.encryption_server_to_client,
            &server.encryption_server_to_client,
        )?
        .to_string(),
        mac_client_to_server: choose_ssh_algorithm(
            &client.mac_client_to_server,
            &server.mac_client_to_server,
        )?
        .to_string(),
        mac_server_to_client: choose_ssh_algorithm(
            &client.mac_server_to_client,
            &server.mac_server_to_client,
        )?
        .to_string(),
        compression_client_to_server: choose_ssh_algorithm(
            &client.compression_client_to_server,
            &server.compression_client_to_server,
        )?
        .to_string(),
        compression_server_to_client: choose_ssh_algorithm(
            &client.compression_server_to_client,
            &server.compression_server_to_client,
        )?
        .to_string(),
    })
}

pub(super) fn build_client_kexinit(cookie: &[u8; 16]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(256);
    payload.push(20);
    payload.extend_from_slice(cookie);
    for names in [
        b"curve25519-sha256".as_slice(),
        b"ssh-ed25519".as_slice(),
        b"aes128-ctr".as_slice(),
        b"aes128-ctr".as_slice(),
        b"hmac-sha2-256".as_slice(),
        b"hmac-sha2-256".as_slice(),
        b"none".as_slice(),
        b"none".as_slice(),
        b"".as_slice(),
        b"".as_slice(),
    ] {
        encode_ssh_string(names, &mut payload).expect("static SSH names fit uint32");
    }
    payload.push(0);
    payload.extend_from_slice(&0u32.to_be_bytes());
    payload
}

pub(super) fn build_client_kex_ecdh_init(client_public: &[u8; 32]) -> Vec<u8> {
    let mut payload = vec![30];
    encode_ssh_string(client_public, &mut payload).expect("X25519 key fits uint32");
    payload
}

pub(super) fn perform_ssh_key_exchange(
    stream: &mut (impl Read + Write),
    url: &SshUrl,
    known_hosts: &KnownHosts,
) -> io::Result<SshTransportKeys> {
    write_ssh_identification(&mut *stream)?;
    stream.flush()?;
    let client_identification = b"SSH-2.0-NiaGit_0.3";
    let server_identification = read_ssh_identification(&mut *stream)?;

    let mut cookie = [0u8; 16];
    rand::SystemRandom::new()
        .fill(&mut cookie)
        .map_err(|_| io::Error::other("SSH KEXINIT cookie generation failed"))?;
    let client_kexinit = build_client_kexinit(&cookie);
    write_plain_ssh_packet(&mut *stream, &client_kexinit)?;
    let server_kexinit = read_expected_ssh_message(&mut *stream, 20)?;
    let client_algorithms = parse_ssh_kexinit(&client_kexinit)?;
    let server_algorithms = parse_ssh_kexinit(&server_kexinit)?;
    let negotiated = negotiate_ssh_algorithms(&client_algorithms, &server_algorithms)?;
    if negotiated.kex != "curve25519-sha256"
        || negotiated.host_key != "ssh-ed25519"
        || negotiated.cipher_client_to_server != "aes128-ctr"
        || negotiated.cipher_server_to_client != "aes128-ctr"
        || negotiated.mac_client_to_server != "hmac-sha2-256"
        || negotiated.mac_server_to_client != "hmac-sha2-256"
        || negotiated.compression_client_to_server != "none"
        || negotiated.compression_server_to_client != "none"
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "SSH server selected an unsupported transport algorithm",
        ));
    }

    let client_kex = Curve25519ClientKex::generate()?;
    let client_public = *client_kex.client_public();
    write_plain_ssh_packet(&mut *stream, &build_client_kex_ecdh_init(&client_public))?;
    let reply = parse_ssh_kex_ecdh_reply(&read_expected_ssh_message(&mut *stream, 31)?)?;
    let shared_secret = Zeroizing::new(client_kex.complete(&reply.server_public)?);
    let exchange_hash = curve25519_exchange_hash(Curve25519ExchangeHashInput {
        client_identification,
        server_identification: &server_identification,
        client_kexinit: &client_kexinit,
        server_kexinit: &server_kexinit,
        server_host_key: &reply.server_host_key,
        client_public: &client_public,
        server_public: &reply.server_public,
        shared_secret: &shared_secret,
    })?;
    verify_ssh_ed25519_host_key(&reply.server_host_key, &reply.signature, &exchange_hash)?;
    known_hosts.verify(&url.host, url.port, &reply.server_host_key)?;

    write_plain_ssh_packet(&mut *stream, &[21])?;
    let _ = read_expected_ssh_message(&mut *stream, 21)?;

    let session_id = exchange_hash;
    let iv_client_to_server = Zeroizing::new(derive_ssh_key(
        &shared_secret,
        &exchange_hash,
        &session_id,
        b'A',
        16,
    )?);
    let iv_server_to_client = Zeroizing::new(derive_ssh_key(
        &shared_secret,
        &exchange_hash,
        &session_id,
        b'B',
        16,
    )?);
    let key_client_to_server = Zeroizing::new(derive_ssh_key(
        &shared_secret,
        &exchange_hash,
        &session_id,
        b'C',
        16,
    )?);
    let key_server_to_client = Zeroizing::new(derive_ssh_key(
        &shared_secret,
        &exchange_hash,
        &session_id,
        b'D',
        16,
    )?);
    let mac_client_to_server = Zeroizing::new(derive_ssh_key(
        &shared_secret,
        &exchange_hash,
        &session_id,
        b'E',
        32,
    )?);
    let mac_server_to_client = Zeroizing::new(derive_ssh_key(
        &shared_secret,
        &exchange_hash,
        &session_id,
        b'F',
        32,
    )?);
    Ok(SshTransportKeys {
        outbound: SshAes128CtrDirection::new(
            &key_client_to_server,
            &iv_client_to_server,
            &mac_client_to_server,
        )?,
        inbound: SshAes128CtrDirection::new(
            &key_server_to_client,
            &iv_server_to_client,
            &mac_server_to_client,
        )?,
        session_id,
    })
}

fn read_expected_ssh_message(stream: &mut impl Read, expected: u8) -> io::Result<Vec<u8>> {
    loop {
        let packet = read_ssh_packet(&mut *stream, 8)?;
        match packet.payload.first().copied() {
            Some(message) if message == expected => return Ok(packet.payload),
            Some(2 | 4) => continue,
            Some(1) => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "SSH server disconnected during key exchange",
                ));
            }
            _ => {
                return Err(invalid_packet(
                    "SSH server sent an unexpected key exchange message",
                ));
            }
        }
    }
}

/// Client side `curve25519-sha256` exchange state.
///
/// The private key is kept inside ring's opaque type until the peer key is
/// validated and the one-shot agreement consumes it. This prevents callers
/// from accidentally reusing an ephemeral key across sessions.
pub(super) struct Curve25519ClientKex {
    private: Option<agreement::EphemeralPrivateKey>,
    client_public: [u8; 32],
}

impl std::fmt::Debug for Curve25519ClientKex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Curve25519ClientKex")
            .field("private", &"<hidden>")
            .field("client_public", &self.client_public)
            .finish()
    }
}

impl Curve25519ClientKex {
    pub(super) fn generate() -> io::Result<Self> {
        let rng = rand::SystemRandom::new();
        let private = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
            .map_err(|_| io::Error::other("SSH X25519 key generation failed"))?;
        let public = private
            .compute_public_key()
            .map_err(|_| io::Error::other("SSH X25519 public key generation failed"))?;
        let client_public = public
            .as_ref()
            .try_into()
            .map_err(|_| invalid_packet("SSH X25519 public key has invalid length"))?;
        Ok(Self {
            private: Some(private),
            client_public,
        })
    }

    pub(super) fn client_public(&self) -> &[u8; 32] {
        &self.client_public
    }

    pub(super) fn complete(mut self, server_public: &[u8]) -> io::Result<Vec<u8>> {
        if server_public.len() != 32 {
            return Err(invalid_packet("SSH X25519 server key has invalid length"));
        }
        let private = self
            .private
            .take()
            .ok_or_else(|| invalid_packet("SSH X25519 exchange was already completed"))?;
        let peer = agreement::UnparsedPublicKey::new(&agreement::X25519, server_public);
        agreement::agree_ephemeral(private, &peer, |shared| shared.to_vec())
            .map_err(|_| invalid_packet("SSH X25519 server key is invalid"))
    }
}

pub(super) struct Curve25519ExchangeHashInput<'a> {
    pub(super) client_identification: &'a [u8],
    pub(super) server_identification: &'a [u8],
    pub(super) client_kexinit: &'a [u8],
    pub(super) server_kexinit: &'a [u8],
    pub(super) server_host_key: &'a [u8],
    pub(super) client_public: &'a [u8; 32],
    pub(super) server_public: &'a [u8],
    pub(super) shared_secret: &'a [u8],
}

/// Compute the RFC 4253 exchange hash for `curve25519-sha256`.
pub(super) fn curve25519_exchange_hash(
    input: Curve25519ExchangeHashInput<'_>,
) -> io::Result<[u8; 32]> {
    if input.server_public.len() != 32 || input.shared_secret.len() != 32 {
        return Err(invalid_packet(
            "SSH X25519 exchange value has invalid length",
        ));
    }
    let mut encoded = Zeroizing::new(Vec::new());
    for value in [
        input.client_identification,
        input.server_identification,
        input.client_kexinit,
        input.server_kexinit,
        input.server_host_key,
        input.client_public.as_slice(),
        input.server_public,
    ] {
        encode_ssh_string(value, &mut encoded)?;
    }
    encode_ssh_mpint(input.shared_secret, &mut encoded)?;
    let digest = digest::digest(&digest::SHA256, &encoded);
    Ok(digest
        .as_ref()
        .try_into()
        .expect("SHA-256 digests are 32 bytes"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SshKexEcdhReply {
    pub(super) server_host_key: Vec<u8>,
    pub(super) server_public: Vec<u8>,
    pub(super) signature: Vec<u8>,
}

pub(super) fn parse_ssh_kex_ecdh_reply(payload: &[u8]) -> io::Result<SshKexEcdhReply> {
    if payload.first().copied() != Some(31) {
        return Err(invalid_packet("SSH payload is not KEX_ECDH_REPLY"));
    }
    let mut reader = SshReader::new(&payload[1..]);
    let server_host_key = reader.string()?.to_vec();
    let server_public = reader.string()?.to_vec();
    let signature = reader.string()?.to_vec();
    reader.finish()?;
    if server_host_key.is_empty() || server_public.is_empty() || signature.is_empty() {
        return Err(invalid_packet("SSH KEX_ECDH_REPLY contains an empty field"));
    }
    Ok(SshKexEcdhReply {
        server_host_key,
        server_public,
        signature,
    })
}

pub(super) fn verify_ssh_ed25519_host_key(
    host_key_blob: &[u8],
    signature_blob: &[u8],
    exchange_hash: &[u8; 32],
) -> io::Result<()> {
    let mut host_key_reader = SshReader::new(host_key_blob);
    let algorithm = host_key_reader.string()?;
    let public_key = host_key_reader.string()?;
    host_key_reader.finish()?;
    if algorithm != b"ssh-ed25519" || public_key.len() != 32 {
        return Err(invalid_packet("SSH server host key is not ssh-ed25519"));
    }

    let mut signature_reader = SshReader::new(signature_blob);
    let signature_algorithm = signature_reader.string()?;
    let signature = signature_reader.string()?;
    signature_reader.finish()?;
    if signature_algorithm != b"ssh-ed25519" || signature.len() != 64 {
        return Err(invalid_packet("SSH server signature is not ssh-ed25519"));
    }

    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(exchange_hash, signature)
        .map_err(|_| invalid_packet("SSH server host key signature is invalid"))
}

pub(super) fn derive_ssh_key(
    shared_secret: &[u8],
    exchange_hash: &[u8; 32],
    session_id: &[u8; 32],
    purpose: u8,
    length: usize,
) -> io::Result<Vec<u8>> {
    if shared_secret.len() != 32 || length > 1024 * 1024 {
        return Err(invalid_packet("SSH key derivation input is invalid"));
    }
    let mut encoded_secret = Zeroizing::new(Vec::new());
    encode_ssh_mpint(shared_secret, &mut encoded_secret)?;
    let mut output = Zeroizing::new(Vec::with_capacity(length));
    let mut first = true;
    while output.len() < length {
        let mut context = digest::Context::new(&digest::SHA256);
        context.update(&encoded_secret);
        context.update(exchange_hash);
        if first {
            context.update(&[purpose]);
            context.update(session_id);
            first = false;
        } else {
            context.update(&output);
        }
        output.extend_from_slice(context.finish().as_ref());
    }
    output.truncate(length);
    Ok(output.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;
    #[test]
    fn ssh_kexinit_decodes_name_lists_and_negotiates_client_preference() {
        fn put_string(value: &[u8], output: &mut Vec<u8>) {
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value);
        }

        let mut payload = vec![20; 17];
        let lists = [
            b"curve25519-sha256,diffie-hellman-group14-sha256".as_slice(),
            b"ssh-ed25519".as_slice(),
            b"aes128-ctr".as_slice(),
            b"aes128-ctr".as_slice(),
            b"hmac-sha2-256".as_slice(),
            b"hmac-sha2-256".as_slice(),
            b"none".as_slice(),
            b"none".as_slice(),
            b"".as_slice(),
            b"".as_slice(),
        ];
        for list in lists {
            put_string(list, &mut payload);
        }
        payload.push(0);
        payload.extend_from_slice(&0u32.to_be_bytes());
        let parsed = parse_ssh_kexinit(&payload).unwrap();
        assert_eq!(parsed.kex_algorithms[0], "curve25519-sha256");

        let negotiated = negotiate_ssh_algorithms(&parsed, &parsed).unwrap();
        assert_eq!(negotiated.kex, "curve25519-sha256");
        assert_eq!(negotiated.host_key, "ssh-ed25519");
        assert_eq!(negotiated.cipher_client_to_server, "aes128-ctr");

        let mut trailing = payload;
        trailing.push(0);
        assert!(parse_ssh_kexinit(&trailing).is_err());

        let generated = build_client_kexinit(&[42; 16]);
        let parsed = parse_ssh_kexinit(&generated).unwrap();
        assert_eq!(parsed.kex_algorithms, ["curve25519-sha256"]);
        assert_eq!(parsed.host_key_algorithms, ["ssh-ed25519"]);
        assert_eq!(parsed.encryption_client_to_server, ["aes128-ctr"]);
        assert_eq!(parsed.mac_client_to_server, ["hmac-sha2-256"]);
        assert_eq!(parsed.compression_client_to_server, ["none"]);

        let init = build_client_kex_ecdh_init(&[17; 32]);
        assert_eq!(init[0], 30);
        assert_eq!(&init[1..5], &32u32.to_be_bytes());
        assert_eq!(&init[5..], &[17; 32]);
    }

    #[test]
    fn ssh_kex_ecdh_reply_verifies_ed25519_host_key_signature() {
        fn put_string(value: &[u8], output: &mut Vec<u8>) {
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value);
        }

        let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap();
        let mut host_key = Vec::new();
        put_string(b"ssh-ed25519", &mut host_key);
        put_string(key_pair.public_key().as_ref(), &mut host_key);
        let exchange_hash = [11u8; 32];
        let signature = key_pair.sign(&exchange_hash);
        let mut signature_blob = Vec::new();
        put_string(b"ssh-ed25519", &mut signature_blob);
        put_string(signature.as_ref(), &mut signature_blob);

        let mut reply_payload = vec![31];
        put_string(&host_key, &mut reply_payload);
        put_string(&[13u8; 32], &mut reply_payload);
        put_string(&signature_blob, &mut reply_payload);
        let reply = parse_ssh_kex_ecdh_reply(&reply_payload).unwrap();
        assert_eq!(reply.server_host_key, host_key);
        assert_eq!(reply.server_public, vec![13u8; 32]);
        verify_ssh_ed25519_host_key(&reply.server_host_key, &reply.signature, &exchange_hash)
            .unwrap();

        let mut wrong_hash = exchange_hash;
        wrong_hash[0] ^= 1;
        assert!(
            verify_ssh_ed25519_host_key(&reply.server_host_key, &reply.signature, &wrong_hash,)
                .is_err()
        );
    }

    #[test]
    fn ssh_kex_ecdh_reply_rejects_malformed_host_key_and_signature() {
        fn put_string(value: &[u8], output: &mut Vec<u8>) {
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value);
        }

        let mut payload = vec![31];
        put_string(b"host-key", &mut payload);
        put_string(&[1u8; 32], &mut payload);
        put_string(b"signature", &mut payload);
        assert!(parse_ssh_kex_ecdh_reply(&payload).is_ok());
        payload.push(0);
        assert!(parse_ssh_kex_ecdh_reply(&payload).is_err());

        let mut host_key = Vec::new();
        put_string(b"ssh-rsa", &mut host_key);
        put_string(&[1u8; 32], &mut host_key);
        let mut signature = Vec::new();
        put_string(b"ssh-ed25519", &mut signature);
        put_string(&[2u8; 64], &mut signature);
        assert!(verify_ssh_ed25519_host_key(&host_key, &signature, &[0; 32]).is_err());
    }

    #[test]
    fn ssh_curve25519_exchange_agrees_and_rejects_low_order_points() {
        let client = Curve25519ClientKex::generate().unwrap();
        let server = Curve25519ClientKex::generate().unwrap();
        let client_public = *client.client_public();
        let server_public = *server.client_public();
        let client_shared = client.complete(&server_public).unwrap();
        let server_shared = server.complete(&client_public).unwrap();
        assert_eq!(client_shared, server_shared);
        assert_eq!(client_shared.len(), 32);

        let invalid = Curve25519ClientKex::generate().unwrap();
        assert!(invalid.complete(&[0; 32]).is_err());
        let invalid = Curve25519ClientKex::generate().unwrap();
        assert!(invalid.complete(&[1; 31]).is_err());
    }

    #[test]
    fn ssh_curve25519_exchange_hash_is_length_checked_and_ordered() {
        let public = [7u8; 32];
        let shared = [9u8; 32];
        let first = curve25519_exchange_hash(Curve25519ExchangeHashInput {
            client_identification: b"SSH-2.0-client",
            server_identification: b"SSH-2.0-server",
            client_kexinit: b"client-kex",
            server_kexinit: b"server-kex",
            server_host_key: b"host-key",
            client_public: &public,
            server_public: &public,
            shared_secret: &shared,
        })
        .unwrap();
        let second = curve25519_exchange_hash(Curve25519ExchangeHashInput {
            client_identification: b"SSH-2.0-server",
            server_identification: b"SSH-2.0-client",
            client_kexinit: b"client-kex",
            server_kexinit: b"server-kex",
            server_host_key: b"host-key",
            client_public: &public,
            server_public: &public,
            shared_secret: &shared,
        })
        .unwrap();
        assert_ne!(first, second);
        assert!(
            curve25519_exchange_hash(Curve25519ExchangeHashInput {
                client_identification: b"client",
                server_identification: b"server",
                client_kexinit: b"client-kex",
                server_kexinit: b"server-kex",
                server_host_key: b"host-key",
                client_public: &public,
                server_public: &[0; 31],
                shared_secret: &shared,
            })
            .is_err()
        );
    }

    #[test]
    fn ssh_key_derivation_is_role_separated_and_extends_hash_output() {
        let shared = [9u8; 32];
        let exchange_hash = [10u8; 32];
        let session_id = [11u8; 32];
        let client_iv = derive_ssh_key(&shared, &exchange_hash, &session_id, b'A', 16).unwrap();
        let server_iv = derive_ssh_key(&shared, &exchange_hash, &session_id, b'B', 16).unwrap();
        assert_ne!(client_iv, server_iv);
        let extended = derive_ssh_key(&shared, &exchange_hash, &session_id, b'A', 64).unwrap();
        assert_eq!(&extended[..16], client_iv);
        assert_eq!(extended.len(), 64);
        assert!(derive_ssh_key(&shared[..31], &exchange_hash, &session_id, b'A', 16).is_err());
    }
}
