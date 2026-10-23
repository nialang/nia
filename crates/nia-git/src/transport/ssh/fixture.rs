// SPDX-License-Identifier: GPL-3.0-or-later
//! Local SSH server and repository fixture shared by checkout and build tests.

use super::kex::*;
use super::wire::*;
use super::*;
use crate::pktline::{PacketLine, decode_packet_lines, encode_packet_line};
use ring::signature::KeyPair;

/// Windows equivalent of `chmod 600` for a fixture private key: drop
/// inherited ACEs and grant only the current user.
#[cfg(windows)]
pub(crate) fn restrict_to_current_user(path: &Path) {
    let user = std::env::var("USERNAME").expect("USERNAME is set on Windows");
    let output = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
        .output()
        .expect("icacls is part of Windows");
    assert!(output.status.success(), "{output:?}");
}

/// Seed of the fixture's `id_ed25519`, which the server accepts by default.
pub(super) const FIXTURE_IDENTITY_SEED: [u8; 32] = [37; 32];

/// Public-key blobs the client offered without and with a signature, in order.
#[derive(Debug)]
pub(super) struct SshFixtureLog {
    client_proposals: SshKexInit,
    pub(super) offered: Vec<Vec<u8>>,
    pub(super) signed: Vec<Vec<u8>>,
}

pub(super) fn ed25519_public_key_blob(seed: &[u8; 32]) -> Vec<u8> {
    let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
    let mut blob = Vec::new();
    encode_ssh_string(b"ssh-ed25519", &mut blob).unwrap();
    encode_ssh_string(key_pair.public_key().as_ref(), &mut blob).unwrap();
    blob
}

pub(crate) fn with_checkout_fixture(
    mutator: impl FnOnce(&Path),
    callback: impl FnOnce(&Path, &str, &Path, &Path, &str),
) {
    with_checkout_fixture_status(0, mutator, callback);
}

pub(super) fn with_checkout_fixture_status(
    exit_status: u32,
    mutator: impl FnOnce(&Path),
    callback: impl FnOnce(&Path, &str, &Path, &Path, &str),
) {
    with_checkout_fixture_accepting(exit_status, &FIXTURE_IDENTITY_SEED, mutator, callback);
}

/// Runs the fixture with a server that authorizes only the Ed25519 key from
/// `accepted_seed`, and returns what the client offered.
pub(super) fn with_checkout_fixture_accepting(
    exit_status: u32,
    accepted_seed: &[u8; 32],
    mutator: impl FnOnce(&Path),
    callback: impl FnOnce(&Path, &str, &Path, &Path, &str),
) -> SshFixtureLog {
    let accepted_key = ed25519_public_key_blob(accepted_seed);
    use std::net::TcpListener;
    use std::process::{Command, Stdio};

    fn put_string(value: &[u8], output: &mut Vec<u8>) {
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(value);
    }

    fn take_string<'a>(bytes: &'a [u8], offset: &mut usize) -> &'a [u8] {
        let length = u32::from_be_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
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
        "[package]\nversion = \"1.0.0\"\nname = \"sshdep\"\n[library]\npath = \"src/lib.nia\"\n",
    )
    .unwrap();
    std::fs::write(
        repository.join("src/lib.nia"),
        b"pub const value: u8 = 7;\n",
    )
    .unwrap();
    mutator(&repository);
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
        let mut ignore = vec![2];
        put_string(b"pre-kex notification", &mut ignore);
        write_plain_ssh_packet(&mut stream, &ignore).unwrap();

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
            SshAes128CtrDirection::new(&derive(b'C', 16), &derive(b'A', 16), &derive(b'E', 32), 3)
                .unwrap();
        let mut outbound =
            SshAes128CtrDirection::new(&derive(b'D', 16), &derive(b'B', 16), &derive(b'F', 32), 4)
                .unwrap();

        let service_request = inbound.read_packet(&mut stream).unwrap().payload;
        assert_eq!(service_request[0], 5);
        let mut offset = 1;
        assert_eq!(take_string(&service_request, &mut offset), b"ssh-userauth");
        let mut service_accept = vec![6];
        put_string(b"ssh-userauth", &mut service_accept);
        write_encrypted_ssh_packet(&mut stream, &mut outbound, &service_accept).unwrap();

        // Answer unsigned queries with PK_OK only for the accepted key, as an
        // OpenSSH server with one authorized key would.
        let mut log = SshFixtureLog {
            client_proposals,
            offered: Vec::new(),
            signed: Vec::new(),
        };
        let mut failure = vec![51];
        put_string(b"publickey", &mut failure);
        failure.push(0);
        loop {
            // A client that has run out of identities closes the connection.
            let Ok(packet) = inbound.read_packet(&mut stream) else {
                return log;
            };
            let auth_request = packet.payload;
            assert_eq!(auth_request[0], 50);
            if log.offered.is_empty() {
                // Servers with a `Banner` setting send it before the first
                // authentication reply.
                let mut banner = vec![53];
                put_string(b"Authorized use only\r\n", &mut banner);
                put_string(b"en", &mut banner);
                write_encrypted_ssh_packet(&mut stream, &mut outbound, &banner).unwrap();
            }
            let mut offset = 1;
            assert_eq!(take_string(&auth_request, &mut offset), b"git");
            assert_eq!(take_string(&auth_request, &mut offset), b"ssh-connection");
            assert_eq!(take_string(&auth_request, &mut offset), b"publickey");
            let has_signature = auth_request[offset];
            offset += 1;
            let algorithm = take_string(&auth_request, &mut offset).to_vec();
            let public_key_blob = take_string(&auth_request, &mut offset).to_vec();
            if has_signature == 0 {
                assert_eq!(offset, auth_request.len());
                log.offered.push(public_key_blob.clone());
                if public_key_blob == accepted_key {
                    let mut ok = vec![60];
                    put_string(&algorithm, &mut ok);
                    put_string(&public_key_blob, &mut ok);
                    write_encrypted_ssh_packet(&mut stream, &mut outbound, &ok).unwrap();
                } else {
                    write_encrypted_ssh_packet(&mut stream, &mut outbound, &failure).unwrap();
                }
                continue;
            }
            assert_eq!(has_signature, 1);
            assert_eq!(algorithm, b"ssh-ed25519");
            let signature_field_offset = offset;
            let signature_blob = take_string(&auth_request, &mut offset);
            assert_eq!(offset, auth_request.len());
            log.signed.push(public_key_blob.clone());
            let mut sig_offset = 0;
            assert_eq!(take_string(signature_blob, &mut sig_offset), b"ssh-ed25519");
            let signature = take_string(signature_blob, &mut sig_offset);
            let mut signed = Vec::new();
            put_string(&session_id, &mut signed);
            signed.extend_from_slice(&auth_request[..signature_field_offset]);
            let mut key_offset = 0;
            assert_eq!(
                take_string(&public_key_blob, &mut key_offset),
                b"ssh-ed25519"
            );
            let public_key = take_string(&public_key_blob, &mut key_offset);
            assert_eq!(key_offset, public_key_blob.len());
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
                .verify(&signed, signature)
                .unwrap();
            if public_key_blob != accepted_key {
                write_encrypted_ssh_packet(&mut stream, &mut outbound, &failure).unwrap();
                continue;
            }
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &[52]).unwrap();
            break;
        }

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
        encode_packet_line(b"\x02Counting objects\r", &mut git_response).unwrap();
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
        let mut status = vec![98];
        status.extend_from_slice(&client_channel.to_be_bytes());
        put_string(b"exit-status", &mut status);
        status.push(0);
        status.extend_from_slice(&exit_status.to_be_bytes());
        write_encrypted_ssh_packet(&mut stream, &mut outbound, &status).unwrap();
        if exit_status == 0 {
            let mut close = vec![97];
            close.extend_from_slice(&client_channel.to_be_bytes());
            write_encrypted_ssh_packet(&mut stream, &mut outbound, &close).unwrap();
        }
        loop {
            let close = inbound.read_packet(&mut stream).unwrap().payload;
            if close[0] == 93 {
                continue;
            }
            assert_eq!(close, [97, 0, 0, 0, 7]);
            break;
        }
        log
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
        openssh_private_key(&FIXTURE_IDENTITY_SEED),
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
    let source = format!("ssh://git@127.0.0.1:{}/org/repo.git", address.port());
    callback(&root, &source, &ssh_config, &repository, &commit_id);
    let log = server.join().unwrap();
    assert!(
        log.client_proposals
            .kex_algorithms
            .contains(&"curve25519-sha256".to_string())
    );
    std::fs::remove_dir_all(root).unwrap();
    log
}
