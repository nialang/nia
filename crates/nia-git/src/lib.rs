//! Nia-owned Git wire and object decoding primitives.
//!
//! Git object validation and checkout live behind this boundary. Package graph
//! code consumes filesystem roots and never sees Git object representations.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_HTTP_RESPONSE: usize = 128 * 1024 * 1024;
const MAX_PACKET_PAYLOAD: usize = 65_516;

mod pktline;
use pktline::{PacketLine, decode_packet_lines, encode_packet_line};
mod object;
use object::{encode_hex, parse_object_id};
mod transport;
use transport::git_tcp::checkout_git;
use transport::http::checkout_http;
use transport::local::{checkout_local, fs_create_dir};
use transport::ssh::checkout_ssh;

fn invalid_packet(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn parse_hex_usize(bytes: &[u8]) -> io::Result<usize> {
    bytes.iter().try_fold(0usize, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => usize::from(byte - b'0'),
            b'a'..=b'f' => usize::from(byte - b'a' + 10),
            b'A'..=b'F' => usize::from(byte - b'A' + 10),
            _ => return Err(invalid_packet("invalid Git packet-line header")),
        };
        Ok((value << 4) | digit)
    })
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

fn read_packet_section(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut section = Vec::new();
    loop {
        let mut header = [0; 4];
        stream.read_exact(&mut header)?;
        let length = parse_hex_usize(&header)?;
        section.extend_from_slice(&header);
        if length == 0 {
            return Ok(section);
        }
        if !(4..=MAX_PACKET_PAYLOAD + 4).contains(&length) {
            return Err(invalid_packet("Git packet-line exceeds protocol limit"));
        }
        let offset = section.len();
        section.resize(offset + length - 4, 0);
        stream.read_exact(&mut section[offset..])?;
    }
}

fn write_pack_checkout(pack: &[u8], commit: &[u8; 20], destination: &Path) -> io::Result<String> {
    let git_dir = destination.join(".git");
    let pack_dir = git_dir.join("objects/pack");
    fs_create_dir(&pack_dir)?;
    std::fs::write(git_dir.join("HEAD"), encode_hex(commit))?;
    let pack_id: [u8; 20] = pack
        .get(pack.len().saturating_sub(20)..)
        .filter(|bytes| bytes.len() == 20)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid_packet("Git upload-pack response has no pack checksum"))?;
    std::fs::write(
        pack_dir.join(format!("pack-{}.pack", encode_hex(&pack_id))),
        pack,
    )?;
    let result = checkout_local(destination, "HEAD", destination);
    let _ = std::fs::remove_dir_all(&git_dir);
    result
}

fn build_upload_pack_request(commit: &[u8; 20]) -> io::Result<Vec<u8>> {
    let mut request = Vec::new();
    encode_packet_line(
        format!("want {} ofs-delta side-band-64k\n", encode_hex(commit)).as_bytes(),
        &mut request,
    )?;
    encode_packet_line(b"deepen 1\n", &mut request)?;
    request.extend_from_slice(b"0000");
    encode_packet_line(b"done\n", &mut request)?;
    Ok(request)
}

fn parse_advertised_refs(body: &[u8]) -> io::Result<Vec<([u8; 20], String)>> {
    let packets = decode_packet_lines(body)?;
    let mut refs = Vec::new();
    for packet in packets {
        let PacketLine::Data(data) = packet else {
            continue;
        };
        if data.starts_with(b"# service=") {
            continue;
        }
        let line = data.split(|byte| *byte == 0).next().unwrap_or(&data);
        let mut fields = line.splitn(2, |byte| *byte == b' ' || *byte == b'\n');
        let Some(id) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        let id = parse_object_id(id)?;
        let name = std::str::from_utf8(name)
            .map_err(|_| invalid_packet("Git advertised ref is not UTF-8"))?
            .trim()
            .to_string();
        if name.starts_with("refs/") || name == "HEAD" {
            refs.push((id, name));
        }
    }
    if refs.is_empty() {
        return Err(invalid_packet("Git server advertised no references"));
    }
    Ok(refs)
}

fn select_advertised_revision(refs: &[([u8; 20], String)], revision: &str) -> io::Result<[u8; 20]> {
    if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let id = parse_object_id(revision.as_bytes())?;
        if refs.iter().any(|(candidate, _)| candidate == &id) {
            return Ok(id);
        }
    }
    let names = if revision == "HEAD" {
        vec!["HEAD".to_string()]
    } else {
        vec![
            revision.to_string(),
            format!("refs/heads/{revision}"),
            format!("refs/tags/{revision}"),
        ]
    };
    names
        .iter()
        .find_map(|name| {
            refs.iter()
                .find(|(_, candidate)| candidate == &format!("{name}^{{}}"))
                .or_else(|| refs.iter().find(|(_, candidate)| candidate == name))
                .map(|(id, _)| *id)
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Git revision was not advertised"))
}

fn parse_upload_pack_response(body: &[u8]) -> io::Result<Vec<u8>> {
    let packets = decode_packet_lines(body)?;
    let mut pack = Vec::new();
    for packet in packets {
        let PacketLine::Data(data) = packet else {
            continue;
        };
        if data.starts_with(b"PACK") {
            pack.extend_from_slice(&data);
            continue;
        }
        if data.is_empty() {
            continue;
        }
        match data[0] {
            1 => pack.extend_from_slice(&data[1..]),
            2 => {
                return Err(io::Error::other(format!(
                    "Git upload-pack reported an error: {}",
                    String::from_utf8_lossy(&data[1..])
                )));
            }
            3 => {
                return Err(io::Error::other(format!(
                    "Git upload-pack failed: {}",
                    String::from_utf8_lossy(&data[1..])
                )));
            }
            _ => {}
        }
    }
    if pack.len() < 32 || &pack[..4] != b"PACK" {
        return Err(invalid_packet("Git upload-pack returned no pack"));
    }
    Ok(pack)
}

#[cfg(test)]
mod tests;

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
    transport::ssh::tests::restrict_to_current_user(path);
}

#[cfg(feature = "test-support")]
pub fn with_test_ssh_checkout_fixture(callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static) {
    transport::ssh::tests::with_ssh_checkout_fixture(callback);
}

#[cfg(feature = "test-support")]
pub fn with_test_ssh_checkout_fixture_mutating_repository(
    mutator: impl FnOnce(&Path) + 'static,
    callback: impl FnOnce(&Path, &str, &Path, &Path) + 'static,
) {
    transport::ssh::tests::with_ssh_checkout_fixture_mutating_repository(mutator, callback);
}
