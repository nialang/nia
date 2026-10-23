// SPDX-License-Identifier: GPL-3.0-or-later
//! Checkout through the public API using real repositories and local wire servers.

use cap_std::{ambient_authority, fs::Dir};
use nia_git::checkout_source;
use std::{
    io::{Read, Write},
    path::Path,
};

// The server fixture owns its wire encoding independently of the client parser.
const MAX_PACKET_PAYLOAD: usize = 65_516;

fn create_checkout_directory(path: &Path) -> Dir {
    std::fs::create_dir_all(path).unwrap();
    Dir::open_ambient_dir(path, ambient_authority()).unwrap()
}

fn parse_hex_usize(header: &[u8]) -> std::io::Result<usize> {
    let header = std::str::from_utf8(header)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    usize::from_str_radix(header, 16)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn encode_packet_line(payload: &[u8], output: &mut Vec<u8>) -> std::io::Result<()> {
    assert!(payload.len() <= MAX_PACKET_PAYLOAD);
    write!(output, "{:04x}", payload.len() + 4)?;
    output.extend_from_slice(payload);
    Ok(())
}

fn read_packet_section(stream: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut section = Vec::new();
    loop {
        let mut header = [0; 4];
        stream.read_exact(&mut header)?;
        section.extend_from_slice(&header);
        let length = parse_hex_usize(&header)?;
        if length == 0 {
            return Ok(section);
        }
        assert!((4..=MAX_PACKET_PAYLOAD + 4).contains(&length));
        let offset = section.len();
        section.resize(offset + length - 4, 0);
        stream.read_exact(&mut section[offset..])?;
    }
}

fn packet_payloads(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut payloads = Vec::new();
    while !bytes.is_empty() {
        let length = parse_hex_usize(&bytes[..4]).unwrap();
        if length >= 4 {
            payloads.push(&bytes[4..length]);
        }
        bytes = &bytes[length.max(4)..];
    }
    payloads
}

#[test]
fn local_repository_checkout_reads_commit_tree_and_blobs() {
    use std::process::Command;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("nia-git-local-{}-{nonce}", std::process::id()));
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
    std::fs::write(
        repository.join("nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"dep\"\n",
    )
    .unwrap();
    std::fs::write(repository.join("src/nested/lib.nia"), "pub fun main() {}\n").unwrap();
    git(&["add", "nia.toml", "src"]);
    git(&["commit", "-qm", "fixture"]);

    let source = format!(
        "file:///{}",
        repository
            .display()
            .to_string()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    let destination_dir = create_checkout_directory(&destination);
    let revision = checkout_source(&source, "HEAD", &destination_dir).unwrap();
    assert_eq!(revision.len(), 40);
    assert_eq!(
        std::fs::read_to_string(destination.join("nia.toml")).unwrap(),
        "[package]\nversion = \"1.0.0\"\nname = \"dep\"\n"
    );
    assert_eq!(
        std::fs::read_to_string(destination.join("src/nested/lib.nia")).unwrap(),
        "pub fun main() {}\n"
    );
    assert!(!destination.join(".git").exists());

    #[cfg(unix)]
    {
        let anchored_destination = root.join("anchored-checkout");
        let moved_destination = root.join("moved-checkout");
        let outside = root.join("outside");
        std::fs::create_dir(&anchored_destination).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let destination_dir =
            Dir::open_ambient_dir(&anchored_destination, ambient_authority()).unwrap();
        std::fs::rename(&anchored_destination, &moved_destination).unwrap();
        std::os::unix::fs::symlink(&outside, &anchored_destination).unwrap();
        checkout_source(&source, "HEAD", &destination_dir).unwrap();
        assert!(moved_destination.join("nia.toml").is_file());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        std::fs::remove_file(&anchored_destination).unwrap();
    }

    #[cfg(windows)]
    {
        use cap_std::fs::{OpenOptions as CapOpenOptions, OpenOptionsExt};
        use std::process::Command;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let anchored_destination = root.join("anchored-checkout");
        let moved_destination = root.join("moved-checkout");
        let outside = root.join("outside");
        std::fs::create_dir(&anchored_destination).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let parent_dir = Dir::open_ambient_dir(&root, ambient_authority()).unwrap();
        let mut options = CapOpenOptions::new();
        options
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        let destination_dir = Dir::from_std_file(
            parent_dir
                .open_with("anchored-checkout", &options)
                .unwrap()
                .into_std(),
        );
        std::fs::rename(&anchored_destination, &moved_destination).unwrap();
        let output = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&anchored_destination)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        checkout_source(&source, "HEAD", &destination_dir).unwrap();
        assert!(moved_destination.join("nia.toml").is_file());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        std::fs::remove_dir(&anchored_destination).unwrap();
    }

    git(&["gc", "--aggressive", "--prune=now"]);
    let packed_destination = root.join("packed-checkout");
    let packed_directory = create_checkout_directory(&packed_destination);
    let packed_revision = checkout_source(&source, "HEAD", &packed_directory).unwrap();
    assert_eq!(packed_revision, revision);
    assert_eq!(
        std::fs::read_to_string(packed_destination.join("src/nested/lib.nia")).unwrap(),
        "pub fun main() {}\n"
    );
    drop(destination_dir);
    drop(packed_directory);
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
    let root = std::env::temp_dir().join(format!("nia-git-http-{}-{nonce}", std::process::id()));
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
    std::fs::write(
        repository.join("nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"dep\"\n",
    )
    .unwrap();
    std::fs::write(repository.join("src/nested/lib.nia"), "pub fun main() {}\n").unwrap();
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
                assert!(
                    packet_payloads(&request[header_end..]).contains(&b"deepen 1\n".as_slice())
                );
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
                encode_packet_line(b"\x02Counting objects\r", &mut body).unwrap();
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
    let destination_dir = create_checkout_directory(&destination);
    let fetched_revision = checkout_source(&source, "main", &destination_dir).unwrap();
    assert_eq!(fetched_revision, revision);
    assert_eq!(
        std::fs::read_to_string(destination.join("src/nested/lib.nia")).unwrap(),
        "pub fun main() {}\n"
    );
    assert!(!destination.join(".git").exists());
    server.join().unwrap();
    drop(destination_dir);
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
    std::fs::write(
        repository.join("nia.toml"),
        "[package]\nversion = \"1.0.0\"\nname = \"dep\"\n",
    )
    .unwrap();
    std::fs::write(repository.join("src/lib.nia"), "pub fun main() {}\n").unwrap();
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
        assert!(String::from_utf8_lossy(&handshake).starts_with("git-upload-pack /repository.git"));
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
        assert!(packet_payloads(&want).contains(&b"deepen 1\n".as_slice()));
        stream.read_exact(&mut header).unwrap();
        let length = parse_hex_usize(&header).unwrap();
        assert!(length >= 4);
        let mut done = vec![0; length - 4];
        stream.read_exact(&mut done).unwrap();
        assert_eq!(&done, b"done\n");

        let mut response = Vec::new();
        encode_packet_line(b"NAK\n", &mut response).unwrap();
        encode_packet_line(b"\x02Counting objects\r", &mut response).unwrap();
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
    let moved_destination = root.join("moved-checkout");
    let outside = root.join("outside");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&outside).unwrap();
    #[cfg(unix)]
    let destination_dir = {
        let directory = Dir::open_ambient_dir(&destination, ambient_authority()).unwrap();
        std::fs::rename(&destination, &moved_destination).unwrap();
        std::os::unix::fs::symlink(&outside, &destination).unwrap();
        directory
    };
    #[cfg(windows)]
    let destination_dir = {
        use cap_std::fs::{OpenOptions as CapOpenOptions, OpenOptionsExt};
        use std::process::Command;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let parent_dir = Dir::open_ambient_dir(&root, ambient_authority()).unwrap();
        let mut options = CapOpenOptions::new();
        options
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        let directory = Dir::from_std_file(
            parent_dir
                .open_with("checkout", &options)
                .unwrap()
                .into_std(),
        );
        std::fs::rename(&destination, &moved_destination).unwrap();
        let output = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&destination)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        directory
    };
    let fetched_revision = checkout_source(&source, "main", &destination_dir).unwrap();
    assert_eq!(fetched_revision, revision);
    assert_eq!(
        std::fs::read_to_string(moved_destination.join("src/lib.nia")).unwrap(),
        "pub fun main() {}\n"
    );
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    #[cfg(unix)]
    std::fs::remove_file(&destination).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(&destination).unwrap();
    server.join().unwrap();
    drop(destination_dir);
    std::fs::remove_dir_all(root).unwrap();
}
