// SPDX-License-Identifier: GPL-3.0-or-later
//! Upload-pack requests, advertised revisions, and side-band responses.

use crate::invalid_packet;
use crate::object::{encode_hex, parse_object_id};
use crate::pktline::{PacketLine, decode_packet_lines, encode_packet_line};
use std::io;

pub(super) fn build_upload_pack_request(commit: &[u8; 20]) -> io::Result<Vec<u8>> {
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

pub(super) fn parse_advertised_refs(body: &[u8]) -> io::Result<Vec<([u8; 20], String)>> {
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

pub(super) fn select_advertised_revision(
    refs: &[([u8; 20], String)],
    revision: &str,
) -> io::Result<[u8; 20]> {
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

pub(super) fn parse_upload_pack_response(body: &[u8]) -> io::Result<Vec<u8>> {
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
            2 => {} // Progress messages may be interleaved with pack data.
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
mod tests {
    use super::*;

    #[test]
    fn packet_lines_decode_control_frames_and_advertised_refs() {
        let id = "0123456789012345678901234567890123456789";
        let advertisement = format!(
            "001e# service=git-upload-pack\n0000{length:04x}{id} refs/heads/main\0side-band-64k\n0000",
            length = id.len() + " refs/heads/main\0side-band-64k\n".len() + 4,
        );
        let refs = parse_advertised_refs(advertisement.as_bytes()).unwrap();
        assert_eq!(refs[0].1, "refs/heads/main");
        assert_eq!(
            select_advertised_revision(&refs, "main").unwrap(),
            parse_object_id(id.as_bytes()).unwrap()
        );
    }

    #[test]
    fn upload_pack_progress_is_ignored_but_fatal_errors_are_reported() {
        let mut expected = b"PACK".to_vec();
        expected.resize(64, 0x2a);
        let mut response = Vec::new();
        encode_packet_line(b"NAK\n", &mut response).unwrap();
        encode_packet_line(b"\x02Counting objects: 3\r", &mut response).unwrap();
        for chunk in expected.chunks(11) {
            let mut data = vec![1];
            data.extend_from_slice(chunk);
            encode_packet_line(&data, &mut response).unwrap();
            encode_packet_line(b"\x02Receiving objects\n", &mut response).unwrap();
        }
        response.extend_from_slice(b"0000");
        assert_eq!(parse_upload_pack_response(&response).unwrap(), expected);
        encode_packet_line(b"\x03repository unavailable\n", &mut response).unwrap();
        let error = parse_upload_pack_response(&response).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains("repository unavailable"));
    }

    #[test]
    fn upload_pack_fetch_requests_depth_one() {
        let request = build_upload_pack_request(&[0x2a; 20]).unwrap();
        let packets = decode_packet_lines(&request).unwrap();
        assert_eq!(
            packets,
            vec![
                PacketLine::Data(
                    format!("want {} ofs-delta side-band-64k\n", "2a".repeat(20)).into_bytes()
                ),
                PacketLine::Data(b"deepen 1\n".to_vec()),
                PacketLine::Flush,
                PacketLine::Data(b"done\n".to_vec()),
            ]
        );
    }
}
