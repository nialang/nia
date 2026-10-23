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

    // Interleaving IGNOREs between banners shares one budget instead of
    // resetting it for each banner.
    let mut ignore = vec![2];
    encode_ssh_string(b"", &mut ignore).unwrap();
    let mut sender = direction();
    let mut interleaved = Vec::new();
    for _ in 0..=MAX_SSH_CONTROL_MESSAGES / 2 {
        write_encrypted_ssh_packet(&mut interleaved, &mut sender, &ignore).unwrap();
        write_encrypted_ssh_packet(&mut interleaved, &mut sender, &banner).unwrap();
    }
    write_encrypted_ssh_packet(&mut interleaved, &mut sender, &[52]).unwrap();
    let mut receiver = direction();
    assert_eq!(
        read_userauth_response(&mut interleaved.as_slice(), &mut receiver)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

/// A server side scripted by a closure that sees each client request and
/// returns the reply packets, run over an in-memory transport.
struct ScriptedServer<F> {
    client: SshAes128CtrDirection,
    server: SshAes128CtrDirection,
    written: Vec<u8>,
    pending: Vec<u8>,
    reply: F,
    requests: Vec<Vec<u8>>,
}

impl<F: FnMut(&[u8]) -> Vec<Vec<u8>>> Read for ScriptedServer<F> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "scripted server has nothing to send",
            ));
        }
        let length = output.len().min(self.pending.len());
        output[..length].copy_from_slice(&self.pending[..length]);
        self.pending.drain(..length);
        Ok(length)
    }
}

impl<F: FnMut(&[u8]) -> Vec<Vec<u8>>> Write for ScriptedServer<F> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(input);
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Every client packet is written and flushed whole, so decrypt it
        // and queue the scripted replies.
        while !self.written.is_empty() {
            let mut cursor = io::Cursor::new(std::mem::take(&mut self.written));
            let request = self.client.read_packet(&mut cursor)?.payload;
            let position = cursor.position() as usize;
            self.written = cursor.into_inner().split_off(position);
            for reply in (self.reply)(&request) {
                write_encrypted_ssh_packet(&mut self.pending, &mut self.server, &reply)?;
            }
            self.requests.push(request);
        }
        Ok(())
    }
}

/// Runs user authentication against `reply`, after the service exchange,
/// and returns the result with every request after the service request.
fn authenticate_against(
    identities: &mut [Box<dyn SshSigner>],
    notes: &[String],
    mut reply: impl FnMut(&[u8]) -> Vec<Vec<u8>>,
) -> (io::Result<()>, Vec<Vec<u8>>) {
    let direction = |seed| SshAes128CtrDirection::new(&[seed; 16], &[2; 16], &[3; 32], 0).unwrap();
    let mut server = ScriptedServer {
        client: direction(1),
        server: direction(4),
        written: Vec::new(),
        pending: Vec::new(),
        reply: move |request: &[u8]| {
            if request[0] == 5 {
                let mut accept = vec![6];
                encode_ssh_string(b"ssh-userauth", &mut accept).unwrap();
                return vec![accept];
            }
            reply(request)
        },
        requests: Vec::new(),
    };
    let mut keys = SshTransportKeys {
        outbound: direction(1),
        inbound: direction(4),
        session_id: [5; 32],
    };
    let result = authenticate_publickey(&mut server, &mut keys, "git", identities, notes);
    server.requests.remove(0);
    (result, server.requests)
}

fn auth_failure(methods: &str, partial: bool) -> Vec<u8> {
    let mut failure = vec![51];
    encode_ssh_string(methods.as_bytes(), &mut failure).unwrap();
    failure.push(u8::from(partial));
    failure
}

fn pk_ok_for(request: &[u8]) -> Vec<u8> {
    let mut reader = SshReader::new(&request[1..]);
    for _ in 0..3 {
        reader.string().unwrap();
    }
    reader.boolean().unwrap();
    let mut ok = vec![60];
    encode_ssh_string(reader.string().unwrap(), &mut ok).unwrap();
    encode_ssh_string(reader.string().unwrap(), &mut ok).unwrap();
    ok
}

fn is_signed(request: &[u8]) -> bool {
    let mut reader = SshReader::new(&request[1..]);
    for _ in 0..3 {
        reader.string().unwrap();
    }
    reader.boolean().unwrap()
}

struct FailingSigner(Vec<u8>);

impl SshSigner for FailingSigner {
    fn public_key_blob(&self) -> &[u8] {
        &self.0
    }

    fn signature_algorithm(&self) -> &[u8] {
        b"ssh-ed25519"
    }

    fn sign_message(&mut self, _: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "SSH agent refused the public-key signing request",
        ))
    }
}

#[test]
fn ssh_auth_moves_to_the_next_identity_when_signing_fails() {
    let mut identities: Vec<Box<dyn SshSigner>> = vec![
        Box::new(FailingSigner(fixture::ed25519_public_key_blob(&[1; 32]))),
        agent_key(2),
    ];
    let (result, requests) = authenticate_against(&mut identities, &[], |request| {
        if is_signed(request) {
            vec![vec![52]]
        } else {
            vec![pk_ok_for(request)]
        }
    });
    result.unwrap();
    // Query 1, query 2, signed 2: the failed key never sent a request.
    assert_eq!(
        requests
            .iter()
            .map(|request| is_signed(request))
            .collect::<Vec<_>>(),
        [false, false, true]
    );
}

#[test]
fn ssh_auth_rejects_pk_ok_for_a_different_key_or_algorithm() {
    for tamper in [
        |ok: &mut Vec<u8>| *ok.last_mut().unwrap() ^= 1,
        |ok: &mut Vec<u8>| ok[5] = b'x',
    ] {
        let mut identities = vec![agent_key(1)];
        let (result, requests) = authenticate_against(&mut identities, &[], |request| {
            let mut ok = pk_ok_for(request);
            tamper(&mut ok);
            vec![ok]
        });
        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("different public key"),
            "{error}"
        );
        assert!(requests.iter().all(|request| !is_signed(request)));
    }
}

#[test]
fn ssh_auth_disconnect_keeps_the_reason_and_the_keys_tried() {
    let mut identities = vec![agent_key(1), agent_key(2), agent_key(3)];
    let notes = ["the identity file was not offered: example".to_string()];
    let mut queries = 0;
    let (result, requests) = authenticate_against(&mut identities, &notes, |_| {
        queries += 1;
        if queries < 2 {
            return vec![auth_failure("publickey", false)];
        }
        let mut disconnect = vec![1];
        disconnect.extend_from_slice(&2u32.to_be_bytes());
        encode_ssh_string(b"Too many authentication failures", &mut disconnect).unwrap();
        encode_ssh_string(b"", &mut disconnect).unwrap();
        vec![disconnect]
    });
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let message = error.to_string();
    assert!(
        message.contains("Too many authentication failures"),
        "{message}"
    );
    let first = ssh_key_fingerprint(&fixture::ed25519_public_key_blob(&[1; 32]));
    let second = ssh_key_fingerprint(&fixture::ed25519_public_key_blob(&[2; 32]));
    assert!(
        message.contains(&format!("{first} (rejected)")),
        "{message}"
    );
    assert!(
        message.contains(&format!("{second} (interrupted)")),
        "{message}"
    );
    assert!(
        message.contains("identity file was not offered"),
        "{message}"
    );
    assert_eq!(requests.len(), 2);
}

#[test]
fn ssh_auth_stops_on_partial_success_and_when_publickey_is_withdrawn() {
    let mut identities = vec![agent_key(1), agent_key(2)];
    let (result, requests) = authenticate_against(&mut identities, &[], |request| {
        if is_signed(request) {
            vec![auth_failure("keyboard-interactive", true)]
        } else {
            vec![pk_ok_for(request)]
        }
    });
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("requires further authentication"),
        "{message}"
    );
    assert!(message.contains("(accepted)"), "{message}");
    assert_eq!(requests.len(), 2, "the second key must not be offered");

    let mut identities = vec![agent_key(1), agent_key(2)];
    let (result, requests) = authenticate_against(&mut identities, &[], |_| {
        vec![auth_failure("password", false)]
    });
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("no longer accepts public-key"),
        "{message}"
    );
    assert_eq!(requests.len(), 1);
}

#[test]
fn unusable_agent_falls_back_to_the_identity_file() {
    fixture::with_checkout_fixture(
        |_| {},
        |root, source, config, _, _| {
            let destination = root.join("checkout");
            std::fs::create_dir(&destination).unwrap();
            let destination_dir = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
            let agent = Err(io::Error::new(io::ErrorKind::TimedOut, "agent timed out"));
            checkout_ssh_with_config(source, "HEAD", &destination_dir, config, agent).unwrap();
        },
    );

    // Without an identity file, the error says why the agent was not used.
    let directory = nia_test_support::test_dir("git-ssh-no-identity");
    let agent = Err(io::Error::new(io::ErrorKind::TimedOut, "agent timed out"));
    let error = match ssh_identity_candidates(agent, &directory) {
        Ok(_) => panic!("no identity is available"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("agent timed out"), "{error}");
}

#[test]
fn invalid_urls_fail_before_the_agent_is_contacted() {
    let destination = nia_test_support::test_dir("git-ssh-invalid-agent");
    let destination_dir = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
    let error = checkout_ssh_with_config_into(
        "ssh://example.test:0/repository.git",
        "HEAD",
        &destination_dir,
        &destination,
        || panic!("the agent must not be contacted for an invalid URL"),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
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
                checkout_ssh_with_config(source, "HEAD", &destination_dir, config, Ok(Vec::new()))
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
                Ok(agent),
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
