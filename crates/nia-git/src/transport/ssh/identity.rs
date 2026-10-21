// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenSSH Ed25519 file identities and signing contract.

use super::read_limited_file;
use super::wire::{SshReader, decode_base64};
use crate::invalid_packet;
use ring::{signature, signature::KeyPair};
use std::io;
use std::path::Path;
use zeroize::Zeroizing;

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
        if !private_block.len().is_multiple_of(8) {
            return Err(invalid_packet("SSH private key block is not aligned"));
        }

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
        let padding = private_reader.take(private_reader.remaining())?;
        if padding.len() > 8
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

#[cfg(test)]
mod tests {
    use super::super::build_publickey_auth_request;
    use super::*;
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
}

pub(super) fn load_ssh_identity(ssh_directory: &Path) -> io::Result<SshEd25519Identity> {
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
    SshEd25519Identity::parse_openssh(&identity_bytes)
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
    use super::super::fixture::restrict_to_current_user;
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

#[cfg(test)]
mod properties {
    use super::super::wire::encode_ssh_string;
    use super::*;
    use crate::transport::http::base64_encode;
    use proptest::prelude::*;

    fn encoded_identity(seed: &[u8; 32], comment: &[u8], padding: Option<usize>) -> Vec<u8> {
        let pair = signature::Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
        let public = pair.public_key().as_ref();
        let mut public_blob = Vec::new();
        encode_ssh_string(b"ssh-ed25519", &mut public_blob).unwrap();
        encode_ssh_string(public, &mut public_blob).unwrap();
        let mut private = Vec::new();
        private.extend_from_slice(&7u32.to_be_bytes());
        private.extend_from_slice(&7u32.to_be_bytes());
        encode_ssh_string(b"ssh-ed25519", &mut private).unwrap();
        encode_ssh_string(public, &mut private).unwrap();
        encode_ssh_string(&[seed.as_slice(), public].concat(), &mut private).unwrap();
        encode_ssh_string(comment, &mut private).unwrap();
        let padding_len = padding.unwrap_or((8 - private.len() % 8) % 8);
        private.extend((0..padding_len).map(|index| (index as u8).wrapping_add(1)));
        let mut outer = b"openssh-key-v1\0".to_vec();
        for value in [b"none".as_slice(), b"none", b""] {
            encode_ssh_string(value, &mut outer).unwrap();
        }
        outer.extend_from_slice(&1u32.to_be_bytes());
        encode_ssh_string(&public_blob, &mut outer).unwrap();
        encode_ssh_string(&private, &mut outer).unwrap();
        let encoded = base64_encode(&outer);
        format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
            std::str::from_utf8(&encoded).unwrap()
        )
        .into_bytes()
    }

    #[test]
    fn oversized_private_key_padding_is_rejected() {
        let input = encoded_identity(&[1; 32], b"test", Some(256));
        assert!(SshEd25519Identity::parse_openssh(&input).is_err());
    }

    #[test]
    fn aligned_private_key_without_padding_is_accepted() {
        // Ed25519 fields consume 131 bytes before the comment. Five comment
        // bytes align the private block to the none cipher's eight-byte block.
        let input = encoded_identity(&[1; 32], b"12345", Some(0));
        assert!(SshEd25519Identity::parse_openssh(&input).is_ok());
    }

    proptest! {
        #[test]
        fn arbitrary_openssh_inputs_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let _ = SshEd25519Identity::parse_openssh(&bytes);
            let encoded = base64_encode(&bytes);
            let armored = format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n", std::str::from_utf8(&encoded).unwrap());
            let _ = SshEd25519Identity::parse_openssh(armored.as_bytes());
        }

        #[test]
        fn generated_openssh_identities_round_trip(seed in any::<[u8; 32]>(), comment in prop::collection::vec(any::<u8>(), 0..64)) {
            let input = encoded_identity(&seed, &comment, None);
            let identity = SshEd25519Identity::parse_openssh(&input).unwrap();
            let pair = signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
            signature::UnparsedPublicKey::new(&signature::ED25519, pair.public_key().as_ref())
                .verify(b"property test", identity.sign(b"property test").as_ref()).unwrap();
        }

        #[test]
        fn oversized_openssh_padding_is_rejected(padding in 9usize..512) {
            prop_assert!(SshEd25519Identity::parse_openssh(&encoded_identity(&[1; 32], b"test", Some(padding))).is_err());
        }
    }
}
