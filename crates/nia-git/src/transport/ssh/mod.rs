// SPDX-License-Identifier: GPL-3.0-or-later
//! SSH transport, authentication, and channel state.

mod agent;
mod channel;
mod cipher;
mod identity;
mod kex;
mod known_hosts;
mod wire;

use agent::agent_identities;
use channel::SshExecChannel;
use cipher::{SshAes128CtrDirection, write_encrypted_ssh_packet};
use identity::{SshSigner, load_ssh_identity};
use kex::perform_ssh_key_exchange;
use known_hosts::{KnownHosts, MAX_KNOWN_HOSTS_BYTES};
use wire::{SshReader, encode_ssh_string};

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};

use super::http::base64_encode;
use super::protocol::{
    build_upload_pack_request, parse_advertised_refs, parse_upload_pack_response,
    select_advertised_revision,
};
use super::{MAX_RESPONSE_BYTES, local::write_pack_checkout_into};
use crate::pktline::read_packet_section;
use crate::{
    input::{read_limited, read_limited_file},
    invalid_packet,
};
use cap_std::fs::Dir;

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_agent_identities(input: &[u8]) {
    let _ = agent::parse_agent_identities(input);
}

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_known_hosts(input: &[u8]) {
    let _ = KnownHosts::parse(input);
}

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_kexinit(input: &[u8]) {
    let _ = kex::parse_ssh_kexinit(input);
}

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_openssh_identity(input: &[u8]) {
    let _ = identity::SshEd25519Identity::parse_openssh(input);
}

const SSH_CHANNEL_WINDOW: u32 = 2_097_152;
const SSH_CHANNEL_MAX_PACKET: usize = 32_768;
const MAX_SSH_CONTROL_MESSAGES: usize = 1024;
const MAX_SSH_CONTROL_BYTES: usize = 1024 * 1024;
/// OpenSSH servers disconnect after `MaxAuthTries` failures, six by default;
/// stay one below so the last offer still gets an answer.
const MAX_SSH_IDENTITY_ATTEMPTS: usize = 5;

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

fn validate_ssh_username(username: &str) -> io::Result<()> {
    if username.is_empty() || username.len() > 256 || username.contains(['\0', '\r', '\n']) {
        return Err(invalid_packet("SSH username is invalid"));
    }
    Ok(())
}

/// Asks whether the server would accept a key, without a signature. Only a
/// `PK_OK` reply leads to signing, so rejected keys never reach the agent.
fn build_publickey_query(username: &str, signer: &dyn SshSigner) -> io::Result<Vec<u8>> {
    validate_ssh_username(username)?;
    let mut request = vec![50];
    encode_ssh_string(username.as_bytes(), &mut request)?;
    encode_ssh_string(b"ssh-connection", &mut request)?;
    encode_ssh_string(b"publickey", &mut request)?;
    request.push(0);
    encode_ssh_string(signer.signature_algorithm(), &mut request)?;
    encode_ssh_string(signer.public_key_blob(), &mut request)?;
    Ok(request)
}

fn build_publickey_auth_request(
    username: &str,
    session_id: &[u8; 32],
    signer: &mut dyn SshSigner,
) -> io::Result<Vec<u8>> {
    validate_ssh_username(username)?;
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

/// Offers each identity in order until the server accepts one, as OpenSSH
/// does. The caller bounds the list below the server's attempt limit;
/// `notes` describe identities it left out and are added to any failure.
fn authenticate_publickey(
    stream: &mut (impl Read + Write),
    keys: &mut SshTransportKeys,
    username: &str,
    identities: &mut [Box<dyn SshSigner>],
    notes: &[String],
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

    // Each line records one offered identity, so whatever ends the attempt
    // (rejection of every key, a disconnect, or a protocol error) still
    // tells the user which keys were tried and how each one fared.
    let mut tried = Vec::new();
    let mut ending = None;
    for signer in identities.iter_mut() {
        let fingerprint = ssh_key_fingerprint(signer.public_key_blob());
        match offer_publickey(stream, keys, username, signer.as_mut()) {
            Ok(SshOfferOutcome::Accepted) => return Ok(()),
            Ok(SshOfferOutcome::Rejected(failure)) => {
                tried.push(format!("{fingerprint} (rejected)"));
                if !failure
                    .methods
                    .split(',')
                    .any(|method| method == "publickey")
                {
                    ending = Some(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "SSH server no longer accepts public-key authentication (methods: {})",
                            failure.methods
                        ),
                    ));
                    break;
                }
            }
            Ok(SshOfferOutcome::PartialSuccess(failure)) => {
                tried.push(format!("{fingerprint} (accepted)"));
                ending = Some(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "SSH server accepted the key but requires further authentication methods \
                         that are not supported (methods: {})",
                        failure.methods
                    ),
                ));
                break;
            }
            // The key was acceptable but could not sign: a declined
            // confirmation, a locked agent, or a missing hardware key.
            // OpenSSH moves on to the next identity, and so does this.
            Ok(SshOfferOutcome::SigningFailed(error)) => {
                tried.push(format!("{fingerprint} (signing failed: {error})"));
            }
            Err(error) => {
                tried.push(format!("{fingerprint} (interrupted)"));
                ending = Some(error);
                break;
            }
        }
    }
    let error = ending.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SSH server rejected every offered identity",
        )
    });
    let mut message = format!("{error}; offered identities: {}", tried.join(", "));
    for note in notes {
        message.push_str("; ");
        message.push_str(note);
    }
    Err(io::Error::new(error.kind(), message))
}

struct SshAuthFailure {
    methods: String,
}

enum SshOfferOutcome {
    Accepted,
    Rejected(SshAuthFailure),
    /// The signature succeeded, but the server wants another method too.
    PartialSuccess(SshAuthFailure),
    SigningFailed(io::Error),
}

fn offer_publickey(
    stream: &mut (impl Read + Write),
    keys: &mut SshTransportKeys,
    username: &str,
    signer: &mut dyn SshSigner,
) -> io::Result<SshOfferOutcome> {
    let query = build_publickey_query(username, signer)?;
    write_encrypted_ssh_packet(stream, &mut keys.outbound, &query)?;
    let response = read_userauth_response(stream, &mut keys.inbound)?;
    if response.first() != Some(&60) {
        // A partial success can only follow a signature, so a FAILURE to
        // the unsigned query is a rejection whatever its flag says.
        let (failure, _) = read_authentication_failure(&response)?;
        return Ok(SshOfferOutcome::Rejected(failure));
    }
    let mut reader = SshReader::new(&response[1..]);
    let algorithm = reader.string()?;
    let public_key = reader.string()?;
    reader.finish()?;
    if algorithm != signer.signature_algorithm() || public_key != signer.public_key_blob() {
        return Err(invalid_packet(
            "SSH server accepted a different public key than offered",
        ));
    }

    let request = match build_publickey_auth_request(username, &keys.session_id, signer) {
        Ok(request) => request,
        Err(error) => return Ok(SshOfferOutcome::SigningFailed(error)),
    };
    write_encrypted_ssh_packet(stream, &mut keys.outbound, &request)?;
    let response = read_userauth_response(stream, &mut keys.inbound)?;
    if response.as_slice() == [52] {
        return Ok(SshOfferOutcome::Accepted);
    }
    Ok(match read_authentication_failure(&response)? {
        (failure, true) => SshOfferOutcome::PartialSuccess(failure),
        (failure, false) => SshOfferOutcome::Rejected(failure),
    })
}

/// Reads an authentication reply, discarding `USERAUTH_BANNER` messages.
/// Servers may send a banner before any reply until authentication
/// succeeds; it is display text for interactive users, so it is validated
/// and dropped. Banners, IGNORE, and DEBUG share one budget per reply, so
/// interleaving them cannot reset the bound.
fn read_userauth_response(
    stream: &mut impl Read,
    direction: &mut SshAes128CtrDirection,
) -> io::Result<Vec<u8>> {
    let mut skipped_messages = 0;
    let mut skipped_bytes: usize = 0;
    loop {
        let payload = direction.read_packet(&mut *stream)?.payload;
        let skip = if payload.first() == Some(&53) {
            let mut reader = SshReader::new(&payload[1..]);
            reader.string()?;
            reader.string()?;
            reader.finish()?;
            true
        } else {
            is_ignorable_ssh_message(&payload)?
        };
        if !skip {
            return Ok(payload);
        }
        skipped_messages += 1;
        skipped_bytes = skipped_bytes.saturating_add(payload.len());
        if skipped_messages > MAX_SSH_CONTROL_MESSAGES || skipped_bytes > MAX_SSH_CONTROL_BYTES {
            return Err(invalid_packet(
                "SSH control messages exceed resource limits",
            ));
        }
    }
}

/// Parses `USERAUTH_FAILURE` into its method list and partial-success flag.
fn read_authentication_failure(payload: &[u8]) -> io::Result<(SshAuthFailure, bool)> {
    match payload.first().copied() {
        Some(51) => {
            let mut reader = SshReader::new(&payload[1..]);
            let methods = String::from_utf8_lossy(reader.string()?).into_owned();
            let partial = reader.boolean()?;
            reader.finish()?;
            Ok((SshAuthFailure { methods }, partial))
        }
        Some(1) => Err(read_disconnect(payload)),
        _ => Err(invalid_packet(
            "SSH server returned an unexpected authentication response",
        )),
    }
}

/// Turns `DISCONNECT` into an error that keeps the server's reason, such as
/// "Too many authentication failures".
fn read_disconnect(payload: &[u8]) -> io::Error {
    let mut reader = SshReader::new(&payload[1..]);
    let description = reader
        .u32()
        .and_then(|_| reader.string())
        .map(|text| String::from_utf8_lossy(text).into_owned());
    match description {
        Ok(description) if !description.is_empty() => io::Error::new(
            io::ErrorKind::ConnectionAborted,
            format!(
                "SSH server disconnected during authentication: {}",
                description.escape_debug()
            ),
        ),
        _ => io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "SSH server disconnected during authentication",
        ),
    }
}

/// Formats a key as `ssh-keygen -l` does, so users can match it to `ssh-add -l`.
fn ssh_key_fingerprint(public_key: &[u8]) -> String {
    let algorithm = SshReader::new(public_key)
        .string()
        .map_or_else(|_| "unknown".into(), String::from_utf8_lossy);
    let digest = ring::digest::digest(&ring::digest::SHA256, public_key);
    let encoded = base64_encode(digest.as_ref());
    let encoded = std::str::from_utf8(&encoded).expect("base64 output is ASCII");
    format!("{algorithm} SHA256:{}", encoded.trim_end_matches('='))
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
    SshExecChannel::new(
        stream,
        keys,
        remote_channel,
        remote_window,
        remote_max_packet,
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn read_expected_encrypted_ssh_message(
    stream: &mut impl Read,
    direction: &mut SshAes128CtrDirection,
    expected: u8,
) -> io::Result<Vec<u8>> {
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
    Err(invalid_packet(
        "SSH server sent an unexpected service message",
    ))
}

fn read_expected_encrypted_ssh_message_any(
    stream: &mut impl Read,
    direction: &mut SshAes128CtrDirection,
) -> io::Result<Vec<u8>> {
    let mut ignored_messages = 0;
    let mut ignored_bytes: usize = 0;
    loop {
        let payload = direction.read_packet(&mut *stream)?.payload;
        if !is_ignorable_ssh_message(&payload)? {
            return Ok(payload);
        }
        ignored_messages += 1;
        ignored_bytes = ignored_bytes.saturating_add(payload.len());
        if ignored_messages > MAX_SSH_CONTROL_MESSAGES || ignored_bytes > MAX_SSH_CONTROL_BYTES {
            return Err(invalid_packet(
                "SSH control messages exceed resource limits",
            ));
        }
    }
}

fn is_ignorable_ssh_message(payload: &[u8]) -> io::Result<bool> {
    match payload.first() {
        Some(2) => {
            let mut reader = SshReader::new(&payload[1..]);
            reader.string()?;
            reader.finish()?;
            Ok(true)
        }
        Some(4) => {
            let mut reader = SshReader::new(&payload[1..]);
            reader.boolean()?;
            reader.string()?;
            reader.string()?;
            reader.finish()?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn checkout_ssh(
    source: &str,
    revision: &str,
    destination_dir: &Dir,
) -> io::Result<String> {
    let ssh_directory = ssh_directory()?;
    checkout_ssh_with_config_into(source, revision, destination_dir, &ssh_directory, || {
        Ok(agent_identities()?
            .into_iter()
            .map(|identity| Box::new(identity) as Box<dyn SshSigner>)
            .collect())
    })
}

#[cfg(test)]
fn checkout_ssh_with_config(
    source: &str,
    revision: &str,
    destination: &Dir,
    ssh_directory: &Path,
    agent: io::Result<Vec<Box<dyn SshSigner>>>,
) -> io::Result<String> {
    checkout_ssh_with_config_into(source, revision, destination, ssh_directory, || agent)
}

/// Identities to offer, with notes about identities left out for the error
/// when every offered identity is rejected.
struct SshIdentityCandidates {
    identities: Vec<Box<dyn SshSigner>>,
    notes: Vec<String>,
}

/// Builds the identities to offer: agent keys in the agent's order, then
/// `id_ed25519` unless the agent already holds it, at most
/// [`MAX_SSH_IDENTITY_ATTEMPTS`] in all. An agent that cannot be listed is
/// skipped, as OpenSSH does, so the identity file is still tried.
fn ssh_identity_candidates(
    agent: io::Result<Vec<Box<dyn SshSigner>>>,
    ssh_directory: &Path,
) -> io::Result<SshIdentityCandidates> {
    let mut notes = Vec::new();
    let mut identities = agent.unwrap_or_else(|error| {
        notes.push(format!("the SSH agent was not used: {error}"));
        Vec::new()
    });
    match load_ssh_identity(ssh_directory) {
        Ok(file) => {
            if !identities
                .iter()
                .any(|identity| identity.public_key_blob() == file.public_key_blob())
            {
                identities.push(Box::new(file));
            }
        }
        // Without an agent key, the identity file is the only credential,
        // so its error is the failure.
        Err(error) if identities.is_empty() => {
            let mut message = error.to_string();
            for note in &notes {
                message.push_str("; ");
                message.push_str(note);
            }
            return Err(io::Error::new(error.kind(), message));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => notes.push(format!("the identity file was not offered: {error}")),
    }
    if identities.len() > MAX_SSH_IDENTITY_ATTEMPTS {
        notes.push(format!(
            "{} more identities were not offered; at most {MAX_SSH_IDENTITY_ATTEMPTS} are tried",
            identities.len() - MAX_SSH_IDENTITY_ATTEMPTS
        ));
        identities.truncate(MAX_SSH_IDENTITY_ATTEMPTS);
    }
    Ok(SshIdentityCandidates { identities, notes })
}

fn checkout_ssh_with_config_into(
    source: &str,
    revision: &str,
    destination_dir: &Dir,
    ssh_directory: &Path,
    agent: impl FnOnce() -> io::Result<Vec<Box<dyn SshSigner>>>,
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
    let SshIdentityCandidates {
        mut identities,
        notes,
    } = ssh_identity_candidates(agent(), ssh_directory)?;

    let address = (url.host.as_str(), url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SSH host has no address"))?;
    let mut stream = TcpStream::connect_timeout(&address, std::time::Duration::from_secs(15))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    let mut keys = perform_ssh_key_exchange(&mut stream, &url, &known_hosts)?;
    authenticate_publickey(&mut stream, &mut keys, &url.user, &mut identities, &notes)?;
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
    write_pack_checkout_into(&pack, &commit, destination_dir)
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
