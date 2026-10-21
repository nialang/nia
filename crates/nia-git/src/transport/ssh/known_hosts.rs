// SPDX-License-Identifier: GPL-3.0-or-later
//! Trusted SSH host matching and revocation.

use super::wire::{SshReader, decode_base64};
use crate::invalid_packet;
use ring::hmac;
use std::io;

pub(super) const MAX_KNOWN_HOSTS_BYTES: usize = 16 * 1024 * 1024;

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

#[cfg(test)]
mod tests {
    use super::*;
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
}

#[cfg(test)]
mod properties {
    use super::super::wire::encode_ssh_string;
    use super::*;
    use crate::transport::http::base64_encode;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn arbitrary_known_hosts_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            if let Ok(hosts) = KnownHosts::parse(&bytes) {
                let _ = hosts.verify("example.test", 22, &[0; 32]);
            }
            // ASCII text reaches line, marker, and key parsing more often than raw bytes.
            let text: Vec<u8> = bytes.iter().map(|byte| byte & 0x7f).collect();
            let _ = KnownHosts::parse(&text);
        }

        #[test]
        fn known_host_key_and_revocation_are_enforced(key in any::<[u8; 32]>(), port in 1u16..=u16::MAX) {
            let mut blob = Vec::new();
            encode_ssh_string(b"ssh-ed25519", &mut blob).unwrap();
            encode_ssh_string(&key, &mut blob).unwrap();
            let encoded = base64_encode(&blob);
            let encoded = std::str::from_utf8(&encoded).unwrap();
            let host = if port == 22 { "example.test".to_owned() } else { format!("[example.test]:{port}") };
            let line = format!("{host} ssh-ed25519 {encoded}\n");
            let hosts = KnownHosts::parse(line.as_bytes()).unwrap();
            prop_assert!(hosts.verify("example.test", port, &blob).is_ok());
            let mut wrong = blob.clone();
            *wrong.last_mut().unwrap() ^= 1;
            prop_assert!(hosts.verify("example.test", port, &wrong).is_err());
            let revoked = KnownHosts::parse(format!("@revoked {line}").as_bytes()).unwrap();
            prop_assert!(revoked.verify("example.test", port, &blob).is_err());
        }
    }
}
