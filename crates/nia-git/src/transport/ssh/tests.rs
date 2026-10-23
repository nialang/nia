// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::checkout_source;
use cap_std::{ambient_authority, fs::Dir};

#[test]
fn encrypted_service_read_validates_and_skips_ignore_and_debug_messages() {
    let direction = || SshAes128CtrDirection::new(&[1; 16], &[2; 16], &[3; 32], 0).unwrap();
    let mut sender = direction();
    let mut input = Vec::new();
    let mut ignore = vec![2];
    encode_ssh_string(b"notice", &mut ignore).unwrap();
    write_encrypted_ssh_packet(&mut input, &mut sender, &ignore).unwrap();
    let mut debug = vec![4, 1];
    encode_ssh_string(b"diagnostic", &mut debug).unwrap();
    encode_ssh_string(b"en", &mut debug).unwrap();
    write_encrypted_ssh_packet(&mut input, &mut sender, &debug).unwrap();
    let mut accepted = vec![6];
    encode_ssh_string(b"ssh-userauth", &mut accepted).unwrap();
    write_encrypted_ssh_packet(&mut input, &mut sender, &accepted).unwrap();
    let mut receiver = direction();
    assert_eq!(
        read_expected_encrypted_ssh_message(&mut input.as_slice(), &mut receiver, 6).unwrap(),
        accepted
    );

    let mut sender = direction();
    let mut malformed = Vec::new();
    write_encrypted_ssh_packet(&mut malformed, &mut sender, &[4, 2]).unwrap();
    let mut receiver = direction();
    assert_eq!(
        read_expected_encrypted_ssh_message_any(&mut malformed.as_slice(), &mut receiver)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );

    let mut sender = direction();
    let mut excessive = Vec::new();
    for _ in 0..=MAX_SSH_CONTROL_MESSAGES {
        let mut ignore = vec![2];
        encode_ssh_string(b"", &mut ignore).unwrap();
        write_encrypted_ssh_packet(&mut excessive, &mut sender, &ignore).unwrap();
    }
    write_encrypted_ssh_packet(&mut excessive, &mut sender, &[52]).unwrap();
    let mut receiver = direction();
    assert_eq!(
        read_expected_encrypted_ssh_message_any(&mut excessive.as_slice(), &mut receiver)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn userauth_reads_skip_bounded_banners() {
    let direction = || SshAes128CtrDirection::new(&[1; 16], &[2; 16], &[3; 32], 0).unwrap();
    let mut banner = vec![53];
    encode_ssh_string(b"Authorized use only", &mut banner).unwrap();
    encode_ssh_string(b"en", &mut banner).unwrap();

    let mut sender = direction();
    let mut input = Vec::new();
    write_encrypted_ssh_packet(&mut input, &mut sender, &banner).unwrap();
    write_encrypted_ssh_packet(&mut input, &mut sender, &[52]).unwrap();
    let mut receiver = direction();
    assert_eq!(
        read_userauth_response(&mut input.as_slice(), &mut receiver).unwrap(),
        [52]
    );

    // A banner missing its language tag is malformed, not skipped.
    let mut sender = direction();
    let mut malformed = Vec::new();
    write_encrypted_ssh_packet(&mut malformed, &mut sender, &banner[..banner.len() - 6]).unwrap();
    let mut receiver = direction();
    assert_eq!(
        read_userauth_response(&mut malformed.as_slice(), &mut receiver)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );

    let mut sender = direction();
    let mut excessive = Vec::new();
    for _ in 0..=MAX_SSH_CONTROL_MESSAGES {
        write_encrypted_ssh_packet(&mut excessive, &mut sender, &banner).unwrap();
    }
    write_encrypted_ssh_packet(&mut excessive, &mut sender, &[52]).unwrap();
    let mut receiver = direction();
    assert_eq!(
        read_userauth_response(&mut excessive.as_slice(), &mut receiver)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn ssh_checkout_does_not_publish_a_pack_when_remote_process_fails() {
    fixture::with_checkout_fixture_status(
        17,
        |_| {},
        |root, source, config, _, _| {
            let destination = root.join("failed-checkout");
            std::fs::create_dir(&destination).unwrap();
            let destination_dir = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
            let error =
                checkout_ssh_with_config(source, "HEAD", &destination_dir, config, Vec::new())
                    .unwrap_err();
            assert!(error.to_string().contains("status 17"), "{error}");
            assert!(std::fs::read_dir(destination).unwrap().next().is_none());
        },
    );
}

fn agent_key(seed: u8) -> Box<dyn SshSigner> {
    Box::new(identity::SshEd25519Identity {
        public_key: fixture::ed25519_public_key_blob(&[seed; 32]),
        private_key: ring::signature::Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap(),
    })
}

/// Checks out from a server that accepts only `accepted_seed`, offering the
/// given agent keys before the fixture's `id_ed25519`.
fn checkout_offering(
    agent_seeds: &[u8],
    accepted_seed: &[u8; 32],
) -> (io::Result<String>, fixture::SshFixtureLog) {
    let mut result = None;
    let log = fixture::with_checkout_fixture_accepting(
        0,
        accepted_seed,
        |_| {},
        |root, source, config, _, _| {
            let destination = root.join("checkout");
            std::fs::create_dir(&destination).unwrap();
            let destination_dir = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
            let agent = agent_seeds.iter().map(|&seed| agent_key(seed)).collect();
            result = Some(checkout_ssh_with_config(
                source,
                "HEAD",
                &destination_dir,
                config,
                agent,
            ));
        },
    );
    (result.unwrap(), log)
}

#[test]
fn ssh_auth_tries_agent_keys_in_order_and_signs_only_the_accepted_one() {
    let (result, log) = checkout_offering(&[1, 2, 3], &[2; 32]);
    result.unwrap();
    assert_eq!(
        log.offered,
        [
            fixture::ed25519_public_key_blob(&[1; 32]),
            fixture::ed25519_public_key_blob(&[2; 32]),
        ]
    );
    assert_eq!(log.signed, [fixture::ed25519_public_key_blob(&[2; 32])]);
}

#[test]
fn ssh_auth_falls_back_to_the_identity_file_after_agent_keys() {
    let file_key = fixture::ed25519_public_key_blob(&fixture::FIXTURE_IDENTITY_SEED);
    let (result, log) = checkout_offering(&[1], &fixture::FIXTURE_IDENTITY_SEED);
    result.unwrap();
    assert_eq!(
        log.offered,
        [fixture::ed25519_public_key_blob(&[1; 32]), file_key.clone()]
    );
    assert_eq!(log.signed, std::slice::from_ref(&file_key));

    // An agent that already holds the file's key offers it once, even when
    // the server rejects every key and the whole list is tried.
    let (result, log) = checkout_offering(&[fixture::FIXTURE_IDENTITY_SEED[0], 1], &[9; 32]);
    assert!(result.is_err());
    assert_eq!(
        log.offered,
        [file_key, fixture::ed25519_public_key_blob(&[1; 32])]
    );
}

#[test]
fn ssh_auth_stops_below_max_auth_tries_and_names_rejected_keys() {
    let (result, log) = checkout_offering(&[1, 2, 3, 4, 5, 6], &[9; 32]);
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(log.offered.len(), MAX_SSH_IDENTITY_ATTEMPTS);
    assert!(log.signed.is_empty(), "rejected keys must not be signed");
    let message = error.to_string();
    for seed in 1..=5 {
        let fingerprint = ssh_key_fingerprint(&fixture::ed25519_public_key_blob(&[seed; 32]));
        assert!(message.contains(&fingerprint), "{message}");
    }
    assert!(
        message.contains("2 more identities were not offered"),
        "{message}"
    );
}

#[test]
fn ssh_key_fingerprints_match_ssh_keygen() {
    let mut blob = Vec::new();
    encode_ssh_string(b"ssh-ed25519", &mut blob).unwrap();
    encode_ssh_string(&[7; 32], &mut blob).unwrap();
    // `ssh-keygen -lf` on the same public key.
    assert_eq!(
        ssh_key_fingerprint(&blob),
        "ssh-ed25519 SHA256:gNSIRW+2Iyiuvsdp/bgjy38bvWHw6wQm3tuoXrl3WjQ"
    );
}

#[test]
fn ssh_publickey_query_carries_no_signature() {
    let signer = agent_key(4);
    let query = build_publickey_query("git", signer.as_ref()).unwrap();
    let mut expected = vec![50];
    for field in [b"git".as_slice(), b"ssh-connection", b"publickey"] {
        encode_ssh_string(field, &mut expected).unwrap();
    }
    expected.push(0);
    encode_ssh_string(b"ssh-ed25519", &mut expected).unwrap();
    encode_ssh_string(signer.public_key_blob(), &mut expected).unwrap();
    assert_eq!(query, expected);
}

#[test]
fn ssh_sources_reject_invalid_urls_before_loading_credentials() {
    let destination = nia_test_support::test_dir("git-ssh-invalid");
    let destination_dir = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
    for source in [
        "ssh://user:password@example.test/repository.git",
        "ssh://example.test/repository.git?query",
        "ssh://example.test:0/repository.git",
        "git@example.test:",
    ] {
        assert!(
            checkout_source(source, "HEAD", &destination_dir).is_err(),
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
fn ssh_publickey_auth_uses_the_signers_signature_algorithm() {
    fn read_string<'a>(bytes: &'a [u8], offset: &mut usize) -> &'a [u8] {
        let length = u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
        *offset += 4;
        let value = &bytes[*offset..*offset + length];
        *offset += length;
        value
    }

    struct RsaSigner {
        public_key: Vec<u8>,
    }
    impl SshSigner for RsaSigner {
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
    let request = build_publickey_auth_request("git", &[3; 32], &mut signer).unwrap();
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
