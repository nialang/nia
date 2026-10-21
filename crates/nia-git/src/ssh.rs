use super::{
    MAX_SSH_IDENTIFICATION_LINE, MAX_SSH_PACKET_BYTES, MAX_SSH_PRE_IDENTIFICATION_LINES, Read,
    Write, invalid_packet, io,
};
use aes::Aes128;
use ctr::cipher::{KeyIvInit, StreamCipher};
use ring::{agreement, digest, hmac, rand, rand::SecureRandom, signature, signature::KeyPair};
use zeroize::Zeroizing;

const SSH_CHANNEL_WINDOW: u32 = 2_097_152;
const SSH_CHANNEL_MAX_PACKET: usize = 32_768;

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
        || host.len() > 255
        || host.contains(['/', '?', '#', '@', '\0', '\r', '\n'])
        || path.is_empty()
        || path.len() > 4096
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

#[derive(Debug)]
pub(super) struct SshTransportKeys {
    pub(super) outbound: SshAes128CtrDirection,
    pub(super) inbound: SshAes128CtrDirection,
    pub(super) session_id: [u8; 32],
}

pub(super) fn build_publickey_auth_request(
    username: &str,
    session_id: &[u8; 32],
    signer: &mut dyn SshSigner,
) -> io::Result<Vec<u8>> {
    if username.is_empty() || username.len() > 256 || username.contains(['\0', '\r', '\n']) {
        return Err(invalid_packet("SSH username is invalid"));
    }
    let mut signed = Vec::new();
    encode_ssh_string(session_id, &mut signed)?;
    signed.push(50);
    encode_ssh_string(username.as_bytes(), &mut signed)?;
    encode_ssh_string(b"ssh-connection", &mut signed)?;
    encode_ssh_string(b"publickey", &mut signed)?;
    signed.push(1);
    encode_ssh_string(signer.signature_algorithm(), &mut signed)?;
    encode_ssh_string(signer.public_key_blob(), &mut signed)?;
    let signature = signer.sign_message(&signed)?;

    let mut signature_blob = Vec::new();
    encode_ssh_string(signer.signature_algorithm(), &mut signature_blob)?;
    encode_ssh_string(&signature, &mut signature_blob)?;
    let mut request = vec![50];
    encode_ssh_string(username.as_bytes(), &mut request)?;
    encode_ssh_string(b"ssh-connection", &mut request)?;
    encode_ssh_string(b"publickey", &mut request)?;
    request.push(1);
    encode_ssh_string(signer.signature_algorithm(), &mut request)?;
    encode_ssh_string(signer.public_key_blob(), &mut request)?;
    encode_ssh_string(&signature_blob, &mut request)?;
    Ok(request)
}

pub(super) fn authenticate_publickey(
    stream: &mut (impl Read + Write),
    keys: &mut SshTransportKeys,
    username: &str,
    signer: &mut dyn SshSigner,
) -> io::Result<()> {
    let mut service_request = vec![5];
    encode_ssh_string(b"ssh-userauth", &mut service_request)?;
    write_encrypted_ssh_packet(stream, &mut keys.outbound, &service_request)?;
    let service_accept = read_expected_encrypted_ssh_message(stream, &mut keys.inbound, 6)?;
    let mut reader = SshReader::new(&service_accept[1..]);
    if reader.string()? != b"ssh-userauth" {
        return Err(invalid_packet("SSH server accepted the wrong service"));
    }
    reader.finish()?;

    let request = build_publickey_auth_request(username, &keys.session_id, signer)?;
    write_encrypted_ssh_packet(stream, &mut keys.outbound, &request)?;
    match read_expected_encrypted_ssh_message_any(stream, &mut keys.inbound)? {
        payload if payload.first() == Some(&52) => Ok(()),
        payload if payload.first() == Some(&51) => {
            let mut reader = SshReader::new(&payload[1..]);
            let methods = String::from_utf8_lossy(reader.string()?).into_owned();
            let partial = reader.boolean()?;
            reader.finish()?;
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "SSH public-key authentication failed (methods: {methods}, partial: {partial})"
                ),
            ))
        }
        _ => Err(invalid_packet(
            "SSH server returned an unexpected authentication response",
        )),
    }
}

pub(super) fn open_git_upload_pack<'a, S: Read + Write>(
    stream: &'a mut S,
    keys: &'a mut SshTransportKeys,
    repository_path: &str,
) -> io::Result<SshExecChannel<'a, S>> {
    if repository_path.is_empty() || repository_path.contains(['\0', '\r', '\n']) {
        return Err(invalid_packet("SSH Git repository path is invalid"));
    }
    let local_channel = 0u32;
    let mut open = vec![90];
    encode_ssh_string(b"session", &mut open)?;
    open.extend_from_slice(&local_channel.to_be_bytes());
    open.extend_from_slice(&SSH_CHANNEL_WINDOW.to_be_bytes());
    open.extend_from_slice(&(SSH_CHANNEL_MAX_PACKET as u32).to_be_bytes());
    write_encrypted_ssh_packet(stream, &mut keys.outbound, &open)?;
    let confirmation = read_expected_encrypted_ssh_message(stream, &mut keys.inbound, 91)?;
    let mut reader = SshReader::new(&confirmation[1..]);
    if reader.u32()? != local_channel {
        return Err(invalid_packet(
            "SSH channel confirmation has the wrong recipient",
        ));
    }
    let remote_channel = reader.u32()?;
    let remote_window = reader.u32()?;
    let remote_max_packet = reader.u32()?;
    reader.finish()?;
    if remote_max_packet == 0 {
        return Err(invalid_packet("SSH server channel packet limit is zero"));
    }

    let command = format!("git-upload-pack {}", shell_quote(repository_path));
    let mut request = vec![98];
    request.extend_from_slice(&remote_channel.to_be_bytes());
    encode_ssh_string(b"exec", &mut request)?;
    request.push(1);
    encode_ssh_string(command.as_bytes(), &mut request)?;
    write_encrypted_ssh_packet(stream, &mut keys.outbound, &request)?;
    let response = read_expected_encrypted_ssh_message_any(stream, &mut keys.inbound)?;
    match response.first().copied() {
        Some(99) => {
            let mut reader = SshReader::new(&response[1..]);
            if reader.u32()? != local_channel {
                return Err(invalid_packet("SSH exec success has the wrong recipient"));
            }
            reader.finish()?;
        }
        Some(100) => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH server rejected the git-upload-pack command",
            ));
        }
        _ => {
            return Err(invalid_packet(
                "SSH server returned an unexpected exec response",
            ));
        }
    }
    Ok(SshExecChannel {
        stream,
        outbound: &mut keys.outbound,
        inbound: &mut keys.inbound,
        local_channel,
        remote_channel,
        remote_window,
        remote_max_packet: usize::try_from(remote_max_packet)
            .map_err(|_| invalid_packet("SSH channel packet limit overflows usize"))?,
        local_window: SSH_CHANNEL_WINDOW,
        local_max_packet: SSH_CHANNEL_MAX_PACKET,
        pending: Vec::new(),
        pending_offset: 0,
        eof: false,
        eof_sent: false,
        close_sent: false,
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) struct SshExecChannel<'a, S: Read + Write> {
    stream: &'a mut S,
    outbound: &'a mut SshAes128CtrDirection,
    inbound: &'a mut SshAes128CtrDirection,
    local_channel: u32,
    remote_channel: u32,
    remote_window: u32,
    remote_max_packet: usize,
    local_window: u32,
    local_max_packet: usize,
    pending: Vec<u8>,
    pending_offset: usize,
    eof: bool,
    eof_sent: bool,
    close_sent: bool,
}

impl<S: Read + Write> SshExecChannel<'_, S> {
    pub(super) fn finish_writing(&mut self) -> io::Result<()> {
        if !self.eof_sent {
            let mut payload = vec![96];
            payload.extend_from_slice(&self.remote_channel.to_be_bytes());
            write_encrypted_ssh_packet(self.stream, self.outbound, &payload)?;
            self.eof_sent = true;
        }
        Ok(())
    }

    fn send_channel_close(&mut self) -> io::Result<()> {
        if !self.close_sent {
            let mut payload = vec![97];
            payload.extend_from_slice(&self.remote_channel.to_be_bytes());
            write_encrypted_ssh_packet(self.stream, self.outbound, &payload)?;
            self.close_sent = true;
        }
        Ok(())
    }

    fn receive_data(&mut self) -> io::Result<()> {
        loop {
            let packet = self.inbound.read_packet(&mut *self.stream)?;
            let Some(message) = packet.payload.first().copied() else {
                return Err(invalid_packet("SSH channel packet is empty"));
            };
            let mut reader = SshReader::new(&packet.payload[1..]);
            match message {
                94 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet("SSH channel data has the wrong recipient"));
                    }
                    let data = reader.string()?;
                    reader.finish()?;
                    if data.is_empty()
                        || data.len() > self.local_window as usize
                        || data.len() > self.local_max_packet
                    {
                        return Err(invalid_packet("SSH channel data exceeds its window"));
                    }
                    self.local_window -= data.len() as u32;
                    self.pending.extend_from_slice(data);
                    self.pending_offset = 0;
                    return Ok(());
                }
                95 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet(
                            "SSH extended channel data has the wrong recipient",
                        ));
                    }
                    let _data_type = reader.u32()?;
                    let data = reader.string()?;
                    reader.finish()?;
                    if data.len() > self.local_window as usize || data.len() > self.local_max_packet
                    {
                        return Err(invalid_packet("SSH extended data exceeds its window"));
                    }
                    self.local_window -= data.len() as u32;
                    self.local_window = self
                        .local_window
                        .checked_add(data.len() as u32)
                        .ok_or_else(|| invalid_packet("SSH channel window overflows"))?;
                    let mut adjust = vec![93];
                    adjust.extend_from_slice(&self.remote_channel.to_be_bytes());
                    adjust.extend_from_slice(&(data.len() as u32).to_be_bytes());
                    write_encrypted_ssh_packet(self.stream, self.outbound, &adjust)?;
                }
                93 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet(
                            "SSH window adjustment has the wrong recipient",
                        ));
                    }
                    let adjustment = reader.u32()?;
                    reader.finish()?;
                    self.remote_window = self
                        .remote_window
                        .checked_add(adjustment)
                        .ok_or_else(|| invalid_packet("SSH remote channel window overflows"))?;
                }
                96 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet("SSH channel EOF has the wrong recipient"));
                    }
                    reader.finish()?;
                    self.eof = true;
                    return Ok(());
                }
                97 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet("SSH channel close has the wrong recipient"));
                    }
                    reader.finish()?;
                    self.eof = true;
                    self.send_channel_close()?;
                    return Ok(());
                }
                98 => continue,
                1 => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "SSH server disconnected during Git transfer",
                    ));
                }
                2 | 4 => continue,
                _ => {
                    return Err(invalid_packet(
                        "SSH server sent an unexpected channel message",
                    ));
                }
            }
        }
    }

    fn wait_for_remote_window(&mut self) -> io::Result<()> {
        while self.remote_window == 0 {
            let packet = self.inbound.read_packet(&mut *self.stream)?;
            let Some(message) = packet.payload.first().copied() else {
                return Err(invalid_packet("SSH channel packet is empty"));
            };
            let mut reader = SshReader::new(&packet.payload[1..]);
            match message {
                93 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet(
                            "SSH window adjustment has the wrong recipient",
                        ));
                    }
                    self.remote_window = self
                        .remote_window
                        .checked_add(reader.u32()?)
                        .ok_or_else(|| invalid_packet("SSH remote channel window overflows"))?;
                    reader.finish()?;
                }
                94 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet("SSH channel data has the wrong recipient"));
                    }
                    let data = reader.string()?;
                    reader.finish()?;
                    if data.is_empty()
                        || data.len() > self.local_window as usize
                        || data.len() > self.local_max_packet
                    {
                        return Err(invalid_packet("SSH channel data exceeds its window"));
                    }
                    self.local_window -= data.len() as u32;
                    self.pending.extend_from_slice(data);
                }
                95 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet(
                            "SSH extended channel data has the wrong recipient",
                        ));
                    }
                    let _data_type = reader.u32()?;
                    let data = reader.string()?;
                    reader.finish()?;
                    if data.len() > self.local_window as usize || data.len() > self.local_max_packet
                    {
                        return Err(invalid_packet("SSH extended data exceeds its window"));
                    }
                    self.local_window -= data.len() as u32;
                    self.local_window = self
                        .local_window
                        .checked_add(data.len() as u32)
                        .ok_or_else(|| invalid_packet("SSH channel window overflows"))?;
                    let mut adjust = vec![93];
                    adjust.extend_from_slice(&self.remote_channel.to_be_bytes());
                    adjust.extend_from_slice(&(data.len() as u32).to_be_bytes());
                    write_encrypted_ssh_packet(self.stream, self.outbound, &adjust)?;
                }
                96 | 97 => {
                    if reader.u32()? != self.local_channel {
                        return Err(invalid_packet(
                            "SSH channel closure has the wrong recipient",
                        ));
                    }
                    self.eof = true;
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "SSH server closed the channel while awaiting window space",
                    ));
                }
                98 => continue,
                2 | 4 => continue,
                _ => {
                    return Err(invalid_packet(
                        "SSH server sent an unexpected channel message",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl<S: Read + Write> Read for SshExecChannel<'_, S> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.pending_offset == self.pending.len() {
            self.pending.clear();
            self.pending_offset = 0;
            if self.eof {
                self.send_channel_close()?;
                return Ok(0);
            }
            self.receive_data()?;
            if self.eof && self.pending.is_empty() {
                self.send_channel_close()?;
                return Ok(0);
            }
        }
        let amount = output.len().min(self.pending.len() - self.pending_offset);
        output[..amount]
            .copy_from_slice(&self.pending[self.pending_offset..self.pending_offset + amount]);
        self.pending_offset += amount;
        let adjustment = u32::try_from(amount)
            .map_err(|_| invalid_packet("SSH channel read length overflows uint32"))?;
        self.local_window = self
            .local_window
            .checked_add(adjustment)
            .ok_or_else(|| invalid_packet("SSH local channel window overflows"))?;
        let mut payload = vec![93];
        payload.extend_from_slice(&self.remote_channel.to_be_bytes());
        payload.extend_from_slice(&adjustment.to_be_bytes());
        write_encrypted_ssh_packet(self.stream, self.outbound, &payload)?;
        Ok(amount)
    }
}

impl<S: Read + Write> Write for SshExecChannel<'_, S> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.is_empty() {
            return Ok(0);
        }
        if self.eof_sent {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "SSH channel write side is closed",
            ));
        }
        if self.remote_window == 0 {
            self.wait_for_remote_window()?;
        }
        let amount = input
            .len()
            .min(self.remote_max_packet)
            .min(self.remote_window as usize);
        let mut payload = vec![94];
        payload.extend_from_slice(&self.remote_channel.to_be_bytes());
        encode_ssh_string(&input[..amount], &mut payload)?;
        write_encrypted_ssh_packet(self.stream, self.outbound, &payload)?;
        self.remote_window -= amount as u32;
        Ok(amount)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl<S: Read + Write> Drop for SshExecChannel<'_, S> {
    fn drop(&mut self) {
        let _ = self.send_channel_close();
    }
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

fn read_expected_encrypted_ssh_message(
    stream: &mut impl Read,
    direction: &mut SshAes128CtrDirection,
    expected: u8,
) -> io::Result<Vec<u8>> {
    loop {
        let payload = read_expected_encrypted_ssh_message_any(stream, direction)?;
        if payload.first() == Some(&expected) {
            return Ok(payload);
        }
        if payload.first() == Some(&1) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSH server disconnected during authentication",
            ));
        }
        if !matches!(payload.first(), Some(2 | 4)) {
            return Err(invalid_packet(
                "SSH server sent an unexpected service message",
            ));
        }
    }
}

fn read_expected_encrypted_ssh_message_any(
    stream: &mut impl Read,
    direction: &mut SshAes128CtrDirection,
) -> io::Result<Vec<u8>> {
    Ok(direction.read_packet(stream)?.payload)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KnownHostEntry {
    patterns: Vec<String>,
    host_key: Vec<u8>,
    revoked: bool,
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
            let (revoked, hosts) = if first.starts_with('@') {
                match first {
                    "@revoked" => (
                        true,
                        fields.next().ok_or_else(|| {
                            invalid_packet("SSH revoked known_hosts line has no host")
                        })?,
                    ),
                    "@cert-authority" => continue,
                    _ => return Err(invalid_packet("SSH known_hosts marker is unsupported")),
                }
            } else {
                (false, first)
            };
            let algorithm = fields
                .next()
                .ok_or_else(|| invalid_packet("SSH known_hosts line has no key type"))?;
            let encoded_key = fields
                .next()
                .ok_or_else(|| invalid_packet("SSH known_hosts line has no key"))?;
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
                revoked,
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
        let mut trusted_key = false;
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
                if entry.revoked {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "SSH host key is revoked",
                    ));
                }
                trusted_key = true;
            }
        }
        if trusted_key {
            Ok(())
        } else if matched {
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
    let mut pattern_index = 0;
    let mut candidate_index = 0;
    let mut last_star = None;
    let mut star_match = 0;
    while candidate_index < candidate.len() {
        if pattern.get(pattern_index).is_some_and(|byte| {
            *byte == b'?' || byte.eq_ignore_ascii_case(&candidate[candidate_index])
        }) {
            pattern_index += 1;
            candidate_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            last_star = Some(pattern_index);
            pattern_index += 1;
            star_match = candidate_index;
        } else if let Some(star_index) = last_star {
            star_match += 1;
            candidate_index = star_match;
            pattern_index = star_index + 1;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&b'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn decode_base64(input: &str) -> io::Result<Vec<u8>> {
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

pub(super) struct SshEd25519Identity {
    pub(super) public_key: Vec<u8>,
    pub(super) private_key: signature::Ed25519KeyPair,
}

impl std::fmt::Debug for SshEd25519Identity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshEd25519Identity")
            .field("public_key", &self.public_key)
            .field("private_key", &"<hidden>")
            .finish()
    }
}

impl SshEd25519Identity {
    pub(super) fn parse_openssh(input: &[u8]) -> io::Result<Self> {
        let text = std::str::from_utf8(input)
            .map_err(|_| invalid_packet("SSH private key is not UTF-8"))?;
        let mut encoded = Zeroizing::new(String::new());
        let mut inside = false;
        for line in text.lines() {
            let line = line.trim();
            if line == "-----BEGIN OPENSSH PRIVATE KEY-----" {
                if inside {
                    return Err(invalid_packet("SSH private key has duplicate headers"));
                }
                inside = true;
                continue;
            }
            if line == "-----END OPENSSH PRIVATE KEY-----" {
                inside = false;
                break;
            }
            if inside {
                if !line
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
                {
                    return Err(invalid_packet("SSH private key base64 is malformed"));
                }
                encoded.push_str(line);
            }
        }
        if inside || encoded.is_empty() {
            return Err(invalid_packet("SSH private key is incomplete"));
        }
        let decoded = Zeroizing::new(decode_base64(&encoded)?);
        let magic = b"openssh-key-v1\0";
        if !decoded.starts_with(magic) {
            return Err(invalid_packet("SSH private key format is unsupported"));
        }
        let mut outer = SshReader::new(&decoded[magic.len()..]);
        let cipher = outer.string()?;
        let kdf = outer.string()?;
        let kdf_options = outer.string()?;
        let key_count = outer.u32()?;
        if cipher != b"none" || kdf != b"none" || !kdf_options.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "encrypted OpenSSH private keys are not supported; load the key into ssh-agent",
            ));
        }
        if key_count != 1 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "OpenSSH private keys must contain exactly one identity",
            ));
        }
        let public_blob = outer.string()?.to_vec();
        let private_block = outer.string()?;
        outer.finish()?;

        let mut public_reader = SshReader::new(&public_blob);
        let public_algorithm = public_reader.string()?;
        let public_key = public_reader.string()?.to_vec();
        public_reader.finish()?;
        if public_algorithm != b"ssh-ed25519" || public_key.len() != 32 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "OpenSSH identity key type is unsupported; only Ed25519 is accepted",
            ));
        }

        let mut private_reader = SshReader::new(private_block);
        let check_a = private_reader.u32()?;
        let check_b = private_reader.u32()?;
        if check_a != check_b {
            return Err(invalid_packet(
                "SSH private key check integers do not match",
            ));
        }
        if private_reader.string()? != b"ssh-ed25519" {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "OpenSSH identity key type is unsupported; only Ed25519 is accepted",
            ));
        }
        let private_public = private_reader.string()?;
        let private_key = private_reader.string()?;
        let _comment = private_reader.string()?;
        let padding = private_reader.take(private_reader.bytes.len() - private_reader.offset)?;
        if padding.is_empty()
            || padding
                .iter()
                .enumerate()
                .any(|(index, byte)| *byte != (index as u8 + 1))
            || private_key.len() != 64
            || private_key[32..] != public_key
            || private_public != public_key
        {
            return Err(invalid_packet(
                "SSH Ed25519 private key contents are invalid",
            ));
        }
        let key_pair = signature::Ed25519KeyPair::from_seed_unchecked(&private_key[..32])
            .map_err(|_| invalid_packet("SSH Ed25519 private key seed is invalid"))?;
        if key_pair.public_key().as_ref() != public_key {
            return Err(invalid_packet(
                "SSH Ed25519 public and private keys do not match",
            ));
        }
        Ok(Self {
            public_key: public_blob,
            private_key: key_pair,
        })
    }

    pub(super) fn public_key_blob(&self) -> &[u8] {
        &self.public_key
    }

    pub(super) fn sign(&self, message: &[u8]) -> signature::Signature {
        self.private_key.sign(message)
    }
}

pub(super) trait SshSigner {
    fn public_key_blob(&self) -> &[u8];
    fn signature_algorithm(&self) -> &[u8];
    fn sign_message(&mut self, message: &[u8]) -> io::Result<Vec<u8>>;
}

impl SshSigner for SshEd25519Identity {
    fn public_key_blob(&self) -> &[u8] {
        self.public_key_blob()
    }

    fn signature_algorithm(&self) -> &[u8] {
        b"ssh-ed25519"
    }

    fn sign_message(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
        Ok(self.sign(message).as_ref().to_vec())
    }
}

trait SshAgentStream: Read + Write {}
impl<T: Read + Write> SshAgentStream for T {}

pub(super) struct SshAgent {
    stream: Box<dyn SshAgentStream>,
    public_key: Vec<u8>,
    signature_algorithm: Vec<u8>,
    signature_flags: u32,
}

impl SshAgent {
    pub(super) fn connect() -> io::Result<Option<Self>> {
        let Some(mut stream) = connect_agent_stream() else {
            return Ok(None);
        };
        let response = agent_request(&mut *stream, &[11])?;
        let public_key = parse_agent_identities(&response)?;
        let Some(public_key) = public_key else {
            return Ok(None);
        };
        let (_, signature_algorithm, signature_flags) = agent_signing_algorithm(&public_key)?;
        Ok(Some(Self {
            stream,
            public_key,
            signature_algorithm,
            signature_flags,
        }))
    }

    fn sign(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
        let mut request = vec![13];
        encode_ssh_string(&self.public_key, &mut request)?;
        encode_ssh_string(message, &mut request)?;
        request.extend_from_slice(&self.signature_flags.to_be_bytes());
        let response = agent_request(&mut *self.stream, &request)?;
        if response.first() != Some(&14) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "SSH agent refused the public-key signing request",
            ));
        }
        let mut reader = SshReader::new(&response[1..]);
        let signature_blob = reader.string()?;
        reader.finish()?;
        let mut signature_reader = SshReader::new(signature_blob);
        if signature_reader.string()? != self.signature_algorithm {
            return Err(invalid_packet(
                "SSH agent returned a signature with the wrong algorithm",
            ));
        }
        let signature = signature_reader.string()?;
        signature_reader.finish()?;
        if signature.is_empty() {
            return Err(invalid_packet(
                "SSH agent returned an empty public-key signature",
            ));
        }
        Ok(signature.to_vec())
    }
}

impl SshSigner for SshAgent {
    fn public_key_blob(&self) -> &[u8] {
        &self.public_key
    }

    fn signature_algorithm(&self) -> &[u8] {
        &self.signature_algorithm
    }

    fn sign_message(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
        self.sign(message)
    }
}

pub(super) fn agent_signing_algorithm(blob: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>, u32)> {
    let mut reader = SshReader::new(blob);
    let algorithm = reader.string()?;
    let result = match algorithm {
        b"ssh-ed25519" if reader.string()?.len() == 32 => {
            (algorithm.to_vec(), algorithm.to_vec(), 0)
        }
        b"ecdsa-sha2-nistp256" => {
            let curve = reader.string()?;
            let point = reader.string()?;
            if curve != b"nistp256" || point.len() != 65 || point[0] != 4 {
                return Err(invalid_packet("SSH agent returned an invalid ECDSA key"));
            }
            (algorithm.to_vec(), algorithm.to_vec(), 0)
        }
        b"ssh-rsa" => {
            let exponent = reader.string()?;
            let modulus = reader.string()?;
            if exponent.is_empty() || modulus.len() < 128 {
                return Err(invalid_packet("SSH agent returned an invalid RSA key"));
            }
            (algorithm.to_vec(), b"rsa-sha2-512".to_vec(), 4)
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SSH agent identity key type is unsupported",
            ));
        }
    };
    reader.finish()?;
    Ok(result)
}

pub(super) fn parse_agent_identities(response: &[u8]) -> io::Result<Option<Vec<u8>>> {
    if response.first() == Some(&5) {
        return Ok(None);
    }
    let Some(message) = response.first().copied() else {
        return Err(invalid_packet(
            "SSH agent returned an empty identities reply",
        ));
    };
    if message != 12 {
        return Err(invalid_packet(
            "SSH agent returned an unexpected identities reply",
        ));
    }
    let mut reader = SshReader::new(&response[1..]);
    let count = reader.u32()?;
    if count > 1024 {
        return Err(invalid_packet("SSH agent returned too many identities"));
    }
    let mut public_keys = Vec::new();
    for _ in 0..count {
        let key = reader.string()?;
        let _comment = reader.string()?;
        if agent_signing_algorithm(key).is_ok() {
            public_keys.push(key.to_vec());
        }
    }
    reader.finish()?;
    let selected = public_keys
        .iter()
        .find(|key| {
            agent_signing_algorithm(key).is_ok_and(|(algorithm, _, _)| algorithm == b"ssh-ed25519")
        })
        .or_else(|| {
            public_keys.iter().find(|key| {
                agent_signing_algorithm(key)
                    .is_ok_and(|(algorithm, _, _)| algorithm == b"ecdsa-sha2-nistp256")
            })
        })
        .or_else(|| {
            public_keys.iter().find(|key| {
                agent_signing_algorithm(key).is_ok_and(|(algorithm, _, _)| algorithm == b"ssh-rsa")
            })
        });
    Ok(selected.cloned())
}

pub(super) fn agent_request<S: Read + Write + ?Sized>(
    stream: &mut S,
    payload: &[u8],
) -> io::Result<Vec<u8>> {
    if payload.is_empty() || payload.len() > MAX_SSH_PACKET_BYTES {
        return Err(invalid_packet("SSH agent request exceeds protocol limit"));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| invalid_packet("SSH agent request exceeds uint32 length"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = usize::try_from(u32::from_be_bytes(length))
        .map_err(|_| invalid_packet("SSH agent response length overflows usize"))?;
    if length == 0 || length > MAX_SSH_PACKET_BYTES {
        return Err(invalid_packet("SSH agent response exceeds protocol limit"));
    }
    let mut response = vec![0; length];
    stream.read_exact(&mut response)?;
    Ok(response)
}

#[cfg(unix)]
fn connect_agent_stream() -> Option<Box<dyn SshAgentStream>> {
    use std::os::unix::net::UnixStream;

    let socket = std::env::var_os("SSH_AUTH_SOCK")?;
    UnixStream::connect(socket)
        .ok()
        .map(|stream| Box::new(stream) as Box<dyn SshAgentStream>)
}

#[cfg(windows)]
fn connect_agent_stream() -> Option<Box<dyn SshAgentStream>> {
    let socket =
        std::env::var_os("SSH_AUTH_SOCK").unwrap_or_else(|| r"\\.\pipe\openssh-ssh-agent".into());
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(socket)
        .ok()
        .map(|stream| Box::new(stream) as Box<dyn SshAgentStream>)
}

#[cfg(not(any(unix, windows)))]
fn connect_agent_stream() -> Option<Box<dyn SshAgentStream>> {
    None
}

pub(super) fn encode_ssh_string(value: &[u8], output: &mut Vec<u8>) -> io::Result<()> {
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

#[cfg(test)]
mod agent_tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    struct TestStream {
        incoming: Cursor<Vec<u8>>,
        outgoing: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for TestStream {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.incoming.read(output)
        }
    }

    impl Write for TestStream {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            self.outgoing.lock().unwrap().extend_from_slice(input);
            Ok(input.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn agent_sign_request_uses_selected_key_and_parses_signature() {
        let mut public_key = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut public_key).unwrap();
        encode_ssh_string(&[8; 32], &mut public_key).unwrap();
        let mut signature_blob = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut signature_blob).unwrap();
        encode_ssh_string(&[9; 64], &mut signature_blob).unwrap();
        let mut response = vec![14];
        encode_ssh_string(&signature_blob, &mut response).unwrap();
        let mut incoming = Vec::new();
        incoming.extend_from_slice(&(response.len() as u32).to_be_bytes());
        incoming.extend_from_slice(&response);
        let outgoing = Arc::new(Mutex::new(Vec::new()));
        let stream = TestStream {
            incoming: Cursor::new(incoming),
            outgoing: Arc::clone(&outgoing),
        };
        let (_, signature_algorithm, signature_flags) =
            agent_signing_algorithm(&public_key).unwrap();
        let mut agent = SshAgent {
            stream: Box::new(stream),
            public_key: public_key.clone(),
            signature_algorithm,
            signature_flags,
        };
        assert_eq!(agent.sign(b"session-bound request").unwrap(), [9; 64]);
        let outgoing = outgoing.lock().unwrap();
        assert_eq!(
            u32::from_be_bytes(outgoing[..4].try_into().unwrap()) as usize,
            outgoing.len() - 4
        );
        let mut request = SshReader::new(&outgoing[4..]);
        assert_eq!(request.take(1).unwrap(), [13]);
        assert_eq!(request.string().unwrap(), public_key);
        assert_eq!(request.string().unwrap(), b"session-bound request");
        assert_eq!(request.u32().unwrap(), 0);
        request.finish().unwrap();
    }
}
