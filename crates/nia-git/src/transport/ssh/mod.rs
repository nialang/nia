// SPDX-License-Identifier: GPL-3.0-or-later
//! SSH transport, authentication, and channel state.

mod agent;
mod cipher;
mod identity;
mod kex;
mod known_hosts;
mod wire;

use agent::SshAgent;
use cipher::{SshAes128CtrDirection, write_encrypted_ssh_packet};
use identity::{SshSigner, load_ssh_identity};
use kex::perform_ssh_key_exchange;
use known_hosts::{KnownHosts, MAX_KNOWN_HOSTS_BYTES};
use wire::{SshReader, encode_ssh_string};

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};

use super::protocol::{
    build_upload_pack_request, parse_advertised_refs, parse_upload_pack_response,
    select_advertised_revision,
};
use super::{MAX_RESPONSE_BYTES, local::write_pack_checkout};
use crate::pktline::read_packet_section;
use crate::{
    input::{read_limited, read_limited_file},
    invalid_packet,
};

const SSH_CHANNEL_WINDOW: u32 = 2_097_152;
const SSH_CHANNEL_MAX_PACKET: usize = 32_768;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SshUrl {
    user: String,
    host: String,
    port: u16,
    path: String,
}

fn parse_ssh_url(source: &str) -> io::Result<SshUrl> {
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

#[derive(Debug)]
struct SshTransportKeys {
    outbound: SshAes128CtrDirection,
    inbound: SshAes128CtrDirection,
    session_id: [u8; 32],
}

fn build_publickey_auth_request(
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

fn authenticate_publickey(
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

fn open_git_upload_pack<'a, S: Read + Write>(
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

struct SshExecChannel<'a, S: Read + Write> {
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
    fn finish_writing(&mut self) -> io::Result<()> {
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

pub(crate) fn checkout_ssh(source: &str, revision: &str, destination: &Path) -> io::Result<String> {
    let ssh_directory = ssh_directory()?;
    checkout_ssh_with_config(source, revision, destination, &ssh_directory, true)
}

fn checkout_ssh_with_config(
    source: &str,
    revision: &str,
    destination: &Path,
    ssh_directory: &Path,
    use_agent: bool,
) -> io::Result<String> {
    let url = parse_ssh_url(source)?;
    let known_hosts_path = ssh_directory.join("known_hosts");
    let known_hosts = KnownHosts::parse(
        &read_limited_file(
            &known_hosts_path,
            MAX_KNOWN_HOSTS_BYTES,
            "SSH known_hosts exceeds size limit",
        )
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not read SSH known_hosts at {}: {error}",
                    known_hosts_path.display()
                ),
            )
        })?,
    )?;
    let mut signer: Box<dyn SshSigner> = if use_agent {
        if let Some(agent) = SshAgent::connect()? {
            Box::new(agent)
        } else {
            Box::new(load_ssh_identity(ssh_directory)?)
        }
    } else {
        Box::new(load_ssh_identity(ssh_directory)?)
    };

    let address = (url.host.as_str(), url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SSH host has no address"))?;
    let mut stream = TcpStream::connect_timeout(&address, std::time::Duration::from_secs(15))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    let mut keys = perform_ssh_key_exchange(&mut stream, &url, &known_hosts)?;
    authenticate_publickey(&mut stream, &mut keys, &url.user, signer.as_mut())?;
    let mut channel = open_git_upload_pack(&mut stream, &mut keys, &url.path)?;

    let advertisement = read_packet_section(&mut channel)?;
    let refs = parse_advertised_refs(&advertisement)?;
    let commit = select_advertised_revision(&refs, revision)?;
    let request = build_upload_pack_request(&commit)?;
    channel.write_all(&request)?;
    channel.flush()?;
    channel.finish_writing()?;
    let response = read_limited(
        channel,
        MAX_RESPONSE_BYTES,
        "Git response exceeds size limit",
    )?;
    let pack = parse_upload_pack_response(&response)?;
    write_pack_checkout(&pack, &commit, destination)
}

fn ssh_directory() -> io::Result<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }
    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory is not configured"))?;
    Ok(PathBuf::from(home).join(".ssh"))
}

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixture;
