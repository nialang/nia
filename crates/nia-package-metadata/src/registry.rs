// SPDX-License-Identifier: GPL-3.0-or-later
//! Strict registry index records and parsing.

use std::io;

use serde::Deserialize;

use crate::{PackageId, PackageVersion};

const INDEX_SCHEMA_VERSION: u32 = 1;
const MAX_INDEX_BYTES: usize = 1024 * 1024;
const MAX_INDEX_PACKAGES: usize = 100_000;

/// One validated versioned registry index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIndex {
    pub packages: Vec<RegistryIndexPackage>,
}

/// Immutable archive record for one published package release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIndexPackage {
    pub package: PackageId,
    pub archive_url: String,
    pub archive_digest: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistryIndex {
    schema_version: u32,
    packages: Vec<RawRegistryIndexPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistryIndexPackage {
    namespace: String,
    name: String,
    version: String,
    archive_url: String,
    archive_digest: String,
}

impl RegistryIndex {
    /// Parses a bounded v1 TOML index and rejects noncanonical record order.
    pub fn parse_toml(input: &[u8]) -> io::Result<Self> {
        if input.len() > MAX_INDEX_BYTES {
            return Err(invalid_index("registry index exceeds the size limit"));
        }
        let raw: RawRegistryIndex = toml::from_slice(input).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid registry index: {error}"),
            )
        })?;
        if raw.schema_version != INDEX_SCHEMA_VERSION {
            return Err(invalid_index("unsupported registry index schema version"));
        }
        if raw.packages.len() > MAX_INDEX_PACKAGES {
            return Err(invalid_index("registry index has too many package records"));
        }

        let mut packages = Vec::with_capacity(raw.packages.len());
        let mut previous: Option<(String, String, String)> = None;
        for record in raw.packages {
            if record.namespace.is_empty()
                || record.namespace.len() > 128
                || record.name.is_empty()
                || record.name.len() > 128
                || record.namespace.contains('/')
                || record.name.contains('/')
            {
                return Err(invalid_index(
                    "registry index contains an invalid package identity",
                ));
            }
            let version = PackageVersion::parse(&record.version).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("registry index contains an invalid SemVer version: {error}"),
                )
            })?;
            let key = (
                record.namespace.clone(),
                record.name.clone(),
                record.version.clone(),
            );
            if previous.as_ref().is_some_and(|previous| previous >= &key) {
                return Err(invalid_index(
                    "registry index package records must be uniquely sorted",
                ));
            }
            previous = Some(key);
            validate_archive_url(&record.archive_url)?;
            let archive_digest = parse_blake3_digest(&record.archive_digest)?;
            packages.push(RegistryIndexPackage {
                package: PackageId {
                    namespace: record.namespace,
                    name: record.name,
                    version,
                },
                archive_url: record.archive_url,
                archive_digest,
            });
        }
        Ok(Self { packages })
    }
}

fn validate_archive_url(url: &str) -> io::Result<()> {
    let Some(authority_and_path) = url.strip_prefix("https://") else {
        return Err(invalid_index("registry archive URL must use HTTPS"));
    };
    let (authority, path) = authority_and_path
        .split_once('/')
        .unwrap_or((authority_and_path, ""));
    if authority.is_empty()
        || authority.contains('@')
        || url.contains(['?', '#'])
        || path
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace() || byte == b'\\')
    {
        return Err(invalid_index("registry archive URL is invalid"));
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or_else(|| invalid_index("registry archive URL is invalid"))?;
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(invalid_index("registry archive URL is invalid"));
        }
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| invalid_index("registry archive URL is invalid"))?,
            )
        };
        (host, port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return Err(invalid_index("registry archive URL is invalid"));
        }
        (host, Some(port))
    } else {
        (authority, None)
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err(invalid_index("registry archive URL is invalid"));
    }
    if let Some(port) = port {
        if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_index("registry archive URL is invalid"));
        }
        if port.parse::<u16>().map_or(true, |port| port == 0) {
            return Err(invalid_index("registry archive URL is invalid"));
        }
    }
    Ok(())
}

fn parse_blake3_digest(value: &str) -> io::Result<[u8; 32]> {
    let Some(hex) = value.strip_prefix("blake3:") else {
        return Err(invalid_index("registry archive digest must use blake3"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid_index("registry archive digest is malformed"));
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .map_err(|_| invalid_index("registry archive digest is malformed"))?;
    }
    Ok(digest)
}

fn invalid_index(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(namespace: &str, name: &str, version: &str, url: &str) -> String {
        format!(
            "[[packages]]\nnamespace = \"{namespace}\"\nname = \"{name}\"\nversion = \"{version}\"\narchive_url = \"{url}\"\narchive_digest = \"blake3:{}\"\n",
            "ab".repeat(32)
        )
    }

    #[test]
    fn registry_index_parses_sorted_https_archive_records() {
        let input = format!(
            "schema_version = 1\n{}{}",
            record(
                "acme",
                "json",
                "1.0.0",
                "https://cdn.example.test/json-1.0.0.nia"
            ),
            record(
                "acme",
                "json",
                "1.1.0",
                "https://cdn.example.test/json-1.1.0.nia"
            )
        );
        let index = RegistryIndex::parse_toml(input.as_bytes()).unwrap();
        assert_eq!(index.packages.len(), 2);
        assert_eq!(index.packages[1].package.version.as_str(), "1.1.0");
        assert_eq!(index.packages[0].archive_digest, [0xab; 32]);
    }

    #[test]
    fn registry_index_fails_closed_on_schema_order_url_and_digest_errors() {
        assert!(RegistryIndex::parse_toml(b"schema_version = 2\npackages = []\n").is_err());
        assert!(
            RegistryIndex::parse_toml(b"schema_version = 1\npackages = []\nunknown = 1\n").is_err()
        );
        let unsorted = format!(
            "schema_version = 1\n{}{}",
            record("acme", "json", "2.0.0", "https://cdn.example.test/json"),
            record("acme", "json", "1.0.0", "https://cdn.example.test/json")
        );
        assert!(RegistryIndex::parse_toml(unsorted.as_bytes()).is_err());
        for (url, digest) in [
            (
                "http://cdn.example.test/json",
                format!("blake3:{}", "ab".repeat(32)),
            ),
            (
                "https://user@cdn.example.test/json",
                format!("blake3:{}", "ab".repeat(32)),
            ),
            (
                "https://cdn.example.test/json?mutable=1",
                format!("blake3:{}", "ab".repeat(32)),
            ),
            ("https://cdn.example.test/json", "sha256:deadbeef".into()),
        ] {
            let input = format!(
                "schema_version = 1\n{}",
                record("acme", "json", "1.0.0", url)
                    .replace(&format!("blake3:{}", "ab".repeat(32)), &digest)
            );
            assert!(RegistryIndex::parse_toml(input.as_bytes()).is_err());
        }
    }
}
