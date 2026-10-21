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
use zeroize::Zeroizing;

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
        return checkout_ssh(source, revision, destination);
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("Git transport is not implemented for `{source}`"),
    ))
}

fn checkout_ssh(source: &str, revision: &str, destination: &Path) -> io::Result<String> {
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
    let url = ssh::parse_ssh_url(source)?;
    let known_hosts_path = ssh_directory.join("known_hosts");
    let known_hosts = ssh::KnownHosts::parse(
        &read_limited_file(&known_hosts_path, 16 * 1024 * 1024).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not read SSH known_hosts at {}: {error}",
                    known_hosts_path.display()
                ),
            )
        })?,
    )?;
    let mut signer: Box<dyn ssh::SshSigner> = if use_agent {
        if let Some(agent) = ssh::SshAgent::connect()? {
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
    let mut keys = ssh::perform_ssh_key_exchange(&mut stream, &url, &known_hosts)?;
    ssh::authenticate_publickey(&mut stream, &mut keys, &url.user, signer.as_mut())?;
    let mut channel = ssh::open_git_upload_pack(&mut stream, &mut keys, &url.path)?;

    let advertisement = read_packet_section(&mut channel)?;
    let refs = parse_advertised_refs(&advertisement)?;
    let commit = select_advertised_revision(&refs, revision)?;
    let mut request = Vec::new();
    encode_packet_line(
        format!("want {} ofs-delta side-band-64k\n", encode_hex(&commit)).as_bytes(),
        &mut request,
    )?;
    request.extend_from_slice(b"0000");
    encode_packet_line(b"done\n", &mut request)?;
    channel.write_all(&request)?;
    channel.flush()?;
    channel.finish_writing()?;
    let mut response = Vec::new();
    channel
        .take((MAX_HTTP_RESPONSE + 1) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_HTTP_RESPONSE {
        return Err(invalid_packet("Git response is too large"));
    }
    let pack = parse_upload_pack_response(&response)?;
    write_pack_checkout(&pack, &commit, destination)
}

fn load_ssh_identity(ssh_directory: &Path) -> io::Result<ssh::SshEd25519Identity> {
    let identity_path = ssh_directory.join("id_ed25519");
    verify_ssh_identity_permissions(&identity_path)?;
    let identity_bytes = Zeroizing::new(read_limited_file(&identity_path, 1024 * 1024).map_err(
        |error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not read SSH identity at {}: {error}",
                    identity_path.display()
                ),
            )
        },
    )?);
    ssh::SshEd25519Identity::parse_openssh(&identity_bytes)
}

fn read_limited_file(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SSH configuration file at {} is too large", path.display()),
        ));
    }
    let mut contents = Vec::new();
    file.take(limit + 1).read_to_end(&mut contents)?;
    if contents.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("SSH configuration file at {} is too large", path.display()),
        ));
    }
    Ok(contents)
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

#[cfg(unix)]
fn verify_ssh_identity_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "SSH identity at {} is accessible by group or others",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Rejects a private key that principals other than the current user,
/// SYSTEM, and Administrators can read, matching the check Win32-OpenSSH
/// applies to identity files.
#[cfg(windows)]
fn verify_ssh_identity_permissions(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_SUCCESS, GENERIC_ALL, GENERIC_READ, HANDLE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetTokenInformation,
        IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY,
        TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_READ_DATA;
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Frees memory the security APIs allocate with `LocalAlloc`.
    struct LocalMemory(*mut core::ffi::c_void);
    impl Drop for LocalMemory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from a Win32 API that documents
                // `LocalFree` as its deallocator and is freed exactly once.
                unsafe { LocalFree(self.0) };
            }
        }
    }

    fn sid_text(sid: PSID) -> String {
        let mut text = core::ptr::null_mut();
        // SAFETY: `sid` points into a live security descriptor or token buffer.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return "an unknown principal".into();
        }
        let text = LocalMemory(text.cast());
        let wide = text.0.cast::<u16>();
        // SAFETY: the API returns a NUL-terminated UTF-16 string.
        let len = (0..)
            .take_while(|&index| unsafe { *wide.add(index) } != 0)
            .count();
        // SAFETY: `len` code units were just read from the same allocation.
        String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(wide, len) })
    }

    let insecure = |detail: String| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "SSH identity at {} {detail}; restrict it to your account",
                path.display()
            ),
        )
    };

    // The current user's SID, kept alive in `token_user` while it is compared.
    let mut token: HANDLE = core::ptr::null_mut();
    // SAFETY: the current-process pseudo handle is always valid to open.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut needed = 0u32;
    // SAFETY: a null buffer with length 0 only queries the required size.
    unsafe { GetTokenInformation(token, TokenUser, core::ptr::null_mut(), 0, &mut needed) };
    let mut token_user = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: the buffer holds at least `needed` bytes and is 8-byte aligned.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            token_user.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    let query_error = io::Error::last_os_error();
    // SAFETY: `token` was opened above and is closed exactly once.
    unsafe { CloseHandle(token) };
    if queried == 0 {
        return Err(query_error);
    }
    // SAFETY: `GetTokenInformation(TokenUser)` filled the buffer with a `TOKEN_USER`.
    let user = unsafe { (*token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let trusted = |sid: PSID| {
        // SAFETY: both SIDs point into buffers that outlive this call.
        unsafe {
            EqualSid(sid, user) != 0
                || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
        }
    };

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut owner: PSID = core::ptr::null_mut();
    let mut dacl: *mut ACL = core::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = core::ptr::null_mut();
    // SAFETY: the path is NUL-terminated; the outputs point into `descriptor`.
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            core::ptr::null_mut(),
            &mut dacl,
            core::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _descriptor = LocalMemory(descriptor);
    if owner.is_null() || !trusted(owner) {
        return Err(insecure(format!("is owned by {}", sid_text(owner))));
    }
    if dacl.is_null() {
        return Err(insecure(
            "has no access control list, so anyone can read it".into(),
        ));
    }
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: `dacl` is a valid ACL inside `descriptor`; `info` is sized for the class.
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            core::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    for index in 0..info.AceCount {
        let mut ace = core::ptr::null_mut();
        // SAFETY: `index` is below the ACL's ACE count.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: every ACE starts with an `ACE_HEADER`.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
            continue;
        }
        // SAFETY: the header identifies this ACE as an `ACCESS_ALLOWED_ACE`,
        // whose SID starts at `SidStart`.
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let sid: PSID = (&allowed.SidStart as *const u32).cast_mut().cast();
        if allowed.Mask & (FILE_READ_DATA | GENERIC_READ | GENERIC_ALL) != 0 && !trusted(sid) {
            return Err(insecure(format!("is readable by {}", sid_text(sid))));
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn verify_ssh_identity_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_identity_permission_tests {
    use super::tests::restrict_to_current_user;
    use super::verify_ssh_identity_permissions;
    use std::process::Command;

    fn icacls(path: &std::path::Path, args: &[&str]) {
        let output = Command::new("icacls")
            .arg(path)
            .args(args)
            .output()
            .expect("icacls is part of Windows");
        assert!(output.status.success(), "{output:?}");
    }

    #[test]
    fn identity_readable_by_everyone_is_rejected_until_restricted() {
        let root =
            std::env::temp_dir().join(format!("nia-ssh-identity-acl-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let key = root.join("id_ed25519");
        std::fs::write(&key, "key").unwrap();

        // A temp-directory file inherits grants for other principals.
        assert!(verify_ssh_identity_permissions(&key).is_err());

        // Only the current user: accepted.
        restrict_to_current_user(&key);
        verify_ssh_identity_permissions(&key).unwrap();

        // Everyone (S-1-1-0) may read: rejected, naming the principal.
        icacls(&key, &["/grant", "*S-1-1-0:R"]);
        let error = verify_ssh_identity_permissions(&key).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("S-1-1-0"), "{error}");

        // Removing that grant makes it acceptable again.
        icacls(&key, &["/remove:g", "*S-1-1-0"]);
        verify_ssh_identity_permissions(&key).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
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
                        if data.len() > self.local_window as usize
                            || data.len() > self.local_max_packet
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
                            return Err(invalid_packet(
                                "SSH channel close has the wrong recipient",
                            ));
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
                        if data.len() > self.local_window as usize
                            || data.len() > self.local_max_packet
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

    pub(super) fn write_plain_ssh_packet(
        stream: &mut impl Write,
        payload: &[u8],
    ) -> io::Result<()> {
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
            let padding =
                private_reader.take(private_reader.bytes.len() - private_reader.offset)?;
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
                agent_signing_algorithm(key)
                    .is_ok_and(|(algorithm, _, _)| algorithm == b"ssh-ed25519")
            })
            .or_else(|| {
                public_keys.iter().find(|key| {
                    agent_signing_algorithm(key)
                        .is_ok_and(|(algorithm, _, _)| algorithm == b"ecdsa-sha2-nistp256")
                })
            })
            .or_else(|| {
                public_keys.iter().find(|key| {
                    agent_signing_algorithm(key)
                        .is_ok_and(|(algorithm, _, _)| algorithm == b"ssh-rsa")
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
        let socket = std::env::var_os("SSH_AUTH_SOCK")
            .unwrap_or_else(|| r"\\.\pipe\openssh-ssh-agent".into());
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpProxy {
    host: String,
    port: u16,
}

fn http_proxy_for_url(url: &HttpGitUrl) -> io::Result<Option<HttpProxy>> {
    http_proxy_for_url_with(url, |name| std::env::var(name).ok())
}

fn http_proxy_for_url_with(
    url: &HttpGitUrl,
    mut get_env: impl FnMut(&str) -> Option<String>,
) -> io::Result<Option<HttpProxy>> {
    let no_proxy = first_nonempty_env(&mut get_env, &["NO_PROXY", "no_proxy"]);
    if no_proxy
        .as_deref()
        .is_some_and(|value| no_proxy_matches(value, &url.host, url.port))
    {
        return Ok(None);
    }

    let proxy = if url.secure {
        first_nonempty_env(&mut get_env, &["HTTPS_PROXY", "https_proxy"])
    } else {
        first_nonempty_env(&mut get_env, &["HTTP_PROXY", "http_proxy"])
    }
    .or_else(|| first_nonempty_env(&mut get_env, &["ALL_PROXY", "all_proxy"]));
    proxy.map(|value| parse_http_proxy(&value)).transpose()
}

fn first_nonempty_env(
    get_env: &mut impl FnMut(&str) -> Option<String>,
    names: &[&str],
) -> Option<String> {
    names
        .iter()
        .find_map(|name| get_env(name).filter(|value| !value.trim().is_empty()))
}

fn no_proxy_matches(no_proxy: &str, host: &str, port: u16) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    no_proxy.split(',').any(|entry| {
        let entry = entry.trim();
        if entry.is_empty() {
            return false;
        }
        if entry == "*" {
            return true;
        }

        let (entry_host, entry_port) = if let Some(entry) = entry.strip_prefix('[') {
            let Some((entry_host, remainder)) = entry.split_once(']') else {
                return false;
            };
            let entry_port = if remainder.is_empty() {
                None
            } else {
                remainder
                    .strip_prefix(':')
                    .and_then(|value| value.parse::<u16>().ok())
            };
            if !remainder.is_empty() && entry_port.is_none() {
                return false;
            }
            (entry_host, entry_port)
        } else if let Some((entry_host, entry_port)) = entry.rsplit_once(':') {
            if entry_host.contains(':') {
                (entry, None)
            } else {
                let Ok(entry_port) = entry_port.parse::<u16>() else {
                    return false;
                };
                (entry_host, Some(entry_port))
            }
        } else {
            (entry, None)
        };
        if entry_port.is_some_and(|entry_port| entry_port != port) {
            return false;
        }

        let entry_host = entry_host
            .trim_start_matches('.')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        !entry_host.is_empty()
            && (host == entry_host
                || host
                    .strip_suffix(&entry_host)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    })
}

fn parse_http_proxy(source: &str) -> io::Result<HttpProxy> {
    let rest = source.trim().strip_prefix("http://").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Git proxy must use an unauthenticated http:// URL",
        )
    })?;
    let (authority, suffix) = rest.split_once(['/', '?', '#']).unwrap_or((rest, ""));
    if authority.is_empty() {
        return Err(invalid_packet("Git proxy URL has an invalid authority"));
    }
    if !suffix.is_empty() && suffix != "/" {
        return Err(invalid_packet("Git proxy URL has an invalid path"));
    }
    if authority.contains('@') {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Git proxy URL credentials are not supported",
        ));
    }
    let (host, port) = parse_http_authority(authority, 8080)?;
    Ok(HttpProxy { host, port })
}

fn parse_http_authority(authority: &str, default_port: u16) -> io::Result<(String, u16)> {
    if let Some(address) = authority.strip_prefix('[') {
        let (host, suffix) = address
            .split_once(']')
            .ok_or_else(|| invalid_packet("Git HTTP URL has an invalid IPv6 host"))?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .ok_or_else(|| invalid_packet("Git HTTP URL has an invalid port"))?
                .parse::<u16>()
                .map_err(|_| invalid_packet("Git HTTP port is invalid"))?
        };
        if host.parse::<std::net::Ipv6Addr>().is_err() || port == 0 {
            return Err(invalid_packet("Git HTTP URL has an invalid authority"));
        }
        return Ok((host.to_string(), port));
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || host.contains(':') || !port.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid_packet("Git HTTP URL has an invalid port"));
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| invalid_packet("Git HTTP port is invalid"))?;
        if port == 0 {
            return Err(invalid_packet("Git HTTP port is invalid"));
        }
        return Ok((host.to_string(), port));
    }
    if authority.contains(['@', '/', '?', '#', '[', ']'])
        || authority
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid_packet("Git HTTP URL has an invalid authority"));
    }
    if authority.is_empty() {
        return Err(invalid_packet("Git HTTP URL has an invalid authority"));
    }
    Ok((authority.to_string(), default_port))
}

fn lookup_http_authorization(url: &HttpGitUrl) -> Option<Zeroizing<Vec<u8>>> {
    use std::process::Command;

    run_http_credential_fill(url, Command::new("git"))
}

fn run_http_credential_fill(
    url: &HttpGitUrl,
    mut command: std::process::Command,
) -> Option<Zeroizing<Vec<u8>>> {
    use std::process::Stdio;

    let mut input = format!("protocol=https\nhost={}", url.host);
    if url.port != 443 {
        input.push_str(&format!(":{}", url.port));
    }
    input.push('\n');
    if url.path != "/" {
        input.push_str("path=");
        input.push_str(url.path.trim_start_matches('/'));
        input.push('\n');
    }
    input.push('\n');

    let mut child = command
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let output = Zeroizing::new(output.stdout);
    let credentials = parse_git_credentials(&output)?;
    let mut plain = Zeroizing::new(Vec::with_capacity(
        credentials.username.len() + credentials.password.len() + 1,
    ));
    plain.extend_from_slice(&credentials.username);
    plain.push(b':');
    plain.extend_from_slice(&credentials.password);
    Some(base64_encode(&plain))
}

struct GitCredentials {
    username: Zeroizing<Vec<u8>>,
    password: Zeroizing<Vec<u8>>,
}

fn parse_git_credentials(output: &[u8]) -> Option<GitCredentials> {
    let mut username = None;
    let mut password = None;
    for line in output.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(separator) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (key, value) = (&line[..separator], &line[separator + 1..]);
        match key {
            b"username" if username.is_none() => username = Some(value.to_vec()),
            b"password" if password.is_none() => password = Some(value.to_vec()),
            b"username" | b"password" => return None,
            _ => {}
        }
    }
    let username = username?;
    let password = password?;
    if username.is_empty()
        || username.contains(&b':')
        || username.iter().chain(&password).any(u8::is_ascii_control)
    {
        return None;
    }
    Some(GitCredentials {
        username: Zeroizing::new(username),
        password: Zeroizing::new(password),
    })
}

fn base64_encode(input: &[u8]) -> Zeroizing<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Zeroizing::new(Vec::with_capacity(input.len().div_ceil(3) * 4));
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[usize::from(first >> 2)]);
        output.push(TABLE[usize::from(((first & 0x03) << 4) | (second >> 4))]);
        output.push(if chunk.len() > 1 {
            TABLE[usize::from(((second & 0x0f) << 2) | (third >> 6))]
        } else {
            b'='
        });
        output.push(if chunk.len() > 2 {
            TABLE[usize::from(third & 0x3f)]
        } else {
            b'='
        });
    }
    output
}

fn explain_http_auth_error(
    error: io::Error,
    url: &HttpGitUrl,
    sent_credentials: bool,
) -> io::Error {
    if url.secure && error.kind() == io::ErrorKind::PermissionDenied {
        let detail = if sent_credentials {
            "Git HTTPS credentials were rejected"
        } else {
            "Git HTTPS server requires credentials; configure a Git credential helper"
        };
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{detail} for {} (HTTP 401)", url.host),
        );
    }
    error
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
    let (host, port) = parse_http_authority(authority, if secure { 443 } else { 80 })?;
    if path.contains(['?', '#'])
        || path
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid_packet("Git HTTP URL contains invalid path syntax"));
    }
    let path = format!("/{}", path.trim_end_matches('/'));
    Ok(HttpGitUrl {
        host,
        port,
        path,
        secure,
    })
}

fn checkout_http(source: &str, revision: &str, destination: &Path) -> io::Result<String> {
    let url = parse_http_git_url(source)?;
    let proxy = http_proxy_for_url(&url)?;
    let authorization = if url.secure {
        lookup_http_authorization(&url)
    } else {
        None
    };
    let info_path = format!("{}/info/refs?service=git-upload-pack", url.path);
    let authorization = authorization.as_ref().map(|value| value.as_slice());
    let info = http_request_with_proxy(
        &url,
        "GET",
        &info_path,
        &[],
        "",
        authorization,
        proxy.as_ref(),
    )
    .map_err(|error| explain_http_auth_error(error, &url, authorization.is_some()))?;
    let refs = parse_advertised_refs(&info)?;
    let commit = select_advertised_revision(&refs, revision)?;

    let mut request = Vec::new();
    let want = format!("want {} ofs-delta side-band-64k\n", encode_hex(&commit));
    encode_packet_line(want.as_bytes(), &mut request)?;
    request.extend_from_slice(b"0000");
    encode_packet_line(b"done\n", &mut request)?;
    let response = http_request_with_proxy(
        &url,
        "POST",
        &format!("{}/git-upload-pack", url.path),
        &request,
        "application/x-git-upload-pack-request",
        authorization,
        proxy.as_ref(),
    )
    .map_err(|error| explain_http_auth_error(error, &url, authorization.is_some()))?;
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

fn http_request_with_proxy(
    url: &HttpGitUrl,
    method: &str,
    path: &str,
    body: &[u8],
    content_type: &str,
    authorization: Option<&[u8]>,
    proxy: Option<&HttpProxy>,
) -> io::Result<Vec<u8>> {
    let (connect_host, connect_port) = proxy.map_or((url.host.as_str(), url.port), |proxy| {
        (proxy.host.as_str(), proxy.port)
    });
    let address = (connect_host, connect_port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Git HTTP host has no address"))?;
    let stream = TcpStream::connect_timeout(&address, std::time::Duration::from_secs(15))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    if proxy.is_some() && url.secure {
        establish_proxy_tunnel(&stream, url)?;
    }
    let request_target = if proxy.is_some() && !url.secure {
        format!(
            "http://{}{}",
            format_authority(&url.host, url.port, 80),
            path
        )
    } else {
        path.to_string()
    };
    if url.secure {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_name = if let Ok(address) = url.host.parse::<std::net::IpAddr>() {
            ServerName::IpAddress(address.into())
        } else {
            ServerName::try_from(url.host.clone()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Git HTTPS host name is invalid",
                )
            })?
        };
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
            &request_target,
            body,
            content_type,
            authorization,
        );
    }
    http_request_stream_with_target(
        stream,
        url,
        method,
        &request_target,
        body,
        content_type,
        authorization,
    )
}

fn establish_proxy_tunnel(mut stream: &TcpStream, url: &HttpGitUrl) -> io::Result<()> {
    let connect_target = format_authority(&url.host, url.port, 0);
    write!(
        stream,
        "CONNECT {connect_target} HTTP/1.1\r\nHost: {connect_target}\r\nConnection: keep-alive\r\n\r\n"
    )?;
    read_proxy_connect_response(stream)
}

fn http_request_stream<S: Read + Write>(
    stream: S,
    url: &HttpGitUrl,
    method: &str,
    path: &str,
    body: &[u8],
    content_type: &str,
    authorization: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    http_request_stream_with_target(stream, url, method, path, body, content_type, authorization)
}

fn http_request_stream_with_target<S: Read + Write>(
    mut stream: S,
    url: &HttpGitUrl,
    method: &str,
    request_target: &str,
    body: &[u8],
    content_type: &str,
    authorization: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    let host_header = format_authority(&url.host, url.port, if url.secure { 443 } else { 80 });
    write!(
        stream,
        "{method} {request_target} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: nia-package-manager/0.3\r\nAccept: application/x-git-upload-pack-advertisement, application/x-git-upload-pack-result\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    if !content_type.is_empty() {
        write!(stream, "Content-Type: {content_type}\r\n")?;
    }
    if let Some(authorization) = authorization {
        stream.write_all(b"Authorization: Basic ")?;
        stream.write_all(authorization)?;
        stream.write_all(b"\r\n")?;
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

fn format_authority(host: &str, port: u16, default_port: u16) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default_port {
        host
    } else {
        format!("{host}:{port}")
    }
}

fn read_proxy_connect_response(mut stream: &TcpStream) -> io::Result<()> {
    let mut response = Vec::new();
    let mut byte = [0; 1];
    while response.len() < 16 * 1024 {
        stream.read_exact(&mut byte)?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            let status = parse_proxy_status(&response)?;
            if (200..300).contains(&status) {
                return Ok(());
            }
            return Err(io::Error::other("Git HTTPS proxy CONNECT request failed"));
        }
    }
    Err(invalid_packet(
        "Git HTTPS proxy response headers are too large",
    ))
}

fn parse_proxy_status(response: &[u8]) -> io::Result<u16> {
    let line_end = response
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| invalid_packet("Git HTTPS proxy response status is malformed"))?;
    let line = std::str::from_utf8(&response[..line_end])
        .map_err(|_| invalid_packet("Git HTTPS proxy response status is malformed"))?;
    let mut fields = line.split_ascii_whitespace();
    let version = fields.next();
    let code = fields.next();
    let status = code
        .filter(|code| code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| invalid_packet("Git HTTPS proxy response status is malformed"))?;
    if !matches!(version, Some("HTTP/1.0" | "HTTP/1.1")) {
        return Err(invalid_packet(
            "Git HTTPS proxy response status is malformed",
        ));
    }
    Ok(status)
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
        let kind = if status == 401 {
            io::ErrorKind::PermissionDenied
        } else {
            io::ErrorKind::Other
        };
        return Err(io::Error::new(
            kind,
            format!("Git HTTP server returned status {status}"),
        ));
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

#[cfg(any(test, feature = "test-support"))]
mod tests {
    use super::ssh::*;
    use super::*;
    use ring::signature::KeyPair;

    #[cfg(feature = "test-support")]
    type SshFixtureCallback = Box<dyn FnOnce(&Path, &str, &Path, &Path)>;
    #[cfg(feature = "test-support")]
    type SshFixtureRepositoryMutator = Box<dyn FnOnce(&Path)>;

    #[cfg(feature = "test-support")]
    thread_local! {
        static SSH_FIXTURE_CALLBACK: std::cell::RefCell<Option<SshFixtureCallback>> = const { std::cell::RefCell::new(None) };
        static SSH_FIXTURE_REPOSITORY_MUTATOR: std::cell::RefCell<Option<SshFixtureRepositoryMutator>> = const { std::cell::RefCell::new(None) };
        static SSH_FIXTURE_SKIP_CLIENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Windows equivalent of `chmod 600` for a fixture private key: drop
    /// inherited ACEs and grant only the current user.
    #[cfg(windows)]
    pub(super) fn restrict_to_current_user(path: &Path) {
        let user = std::env::var("USERNAME").expect("USERNAME is set on Windows");
        let output = std::process::Command::new("icacls")
            .arg(path)
            .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
            .output()
            .expect("icacls is part of Windows");
        assert!(output.status.success(), "{output:?}");
    }

    #[cfg(feature = "test-support")]
    pub(super) fn with_ssh_checkout_fixture(
        callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static,
    ) {
        with_ssh_checkout_fixture_mutating_repository(|_| {}, callback);
    }

    #[cfg(feature = "test-support")]
    pub(super) fn with_ssh_checkout_fixture_mutating_repository(
        mutator: impl FnOnce(&Path) + 'static,
        callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static,
    ) {
        SSH_FIXTURE_CALLBACK.with(|slot| {
            assert!(slot.borrow_mut().replace(Box::new(callback)).is_none());
        });
        SSH_FIXTURE_REPOSITORY_MUTATOR.with(|slot| {
            assert!(slot.borrow_mut().replace(Box::new(mutator)).is_none());
        });
        SSH_FIXTURE_SKIP_CLIENT.with(|skip| skip.set(true));
        ssh_key_exchange_authenticates_and_opens_git_exec_channel_impl();
    }

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
    fn git_http_credentials_are_parsed_and_basic_encoded() {
        let credentials = parse_git_credentials(
            b"protocol=https\nhost=example.test\nusername=nia\npassword=secret\n",
        )
        .unwrap();
        let mut plain = credentials.username.to_vec();
        plain.push(b':');
        plain.extend_from_slice(&credentials.password);
        assert_eq!(&*base64_encode(&plain), b"bmlhOnNlY3JldA==");

        assert!(parse_git_credentials(b"username=nia\n").is_none());
        assert!(parse_git_credentials(b"username=nia\npassword=one\npassword=two\n").is_none());
        assert!(parse_git_credentials(b"username=nia:other\npassword=secret\n").is_none());
    }

    #[test]
    fn git_http_credential_fill_uses_the_configured_helper() {
        use std::process::Command;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nia-git-credential-helper-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("gitconfig");
        let store = root.join("credentials");
        let store_path = store.display().to_string().replace('\\', "/");
        let store_text = "https://nia:secret@example.test/repository.git\nhttps://other:ignored@example.test/other.git\n";
        std::fs::write(&store, store_text).unwrap();
        let store_arg = format!("credential.helper=store --file=\"{store_path}\"");
        std::fs::write(
            &config,
            format!(
                "[credential]\n\thelper = store --file=\"{store_path}\"\n\tuseHttpPath = true\n"
            ),
        )
        .unwrap();
        let url = parse_http_git_url("https://example.test/repository.git").unwrap();
        let mut command = Command::new("git");
        command
            .args(["-c", &store_arg, "-c", "credential.useHttpPath=true"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &config);
        assert_eq!(
            &*run_http_credential_fill(&url, command).unwrap(),
            b"bmlhOnNlY3JldA=="
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_http_authorization_is_sent_as_a_header_and_401_is_actionable() {
        use std::io::Cursor;

        struct TestStream {
            response: Cursor<Vec<u8>>,
            written: Vec<u8>,
        }
        impl Read for TestStream {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.response.read(output)
            }
        }
        impl Write for TestStream {
            fn write(&mut self, input: &[u8]) -> io::Result<usize> {
                self.written.extend_from_slice(input);
                Ok(input.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let url = parse_http_git_url("https://example.test/repository.git").unwrap();
        let mut stream = TestStream {
            response: Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()),
            written: Vec::new(),
        };
        http_request_stream(
            &mut stream,
            &url,
            "GET",
            "/repository.git/info/refs",
            &[],
            "",
            Some(b"bmlhOnNlY3JldA=="),
        )
        .unwrap();
        assert!(
            stream
                .written
                .windows(b"Authorization: Basic bmlhOnNlY3JldA==\r\n".len())
                .any(|window| window == b"Authorization: Basic bmlhOnNlY3JldA==\r\n")
        );

        let unauthorized =
            parse_http_response(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                .unwrap_err();
        let error = explain_http_auth_error(unauthorized, &url, false);
        assert!(
            error
                .to_string()
                .contains("configure a Git credential helper")
        );
        assert!(!error.to_string().contains("secret"));
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
        for invalid in [
            "http://example.test:0/repository.git",
            "http://exa mple.test/repository.git",
            "http://example.test/repo\r\nInjected: header",
            "http://[not-ipv6]/repository.git",
        ] {
            assert!(parse_http_git_url(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(
            parse_http_git_url("https://[2001:db8::1]:8443/repository.git")
                .unwrap()
                .host,
            "2001:db8::1"
        );
    }

    #[test]
    fn http_proxy_selection_obeys_protocol_and_no_proxy_rules() {
        use std::collections::HashMap;

        let secure = parse_http_git_url("https://git.example.test/repository.git").unwrap();
        let mut vars = HashMap::from([
            ("HTTPS_PROXY", "http://secure-proxy.test:9000"),
            ("HTTP_PROXY", "http://plain-proxy.test:8000"),
            ("ALL_PROXY", "http://fallback-proxy.test"),
        ]);
        let proxy =
            http_proxy_for_url_with(&secure, |name| vars.get(name).map(|v| v.to_string())).unwrap();
        assert_eq!(
            proxy,
            Some(HttpProxy {
                host: "secure-proxy.test".into(),
                port: 9000,
            })
        );

        vars.insert("NO_PROXY", ".example.test:443,localhost");
        assert_eq!(
            http_proxy_for_url_with(&secure, |name| vars.get(name).map(|v| v.to_string())).unwrap(),
            None
        );
        vars.insert("NO_PROXY", "example.test:80");
        assert!(
            http_proxy_for_url_with(&secure, |name| vars.get(name).map(|v| v.to_string()))
                .unwrap()
                .is_some()
        );

        let plain = parse_http_git_url("http://git.example.test/repository.git").unwrap();
        vars.remove("HTTP_PROXY");
        vars.remove("NO_PROXY");
        assert_eq!(
            http_proxy_for_url_with(&plain, |name| vars.get(name).map(|v| v.to_string()))
                .unwrap()
                .unwrap()
                .host,
            "fallback-proxy.test"
        );
    }

    #[test]
    fn http_proxy_receives_absolute_target_and_origin_host_header() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let url = parse_http_git_url("http://git.example.test:8081/repository.git").unwrap();
        let proxy = HttpProxy {
            host: address.ip().to_string(),
            port: address.port(),
        };
        http_request_with_proxy(
            &url,
            "GET",
            "/repository.git/info/refs?service=git-upload-pack",
            &[],
            "",
            None,
            Some(&proxy),
        )
        .unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with(
            "GET http://git.example.test:8081/repository.git/info/refs?service=git-upload-pack HTTP/1.1\r\n"
        ));
        assert!(request.contains("\r\nHost: git.example.test:8081\r\n"));
        assert!(!request.contains("Authorization:"));
    }

    #[test]
    fn https_proxy_tunnel_uses_origin_authority_without_credentials() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let url = parse_http_git_url("https://[2001:db8::1]/repository.git").unwrap();
        let stream = TcpStream::connect(address).unwrap();
        establish_proxy_tunnel(&stream, &url).unwrap();
        let request = server.join().unwrap();
        assert!(
            request
                .starts_with("CONNECT [2001:db8::1]:443 HTTP/1.1\r\nHost: [2001:db8::1]:443\r\n")
        );
        assert!(!request.contains("Authorization:"));
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
    fn ssh_sources_reject_invalid_urls_before_loading_credentials() {
        let destination =
            std::env::temp_dir().join(format!("nia-git-ssh-invalid-{}", std::process::id()));
        for source in [
            "ssh://user:password@example.test/repository.git",
            "ssh://example.test/repository.git?query",
            "ssh://example.test:0/repository.git",
            "git@example.test:",
        ] {
            assert!(
                checkout_source(source, "HEAD", &destination).is_err(),
                "{source}"
            );
        }
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

        let encoded_blob = base64(&key_blob);
        let malformed = format!("host.example.test ssh-ed25519 {encoded_blob}x\n");
        assert!(KnownHosts::parse(malformed.as_bytes()).is_err());

        let pathological_pattern = format!("{}b", "*a".repeat(25));
        let bounded_matcher = KnownHosts::parse(
            format!("{pathological_pattern} ssh-ed25519 {encoded_blob}\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(
            bounded_matcher
                .verify(&format!("{}c", "a".repeat(25)), 22, &key_blob)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );

        let revoked = KnownHosts::parse(
            format!("@revoked good.example.test ssh-ed25519 {encoded_blob}\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(
            revoked
                .verify("good.example.test", 22, &key_blob)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        let ca_only = KnownHosts::parse(
            format!("@cert-authority *.example.test ssh-ed25519 {encoded_blob}\n").as_bytes(),
        )
        .unwrap();
        assert_eq!(
            ca_only
                .verify("good.example.test", 22, &key_blob)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );

        let salt = [23u8; 20];
        let hashed_host = |candidate: &str| {
            let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &salt);
            let digest = ring::hmac::sign(&key, candidate.as_bytes());
            format!("|1|{}|{}", base64(&salt), base64(digest.as_ref()))
        };
        let hashed_default = KnownHosts::parse(
            format!(
                "{} ssh-ed25519 {encoded_blob}\n",
                hashed_host("hashed.example.test")
            )
            .as_bytes(),
        )
        .unwrap();
        hashed_default
            .verify("hashed.example.test", 22, &key_blob)
            .unwrap();
        assert_eq!(
            hashed_default
                .verify("other.example.test", 22, &key_blob)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );

        let hashed_non_default = KnownHosts::parse(
            format!(
                "{} ssh-ed25519 {encoded_blob}\n",
                hashed_host("[hashed.example.test]:2222")
            )
            .as_bytes(),
        )
        .unwrap();
        hashed_non_default
            .verify("hashed.example.test", 2222, &key_blob)
            .unwrap();
        assert_eq!(
            hashed_non_default
                .verify("hashed.example.test", 22, &key_blob)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
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

    #[cfg(test)]
    #[test]
    fn ssh_key_exchange_authenticates_and_opens_git_exec_channel() {
        ssh_key_exchange_authenticates_and_opens_git_exec_channel_impl();
    }

    #[cfg(any(test, feature = "test-support"))]
    fn ssh_key_exchange_authenticates_and_opens_git_exec_channel_impl() {
        use std::net::TcpListener;
        use std::process::{Command, Stdio};

        fn put_string(value: &[u8], output: &mut Vec<u8>) {
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value);
        }

        fn take_string<'a>(bytes: &'a [u8], offset: &mut usize) -> &'a [u8] {
            let length =
                u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
            *offset += 4;
            let value = &bytes[*offset..*offset + length];
            *offset += length;
            value
        }

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

        fn openssh_private_key(seed: &[u8; 32]) -> Vec<u8> {
            let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
            let mut public_blob = Vec::new();
            put_string(b"ssh-ed25519", &mut public_blob);
            put_string(key_pair.public_key().as_ref(), &mut public_blob);
            let mut private_block = Vec::new();
            private_block.extend_from_slice(&0x1122_3344u32.to_be_bytes());
            private_block.extend_from_slice(&0x1122_3344u32.to_be_bytes());
            put_string(b"ssh-ed25519", &mut private_block);
            put_string(key_pair.public_key().as_ref(), &mut private_block);
            let mut private = seed.to_vec();
            private.extend_from_slice(key_pair.public_key().as_ref());
            put_string(&private, &mut private_block);
            put_string(b"test identity", &mut private_block);
            let padding_len = 8 - (private_block.len() % 8);
            private_block.extend((1..=padding_len).map(|byte| byte as u8));

            let mut encoded = b"openssh-key-v1\0".to_vec();
            put_string(b"none", &mut encoded);
            put_string(b"none", &mut encoded);
            put_string(b"", &mut encoded);
            encoded.extend_from_slice(&1u32.to_be_bytes());
            put_string(&public_blob, &mut encoded);
            put_string(&private_block, &mut encoded);
            format!(
                "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
                base64(&encoded)
            )
            .into_bytes()
        }

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("nia-ssh-checkout-{nonce}"));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet", "--initial-branch=main"])
                .arg(&repository)
                .status()
                .unwrap()
                .success()
        );
        for (key, value) in [
            ("user.name", "Nia Test"),
            ("user.email", "nia@example.test"),
            ("core.autocrlf", "false"),
        ] {
            assert!(
                Command::new("git")
                    .args(["-C"])
                    .arg(&repository)
                    .args(["config", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(repository.join("file.txt"), b"native SSH checkout\n").unwrap();
        std::fs::create_dir_all(repository.join("src")).unwrap();
        std::fs::write(
            repository.join("nia.toml"),
            "[package]\nname = \"sshdep\"\n[library]\npath = \"src/lib.nia\"\n",
        )
        .unwrap();
        std::fs::write(
            repository.join("src/lib.nia"),
            b"pub const value: u8 = 7;\n",
        )
        .unwrap();
        #[cfg(feature = "test-support")]
        SSH_FIXTURE_REPOSITORY_MUTATOR.with(|slot| {
            if let Some(mutator) = slot.borrow_mut().take() {
                mutator(&repository);
            }
        });
        assert!(
            Command::new("git")
                .args(["-C"])
                .arg(&repository)
                .args(["add", "-A"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["-C"])
                .arg(&repository)
                .args(["commit", "--quiet", "-m", "fixture"])
                .status()
                .unwrap()
                .success()
        );
        let commit_output = Command::new("git")
            .args(["-C"])
            .arg(&repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(commit_output.status.success());
        let commit_id = String::from_utf8(commit_output.stdout)
            .unwrap()
            .trim()
            .to_owned();
        let mut pack_process = Command::new("git")
            .args(["-C"])
            .arg(&repository)
            .args(["pack-objects", "--stdout", "--revs"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        pack_process
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{commit_id}\n").as_bytes())
            .unwrap();
        let pack_output = pack_process.wait_with_output().unwrap();
        assert!(pack_output.status.success());
        let pack = pack_output.stdout;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server_commit_id = commit_id.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            assert!(
                read_ssh_identification(&mut stream)
                    .unwrap()
                    .starts_with(b"SSH-2.0-NiaGit")
            );
            stream.write_all(b"SSH-2.0-TestServer\r\n").unwrap();

            let client_kexinit = read_ssh_packet(&mut stream, 8).unwrap().payload;
            let client_proposals = parse_ssh_kexinit(&client_kexinit).unwrap();
            let server_kexinit = build_client_kexinit(&[31; 16]);
            write_plain_ssh_packet(&mut stream, &server_kexinit).unwrap();

            let client_init = read_ssh_packet(&mut stream, 8).unwrap().payload;
            assert_eq!(client_init[0], 30);
            let client_public: [u8; 32] = client_init[5..37].try_into().unwrap();
            let server_kex = Curve25519ClientKex::generate().unwrap();
            let server_public = *server_kex.client_public();
            let shared_secret = server_kex.complete(&client_public).unwrap();
            let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[19; 32]).unwrap();
            let mut host_key = Vec::new();
            put_string(b"ssh-ed25519", &mut host_key);
            put_string(key_pair.public_key().as_ref(), &mut host_key);
            let exchange_hash = curve25519_exchange_hash(Curve25519ExchangeHashInput {
                client_identification: b"SSH-2.0-NiaGit_0.3",
                server_identification: b"SSH-2.0-TestServer",
                client_kexinit: &client_kexinit,
                server_kexinit: &server_kexinit,
                server_host_key: &host_key,
                client_public: &client_public,
                server_public: &server_public,
                shared_secret: &shared_secret,
            })
            .unwrap();
            let signature = key_pair.sign(&exchange_hash);
            let mut signature_blob = Vec::new();
            put_string(b"ssh-ed25519", &mut signature_blob);
            put_string(signature.as_ref(), &mut signature_blob);
            let mut reply = vec![31];
            put_string(&host_key, &mut reply);
            put_string(&server_public, &mut reply);
            put_string(&signature_blob, &mut reply);
            write_plain_ssh_packet(&mut stream, &reply).unwrap();

            assert_eq!(read_ssh_packet(&mut stream, 8).unwrap().payload, [21]);
            write_plain_ssh_packet(&mut stream, &[21]).unwrap();

            let session_id = exchange_hash;
            let derive = |letter, length| {
                derive_ssh_key(&shared_secret, &exchange_hash, &session_id, letter, length).unwrap()
            };
            let mut inbound =
                SshAes128CtrDirection::new(&derive(b'C', 16), &derive(b'A', 16), &derive(b'E', 32))
                    .unwrap();
            let mut outbound =
                SshAes128CtrDirection::new(&derive(b'D', 16), &derive(b'B', 16), &derive(b'F', 32))
                    .unwrap();

            let service_request = inbound.read_packet(&mut stream).unwrap().payload;
            assert_eq!(service_request[0], 5);
            let mut offset = 1;
            assert_eq!(take_string(&service_request, &mut offset), b"ssh-userauth");
            let mut service_accept = vec![6];
            put_string(b"ssh-userauth", &mut service_accept);
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &service_accept).unwrap();

            let auth_request = inbound.read_packet(&mut stream).unwrap().payload;
            assert_eq!(auth_request[0], 50);
            let mut offset = 1;
            assert_eq!(take_string(&auth_request, &mut offset), b"git");
            assert_eq!(take_string(&auth_request, &mut offset), b"ssh-connection");
            assert_eq!(take_string(&auth_request, &mut offset), b"publickey");
            assert_eq!(auth_request[offset], 1);
            offset += 1;
            assert_eq!(take_string(&auth_request, &mut offset), b"ssh-ed25519");
            let public_key_blob = take_string(&auth_request, &mut offset);
            let signature_field_offset = offset;
            let signature_blob = take_string(&auth_request, &mut offset);
            assert_eq!(offset, auth_request.len());
            let mut sig_offset = 0;
            assert_eq!(take_string(signature_blob, &mut sig_offset), b"ssh-ed25519");
            let signature = take_string(signature_blob, &mut sig_offset);
            let mut signed = Vec::new();
            put_string(&session_id, &mut signed);
            signed.extend_from_slice(&auth_request[..signature_field_offset]);
            let mut key_offset = 0;
            assert_eq!(
                take_string(public_key_blob, &mut key_offset),
                b"ssh-ed25519"
            );
            let public_key = take_string(public_key_blob, &mut key_offset);
            assert_eq!(key_offset, public_key_blob.len());
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
                .verify(&signed, signature)
                .unwrap();
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &[52]).unwrap();

            let open = inbound.read_packet(&mut stream).unwrap().payload;
            assert_eq!(open[0], 90);
            let mut offset = 1;
            assert_eq!(take_string(&open, &mut offset), b"session");
            let client_channel = u32::from_be_bytes(open[offset..offset + 4].try_into().unwrap());
            assert_eq!(client_channel, 0);
            let mut confirmation = vec![91];
            confirmation.extend_from_slice(&client_channel.to_be_bytes());
            confirmation.extend_from_slice(&7u32.to_be_bytes());
            confirmation.extend_from_slice(&0u32.to_be_bytes());
            confirmation.extend_from_slice(&32_768u32.to_be_bytes());
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &confirmation).unwrap();

            let exec = inbound.read_packet(&mut stream).unwrap().payload;
            assert_eq!(exec[0], 98);
            let mut offset = 1;
            assert_eq!(
                u32::from_be_bytes(exec[offset..offset + 4].try_into().unwrap()),
                7
            );
            offset += 4;
            assert_eq!(take_string(&exec, &mut offset), b"exec");
            assert_eq!(exec[offset], 1);
            offset += 1;
            assert_eq!(
                take_string(&exec, &mut offset),
                b"git-upload-pack '/org/repo.git'"
            );
            assert_eq!(offset, exec.len());
            let mut success = vec![99];
            success.extend_from_slice(&client_channel.to_be_bytes());
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &success).unwrap();

            let mut advertisement = Vec::new();
            encode_packet_line(
                format!("{server_commit_id} HEAD\0side-band-64k ofs-delta\n").as_bytes(),
                &mut advertisement,
            )
            .unwrap();
            advertisement.extend_from_slice(b"0000");
            let mut channel_data = vec![94];
            channel_data.extend_from_slice(&client_channel.to_be_bytes());
            put_string(&advertisement, &mut channel_data);
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &channel_data).unwrap();

            let mut adjust = vec![93];
            adjust.extend_from_slice(&client_channel.to_be_bytes());
            adjust.extend_from_slice(&4096u32.to_be_bytes());
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &adjust).unwrap();
            let mut client_git_request = Vec::new();
            loop {
                let packet = inbound.read_packet(&mut stream).unwrap().payload;
                match packet.first().copied() {
                    Some(93) => assert_eq!(packet.len(), 9),
                    Some(94) => {
                        let mut offset = 1;
                        assert_eq!(
                            u32::from_be_bytes(packet[offset..offset + 4].try_into().unwrap()),
                            7
                        );
                        offset += 4;
                        client_git_request.extend_from_slice(take_string(&packet, &mut offset));
                        assert_eq!(offset, packet.len());
                    }
                    Some(96) => {
                        assert_eq!(packet, [96, 0, 0, 0, 7]);
                        break;
                    }
                    _ => panic!("unexpected SSH channel message: {packet:?}"),
                }
            }
            let request_packets = decode_packet_lines(&client_git_request).unwrap();
            assert!(request_packets.iter().any(|packet| matches!(packet, PacketLine::Data(data) if String::from_utf8_lossy(data).contains(&format!("want {server_commit_id}")))));
            assert!(
                request_packets
                    .iter()
                    .any(|packet| matches!(packet, PacketLine::Data(data) if data == b"done\n"))
            );

            let mut git_response = Vec::new();
            for chunk in pack.chunks(60_000) {
                let mut sideband = Vec::with_capacity(chunk.len() + 1);
                sideband.push(1);
                sideband.extend_from_slice(chunk);
                encode_packet_line(&sideband, &mut git_response).unwrap();
            }
            git_response.extend_from_slice(b"0000");
            for chunk in git_response.chunks(30_000) {
                let mut channel_data = vec![94];
                channel_data.extend_from_slice(&client_channel.to_be_bytes());
                put_string(chunk, &mut channel_data);
                write_encrypted_ssh_packet(&mut stream, &mut outbound, &channel_data).unwrap();
            }
            let window_adjust = inbound.read_packet(&mut stream).unwrap().payload;
            assert_eq!(window_adjust[0], 93);
            let mut eof = vec![96];
            eof.extend_from_slice(&client_channel.to_be_bytes());
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &eof).unwrap();
            loop {
                let close = inbound.read_packet(&mut stream).unwrap().payload;
                if close[0] == 93 {
                    continue;
                }
                assert_eq!(close, [97, 0, 0, 0, 7]);
                break;
            }
            client_proposals
        });

        let ssh_config = root.join("ssh");
        std::fs::create_dir_all(&ssh_config).unwrap();
        let mut host_key = Vec::new();
        put_string(b"ssh-ed25519", &mut host_key);
        put_string(
            ring::signature::Ed25519KeyPair::from_seed_unchecked(&[19; 32])
                .unwrap()
                .public_key()
                .as_ref(),
            &mut host_key,
        );
        std::fs::write(
            ssh_config.join("known_hosts"),
            format!(
                "[127.0.0.1]:{} ssh-ed25519 {}\n",
                address.port(),
                base64(&host_key)
            ),
        )
        .unwrap();
        std::fs::write(
            ssh_config.join("id_ed25519"),
            openssh_private_key(&[37; 32]),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                ssh_config.join("id_ed25519"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        #[cfg(windows)]
        restrict_to_current_user(&ssh_config.join("id_ed25519"));
        let destination = root.join("checkout");
        let source = format!("ssh://git@127.0.0.1:{}/org/repo.git", address.port());
        #[cfg(feature = "test-support")]
        SSH_FIXTURE_CALLBACK.with(|slot| {
            if let Some(callback) = slot.borrow_mut().take() {
                callback(&root, &source, &ssh_config, &repository);
            }
        });
        #[cfg(feature = "test-support")]
        if SSH_FIXTURE_SKIP_CLIENT.with(std::cell::Cell::get) {
            server.join().unwrap();
            std::fs::remove_dir_all(root).unwrap();
            return;
        }
        let checked_out =
            match checkout_ssh_with_config(&source, "HEAD", &destination, &ssh_config, false) {
                Ok(checked_out) => checked_out,
                // Do not join the server here: a client that fails before
                // connecting leaves it blocked in `accept`, which would hang the
                // test instead of failing it.
                Err(error) => panic!("SSH checkout failed: {error}"),
            };
        assert_eq!(checked_out, commit_id);
        assert_eq!(
            std::fs::read(destination.join("file.txt")).unwrap(),
            b"native SSH checkout\n"
        );
        assert!(
            server
                .join()
                .unwrap()
                .kex_algorithms
                .contains(&"curve25519-sha256".to_string())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn openssh_unencrypted_ed25519_private_key_is_strictly_parsed() {
        fn put_string(value: &[u8], output: &mut Vec<u8>) {
            output.extend_from_slice(&(value.len() as u32).to_be_bytes());
            output.extend_from_slice(value);
        }

        fn take_string<'a>(bytes: &'a [u8], offset: &mut usize) -> &'a [u8] {
            let length =
                u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
            *offset += 4;
            let value = &bytes[*offset..*offset + length];
            *offset += length;
            value
        }

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

        let seed = [23u8; 32];
        let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let public = key_pair.public_key().as_ref();
        let mut public_blob = Vec::new();
        put_string(b"ssh-ed25519", &mut public_blob);
        put_string(public, &mut public_blob);
        let mut private_block = Vec::new();
        private_block.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        private_block.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        put_string(b"ssh-ed25519", &mut private_block);
        put_string(public, &mut private_block);
        let mut private_key = seed.to_vec();
        private_key.extend_from_slice(public);
        put_string(&private_key, &mut private_block);
        put_string(b"test identity", &mut private_block);
        let padding_len = 8 - (private_block.len() % 8);
        private_block.extend((1..=padding_len).map(|byte| byte as u8));

        let mut encoded = b"openssh-key-v1\0".to_vec();
        put_string(b"none", &mut encoded);
        put_string(b"none", &mut encoded);
        put_string(b"", &mut encoded);
        encoded.extend_from_slice(&1u32.to_be_bytes());
        put_string(&public_blob, &mut encoded);
        put_string(&private_block, &mut encoded);
        let pem = format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
            base64(&encoded)
        );
        let mut identity = SshEd25519Identity::parse_openssh(pem.as_bytes()).unwrap();
        assert_eq!(identity.public_key_blob(), public_blob);
        let signature = identity.sign(b"signed test data");
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
            .verify(b"signed test data", signature.as_ref())
            .unwrap();

        let session_id = [29u8; 32];
        let request = build_publickey_auth_request("git", &session_id, &mut identity).unwrap();
        assert_eq!(request[0], 50);
        let mut offset = 1;
        assert_eq!(take_string(&request, &mut offset), b"git");
        assert_eq!(take_string(&request, &mut offset), b"ssh-connection");
        assert_eq!(take_string(&request, &mut offset), b"publickey");
        assert_eq!(request[offset], 1);
        offset += 1;
        assert_eq!(take_string(&request, &mut offset), b"ssh-ed25519");
        assert_eq!(take_string(&request, &mut offset), public_blob);
        let signature_field_offset = offset;
        let signature_blob = take_string(&request, &mut offset);
        assert_eq!(offset, request.len());
        let mut signature_blob_offset = 0;
        assert_eq!(
            take_string(signature_blob, &mut signature_blob_offset),
            b"ssh-ed25519"
        );
        let auth_signature = take_string(signature_blob, &mut signature_blob_offset);
        assert_eq!(signature_blob_offset, signature_blob.len());
        let mut signed = Vec::new();
        put_string(&session_id, &mut signed);
        signed.extend_from_slice(&request[..signature_field_offset]);
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
            .verify(&signed, auth_signature)
            .unwrap();

        private_block[4] ^= 1;
        let mut malformed = b"openssh-key-v1\0".to_vec();
        put_string(b"none", &mut malformed);
        put_string(b"none", &mut malformed);
        put_string(b"", &mut malformed);
        malformed.extend_from_slice(&1u32.to_be_bytes());
        put_string(&public_blob, &mut malformed);
        put_string(&private_block, &mut malformed);
        let malformed_pem = format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
            base64(&malformed)
        );
        assert!(SshEd25519Identity::parse_openssh(malformed_pem.as_bytes()).is_err());
    }

    #[test]
    fn ssh_agent_identity_reply_selects_ed25519_and_rejects_bad_frames() {
        let mut ed25519 = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut ed25519).unwrap();
        encode_ssh_string(&[7; 32], &mut ed25519).unwrap();
        let mut rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut rsa).unwrap();
        encode_ssh_string(&[1, 2, 3], &mut rsa).unwrap();

        let mut identities = vec![12];
        identities.extend_from_slice(&2u32.to_be_bytes());
        encode_ssh_string(&rsa, &mut identities).unwrap();
        encode_ssh_string(b"rsa fixture", &mut identities).unwrap();
        encode_ssh_string(&ed25519, &mut identities).unwrap();
        encode_ssh_string(b"ed25519 fixture", &mut identities).unwrap();
        assert_eq!(parse_agent_identities(&identities).unwrap(), Some(ed25519));

        let mut ecdsa = Vec::new();
        encode_ssh_string(b"ecdsa-sha2-nistp256", &mut ecdsa).unwrap();
        encode_ssh_string(b"nistp256", &mut ecdsa).unwrap();
        encode_ssh_string(&[vec![4], vec![7; 64]].concat(), &mut ecdsa).unwrap();
        let mut rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut rsa).unwrap();
        encode_ssh_string(&[1, 0, 1], &mut rsa).unwrap();
        encode_ssh_string(&[0x7f; 256], &mut rsa).unwrap();
        let mut identities = vec![12];
        identities.extend_from_slice(&2u32.to_be_bytes());
        encode_ssh_string(&rsa, &mut identities).unwrap();
        encode_ssh_string(b"rsa fixture", &mut identities).unwrap();
        encode_ssh_string(&ecdsa, &mut identities).unwrap();
        encode_ssh_string(b"ecdsa fixture", &mut identities).unwrap();
        assert_eq!(
            parse_agent_identities(&identities).unwrap(),
            Some(ecdsa.clone())
        );
        assert_eq!(
            ssh::agent_signing_algorithm(&rsa).unwrap(),
            (b"ssh-rsa".to_vec(), b"rsa-sha2-512".to_vec(), 4)
        );
        assert_eq!(parse_agent_identities(&[5]).unwrap(), None);
        assert!(parse_agent_identities(&[12, 0, 0, 0]).is_err());
    }

    #[test]
    fn ssh_agent_prefers_ecdsa_then_rsa_and_uses_rsa_sha2_flags() {
        let mut rsa = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut rsa).unwrap();
        encode_ssh_string(&[1, 0, 1], &mut rsa).unwrap();
        encode_ssh_string(&[0x7f; 256], &mut rsa).unwrap();
        let mut identities = vec![12];
        identities.extend_from_slice(&1u32.to_be_bytes());
        encode_ssh_string(&rsa, &mut identities).unwrap();
        encode_ssh_string(b"rsa fixture", &mut identities).unwrap();
        assert_eq!(
            ssh::parse_agent_identities(&identities).unwrap(),
            Some(rsa.clone())
        );
        assert_eq!(
            ssh::agent_signing_algorithm(&rsa).unwrap(),
            (b"ssh-rsa".to_vec(), b"rsa-sha2-512".to_vec(), 4)
        );
    }

    #[test]
    fn ssh_publickey_auth_uses_the_signers_signature_algorithm() {
        fn read_string<'a>(bytes: &'a [u8], offset: &mut usize) -> &'a [u8] {
            let length =
                u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
            *offset += 4;
            let value = &bytes[*offset..*offset + length];
            *offset += length;
            value
        }

        struct RsaSigner {
            public_key: Vec<u8>,
        }
        impl ssh::SshSigner for RsaSigner {
            fn public_key_blob(&self) -> &[u8] {
                &self.public_key
            }

            fn signature_algorithm(&self) -> &[u8] {
                b"rsa-sha2-512"
            }

            fn sign_message(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
                assert!(!message.is_empty());
                Ok(vec![0x5a; 256])
            }
        }

        let mut public_key = Vec::new();
        encode_ssh_string(b"ssh-rsa", &mut public_key).unwrap();
        encode_ssh_string(&[1, 0, 1], &mut public_key).unwrap();
        encode_ssh_string(&[0x7f; 256], &mut public_key).unwrap();
        let mut signer = RsaSigner { public_key };
        let request = ssh::build_publickey_auth_request("git", &[3; 32], &mut signer).unwrap();
        let payload = &request[1..];
        let mut offset = 0;
        assert_eq!(read_string(payload, &mut offset), b"git");
        assert_eq!(read_string(payload, &mut offset), b"ssh-connection");
        assert_eq!(read_string(payload, &mut offset), b"publickey");
        assert_eq!(payload[offset], 1);
        offset += 1;
        assert_eq!(read_string(payload, &mut offset), b"rsa-sha2-512");
        assert_eq!(read_string(payload, &mut offset), signer.public_key);
        let signature_blob = read_string(payload, &mut offset);
        assert_eq!(offset, payload.len());
        let mut signature_offset = 0;
        assert_eq!(
            read_string(signature_blob, &mut signature_offset),
            b"rsa-sha2-512"
        );
        assert_eq!(
            read_string(signature_blob, &mut signature_offset),
            [0x5a; 256]
        );
        assert_eq!(signature_offset, signature_blob.len());
    }

    #[test]
    fn ssh_agent_request_uses_bounded_length_framing() {
        use std::io::Cursor;

        struct TestStream {
            incoming: Cursor<Vec<u8>>,
            outgoing: Vec<u8>,
        }
        impl Read for TestStream {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.incoming.read(output)
            }
        }
        impl Write for TestStream {
            fn write(&mut self, input: &[u8]) -> io::Result<usize> {
                self.outgoing.extend_from_slice(input);
                Ok(input.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut stream = TestStream {
            incoming: Cursor::new(vec![0, 0, 0, 1, 5]),
            outgoing: Vec::new(),
        };
        assert_eq!(agent_request(&mut stream, &[11]).unwrap(), [5]);
        assert_eq!(stream.outgoing, [0, 0, 0, 1, 11]);
        assert!(agent_request(&mut stream, &[]).is_err());
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

/// Restricts a copied fixture identity the way `chmod 600` would, so it
/// passes the SSH identity permission check.
#[cfg(feature = "test-support")]
pub fn restrict_test_identity_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[cfg(windows)]
    tests::restrict_to_current_user(path);
}

#[cfg(feature = "test-support")]
pub fn with_test_ssh_checkout_fixture(callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static) {
    tests::with_ssh_checkout_fixture(callback);
}

#[cfg(feature = "test-support")]
pub fn with_test_ssh_checkout_fixture_mutating_repository(
    mutator: impl FnOnce(&Path) + 'static,
    callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static,
) {
    tests::with_ssh_checkout_fixture_mutating_repository(mutator, callback);
}
