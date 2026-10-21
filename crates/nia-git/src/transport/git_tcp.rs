// SPDX-License-Identifier: GPL-3.0-or-later
//! Git smart protocol over the native TCP transport.

use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::Path,
};

use super::protocol::{
    build_upload_pack_request, parse_advertised_refs, parse_upload_pack_response,
    select_advertised_revision,
};
use super::{MAX_RESPONSE_BYTES, local::write_pack_checkout};
use crate::invalid_packet;
use crate::pktline::encode_packet_line;
use crate::pktline::read_packet_section;

#[derive(Debug, Clone)]
pub(crate) struct GitTcpUrl {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) path: String,
}

pub(crate) fn parse_git_tcp_url(source: &str) -> io::Result<GitTcpUrl> {
    let rest = source
        .strip_prefix("git://")
        .ok_or_else(|| invalid_packet("Git TCP source must use git://"))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.contains('@') || authority.contains(['?', '#']) {
        return Err(invalid_packet("Git TCP URL has an invalid authority"));
    }
    let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid_packet("Git TCP URL has an invalid port"));
        }
        (
            host.to_string(),
            port.parse::<u16>()
                .map_err(|_| invalid_packet("Git TCP port is invalid"))?,
        )
    } else {
        (authority.to_string(), 9418)
    };
    if path.is_empty() || path.contains(['?', '#', '\0', '\r', '\n']) {
        return Err(invalid_packet("Git TCP URL has an invalid repository path"));
    }
    Ok(GitTcpUrl {
        host,
        port,
        path: format!("/{path}"),
    })
}

pub(crate) fn checkout_git(source: &str, revision: &str, destination: &Path) -> io::Result<String> {
    let url = parse_git_tcp_url(source)?;
    let address = format!("{}:{}", url.host, url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Git host has no address"))?;
    let mut stream = TcpStream::connect_timeout(&address, std::time::Duration::from_secs(15))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;

    let handshake = format!("git-upload-pack {}\0host={}\0", url.path, url.host);
    let mut request = Vec::new();
    encode_packet_line(handshake.as_bytes(), &mut request)?;
    stream.write_all(&request)?;
    let advertisement = read_packet_section(&mut stream)?;
    let refs = parse_advertised_refs(&advertisement)?;
    let commit = select_advertised_revision(&refs, revision)?;

    let request = build_upload_pack_request(&commit)?;
    stream.write_all(&request)?;
    stream.flush()?;
    let mut response = Vec::new();
    stream
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)?;
    if response.len() > MAX_RESPONSE_BYTES {
        return Err(invalid_packet("Git response is too large"));
    }
    let pack = parse_upload_pack_response(&response)?;
    write_pack_checkout(&pack, &commit, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_tcp_url_parser_defaults_port_and_rejects_credentials() {
        let default = parse_git_tcp_url("git://example.test/org/repository.git").unwrap();
        assert_eq!(default.host, "example.test");
        assert_eq!(default.port, 9418);
        assert_eq!(default.path, "/org/repository.git");
        let custom = parse_git_tcp_url("git://example.test:19418/org/repository.git").unwrap();
        assert_eq!(custom.port, 19418);
        assert!(parse_git_tcp_url("git://user@example.test/repository.git").is_err());
        assert!(parse_git_tcp_url("git://example.test/repo\n.git").is_err());
    }
}
