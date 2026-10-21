//! Nia-owned Git wire and object decoding primitives.
//!
//! Git object validation and checkout live behind this boundary. Package graph
//! code consumes filesystem roots and never sees Git object representations.

use std::io;
#[cfg(feature = "test-support")]
use std::path::Path;
use std::path::PathBuf;

use cap_std::fs::Dir;

mod input;
mod object;
mod pktline;
mod transport;
use transport::git_tcp::checkout_git;
use transport::http::checkout_http;
use transport::ssh::checkout_ssh;

#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    pub fn packet_lines(input: &[u8]) {
        let _ = crate::pktline::decode_packet_lines(input);
    }

    pub fn loose_object(input: &[u8]) {
        crate::object::fuzz_loose_object(input);
    }

    pub fn delta(input: &[u8]) {
        crate::object::fuzz_delta(input);
    }

    pub fn pack_object(input: &[u8]) {
        if let Some((wanted, pack)) = input.split_at_checked(20) {
            let wanted: &[u8; 20] = wanted.try_into().expect("split at twenty bytes");
            crate::object::fuzz_pack_object(pack, wanted);
        }
    }

    pub fn http_response(input: &[u8]) {
        crate::transport::fuzz_http_response(input);
    }

    pub fn ssh_agent_identities(input: &[u8]) {
        crate::transport::ssh::fuzz_agent_identities(input);
    }

    pub fn ssh_kexinit(input: &[u8]) {
        crate::transport::ssh::fuzz_kexinit(input);
    }

    pub fn openssh_identity(input: &[u8]) {
        crate::transport::ssh::fuzz_openssh_identity(input);
    }

    pub fn known_hosts(input: &[u8]) {
        crate::transport::ssh::fuzz_known_hosts(input);
    }
}

fn invalid_packet(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Checks out a commit into an already opened, empty directory without Git metadata.
///
/// All output remains relative to the directory handle, even if its original
/// path is later replaced. On failure, a partial tree may remain for the caller
/// to discard.
pub fn checkout_source(source: &str, revision: &str, destination_dir: &Dir) -> io::Result<String> {
    if destination_dir.entries()?.next().transpose()?.is_some() {
        return Err(invalid_packet("Git checkout destination must be empty"));
    }
    if let Some(repository) = source.strip_prefix("file:///") {
        let repository = if cfg!(windows) && repository.as_bytes().get(1) == Some(&b':') {
            PathBuf::from(repository)
        } else {
            PathBuf::from(format!("/{repository}"))
        };
        return transport::local::checkout_local_into(&repository, revision, destination_dir);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return checkout_http(source, revision, destination_dir);
    }
    if source.starts_with("git://") {
        return checkout_git(source, revision, destination_dir);
    }
    if source.starts_with("ssh://") || source.starts_with("git@") {
        return checkout_ssh(source, revision, destination_dir);
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("Git transport is not implemented for `{source}`"),
    ))
}

/// Fetches a bounded public HTTPS resource with the Git transport's TLS and
/// proxy policy, without sending Git credential-helper credentials.
pub fn fetch_https(source: &str) -> io::Result<Vec<u8>> {
    transport::http::fetch_https(source)
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
    transport::ssh::fixture::restrict_to_current_user(path);
}

#[cfg(feature = "test-support")]
pub fn with_test_ssh_checkout_fixture(callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static) {
    with_test_ssh_checkout_fixture_mutating_repository(|_| {}, callback);
}

#[cfg(feature = "test-support")]
pub fn with_test_ssh_checkout_fixture_mutating_repository(
    mutator: impl FnOnce(&Path) + 'static,
    callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static,
) {
    transport::ssh::fixture::with_checkout_fixture(mutator, |root, source, config, repo, _| {
        callback(root, source, config, repo);
    });
}
