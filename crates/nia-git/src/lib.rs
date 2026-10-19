//! Nia-owned Git wire and object decoding primitives.
//!
//! Git object validation and checkout live behind this boundary. Package graph
//! code consumes filesystem roots and never sees Git object representations.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use flate2::read::ZlibDecoder;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use sha1::{Digest, Sha1};

const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_HTTP_RESPONSE: usize = 128 * 1024 * 1024;
const MAX_PACKET_PAYLOAD: usize = 65_516;
const MAX_SSH_IDENTIFICATION_LINE: usize = 255;
const MAX_SSH_PRE_IDENTIFICATION_LINES: usize = 50;
const MAX_SSH_PACKET_BYTES: usize = 256 * 1024;

fn invalid_packet(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PacketLine {
    Data(Vec<u8>),
    Flush,
    Delimiter,
    ResponseEnd,
}

fn decode_packet_lines(mut input: &[u8]) -> io::Result<Vec<PacketLine>> {
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

fn encode_packet_line(payload: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitObject {
    kind: ObjectKind,
    data: Vec<u8>,
    id: [u8; 20],
}

fn decode_loose_object(compressed: &[u8]) -> io::Result<GitObject> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut bytes = Vec::new();
    (&mut decoder)
        .take((MAX_OBJECT_BYTES + 128) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_OBJECT_BYTES + 128 {
        return Err(invalid_packet("Git object exceeds size limit"));
    }
    if decoder.total_in() as usize != compressed.len() {
        return Err(invalid_packet("Git object has trailing compressed data"));
    }
    let separator = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| invalid_packet("Git object header is missing its terminator"))?;
    let header = std::str::from_utf8(&bytes[..separator])
        .map_err(|_| invalid_packet("Git object header is not ASCII"))?;
    let (kind, declared_size) = header
        .split_once(' ')
        .ok_or_else(|| invalid_packet("Git object header is malformed"))?;
    let kind = match kind {
        "commit" => ObjectKind::Commit,
        "tree" => ObjectKind::Tree,
        "blob" => ObjectKind::Blob,
        "tag" => ObjectKind::Tag,
        _ => return Err(invalid_packet("unsupported Git object type")),
    };
    let declared_size = declared_size
        .parse::<usize>()
        .map_err(|_| invalid_packet("Git object size is malformed"))?;
    let data = &bytes[separator + 1..];
    if data.len() != declared_size || declared_size > MAX_OBJECT_BYTES {
        return Err(invalid_packet("Git object size does not match its header"));
    }
    let mut hash = Sha1::new();
    hash.update(&bytes[..separator + 1]);
    hash.update(data);
    let id: [u8; 20] = hash.finalize().into();
    Ok(GitObject {
        kind,
        data: data.to_vec(),
        id,
    })
}

#[cfg(test)]
fn encode_loose_object(kind: ObjectKind, data: &[u8]) -> io::Result<Vec<u8>> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    if data.len() > MAX_OBJECT_BYTES {
        return Err(invalid_packet("Git object exceeds size limit"));
    }
    let kind = match kind {
        ObjectKind::Commit => "commit",
        ObjectKind::Tree => "tree",
        ObjectKind::Blob => "blob",
        ObjectKind::Tag => "tag",
    };
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    write!(encoder, "{kind} {}\0", data.len())?;
    encoder.write_all(data)?;
    encoder.finish()
}

fn checkout_local(repository: &Path, revision: &str, destination: &Path) -> io::Result<String> {
    let git_dir = repository.join(".git");
    let git_dir = if git_dir.is_dir() {
        git_dir
    } else {
        repository.to_path_buf()
    };
    let commit_id = resolve_revision(&git_dir, revision)?;
    let commit = read_loose_object(&git_dir, &commit_id)?;
    if commit.kind != ObjectKind::Commit {
        return Err(invalid_packet("selected Git revision is not a commit"));
    }
    let tree_line = commit
        .data
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(b"tree "))
        .ok_or_else(|| invalid_packet("Git commit has no tree"))?;
    let tree_id = parse_object_id(tree_line)?;
    fs_create_dir(destination)?;
    checkout_tree(&git_dir, &tree_id, destination)?;
    Ok(encode_hex(&commit_id))
}

pub fn checkout_source(source: &str, revision: &str, destination: &Path) -> io::Result<String> {
    if let Some(repository) = source.strip_prefix("file:///") {
        let repository = if cfg!(windows) && repository.as_bytes().get(1) == Some(&b':') {
            PathBuf::from(repository)
        } else {
            PathBuf::from(format!("/{repository}"))
        };
        return checkout_local(&repository, revision, destination);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return checkout_http(source, revision, destination);
    }
    if source.starts_with("git://") {
        return checkout_git(source, revision, destination);
    }
    if source.starts_with("ssh://") || source.starts_with("git@") {
        let _ = ssh::parse_ssh_url(source)?;
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "SSH Git transport negotiation is not enabled yet",
        ));
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("Git transport is not implemented for `{source}`"),
    ))
}

mod ssh {
    // The protocol primitives are staged here before the transport state machine
    // consumes them; keeping the allowance local preserves the rest of the crate's
    // dead-code checks.
    #![allow(dead_code)]

    use super::{
        MAX_SSH_IDENTIFICATION_LINE, MAX_SSH_PACKET_BYTES, MAX_SSH_PRE_IDENTIFICATION_LINES, Read,
        Write, invalid_packet, io,
    };
    use aes::Aes128;
    use ctr::cipher::{KeyIvInit, StreamCipher};
    use ring::{agreement, digest, hmac, rand, signature};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct SshUrl {
        pub(super) user: String,
        pub(super) host: String,
        pub(super) port: u16,
        pub(super) path: String,
    }

    pub(super) fn parse_ssh_url(source: &str) -> io::Result<SshUrl> {
        let (user, host, port, path) = if let Some(rest) = source.strip_prefix("ssh://") {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let (user, authority) = authority
                .rsplit_once('@')
                .map_or(("git", authority), |(user, authority)| (user, authority));
            let (host, port) = parse_ssh_authority(authority)?;
            (user, host, port, path)
        } else {
            let (user, rest) = source
                .split_once('@')
                .ok_or_else(|| invalid_packet("SSH source must include a user"))?;
            let (host, path) = rest
                .split_once(':')
                .ok_or_else(|| invalid_packet("SCP-style SSH source must include a path"))?;
            (user, host.to_string(), 22, path)
        };
        if user.is_empty()
            || user.len() > 128
            || host.is_empty()
            || host.contains(['/', '?', '#', '@', '\0', '\r', '\n'])
            || path.is_empty()
            || path.contains(['?', '#', '\0', '\r', '\n'])
            || user.contains(['/', ':', '@', '\0', '\r', '\n'])
        {
            return Err(invalid_packet("SSH source contains invalid URL components"));
        }
        Ok(SshUrl {
            user: user.to_string(),
            host,
            port,
            path: format!("/{path}"),
        })
    }

    fn parse_ssh_authority(authority: &str) -> io::Result<(String, u16)> {
        if authority.is_empty() || authority.contains(['?', '#', '\0', '\r', '\n']) {
            return Err(invalid_packet("SSH URL has an invalid authority"));
        }
        if let Some(host) = authority.strip_prefix('[') {
            let (host, port) = host
                .split_once(']')
                .ok_or_else(|| invalid_packet("SSH URL has an invalid IPv6 authority"))?;
            let port = if port.is_empty() {
                ""
            } else {
                port.strip_prefix(':')
                    .ok_or_else(|| invalid_packet("SSH URL has an invalid IPv6 authority"))?
            };
            let port = if port.is_empty() {
                22
            } else {
                parse_ssh_port(port)?
            };
            if host.is_empty() {
                return Err(invalid_packet("SSH URL has an empty host"));
            }
            return Ok((host.to_string(), port));
        }
        if let Some((host, port)) = authority.rsplit_once(':') {
            if host.is_empty() || host.contains(':') {
                return Err(invalid_packet("SSH URL has an empty host"));
            }
            return Ok((host.to_string(), parse_ssh_port(port)?));
        }
        Ok((authority.to_string(), 22))
    }

    fn parse_ssh_port(port: &str) -> io::Result<u16> {
        if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_packet("SSH port is invalid"));
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| invalid_packet("SSH port is invalid"))?;
        if port == 0 {
            return Err(invalid_packet("SSH port is invalid"));
        }
        Ok(port)
    }

    fn write_ssh_identification(mut stream: impl Write) -> io::Result<()> {
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

    pub(super) fn read_ssh_packet(
        mut stream: impl Read,
        block_size: usize,
    ) -> io::Result<SshPacket> {
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
            host_key: choose_ssh_algorithm(
                &client.host_key_algorithms,
                &server.host_key_algorithms,
            )?
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
        let mut encoded = Vec::new();
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

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct KnownHostEntry {
        patterns: Vec<String>,
        host_key: Vec<u8>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub(super) struct KnownHosts {
        entries: Vec<KnownHostEntry>,
    }

    impl KnownHosts {
        pub(super) fn parse(input: &[u8]) -> io::Result<Self> {
            let text = std::str::from_utf8(input)
                .map_err(|_| invalid_packet("SSH known_hosts is not UTF-8"))?;
            let mut entries = Vec::new();
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut fields = line.split_whitespace();
                let first = fields
                    .next()
                    .ok_or_else(|| invalid_packet("SSH known_hosts line is empty"))?;
                let hosts = if first.starts_with('@') {
                    return Err(invalid_packet("SSH known_hosts markers are unsupported"));
                } else {
                    first
                };
                let algorithm = fields
                    .next()
                    .ok_or_else(|| invalid_packet("SSH known_hosts line has no key type"))?;
                let encoded_key = fields
                    .next()
                    .ok_or_else(|| invalid_packet("SSH known_hosts line has no key"))?;
                if fields.next().is_some() && !line.contains(" ") && !line.contains('\t') {
                    return Err(invalid_packet("SSH known_hosts line is malformed"));
                }
                let key_blob = decode_base64(encoded_key)?;
                let mut key_reader = SshReader::new(&key_blob);
                if key_reader.string()? != algorithm.as_bytes() {
                    return Err(invalid_packet(
                        "SSH known_hosts key type does not match its blob",
                    ));
                }
                if key_blob.is_empty() {
                    return Err(invalid_packet("SSH known_hosts key is empty"));
                }
                let patterns = hosts
                    .split(',')
                    .filter(|pattern| !pattern.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if patterns.is_empty() {
                    return Err(invalid_packet("SSH known_hosts host pattern is empty"));
                }
                entries.push(KnownHostEntry {
                    patterns,
                    host_key: key_blob,
                });
            }
            Ok(Self { entries })
        }

        pub(super) fn verify(&self, host: &str, port: u16, host_key: &[u8]) -> io::Result<()> {
            let host_port = if port == 22 {
                host.to_owned()
            } else {
                format!("[{host}]:{port}")
            };
            let mut matched = false;
            for entry in &self.entries {
                if !entry
                    .patterns
                    .iter()
                    .filter(|pattern| !pattern.starts_with('!'))
                    .any(|pattern| known_host_pattern_matches(pattern, &host_port, host, port))
                {
                    continue;
                }
                if entry.patterns.iter().any(|pattern| {
                    pattern.starts_with('!')
                        && known_host_pattern_matches(pattern, &host_port, host, port)
                }) {
                    continue;
                }
                matched = true;
                if entry.host_key == host_key {
                    return Ok(());
                }
            }
            if matched {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "SSH host key does not match known_hosts",
                ))
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "SSH host is not present in known_hosts",
                ))
            }
        }
    }

    fn known_host_pattern_matches(pattern: &str, host_port: &str, host: &str, port: u16) -> bool {
        let pattern = pattern.strip_prefix('!').unwrap_or(pattern);
        if pattern.starts_with("|1|") {
            return hashed_known_host_matches(pattern, host_port)
                || (port == 22 && hashed_known_host_matches(pattern, host));
        }
        let candidate = if pattern.starts_with('[') {
            host_port
        } else {
            host
        };
        glob_match(pattern.as_bytes(), candidate.as_bytes())
    }

    fn hashed_known_host_matches(pattern: &str, candidate: &str) -> bool {
        let mut fields = pattern.split('|');
        let _ = fields.next();
        let version = fields.next();
        let salt = fields.next().and_then(|value| decode_base64(value).ok());
        let digest = fields.next().and_then(|value| decode_base64(value).ok());
        let (Some("1"), Some(salt), Some(expected)) = (version, salt, digest) else {
            return false;
        };
        let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &salt);
        hmac::verify(&key, candidate.as_bytes(), &expected).is_ok()
    }

    fn glob_match(pattern: &[u8], candidate: &[u8]) -> bool {
        let mut states = vec![(0usize, 0usize)];
        while let Some((pattern_index, candidate_index)) = states.pop() {
            if pattern_index == pattern.len() {
                if candidate_index == candidate.len() {
                    return true;
                }
                continue;
            }
            match pattern[pattern_index] {
                b'*' => {
                    states.push((pattern_index + 1, candidate_index));
                    if candidate_index < candidate.len() {
                        states.push((pattern_index, candidate_index + 1));
                    }
                }
                b'?' if candidate_index < candidate.len() => {
                    states.push((pattern_index + 1, candidate_index + 1));
                }
                byte if candidate_index < candidate.len()
                    && byte.eq_ignore_ascii_case(&candidate[candidate_index]) =>
                {
                    states.push((pattern_index + 1, candidate_index + 1));
                }
                _ => {}
            }
        }
        false
    }

    fn decode_base64(input: &str) -> io::Result<Vec<u8>> {
        if input.is_empty() || input.len() % 4 == 1 {
            return Err(invalid_packet("SSH known_hosts base64 is malformed"));
        }
        let mut output = Vec::with_capacity(input.len() * 3 / 4);
        let mut buffer = 0u32;
        let mut bits = 0u8;
        for byte in input.bytes() {
            if byte == b'=' {
                break;
            }
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

    fn encode_ssh_string(value: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
        let length = u32::try_from(value.len())
            .map_err(|_| invalid_packet("SSH string exceeds uint32 length"))?;
        output.extend_from_slice(&length.to_be_bytes());
        output.extend_from_slice(value);
        Ok(())
    }

    fn encode_ssh_mpint(value: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
        let first = value.iter().position(|byte| *byte != 0);
        let value = first.map_or(&[][..], |index| &value[index..]);
        let mut encoded = Vec::with_capacity(value.len() + 1);
        if value.first().is_some_and(|byte| *byte & 0x80 != 0) {
            encoded.push(0);
        }
        encoded.extend_from_slice(value);
        encode_ssh_string(&encoded, output)
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
        let mut encoded_secret = Vec::new();
        encode_ssh_mpint(shared_secret, &mut encoded_secret)?;
        let mut output = Vec::with_capacity(length);
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
        Ok(output)
    }

    struct SshReader<'a> {
        bytes: &'a [u8],
        offset: usize,
    }

    impl<'a> SshReader<'a> {
        fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, offset: 0 }
        }

        fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
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

        fn u32(&mut self) -> io::Result<u32> {
            Ok(u32::from_be_bytes(
                self.take(4)?.try_into().expect("length is four"),
            ))
        }

        fn boolean(&mut self) -> io::Result<bool> {
            Ok(self.take(1)?[0] != 0)
        }

        fn string(&mut self) -> io::Result<&'a [u8]> {
            let length = usize::try_from(self.u32()?)
                .map_err(|_| invalid_packet("SSH string length overflows usize"))?;
            self.take(length)
        }

        fn name_list(&mut self) -> io::Result<Vec<String>> {
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

        fn finish(&self) -> io::Result<()> {
            if self.offset == self.bytes.len() {
                Ok(())
            } else {
                Err(invalid_packet("SSH payload has trailing bytes"))
            }
        }
    }
}

#[derive(Debug, Clone)]
struct GitTcpUrl {
    host: String,
    port: u16,
    path: String,
}

fn parse_git_tcp_url(source: &str) -> io::Result<GitTcpUrl> {
    let rest = source
        .strip_prefix("git://")
        .ok_or_else(|| invalid_packet("Git TCP source must use git://"))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.contains('@') || authority.contains(['?', '#']) {
        return Err(invalid_packet("Git TCP URL has an invalid authority"));
    }
    let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_packet("Git TCP URL has an invalid port"));
        }
        (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| invalid_packet("Git TCP port is invalid"))?,
        )
    } else {
        (authority.to_string(), 9418)
    };
    if path.is_empty() || path.contains(['?', '#', '\0', '\r', '\n']) {
        return Err(invalid_packet("Git TCP URL has an invalid repository path"));
    }
    Ok(GitTcpUrl {
        host,
        port,
        path: format!("/{path}"),
    })
}

fn checkout_git(source: &str, revision: &str, destination: &Path) -> io::Result<String> {
    let url = parse_git_tcp_url(source)?;
    let address = format!("{}:{}", url.host, url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Git host has no address"))?;
    let mut stream = TcpStream::connect_timeout(&address, std::time::Duration::from_secs(15))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    let handshake = format!("git-upload-pack {}\0host={}\0", url.path, url.host);
    let mut request = Vec::new();
    encode_packet_line(handshake.as_bytes(), &mut request)?;
    stream.write_all(&request)?;
    let advertisement = read_packet_section(&mut stream)?;
    let refs = parse_advertised_refs(&advertisement)?;
    let commit = select_advertised_revision(&refs, revision)?;

    let mut request = Vec::new();
    encode_packet_line(
        format!("want {} ofs-delta side-band-64k\n", encode_hex(&commit)).as_bytes(),
        &mut request,
    )?;
    request.extend_from_slice(b"0000");
    encode_packet_line(b"done\n", &mut request)?;
    stream.write_all(&request)?;
    stream.flush()?;
    let mut response = Vec::new();
    stream
        .take((MAX_HTTP_RESPONSE + 1) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_HTTP_RESPONSE {
        return Err(invalid_packet("Git response is too large"));
    }
    let pack = parse_upload_pack_response(&response)?;
    write_pack_checkout(&pack, &commit, destination)
}

fn read_packet_section(stream: &mut impl Read) -> io::Result<Vec<u8>> {
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

fn write_pack_checkout(pack: &[u8], commit: &[u8; 20], destination: &Path) -> io::Result<String> {
    let git_dir = destination.join(".git");
    let pack_dir = git_dir.join("objects/pack");
    fs_create_dir(&pack_dir)?;
    std::fs::write(git_dir.join("HEAD"), encode_hex(commit))?;
    let pack_id: [u8; 20] = pack
        .get(pack.len().saturating_sub(20)..)
        .filter(|bytes| bytes.len() == 20)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid_packet("Git upload-pack response has no pack checksum"))?;
    std::fs::write(
        pack_dir.join(format!("pack-{}.pack", encode_hex(&pack_id))),
        pack,
    )?;
    let result = checkout_local(destination, "HEAD", destination);
    let _ = std::fs::remove_dir_all(&git_dir);
    result
}

#[derive(Debug, Clone)]
struct HttpGitUrl {
    host: String,
    port: u16,
    path: String,
    secure: bool,
}

fn parse_http_git_url(source: &str) -> io::Result<HttpGitUrl> {
    let (secure, rest) = if let Some(rest) = source.strip_prefix("http://") {
        (false, rest)
    } else if let Some(rest) = source.strip_prefix("https://") {
        (true, rest)
    } else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Git HTTP source must use http:// or https://",
        ));
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.contains('@') || authority.contains(['?', '#']) {
        return Err(invalid_packet("Git HTTP URL has an invalid authority"));
    }
    let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_packet("Git HTTP URL has an invalid port"));
        }
        (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| invalid_packet("Git HTTP port is invalid"))?,
        )
    } else {
        (authority.to_string(), 80)
    };
    if host.contains(['/', '?', '#']) || path.contains(['?', '#']) {
        return Err(invalid_packet("Git HTTP URL contains invalid path syntax"));
    }
    let path = format!("/{}", path.trim_end_matches('/'));
    Ok(HttpGitUrl {
        host,
        port: if secure && port == 80 { 443 } else { port },
        path,
        secure,
    })
}

fn checkout_http(source: &str, revision: &str, destination: &Path) -> io::Result<String> {
    let url = parse_http_git_url(source)?;
    let info_path = format!("{}/info/refs?service=git-upload-pack", url.path);
    let info = http_request(&url, "GET", &info_path, &[], "")?;
    let refs = parse_advertised_refs(&info)?;
    let commit = select_advertised_revision(&refs, revision)?;

    let mut request = Vec::new();
    let want = format!("want {} ofs-delta side-band-64k\n", encode_hex(&commit));
    encode_packet_line(want.as_bytes(), &mut request)?;
    request.extend_from_slice(b"0000");
    encode_packet_line(b"done\n", &mut request)?;
    let response = http_request(
        &url,
        "POST",
        &format!("{}/git-upload-pack", url.path),
        &request,
        "application/x-git-upload-pack-request",
    )?;
    let pack = parse_upload_pack_response(&response)?;

    let git_dir = destination.join(".git");
    let pack_dir = git_dir.join("objects/pack");
    fs_create_dir(&pack_dir)?;
    std::fs::write(git_dir.join("HEAD"), encode_hex(&commit))?;
    let pack_id: [u8; 20] = pack
        .get(pack.len().saturating_sub(20)..)
        .filter(|bytes| bytes.len() == 20)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid_packet("Git upload-pack response has no pack checksum"))?;
    std::fs::write(
        pack_dir.join(format!("pack-{}.pack", encode_hex(&pack_id))),
        pack,
    )?;
    let result = checkout_local(destination, "HEAD", destination);
    let _ = std::fs::remove_dir_all(&git_dir);
    result
}

fn parse_advertised_refs(body: &[u8]) -> io::Result<Vec<([u8; 20], String)>> {
    let packets = decode_packet_lines(body)?;
    let mut refs = Vec::new();
    for packet in packets {
        let PacketLine::Data(data) = packet else {
            continue;
        };
        if data.starts_with(b"# service=") {
            continue;
        }
        let line = data.split(|byte| *byte == 0).next().unwrap_or(&data);
        let mut fields = line.splitn(2, |byte| *byte == b' ' || *byte == b'\n');
        let Some(id) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        let id = parse_object_id(id)?;
        let name = std::str::from_utf8(name)
            .map_err(|_| invalid_packet("Git advertised ref is not UTF-8"))?
            .trim()
            .to_string();
        if name.starts_with("refs/") || name == "HEAD" {
            refs.push((id, name));
        }
    }
    if refs.is_empty() {
        return Err(invalid_packet("Git server advertised no references"));
    }
    Ok(refs)
}

fn select_advertised_revision(refs: &[([u8; 20], String)], revision: &str) -> io::Result<[u8; 20]> {
    if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let id = parse_object_id(revision.as_bytes())?;
        if refs.iter().any(|(candidate, _)| candidate == &id) {
            return Ok(id);
        }
    }
    let names = if revision == "HEAD" {
        vec!["HEAD".to_string()]
    } else {
        vec![
            revision.to_string(),
            format!("refs/heads/{revision}"),
            format!("refs/tags/{revision}"),
        ]
    };
    names
        .iter()
        .find_map(|name| {
            refs.iter()
                .find(|(_, candidate)| candidate == &format!("{name}^{{}}"))
                .or_else(|| refs.iter().find(|(_, candidate)| candidate == name))
                .map(|(id, _)| *id)
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Git revision was not advertised"))
}

fn parse_upload_pack_response(body: &[u8]) -> io::Result<Vec<u8>> {
    let packets = decode_packet_lines(body)?;
    let mut pack = Vec::new();
    for packet in packets {
        let PacketLine::Data(data) = packet else {
            continue;
        };
        if data.starts_with(b"PACK") {
            pack.extend_from_slice(&data);
            continue;
        }
        if data.is_empty() {
            continue;
        }
        match data[0] {
            1 => pack.extend_from_slice(&data[1..]),
            2 => {
                return Err(io::Error::other(format!(
                    "Git upload-pack reported an error: {}",
                    String::from_utf8_lossy(&data[1..])
                )));
            }
            3 => {
                return Err(io::Error::other(format!(
                    "Git upload-pack failed: {}",
                    String::from_utf8_lossy(&data[1..])
                )));
            }
            _ => {}
        }
    }
    if pack.len() < 32 || &pack[..4] != b"PACK" {
        return Err(invalid_packet("Git upload-pack returned no pack"));
    }
    Ok(pack)
}

fn http_request(
    url: &HttpGitUrl,
    method: &str,
    path: &str,
    body: &[u8],
    content_type: &str,
) -> io::Result<Vec<u8>> {
    let address = format!("{}:{}", url.host, url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Git HTTP host has no address"))?;
    let stream = TcpStream::connect_timeout(&address, std::time::Duration::from_secs(15))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    if url.secure {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_name = ServerName::try_from(url.host.clone()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Git HTTPS host name is invalid",
            )
        })?;
        let tls = ClientConnection::new(Arc::new(config), server_name).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Git HTTPS setup failed: {error}"),
            )
        })?;
        return http_request_stream(
            StreamOwned::new(tls, stream),
            url,
            method,
            path,
            body,
            content_type,
        );
    }
    http_request_stream(stream, url, method, path, body, content_type)
}

fn http_request_stream<S: Read + Write>(
    mut stream: S,
    url: &HttpGitUrl,
    method: &str,
    path: &str,
    body: &[u8],
    content_type: &str,
) -> io::Result<Vec<u8>> {
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: nia-package-manager/0.3\r\nAccept: application/x-git-upload-pack-advertisement, application/x-git-upload-pack-result\r\nConnection: close\r\nContent-Length: {}\r\n",
        if (url.secure && url.port == 443) || (!url.secure && url.port == 80) {
            url.host.clone()
        } else {
            format!("{}:{}", url.host, url.port)
        },
        body.len()
    )?;
    if !content_type.is_empty() {
        write!(stream, "Content-Type: {content_type}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)?;
    let mut response = Vec::new();
    stream
        .take((MAX_HTTP_RESPONSE + 1) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_HTTP_RESPONSE {
        return Err(invalid_packet("Git HTTP response is too large"));
    }
    parse_http_response(&response)
}

fn parse_http_response(response: &[u8]) -> io::Result<Vec<u8>> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| invalid_packet("Git HTTP response has no header terminator"))?;
    let header = std::str::from_utf8(&response[..header_end])
        .map_err(|_| invalid_packet("Git HTTP response headers are not UTF-8"))?;
    let mut lines = header.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| invalid_packet("Git HTTP response status is malformed"))?;
    if !(200..300).contains(&status) {
        return Err(io::Error::other(format!(
            "Git HTTP server returned status {status}"
        )));
    }
    let mut chunked = false;
    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.to_ascii_lowercase().as_str() {
                "transfer-encoding" if value.trim().eq_ignore_ascii_case("chunked") => {
                    chunked = true
                }
                "content-length" => {
                    content_length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .map_err(|_| invalid_packet("Git HTTP content length is malformed"))?,
                    )
                }
                _ => {}
            }
        }
    }
    let body = &response[header_end + 4..];
    if chunked {
        decode_chunked_body(body)
    } else if let Some(length) = content_length {
        if body.len() < length {
            return Err(invalid_packet("Git HTTP body is truncated"));
        }
        Ok(body[..length].to_vec())
    } else {
        Ok(body.to_vec())
    }
}

fn decode_chunked_body(mut input: &[u8]) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| invalid_packet("Git HTTP chunk header is truncated"))?;
        let size = usize::from_str_radix(
            std::str::from_utf8(&input[..line_end])
                .map_err(|_| invalid_packet("Git HTTP chunk size is invalid"))?
                .split(';')
                .next()
                .unwrap_or_default(),
            16,
        )
        .map_err(|_| invalid_packet("Git HTTP chunk size is invalid"))?;
        input = &input[line_end + 2..];
        if size == 0 {
            return Ok(output);
        }
        if input.len() < size + 2 || &input[size..size + 2] != b"\r\n" {
            return Err(invalid_packet("Git HTTP chunk is truncated"));
        }
        output.extend_from_slice(&input[..size]);
        if output.len() > MAX_HTTP_RESPONSE {
            return Err(invalid_packet("Git HTTP response is too large"));
        }
        input = &input[size + 2..];
    }
}

fn fs_create_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

fn checkout_tree(git_dir: &Path, tree_id: &[u8; 20], directory: &Path) -> io::Result<()> {
    let tree = read_loose_object(git_dir, tree_id)?;
    if tree.kind != ObjectKind::Tree {
        return Err(invalid_packet("Git tree entry does not name a tree"));
    }
    let mut input = tree.data.as_slice();
    while !input.is_empty() {
        let mode_end = input
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| invalid_packet("Git tree entry has no mode separator"))?;
        let mode = std::str::from_utf8(&input[..mode_end])
            .map_err(|_| invalid_packet("Git tree mode is not ASCII"))?;
        input = &input[mode_end + 1..];
        let name_end = input
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| invalid_packet("Git tree entry has no name terminator"))?;
        let name = std::str::from_utf8(&input[..name_end])
            .map_err(|_| invalid_packet("Git tree path is not UTF-8"))?;
        validate_tree_name(name)?;
        input = &input[name_end + 1..];
        if input.len() < 20 {
            return Err(invalid_packet("truncated Git tree object id"));
        }
        let id: [u8; 20] = input[..20].try_into().expect("length checked");
        input = &input[20..];
        let path = directory.join(name);
        match mode {
            "40000" | "040000" => {
                fs_create_dir(&path)?;
                checkout_tree(git_dir, &id, &path)?;
            }
            "100644" | "100755" | "100664" => {
                let blob = read_loose_object(git_dir, &id)?;
                if blob.kind != ObjectKind::Blob {
                    return Err(invalid_packet("Git file entry does not name a blob"));
                }
                if let Some(parent) = path.parent() {
                    fs_create_dir(parent)?;
                }
                std::fs::write(path, blob.data)?;
            }
            _ => return Err(invalid_packet("unsupported Git tree entry mode")),
        }
    }
    Ok(())
}

fn validate_tree_name(name: &str) -> io::Result<()> {
    let path = Path::new(name);
    if name.is_empty()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
        || name == "."
        || name == ".."
    {
        return Err(invalid_packet("Git tree contains an unsafe path"));
    }
    Ok(())
}

fn resolve_revision(git_dir: &Path, revision: &str) -> io::Result<[u8; 20]> {
    if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let id = parse_object_id(revision.as_bytes())?;
        let object = read_loose_object(git_dir, &id)?;
        if object.kind == ObjectKind::Commit {
            return Ok(id);
        }
    }
    let candidates = [
        git_dir.join("refs").join("heads").join(revision),
        git_dir.join("refs").join("tags").join(revision),
        git_dir.join("refs").join(revision),
    ];
    for path in candidates {
        if path.is_file() {
            let text = std::fs::read_to_string(path)?;
            let line = text.trim();
            let id = if let Some(target) = line.strip_prefix("ref: ") {
                resolve_symbolic_ref(git_dir, target, 0)?
            } else {
                parse_object_id(line.as_bytes())?
            };
            let object = read_loose_object(git_dir, &id)?;
            if object.kind == ObjectKind::Commit {
                return Ok(id);
            }
            if object.kind == ObjectKind::Tag {
                let target = object
                    .data
                    .split(|byte| *byte == b'\n')
                    .find_map(|line| line.strip_prefix(b"object "))
                    .ok_or_else(|| invalid_packet("Git tag has no target object"))?;
                return parse_object_id(target);
            }
            return Err(invalid_packet("Git reference does not name a commit"));
        }
    }
    if revision == "HEAD" {
        return resolve_symbolic_ref_file(git_dir, &git_dir.join("HEAD"), 0);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git revision was not found",
    ))
}

fn resolve_symbolic_ref(git_dir: &Path, target: &str, depth: usize) -> io::Result<[u8; 20]> {
    if depth > 8 || !target.starts_with("refs/") {
        return Err(invalid_packet("Git symbolic reference is invalid"));
    }
    let path = git_dir.join(target);
    if path.is_file() {
        resolve_symbolic_ref_file(git_dir, &path, depth)
    } else {
        resolve_packed_ref(git_dir, target)
    }
}

fn resolve_packed_ref(git_dir: &Path, target: &str) -> io::Result<[u8; 20]> {
    let packed = std::fs::read_to_string(git_dir.join("packed-refs"))?;
    for line in packed.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        if let Some((id, name)) = line.split_once(' ')
            && name == target
        {
            return parse_object_id(id.as_bytes());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git reference was not found",
    ))
}

fn resolve_symbolic_ref_file(git_dir: &Path, path: &Path, depth: usize) -> io::Result<[u8; 20]> {
    let text = std::fs::read_to_string(path)?;
    let line = text.trim();
    let id = if let Some(target) = line.strip_prefix("ref: ") {
        resolve_symbolic_ref(git_dir, target, depth + 1)?
    } else {
        parse_object_id(line.as_bytes())?
    };
    let object = read_loose_object(git_dir, &id)?;
    if object.kind == ObjectKind::Commit {
        Ok(id)
    } else {
        Err(invalid_packet("Git HEAD does not resolve to a commit"))
    }
}

fn read_loose_object(git_dir: &Path, id: &[u8; 20]) -> io::Result<GitObject> {
    let hex = encode_hex(id);
    let path = git_dir.join("objects").join(&hex[..2]).join(&hex[2..]);
    match std::fs::read(path) {
        Ok(compressed) => {
            let object = decode_loose_object(&compressed)?;
            if object.id != *id {
                return Err(invalid_packet("Git object id does not match its contents"));
            }
            Ok(object)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => read_packed_object(git_dir, id),
        Err(error) => Err(error),
    }
}

fn read_packed_object(git_dir: &Path, wanted: &[u8; 20]) -> io::Result<GitObject> {
    let pack_dir = git_dir.join("objects").join("pack");
    let entries = match std::fs::read_dir(&pack_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Git object was not found in loose or packed storage",
            ));
        }
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("pack") {
            continue;
        }
        let pack = std::fs::read(&path)?;
        match decode_pack_object(&pack, wanted) {
            Ok(object) => return Ok(object),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git object was not found in packed storage",
    ))
}

fn decode_pack_object(pack: &[u8], wanted: &[u8; 20]) -> io::Result<GitObject> {
    if pack.len() < 12 || &pack[..4] != b"PACK" {
        return Err(invalid_packet("Git pack header is malformed"));
    }
    if pack.len() < 32 {
        return Err(invalid_packet("Git pack is missing its checksum"));
    }
    let payload_end = pack.len() - 20;
    let mut checksum = Sha1::new();
    checksum.update(&pack[..payload_end]);
    let checksum: [u8; 20] = checksum.finalize().into();
    if checksum != pack[payload_end..] {
        return Err(invalid_packet("Git pack checksum does not match"));
    }
    let version = u32::from_be_bytes(pack[4..8].try_into().expect("length checked"));
    if !matches!(version, 2 | 3) {
        return Err(invalid_packet("unsupported Git pack version"));
    }
    let count = u32::from_be_bytes(pack[8..12].try_into().expect("length checked"));
    if count > 1_000_000 {
        return Err(invalid_packet("Git pack contains too many objects"));
    }
    let mut cursor = 12usize;
    let mut objects = std::collections::BTreeMap::<usize, GitObject>::new();
    let mut unresolved = Vec::new();
    for _ in 0..count {
        let offset = cursor;
        let (kind, declared_size) = read_pack_header(pack, &mut cursor)?;
        match kind {
            1..=4 => {
                let (data, consumed) =
                    read_zlib_stream(&pack[cursor..payload_end], Some(declared_size))?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                let object = make_object(pack_kind(kind)?, data)?;
                objects.insert(offset, object);
            }
            6 => {
                let base_offset = read_ofs_delta_base(pack, &mut cursor, offset)?;
                let (delta, consumed) = read_zlib_stream(&pack[cursor..payload_end], None)?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                unresolved.push((offset, base_offset, None, delta));
            }
            7 => {
                if pack.len().saturating_sub(cursor) < 20 {
                    return Err(invalid_packet("truncated Git ref-delta base id"));
                }
                let base_id: [u8; 20] = pack[cursor..cursor + 20]
                    .try_into()
                    .expect("length checked");
                cursor += 20;
                let (delta, consumed) = read_zlib_stream(&pack[cursor..payload_end], None)?;
                cursor = cursor
                    .checked_add(consumed)
                    .ok_or_else(|| invalid_packet("Git pack offset overflow"))?;
                unresolved.push((offset, 0, Some(base_id), delta));
            }
            _ => return Err(invalid_packet("unsupported Git pack object type")),
        }
    }
    while !unresolved.is_empty() {
        let mut progress = false;
        let mut remaining = Vec::with_capacity(unresolved.len());
        for (offset, base_offset, base_id, delta) in unresolved {
            let base = if let Some(base_id) = base_id {
                objects
                    .values()
                    .find(|object| object.id == base_id)
                    .cloned()
            } else {
                objects.get(&base_offset).cloned()
            };
            let Some(base) = base else {
                remaining.push((offset, base_offset, base_id, delta));
                continue;
            };
            let data = apply_delta(&base.data, &delta)?;
            let object = make_object(base.kind, data)?;
            objects.insert(offset, object);
            progress = true;
        }
        if !progress {
            return Err(invalid_packet("Git delta base object is missing"));
        }
        unresolved = remaining;
    }
    if cursor != payload_end {
        return Err(invalid_packet("Git pack has trailing object data"));
    }
    objects
        .into_values()
        .find(|object| &object.id == wanted)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Git packed object id was not found",
            )
        })
}

fn read_pack_header(pack: &[u8], cursor: &mut usize) -> io::Result<(u8, usize)> {
    let first = *pack
        .get(*cursor)
        .ok_or_else(|| invalid_packet("truncated Git pack object header"))?;
    *cursor += 1;
    let kind = (first >> 4) & 7;
    let mut size = usize::from(first & 0x0f);
    let mut shift = 4;
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = *pack
            .get(*cursor)
            .ok_or_else(|| invalid_packet("truncated Git pack object size"))?;
        *cursor += 1;
        size |= usize::from(byte & 0x7f)
            .checked_shl(shift)
            .ok_or_else(|| invalid_packet("Git pack object size overflows"))?;
        shift += 7;
        if shift > 63 || size > MAX_OBJECT_BYTES {
            return Err(invalid_packet("Git pack object exceeds size limit"));
        }
    }
    Ok((kind, size))
}

fn read_ofs_delta_base(pack: &[u8], cursor: &mut usize, object_offset: usize) -> io::Result<usize> {
    let first = *pack
        .get(*cursor)
        .ok_or_else(|| invalid_packet("truncated Git ofs-delta base offset"))?;
    *cursor += 1;
    let mut value = usize::from(first & 0x7f);
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = *pack
            .get(*cursor)
            .ok_or_else(|| invalid_packet("truncated Git ofs-delta base offset"))?;
        *cursor += 1;
        value = value
            .checked_add(1)
            .and_then(|value| value.checked_shl(7))
            .and_then(|value| value.checked_add(usize::from(byte & 0x7f)))
            .ok_or_else(|| invalid_packet("Git ofs-delta base offset overflows"))?;
    }
    object_offset
        .checked_sub(value)
        .ok_or_else(|| invalid_packet("Git ofs-delta base offset is invalid"))
}

fn read_zlib_stream(input: &[u8], expected_size: Option<usize>) -> io::Result<(Vec<u8>, usize)> {
    let mut decoder = ZlibDecoder::new(input);
    let mut data = Vec::new();
    (&mut decoder)
        .take((expected_size.unwrap_or(MAX_OBJECT_BYTES) + 1) as u64)
        .read_to_end(&mut data)?;
    if data.len() > MAX_OBJECT_BYTES
        || expected_size.is_some_and(|expected| data.len() != expected)
        || decoder.total_in() == 0
    {
        return Err(invalid_packet(
            "Git pack object size does not match its header",
        ));
    }
    Ok((data, decoder.total_in() as usize))
}

fn pack_kind(kind: u8) -> io::Result<ObjectKind> {
    match kind {
        1 => Ok(ObjectKind::Commit),
        2 => Ok(ObjectKind::Tree),
        3 => Ok(ObjectKind::Blob),
        4 => Ok(ObjectKind::Tag),
        _ => Err(invalid_packet("unsupported Git pack object type")),
    }
}

fn make_object(kind: ObjectKind, data: Vec<u8>) -> io::Result<GitObject> {
    let name = match kind {
        ObjectKind::Commit => "commit",
        ObjectKind::Tree => "tree",
        ObjectKind::Blob => "blob",
        ObjectKind::Tag => "tag",
    };
    let header = format!("{name} {}\0", data.len());
    let mut hash = Sha1::new();
    hash.update(header.as_bytes());
    hash.update(&data);
    Ok(GitObject {
        kind,
        data,
        id: hash.finalize().into(),
    })
}

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

fn apply_delta(base: &[u8], delta: &[u8]) -> io::Result<Vec<u8>> {
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

fn parse_object_id(bytes: &[u8]) -> io::Result<[u8; 20]> {
    if bytes.len() != 40 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(invalid_packet(
            "Git object id must be 40 hexadecimal digits",
        ));
    }
    let mut id = [0; 20];
    for (index, pair) in bytes.chunks(2).enumerate() {
        id[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Ok(id)
}

fn hex_value(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(invalid_packet("invalid hexadecimal digit")),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::ssh::*;
    use super::*;
    use ring::signature::KeyPair;

    #[test]
    fn loose_objects_validate_type_size_and_git_object_identity() {
        let data = b"tree 0000000000000000000000000000000000000000\n";
        let encoded = encode_loose_object(ObjectKind::Commit, data).unwrap();
        let decoded = decode_loose_object(&encoded).unwrap();
        assert_eq!(decoded.kind, ObjectKind::Commit);
        assert_eq!(decoded.data, data);
        assert_eq!(decoded.id.len(), 20);

        let mut corrupt = encoded;
        corrupt.push(0);
        assert_eq!(
            decode_loose_object(&corrupt).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn deltas_copy_and_insert_with_checked_sizes() {
        // Source size 5, result size 10: copy `hello`, then insert `! nia`.
        let delta = [5, 10, 0x90, 5, 5, b'!', b' ', b'n', b'i', b'a'];
        assert_eq!(apply_delta(b"hello", &delta).unwrap(), b"hello! nia");

        // A zero copy length means Git's 0x10000 default, which must still be
        // rejected when it exceeds the base object.
        let invalid = [1, 1, 0x80];
        assert_eq!(
            apply_delta(b"x", &invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn packet_lines_decode_control_frames_and_advertised_refs() {
        let id = "0123456789012345678901234567890123456789";
        let advertisement = format!(
            "001e# service=git-upload-pack\n0000{length:04x}{id} refs/heads/main\0side-band-64k\n0000",
            length = id.len() + " refs/heads/main\0side-band-64k\n".len() + 4,
        );
        let refs = parse_advertised_refs(advertisement.as_bytes()).unwrap();
        assert_eq!(refs[0].1, "refs/heads/main");
        assert_eq!(
            select_advertised_revision(&refs, "main").unwrap(),
            parse_object_id(id.as_bytes()).unwrap()
        );
    }

    #[test]
    fn http_response_parser_handles_content_length_and_chunked_bodies() {
        let fixed = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(parse_http_response(fixed).unwrap(), b"abc");
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(chunked).unwrap(), b"abc");
        let rejected = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            parse_http_response(rejected).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn http_url_parser_selects_tls_and_rejects_embedded_credentials() {
        let plain = parse_http_git_url("http://example.test/repository.git").unwrap();
        assert!(!plain.secure);
        assert_eq!(plain.port, 80);
        let secure = parse_http_git_url("https://example.test/repository.git").unwrap();
        assert!(secure.secure);
        assert_eq!(secure.port, 443);
        assert_eq!(
            parse_http_git_url("https://user@example.test/repository.git")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn git_tcp_url_parser_defaults_port_and_rejects_credentials() {
        let default = parse_git_tcp_url("git://example.test/org/repository.git").unwrap();
        assert_eq!(default.host, "example.test");
        assert_eq!(default.port, 9418);
        assert_eq!(default.path, "/org/repository.git");
        let custom = parse_git_tcp_url("git://example.test:19418/org/repository.git").unwrap();
        assert_eq!(custom.port, 19418);
        assert!(parse_git_tcp_url("git://user@example.test/repository.git").is_err());
        assert!(parse_git_tcp_url("git://example.test/repo\n.git").is_err());
    }

    #[test]
    fn ssh_sources_are_validated_before_transport_is_reported_unavailable() {
        let destination =
            std::env::temp_dir().join(format!("nia-git-ssh-unsupported-{}", std::process::id()));
        let error = checkout_source(
            "ssh://git@example.test/org/repository.git",
            "HEAD",
            &destination,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("negotiation is not enabled"));
        let error = checkout_source("git@example.test:org/repository.git", "HEAD", &destination)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(checkout_source("ssh://host", "HEAD", &destination).is_err());
    }

    #[test]
    fn ssh_url_parser_handles_url_and_scp_forms() {
        let url = parse_ssh_url("ssh://git@example.test:2222/org/repository.git").unwrap();
        assert_eq!(url.user, "git");
        assert_eq!(url.host, "example.test");
        assert_eq!(url.port, 2222);
        assert_eq!(url.path, "/org/repository.git");

        let scp = parse_ssh_url("git@example.test:org/repository.git").unwrap();
        assert_eq!(scp.port, 22);
        assert_eq!(scp.path, "/org/repository.git");

        let ipv6 = parse_ssh_url("ssh://git@[2001:db8::1]/repo.git").unwrap();
        assert_eq!(ipv6.host, "2001:db8::1");
        for invalid in [
            "ssh://user:password@example.test/repo.git",
            "ssh://host/repo.git?query",
            "ssh://host:0/repo.git",
            "ssh://[2001:db8::1]garbage/repo.git",
            "ssh://host:12:3/repo.git",
            "git@example.test:",
        ] {
            assert!(parse_ssh_url(invalid).is_err(), "{invalid}");
        }
    }

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
    fn ssh_known_hosts_matches_plain_and_negated_patterns() {
        fn base64(input: &[u8]) -> String {
            const TABLE: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut output = String::new();
            for chunk in input.chunks(3) {
                let first = chunk[0];
                output.push(TABLE[(first >> 2) as usize] as char);
                let second = chunk.get(1).copied();
                output.push(
                    TABLE[((first & 3) << 4 | second.map_or(0, |byte| byte >> 4)) as usize] as char,
                );
                if let Some(second) = second {
                    let third = chunk.get(2).copied();
                    output.push(
                        TABLE[((second & 15) << 2 | third.map_or(0, |byte| byte >> 6)) as usize]
                            as char,
                    );
                    output.push(third.map_or('=', |byte| TABLE[(byte & 63) as usize] as char));
                } else {
                    output.push_str("==");
                }
            }
            output
        }

        fn put_string(value: &[u8], output: &mut Vec<u8>) {
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value);
        }
        let public = [7u8; 32];
        let mut key_blob = Vec::new();
        put_string(b"ssh-ed25519", &mut key_blob);
        put_string(&public, &mut key_blob);
        let line = format!(
            "*.example.test,!bad.example.test ssh-ed25519 {}\n",
            base64(&key_blob)
        );
        let known_hosts = KnownHosts::parse(line.as_bytes()).unwrap();
        known_hosts
            .verify("good.example.test", 22, &key_blob)
            .unwrap();
        assert_eq!(
            known_hosts
                .verify("bad.example.test", 22, &key_blob)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        let mut wrong = key_blob.clone();
        *wrong.last_mut().unwrap() ^= 1;
        assert_eq!(
            known_hosts
                .verify("good.example.test", 22, &wrong)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
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

    #[test]
    fn packed_object_checksums_are_part_of_the_trust_boundary() {
        let mut pack = b"PACK".to_vec();
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&0u32.to_be_bytes());
        let checksum = Sha1::digest(&pack);
        pack.extend_from_slice(&checksum);
        let wanted = [0; 20];
        assert!(decode_pack_object(&pack, &wanted).is_err());
        *pack.last_mut().unwrap() ^= 1;
        assert_eq!(
            decode_pack_object(&pack, &wanted).unwrap_err().to_string(),
            "Git pack checksum does not match"
        );
    }

    #[test]
    fn local_repository_checkout_reads_commit_tree_and_blobs() {
        use std::process::Command;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("nia-git-local-{}-{nonce}", std::process::id()));
        let repository = root.join("repository");
        let destination = root.join("checkout");
        std::fs::create_dir_all(repository.join("src/nested")).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repository)
                .output()
                .expect("Git executable is required for the test fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "nia-test@example.invalid"]);
        git(&["config", "user.name", "Nia Test"]);
        std::fs::write(repository.join("nia.toml"), "[package]\nname = \"dep\"\n").unwrap();
        std::fs::write(repository.join("src/nested/lib.nia"), "pub fn main() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);

        let revision = checkout_local(&repository, "HEAD", &destination).unwrap();
        assert_eq!(revision.len(), 40);
        assert_eq!(
            std::fs::read_to_string(destination.join("nia.toml")).unwrap(),
            "[package]\nname = \"dep\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("src/nested/lib.nia")).unwrap(),
            "pub fn main() {}\n"
        );
        assert!(!destination.join(".git").exists());

        git(&["gc", "--aggressive", "--prune=now"]);
        let packed_destination = root.join("packed-checkout");
        let packed_revision = checkout_local(&repository, "HEAD", &packed_destination).unwrap();
        assert_eq!(packed_revision, revision);
        assert_eq!(
            std::fs::read_to_string(packed_destination.join("src/nested/lib.nia")).unwrap(),
            "pub fn main() {}\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn http_smart_protocol_fetches_a_real_pack_without_system_git() {
        use std::net::TcpListener;
        use std::process::Command;
        use std::thread;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("nia-git-http-{}-{nonce}", std::process::id()));
        let repository = root.join("repository");
        let destination = root.join("checkout");
        std::fs::create_dir_all(repository.join("src/nested")).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repository)
                .output()
                .expect("Git executable is required for the test fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "nia-test@example.invalid"]);
        git(&["config", "user.name", "Nia Test"]);
        std::fs::write(repository.join("nia.toml"), "[package]\nname = \"dep\"\n").unwrap();
        std::fs::write(repository.join("src/nested/lib.nia"), "pub fn main() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);
        let revision = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repository)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(&["gc", "--aggressive", "--prune=now"]);
        let pack_path = std::fs::read_dir(repository.join(".git/objects/pack"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("pack"))
            .unwrap();
        let pack = std::fs::read(pack_path).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let advertised_revision = revision.clone();
        let server = thread::spawn(move || {
            for connection_index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let header_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let request_headers = std::str::from_utf8(&request[..header_end - 4]).unwrap();
                let request_body_length = request_headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|length| length.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + request_body_length {
                    let read = stream.read(&mut buffer).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let mut body = Vec::new();
                if connection_index == 0 {
                    encode_packet_line(b"# service=git-upload-pack\n", &mut body).unwrap();
                    body.extend_from_slice(b"0000");
                    let advertised = format!(
                        "{} refs/heads/main\0ofs-delta side-band-64k\n",
                        advertised_revision
                    );
                    encode_packet_line(advertised.as_bytes(), &mut body).unwrap();
                    body.extend_from_slice(b"0000");
                } else {
                    encode_packet_line(b"NAK\n", &mut body).unwrap();
                    for chunk in pack.chunks(MAX_PACKET_PAYLOAD - 1) {
                        let mut sideband = Vec::with_capacity(chunk.len() + 1);
                        sideband.push(1);
                        sideband.extend_from_slice(chunk);
                        encode_packet_line(&sideband, &mut body).unwrap();
                    }
                    body.extend_from_slice(b"0000");
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });

        let source = format!("http://127.0.0.1:{port}/repository.git");
        let fetched_revision = checkout_source(&source, "main", &destination).unwrap();
        assert_eq!(fetched_revision, revision);
        assert_eq!(
            std::fs::read_to_string(destination.join("src/nested/lib.nia")).unwrap(),
            "pub fn main() {}\n"
        );
        assert!(!destination.join(".git").exists());
        server.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_tcp_smart_protocol_fetches_a_real_pack_without_system_git() {
        use std::net::TcpListener;
        use std::process::Command;
        use std::thread;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("nia-git-tcp-{}-{nonce}", std::process::id()));
        let repository = root.join("repository");
        let destination = root.join("checkout");
        std::fs::create_dir_all(repository.join("src")).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repository)
                .output()
                .expect("Git executable is required for the test fixture");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "nia-test@example.invalid"]);
        git(&["config", "user.name", "Nia Test"]);
        std::fs::write(repository.join("nia.toml"), "[package]\nname = \"dep\"\n").unwrap();
        std::fs::write(repository.join("src/lib.nia"), "pub fn main() {}\n").unwrap();
        git(&["add", "nia.toml", "src"]);
        git(&["commit", "-qm", "fixture"]);
        let revision = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repository)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(&["gc", "--aggressive", "--prune=now"]);
        let pack_path = std::fs::read_dir(repository.join(".git/objects/pack"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("pack"))
            .unwrap();
        let pack = std::fs::read(pack_path).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let advertised_revision = revision.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            let length = parse_hex_usize(&header).unwrap();
            let mut handshake = vec![0; length - 4];
            stream.read_exact(&mut handshake).unwrap();
            assert!(
                String::from_utf8_lossy(&handshake).starts_with("git-upload-pack /repository.git")
            );
            assert!(handshake.ends_with(b"\0"));
            assert!(!handshake.ends_with(b"\0\0"));

            let mut advertisement = Vec::new();
            let advertised = format!(
                "{} refs/heads/main\0ofs-delta side-band-64k\n",
                advertised_revision
            );
            encode_packet_line(advertised.as_bytes(), &mut advertisement).unwrap();
            advertisement.extend_from_slice(b"0000");
            stream.write_all(&advertisement).unwrap();

            let _want = read_packet_section(&mut stream).unwrap();
            stream.read_exact(&mut header).unwrap();
            let length = parse_hex_usize(&header).unwrap();
            assert!(length >= 4);
            let mut done = vec![0; length - 4];
            stream.read_exact(&mut done).unwrap();
            assert_eq!(&done, b"done\n");

            let mut response = Vec::new();
            encode_packet_line(b"NAK\n", &mut response).unwrap();
            for chunk in pack.chunks(MAX_PACKET_PAYLOAD - 1) {
                let mut sideband = Vec::with_capacity(chunk.len() + 1);
                sideband.push(1);
                sideband.extend_from_slice(chunk);
                encode_packet_line(&sideband, &mut response).unwrap();
            }
            response.extend_from_slice(b"0000");
            stream.write_all(&response).unwrap();
        });

        let source = format!("git://127.0.0.1:{port}/repository.git");
        let fetched_revision = checkout_source(&source, "main", &destination).unwrap();
        assert_eq!(fetched_revision, revision);
        assert_eq!(
            std::fs::read_to_string(destination.join("src/lib.nia")).unwrap(),
            "pub fn main() {}\n"
        );
        assert!(!destination.join(".git").exists());
        server.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
