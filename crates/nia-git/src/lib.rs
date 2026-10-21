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
    fn upload_pack_fetch_requests_depth_one() {
        let request = build_upload_pack_request(&[0x2a; 20]).unwrap();
        let packets = decode_packet_lines(&request).unwrap();
        assert_eq!(
            packets,
            vec![
                PacketLine::Data(
                    format!("want {} ofs-delta side-band-64k\n", "2a".repeat(20)).into_bytes()
                ),
                PacketLine::Data(b"deepen 1\n".to_vec()),
                PacketLine::Flush,
                PacketLine::Data(b"done\n".to_vec()),
            ]
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
                if connection_index == 1 {
                    let packets = decode_packet_lines(&request[header_end..]).unwrap();
                    assert!(packets.contains(&PacketLine::Data(b"deepen 1\n".to_vec())));
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

            let want = read_packet_section(&mut stream).unwrap();
            assert!(
                decode_packet_lines(&want)
                    .unwrap()
                    .contains(&PacketLine::Data(b"deepen 1\n".to_vec()))
            );
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
