//! Nia-owned Git wire and object decoding primitives.
//!
//! Git object validation and checkout live behind this boundary. Package graph
//! code consumes filesystem roots and never sees Git object representations.

use std::io;
use std::path::{Path, PathBuf};

mod input;
mod object;
mod pktline;
mod transport;
use transport::git_tcp::checkout_git;
use transport::http::checkout_http;
use transport::local::checkout_local;
use transport::ssh::checkout_ssh;

fn invalid_packet(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
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
