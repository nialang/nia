// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::checkout_source;

#[test]
fn ssh_checkout_does_not_publish_a_pack_when_remote_process_fails() {
    fixture::with_checkout_fixture_status(
        17,
        |_| {},
        |root, source, config, _, _| {
            let destination = root.join("failed-checkout");
            let error =
                checkout_ssh_with_config(source, "HEAD", &destination, config, false).unwrap_err();
            assert!(error.to_string().contains("status 17"), "{error}");
            assert!(!destination.exists());
        },
    );
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
