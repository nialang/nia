//! Nia-owned Git wire and object decoding primitives.
//!
//! Git object validation and checkout live behind this boundary. Package graph
//! code consumes filesystem roots and never sees Git object representations.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_HTTP_RESPONSE: usize = 128 * 1024 * 1024;
const MAX_PACKET_PAYLOAD: usize = 65_516;
const MAX_SSH_IDENTIFICATION_LINE: usize = 255;
const MAX_SSH_PRE_IDENTIFICATION_LINES: usize = 50;
const MAX_SSH_PACKET_BYTES: usize = 256 * 1024;

mod pktline;
use pktline::{PacketLine, decode_packet_lines, encode_packet_line};
mod object;
use object::{encode_hex, parse_object_id};
mod transport;
use transport::git_tcp::checkout_git;
#[cfg(test)]
use transport::git_tcp::parse_git_tcp_url;
use transport::http::checkout_http;
#[cfg(test)]
use transport::http::{
    HttpProxy, base64_encode, establish_proxy_tunnel, explain_http_auth_error,
    http_proxy_for_url_with, http_request_stream, http_request_with_proxy, parse_git_credentials,
    parse_http_git_url, parse_http_response, run_http_credential_fill,
};
use transport::local::{checkout_local, fs_create_dir};

fn invalid_packet(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
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

/// Fetches a bounded HTTPS resource with the Git transport's TLS and proxy policy.
pub fn fetch_https(source: &str) -> io::Result<Vec<u8>> {
    transport::http::fetch_https(source)
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
    let request = build_upload_pack_request(&commit)?;
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

mod ssh;

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

fn build_upload_pack_request(commit: &[u8; 20]) -> io::Result<Vec<u8>> {
    let mut request = Vec::new();
    encode_packet_line(
        format!("want {} ofs-delta side-band-64k\n", encode_hex(commit)).as_bytes(),
        &mut request,
    )?;
    encode_packet_line(b"deepen 1\n", &mut request)?;
    request.extend_from_slice(b"0000");
    encode_packet_line(b"done\n", &mut request)?;
    Ok(request)
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

#[cfg(any(test, feature = "test-support"))]
mod tests;

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
