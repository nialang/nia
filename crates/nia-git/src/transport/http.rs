// SPDX-License-Identifier: GPL-3.0-or-later
//! HTTP URL and proxy policy for Git transport.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::MAX_RESPONSE_BYTES;
use super::local::write_pack_checkout_into;
use super::protocol::{
    build_upload_pack_request, parse_advertised_refs, parse_upload_pack_response,
    select_advertised_revision,
};
use crate::{input::read_limited, invalid_packet};
use cap_std::fs::Dir;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use zeroize::Zeroizing;

const MAX_PROXY_HEADER_BYTES: usize = 16 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const CREDENTIAL_HELPER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CREDENTIAL_HELPER_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct HttpGitUrl {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) path: String,
    pub(crate) secure: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HttpProxy {
    pub(crate) host: String,
    pub(crate) port: u16,
}

pub(crate) fn http_proxy_for_url(url: &HttpGitUrl) -> io::Result<Option<HttpProxy>> {
    http_proxy_for_url_with(url, |name| std::env::var(name).ok())
}

pub(crate) fn http_proxy_for_url_with(
    url: &HttpGitUrl,
    mut get_env: impl FnMut(&str) -> Option<String>,
) -> io::Result<Option<HttpProxy>> {
    let no_proxy = first_nonempty_env(&mut get_env, &["NO_PROXY", "no_proxy"]);
    if no_proxy
        .as_deref()
        .is_some_and(|value| no_proxy_matches(value, &url.host, url.port))
    {
        return Ok(None);
    }

    let proxy = if url.secure {
        first_nonempty_env(&mut get_env, &["HTTPS_PROXY", "https_proxy"])
    } else {
        first_nonempty_env(&mut get_env, &["HTTP_PROXY", "http_proxy"])
    }
    .or_else(|| first_nonempty_env(&mut get_env, &["ALL_PROXY", "all_proxy"]));
    proxy.map(|value| parse_http_proxy(&value)).transpose()
}

fn first_nonempty_env(
    get_env: &mut impl FnMut(&str) -> Option<String>,
    names: &[&str],
) -> Option<String> {
    names
        .iter()
        .find_map(|name| get_env(name).filter(|value| !value.trim().is_empty()))
}

fn no_proxy_matches(no_proxy: &str, host: &str, port: u16) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    no_proxy.split(',').any(|entry| {
        let entry = entry.trim();
        if entry.is_empty() {
            return false;
        }
        if entry == "*" {
            return true;
        }

        let (entry_host, entry_port) = if let Some(entry) = entry.strip_prefix('[') {
            let Some((entry_host, remainder)) = entry.split_once(']') else {
                return false;
            };
            let entry_port = if remainder.is_empty() {
                None
            } else {
                remainder
                    .strip_prefix(':')
                    .and_then(|value| value.parse::<u16>().ok())
            };
            if !remainder.is_empty() && entry_port.is_none() {
                return false;
            }
            (entry_host, entry_port)
        } else if let Some((entry_host, entry_port)) = entry.rsplit_once(':') {
            if entry_host.contains(':') {
                (entry, None)
            } else {
                let Ok(entry_port) = entry_port.parse::<u16>() else {
                    return false;
                };
                (entry_host, Some(entry_port))
            }
        } else {
            (entry, None)
        };
        if entry_port.is_some_and(|entry_port| entry_port != port) {
            return false;
        }

        let entry_host = entry_host
            .trim_start_matches('.')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        !entry_host.is_empty()
            && (host == entry_host
                || host
                    .strip_suffix(&entry_host)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    })
}

fn parse_http_proxy(source: &str) -> io::Result<HttpProxy> {
    let rest = source.trim().strip_prefix("http://").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Git proxy must use an unauthenticated http:// URL",
        )
    })?;
    let (authority, suffix) = rest.split_once(['/', '?', '#']).unwrap_or((rest, ""));
    if authority.is_empty() {
        return Err(invalid_packet("Git proxy URL has an invalid authority"));
    }
    if !suffix.is_empty() && suffix != "/" {
        return Err(invalid_packet("Git proxy URL has an invalid path"));
    }
    if authority.contains('@') {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Git proxy URL credentials are not supported",
        ));
    }
    let (host, port) = parse_http_authority(authority, 8080)?;
    Ok(HttpProxy { host, port })
}

fn parse_http_authority(authority: &str, default_port: u16) -> io::Result<(String, u16)> {
    if authority
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid_packet("Git HTTP URL has an invalid authority"));
    }
    if let Some(address) = authority.strip_prefix('[') {
        let (host, suffix) = address
            .split_once(']')
            .ok_or_else(|| invalid_packet("Git HTTP URL has an invalid IPv6 host"))?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .ok_or_else(|| invalid_packet("Git HTTP URL has an invalid port"))?
                .parse::<u16>()
                .map_err(|_| invalid_packet("Git HTTP port is invalid"))?
        };
        if host.parse::<std::net::Ipv6Addr>().is_err() || port == 0 {
            return Err(invalid_packet("Git HTTP URL has an invalid authority"));
        }
        return Ok((host.to_string(), port));
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || host.contains(':') || !port.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid_packet("Git HTTP URL has an invalid port"));
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| invalid_packet("Git HTTP port is invalid"))?;
        if port == 0 {
            return Err(invalid_packet("Git HTTP port is invalid"));
        }
        return Ok((host.to_string(), port));
    }
    if authority.contains(['@', '/', '?', '#', '[', ']'])
        || authority
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid_packet("Git HTTP URL has an invalid authority"));
    }
    if authority.is_empty() {
        return Err(invalid_packet("Git HTTP URL has an invalid authority"));
    }
    Ok((authority.to_string(), default_port))
}

pub(crate) fn parse_http_git_url(source: &str) -> io::Result<HttpGitUrl> {
    let (secure, rest) = if let Some(rest) = source.strip_prefix("http://") {
        (false, rest)
    } else if let Some(rest) = source.strip_prefix("https://") {
        (true, rest)
    } else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Git HTTP source must use http:// or https://",
        ));
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.contains('@') || authority.contains(['?', '#']) {
        return Err(invalid_packet("Git HTTP URL has an invalid authority"));
    }
    let (host, port) = parse_http_authority(authority, if secure { 443 } else { 80 })?;
    if path.contains(['?', '#'])
        || path
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid_packet("Git HTTP URL contains invalid path syntax"));
    }
    let path = format!("/{}", path.trim_end_matches('/'));
    Ok(HttpGitUrl {
        host,
        port,
        path,
        secure,
    })
}

pub(crate) fn explain_http_auth_error(
    error: io::Error,
    url: &HttpGitUrl,
    sent_credentials: bool,
) -> io::Error {
    if url.secure && error.kind() == io::ErrorKind::PermissionDenied {
        let detail = if sent_credentials {
            "Git HTTPS credentials were rejected"
        } else {
            "Git HTTPS server requires credentials; configure a Git credential helper"
        };
        return io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{detail} for {} (HTTP 401)", url.host),
        );
    }
    error
}

pub(crate) fn checkout_http(
    source: &str,
    revision: &str,
    destination_dir: &Dir,
) -> io::Result<String> {
    let url = parse_http_git_url(source)?;
    let proxy = http_proxy_for_url(&url)?;
    let authorization = if url.secure {
        lookup_http_authorization(&url)
    } else {
        None
    };
    let info_path = format!("{}/info/refs?service=git-upload-pack", url.path);
    let authorization = authorization.as_ref().map(|value| value.as_slice());
    let info = http_request_with_proxy(
        &url,
        "GET",
        &info_path,
        &[],
        "",
        authorization,
        proxy.as_ref(),
    )
    .map_err(|error| explain_http_auth_error(error, &url, authorization.is_some()))?;
    let refs = parse_advertised_refs(&info)?;
    let commit = select_advertised_revision(&refs, revision)?;

    let request = build_upload_pack_request(&commit)?;
    let response = http_request_with_proxy(
        &url,
        "POST",
        &format!("{}/git-upload-pack", url.path),
        &request,
        "application/x-git-upload-pack-request",
        authorization,
        proxy.as_ref(),
    )
    .map_err(|error| explain_http_auth_error(error, &url, authorization.is_some()))?;
    let pack = parse_upload_pack_response(&response)?;

    write_pack_checkout_into(&pack, &commit, destination_dir)
}

/// Fetches one public HTTPS resource using the Git HTTP proxy and TLS policy.
/// Resource requests do not inherit Git credentials from the credential helper.
pub fn fetch_https(source: &str) -> io::Result<Vec<u8>> {
    let url = parse_http_git_url(source)?;
    if !url.secure {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "registry resources must use HTTPS",
        ));
    }
    let proxy = http_proxy_for_url(&url)?;
    http_request_with_proxy(&url, "GET", &url.path, &[], "", None, proxy.as_ref())
}

#[cfg(test)]
pub(crate) fn fetch_https_with_test_root(source: &str, root_der: &[u8]) -> io::Result<Vec<u8>> {
    let url = parse_http_git_url(source)?;
    if !url.secure {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "registry resources must use HTTPS",
        ));
    }
    let mut roots = RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(root_der.to_vec()))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    http_request_with_roots(
        &url,
        HttpRequest {
            method: "GET",
            path: &url.path,
            body: &[],
            content_type: "",
            authorization: None,
            proxy: None,
        },
        roots,
    )
}

pub(crate) fn http_request_with_proxy(
    url: &HttpGitUrl,
    method: &str,
    path: &str,
    body: &[u8],
    content_type: &str,
    authorization: Option<&[u8]>,
    proxy: Option<&HttpProxy>,
) -> io::Result<Vec<u8>> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    http_request_with_roots(
        url,
        HttpRequest {
            method,
            path,
            body,
            content_type,
            authorization,
            proxy,
        },
        roots,
    )
}

struct HttpRequest<'a> {
    method: &'a str,
    path: &'a str,
    body: &'a [u8],
    content_type: &'a str,
    authorization: Option<&'a [u8]>,
    proxy: Option<&'a HttpProxy>,
}

fn http_request_with_roots(
    url: &HttpGitUrl,
    request: HttpRequest<'_>,
    roots: RootCertStore,
) -> io::Result<Vec<u8>> {
    let (connect_host, connect_port) = request
        .proxy
        .map_or((url.host.as_str(), url.port), |proxy| {
            (proxy.host.as_str(), proxy.port)
        });
    let address = (connect_host, connect_port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Git HTTP host has no address"))?;
    let stream = TcpStream::connect_timeout(&address, std::time::Duration::from_secs(15))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
    if request.proxy.is_some() && url.secure {
        establish_proxy_tunnel(&stream, url)?;
    }
    let request_target = if request.proxy.is_some() && !url.secure {
        format!(
            "http://{}{}",
            format_authority(&url.host, url.port, 80),
            request.path
        )
    } else {
        request.path.to_string()
    };
    if url.secure {
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server_name = if let Ok(address) = url.host.parse::<std::net::IpAddr>() {
            ServerName::IpAddress(address.into())
        } else {
            ServerName::try_from(url.host.clone()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Git HTTPS host name is invalid",
                )
            })?
        };
        let tls = ClientConnection::new(Arc::new(config), server_name).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Git HTTPS setup failed: {error}"),
            )
        })?;
        return http_request_stream(
            StreamOwned::new(tls, stream),
            url,
            request.method,
            &request_target,
            request.body,
            request.content_type,
            request.authorization,
        );
    }
    http_request_stream_with_target(
        stream,
        url,
        request.method,
        &request_target,
        request.body,
        request.content_type,
        request.authorization,
    )
}

pub(crate) fn establish_proxy_tunnel(mut stream: &TcpStream, url: &HttpGitUrl) -> io::Result<()> {
    let connect_target = format_authority(&url.host, url.port, 0);
    write!(
        stream,
        "CONNECT {connect_target} HTTP/1.1\r\nHost: {connect_target}\r\nConnection: keep-alive\r\n\r\n"
    )?;
    read_proxy_connect_response(stream)
}

pub(crate) fn http_request_stream<S: Read + Write>(
    stream: S,
    url: &HttpGitUrl,
    method: &str,
    path: &str,
    body: &[u8],
    content_type: &str,
    authorization: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    http_request_stream_with_target(stream, url, method, path, body, content_type, authorization)
}

fn http_request_stream_with_target<S: Read + Write>(
    mut stream: S,
    url: &HttpGitUrl,
    method: &str,
    request_target: &str,
    body: &[u8],
    content_type: &str,
    authorization: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    let host_header = format_authority(&url.host, url.port, if url.secure { 443 } else { 80 });
    write!(
        stream,
        "{method} {request_target} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: nia-package-manager/0.3\r\nAccept: application/x-git-upload-pack-advertisement, application/x-git-upload-pack-result\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    )?;
    if !content_type.is_empty() {
        write!(stream, "Content-Type: {content_type}\r\n")?;
    }
    if let Some(authorization) = authorization {
        stream.write_all(b"Authorization: Basic ")?;
        stream.write_all(authorization)?;
        stream.write_all(b"\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)?;
    let response = read_limited(
        stream,
        MAX_RESPONSE_BYTES,
        "Git HTTP response exceeds size limit",
    )?;
    parse_http_response(&response)
}

fn format_authority(host: &str, port: u16, default_port: u16) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default_port {
        host
    } else {
        format!("{host}:{port}")
    }
}

fn read_proxy_connect_response(mut stream: &TcpStream) -> io::Result<()> {
    let mut response = Vec::new();
    let mut byte = [0; 1];
    while response.len() < MAX_PROXY_HEADER_BYTES {
        stream.read_exact(&mut byte)?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            let status = parse_proxy_status(&response)?;
            if (200..300).contains(&status) {
                return Ok(());
            }
            return Err(io::Error::other("Git HTTPS proxy CONNECT request failed"));
        }
    }
    Err(invalid_packet(
        "Git HTTPS proxy response headers are too large",
    ))
}

fn parse_proxy_status(response: &[u8]) -> io::Result<u16> {
    let line_end = response
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| invalid_packet("Git HTTPS proxy response status is malformed"))?;
    let line = std::str::from_utf8(&response[..line_end])
        .map_err(|_| invalid_packet("Git HTTPS proxy response status is malformed"))?;
    parse_http_status(line)
}

fn parse_http_status(line: &str) -> io::Result<u16> {
    if line.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(invalid_packet("Git HTTP response status is malformed"));
    }
    let mut fields = line.splitn(3, ' ');
    let version = fields.next();
    let code = fields.next();
    let status = code
        .filter(|code| code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| invalid_packet("Git HTTP response status is malformed"))?;
    if !matches!(version, Some("HTTP/1.0" | "HTTP/1.1")) || !(100..=599).contains(&status) {
        return Err(invalid_packet("Git HTTP response status is malformed"));
    }
    Ok(status)
}

fn parse_http_field(line: &str) -> io::Result<(&str, &str)> {
    let (name, value) = line
        .split_once(':')
        .ok_or_else(|| invalid_packet("Git HTTP header field is malformed"))?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        || value
            .bytes()
            .any(|byte| (byte < 0x20 && byte != b'\t') || byte == 0x7f)
    {
        return Err(invalid_packet("Git HTTP header field is malformed"));
    }
    Ok((name, value.trim_matches([' ', '\t'])))
}

pub(crate) fn parse_http_response(response: &[u8]) -> io::Result<Vec<u8>> {
    if response.len() > MAX_RESPONSE_BYTES {
        return Err(invalid_packet("Git HTTP response exceeds size limit"));
    }
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| invalid_packet("Git HTTP response has no header terminator"))?;
    if header_end + 4 > MAX_HTTP_HEADER_BYTES {
        return Err(invalid_packet(
            "Git HTTP response headers exceed size limit",
        ));
    }
    let header = std::str::from_utf8(&response[..header_end])
        .map_err(|_| invalid_packet("Git HTTP response headers are not UTF-8"))?;
    let mut lines = header.split("\r\n");
    let status = parse_http_status(lines.next().unwrap_or_default())?;
    if !(200..300).contains(&status) {
        let kind = if status == 401 {
            io::ErrorKind::PermissionDenied
        } else {
            io::ErrorKind::Other
        };
        return Err(io::Error::new(
            kind,
            format!("Git HTTP server returned status {status}"),
        ));
    }
    let mut chunked = false;
    let mut content_length = None;
    for line in lines {
        let (name, value) = parse_http_field(line)?;
        match name.to_ascii_lowercase().as_str() {
            "transfer-encoding" => {
                if chunked || !value.eq_ignore_ascii_case("chunked") {
                    return Err(invalid_packet(
                        "Git HTTP transfer encoding is unsupported or repeated",
                    ));
                }
                chunked = true;
            }
            "content-length" => {
                if content_length.is_some()
                    || value.is_empty()
                    || !value.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(invalid_packet(
                        "Git HTTP content length is malformed or repeated",
                    ));
                }
                content_length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| invalid_packet("Git HTTP content length is malformed"))?,
                )
            }
            _ => {}
        }
    }
    if chunked && content_length.is_some() {
        return Err(invalid_packet(
            "Git HTTP response has conflicting body framing",
        ));
    }
    let body = &response[header_end + 4..];
    if chunked {
        decode_chunked_body(body)
    } else if let Some(length) = content_length {
        if body.len() != length {
            return Err(invalid_packet(
                "Git HTTP body length does not match content length",
            ));
        }
        Ok(body[..length].to_vec())
    } else {
        Ok(body.to_vec())
    }
}

fn decode_chunked_body(mut input: &[u8]) -> io::Result<Vec<u8>> {
    if input.len() > MAX_RESPONSE_BYTES {
        return Err(invalid_packet("Git HTTP response exceeds size limit"));
    }
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| invalid_packet("Git HTTP chunk header is truncated"))?;
        if line_end + 2 > MAX_HTTP_HEADER_BYTES {
            return Err(invalid_packet("Git HTTP chunk header exceeds size limit"));
        }
        let header = &input[..line_end];
        if header
            .iter()
            .any(|byte| (*byte < 0x20 && *byte != b'\t') || *byte == 0x7f)
        {
            return Err(invalid_packet("Git HTTP chunk header is invalid"));
        }
        let size_text = header
            .split(|byte| *byte == b';')
            .next()
            .unwrap_or_default();
        if size_text.is_empty() || !size_text.iter().all(u8::is_ascii_hexdigit) {
            return Err(invalid_packet("Git HTTP chunk size is invalid"));
        }
        let size = usize::from_str_radix(std::str::from_utf8(size_text).unwrap(), 16)
            .map_err(|_| invalid_packet("Git HTTP chunk size is invalid"))?;
        input = &input[line_end + 2..];
        if size == 0 {
            let trailer_start = input.len();
            loop {
                let line_end = input
                    .windows(2)
                    .position(|window| window == b"\r\n")
                    .ok_or_else(|| invalid_packet("Git HTTP trailers are truncated"))?;
                if trailer_start - input.len() + line_end + 2 > MAX_HTTP_HEADER_BYTES {
                    return Err(invalid_packet("Git HTTP trailers exceed size limit"));
                }
                let line = &input[..line_end];
                input = &input[line_end + 2..];
                if line.is_empty() {
                    if !input.is_empty() {
                        return Err(invalid_packet("Git HTTP chunked body has trailing data"));
                    }
                    break;
                }
                let line = std::str::from_utf8(line)
                    .map_err(|_| invalid_packet("Git HTTP trailer is not UTF-8"))?;
                let (name, _) = parse_http_field(line)?;
                if name.eq_ignore_ascii_case("content-length")
                    || name.eq_ignore_ascii_case("transfer-encoding")
                {
                    return Err(invalid_packet("Git HTTP trailer contains a framing field"));
                }
            }
            return Ok(output);
        }
        let chunk_end = size
            .checked_add(2)
            .ok_or_else(|| invalid_packet("Git HTTP chunk size overflows"))?;
        if input.len() < chunk_end || &input[size..chunk_end] != b"\r\n" {
            return Err(invalid_packet("Git HTTP chunk is truncated"));
        }
        if size > MAX_RESPONSE_BYTES.saturating_sub(output.len()) {
            return Err(invalid_packet("Git HTTP response exceeds size limit"));
        }
        output.extend_from_slice(&input[..size]);
        input = &input[chunk_end..];
    }
}

pub(crate) fn lookup_http_authorization(url: &HttpGitUrl) -> Option<Zeroizing<Vec<u8>>> {
    run_http_credential_fill(url, Command::new("git"))
}

pub(crate) fn run_http_credential_fill(
    url: &HttpGitUrl,
    command: Command,
) -> Option<Zeroizing<Vec<u8>>> {
    run_http_credential_fill_with_timeout(url, command, CREDENTIAL_HELPER_TIMEOUT)
}

fn run_http_credential_fill_with_timeout(
    url: &HttpGitUrl,
    mut command: Command,
    timeout: Duration,
) -> Option<Zeroizing<Vec<u8>>> {
    let mut input = format!("protocol=https\nhost={}", url.host);
    if url.port != 443 {
        input.push_str(&format!(":{}", url.port));
    }
    input.push('\n');
    if url.path != "/" {
        input.push_str("path=");
        input.push_str(url.path.trim_start_matches('/'));
        input.push('\n');
    }
    input.push('\n');

    prepare_credential_helper_command(&mut command);
    let mut child = command
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(mut stdin) = child.stdin.take() else {
        terminate_credential_helper(&mut child);
        let _ = child.wait();
        return None;
    };
    if stdin.write_all(input.as_bytes()).is_err() {
        terminate_credential_helper(&mut child);
        let _ = child.wait();
        return None;
    }
    drop(stdin);
    let Some(stdout) = child.stdout.take() else {
        terminate_credential_helper(&mut child);
        let _ = child.wait();
        return None;
    };
    let (output_sender, output_receiver) = std::sync::mpsc::channel();
    let output_reader = thread::spawn(move || {
        let mut output = Zeroizing::new(Vec::new());
        let result = stdout
            .take((MAX_CREDENTIAL_HELPER_OUTPUT + 1) as u64)
            .read_to_end(&mut output)
            .map(|_| output);
        let _ = output_sender.send(result);
    });

    let started = Instant::now();
    let mut captured_output = None;
    let status = loop {
        if captured_output.is_none()
            && let Ok(output) = output_receiver.try_recv()
        {
            let Ok(output) = output else {
                terminate_credential_helper(&mut child);
                let _ = child.wait();
                let _ = output_reader.join();
                return None;
            };
            if output.len() > MAX_CREDENTIAL_HELPER_OUTPUT {
                terminate_credential_helper(&mut child);
                let _ = child.wait();
                let _ = output_reader.join();
                return None;
            }
            captured_output = Some(output);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                terminate_credential_helper(&mut child);
                let _ = child.wait();
                let _ = output_reader.join();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                terminate_credential_helper(&mut child);
                let _ = child.wait();
                let _ = output_reader.join();
                return None;
            }
        }
    };
    let output = match captured_output {
        Some(output) => output,
        None => match output_receiver.recv_timeout(timeout.saturating_sub(started.elapsed())) {
            Ok(Ok(output)) => output,
            Ok(Err(_)) => {
                terminate_credential_helper(&mut child);
                let _ = child.wait();
                let _ = output_reader.join();
                return None;
            }
            Err(_) => {
                terminate_credential_helper(&mut child);
                let _ = child.wait();
                let _ = output_reader.join();
                return None;
            }
        },
    };
    let _ = output_reader.join();
    if output.len() > MAX_CREDENTIAL_HELPER_OUTPUT || !status.success() {
        return None;
    }
    let credentials = parse_git_credentials(&output)?;
    let mut plain = Zeroizing::new(Vec::with_capacity(
        credentials.username.len() + credentials.password.len() + 1,
    ));
    plain.extend_from_slice(&credentials.username);
    plain.push(b':');
    plain.extend_from_slice(&credentials.password);
    Some(base64_encode(&plain))
}

#[cfg(unix)]
fn prepare_credential_helper_command(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(windows)]
fn prepare_credential_helper_command(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(0x0000_0200);
}

#[cfg(not(any(unix, windows)))]
fn prepare_credential_helper_command(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_credential_helper(child: &mut Child) {
    let Ok(group) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };
    // SAFETY: the child was placed in a new process group with its PID as ID.
    unsafe {
        libc::kill(-group, libc::SIGTERM);
    }
    thread::sleep(Duration::from_millis(100));
    // SAFETY: this terminates the same child-owned process group.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn terminate_credential_helper(child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

#[cfg(not(any(unix, windows)))]
fn terminate_credential_helper(child: &mut Child) {
    let _ = child.kill();
}

pub(crate) struct GitCredentials {
    pub(crate) username: Zeroizing<Vec<u8>>,
    pub(crate) password: Zeroizing<Vec<u8>>,
}

pub(crate) fn parse_git_credentials(output: &[u8]) -> Option<GitCredentials> {
    let mut username = None;
    let mut password = None;
    for line in output.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(separator) = line.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (key, value) = (&line[..separator], &line[separator + 1..]);
        match key {
            b"username" if username.is_none() => username = Some(value.to_vec()),
            b"password" if password.is_none() => password = Some(value.to_vec()),
            b"username" | b"password" => return None,
            _ => {}
        }
    }
    let username = username?;
    let password = password?;
    if username.is_empty()
        || username.contains(&b':')
        || username.iter().chain(&password).any(u8::is_ascii_control)
    {
        return None;
    }
    Some(GitCredentials {
        username: Zeroizing::new(username),
        password: Zeroizing::new(password),
    })
}

pub(crate) fn base64_encode(input: &[u8]) -> Zeroizing<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Zeroizing::new(Vec::with_capacity(input.len().div_ceil(3) * 4));
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[usize::from(first >> 2)]);
        output.push(TABLE[usize::from(((first & 0x03) << 4) | (second >> 4))]);
        output.push(if chunk.len() > 1 {
            TABLE[usize::from(((second & 0x0f) << 2) | (third >> 6))]
        } else {
            b'='
        });
        output.push(if chunk.len() > 2 {
            TABLE[usize::from(third & 0x3f)]
        } else {
            b'='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Read, Write};
    use std::net::TcpStream;

    #[test]
    fn chunk_lengths_near_usize_max_are_rejected() {
        for length in [usize::MAX, usize::MAX - 1] {
            let body = format!("{length:x}\r\n");
            assert!(decode_chunked_body(body.as_bytes()).is_err());
        }
    }

    #[test]
    fn http_rejects_ambiguous_or_malformed_response_framing() {
        for response in [
            "garbage 200 OK\r\n\r\n",
            "HTTP/1.1 +200 OK\r\n\r\n",
            "HTTP/1.1 0200 OK\r\n\r\n",
            "HTTP/1.1 200 OK\nX-Test: value\r\n\r\n",
            "HTTP/1.1 200 OK\r\nBroken header\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length : 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\ncontent-length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nContent-Length: +1\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nContent-Length: 1, 1\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n0\r\n\r\n",
        ] {
            assert_eq!(
                parse_http_response(response.as_bytes()).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{response:?}"
            );
        }
    }

    #[test]
    fn chunked_bodies_require_complete_valid_trailers_and_no_extra_data() {
        assert_eq!(
            decode_chunked_body(b"3;name=value\r\nabc\r\n0\r\nX-Checksum: value\r\n\r\n").unwrap(),
            b"abc"
        );
        for body in [
            "+1\r\nx\r\n0\r\n\r\n",
            "1 \r\nx\r\n0\r\n\r\n",
            "1;bad\nheader\r\nx\r\n0\r\n\r\n",
            "0\r\n",
            "0\r\nX-Test: value\r\n",
            "0\r\ninvalid\r\n\r\n",
            "0\r\nContent-Length: 0\r\n\r\n",
            "0\r\nTransfer-Encoding: chunked\r\n\r\n",
            "0\r\n\r\nextra",
        ] {
            assert_eq!(
                decode_chunked_body(body.as_bytes()).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "{body:?}"
            );
        }
        let valid = b"3\r\nabc\r\n0\r\nX-Test: value\r\n\r\n";
        for length in 0..valid.len() {
            assert!(
                decode_chunked_body(&valid[..length]).is_err(),
                "length={length}"
            );
        }
    }

    #[test]
    fn http_header_and_trailer_limits_accept_exact_boundary() {
        let prefix = "HTTP/1.1 200 OK\r\nX-Padding: ";
        let mut response = format!(
            "{prefix}{}\r\n\r\n",
            "x".repeat(MAX_HTTP_HEADER_BYTES - prefix.len() - 4)
        );
        assert!(parse_http_response(response.as_bytes()).unwrap().is_empty());
        response.insert(prefix.len(), 'x');
        assert!(parse_http_response(response.as_bytes()).is_err());
        let mut body = format!(
            "0\r\nX-Padding: {}\r\n\r\n",
            "x".repeat(MAX_HTTP_HEADER_BYTES - "X-Padding: ".len() - 4)
        );
        assert!(decode_chunked_body(body.as_bytes()).unwrap().is_empty());
        body.insert(3, 'x');
        assert!(decode_chunked_body(body.as_bytes()).is_err());
    }

    #[test]
    fn http_authorities_with_ports_reject_request_injection() {
        for host in [
            "example.test\r\nX-Injected: value",
            "example.test\n",
            "example.test ",
            "example.test\t",
        ] {
            for scheme in ["http", "https"] {
                assert!(parse_http_git_url(&format!("{scheme}://{host}:8080/repository")).is_err());
            }
            assert!(parse_http_proxy(&format!("http://{host}:8080")).is_err());
        }
    }

    #[test]
    fn http_response_parser_handles_content_length_and_chunked_bodies() {
        let fixed = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(parse_http_response(fixed).unwrap(), b"abc");
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(chunked).unwrap(), b"abc");
        let rejected = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(
            parse_http_response(rejected).unwrap_err().kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn https_resource_fetch_performs_tls_handshake_and_reads_response() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;
        use std::sync::Arc;

        let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        let certificate = certified.cert.der().to_vec();
        let private_key = certified.signing_key.serialize_der();
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(certificate.clone())],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(private_key),
                ),
            )
            .unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let connection = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
            let mut stream = rustls::StreamOwned::new(connection, stream);
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"GET /resource HTTP/1.1\r\n"));
            assert!(
                !request
                    .windows(b"Authorization:".len())
                    .any(|window| window.eq_ignore_ascii_case(b"Authorization:"))
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nregistry-body")
                .unwrap();
            stream.conn.send_close_notify();
            stream.flush().unwrap();
        });

        let body =
            fetch_https_with_test_root(&format!("https://127.0.0.1:{port}/resource"), &certificate)
                .unwrap();
        assert_eq!(body, b"registry-body");
        server.join().unwrap();
    }

    #[test]
    fn git_http_credentials_are_parsed_and_basic_encoded() {
        let credentials = parse_git_credentials(
            b"protocol=https\nhost=example.test\nusername=nia\npassword=secret\n",
        )
        .unwrap();
        let mut plain = credentials.username.to_vec();
        plain.push(b':');
        plain.extend_from_slice(&credentials.password);
        assert_eq!(&*base64_encode(&plain), b"bmlhOnNlY3JldA==");

        assert!(parse_git_credentials(b"username=nia\n").is_none());
        assert!(parse_git_credentials(b"username=nia\npassword=one\npassword=two\n").is_none());
        assert!(parse_git_credentials(b"username=nia:other\npassword=secret\n").is_none());
    }

    #[test]
    fn git_http_credential_fill_uses_the_configured_helper() {
        use std::process::Command;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nia-git-credential-helper-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("gitconfig");
        let store = root.join("credentials");
        let store_path = store.display().to_string().replace('\\', "/");
        let store_text = "https://nia:secret@example.test/repository.git\nhttps://other:ignored@example.test/other.git\n";
        std::fs::write(&store, store_text).unwrap();
        let store_arg = format!("credential.helper=store --file=\"{store_path}\"");
        std::fs::write(
            &config,
            format!(
                "[credential]\n\thelper = store --file=\"{store_path}\"\n\tuseHttpPath = true\n"
            ),
        )
        .unwrap();
        let url = parse_http_git_url("https://example.test/repository.git").unwrap();
        let mut command = Command::new("git");
        command
            .args(["-c", &store_arg, "-c", "credential.useHttpPath=true"])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &config);
        assert_eq!(
            &*run_http_credential_fill(&url, command).unwrap(),
            b"bmlhOnNlY3JldA=="
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_http_credential_fill_times_out_and_reaps_hanging_helpers() {
        use std::process::Command;
        use std::time::Instant;

        let url = parse_http_git_url("https://example.test/repository.git").unwrap();
        let command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30"]);
            command
        };
        let started = Instant::now();
        assert!(
            run_http_credential_fill_with_timeout(&url, command, Duration::from_millis(50),)
                .is_none()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn git_http_authorization_is_sent_as_a_header_and_401_is_actionable() {
        use std::io::Cursor;

        struct TestStream {
            response: Cursor<Vec<u8>>,
            written: Vec<u8>,
        }
        impl Read for TestStream {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                self.response.read(output)
            }
        }
        impl Write for TestStream {
            fn write(&mut self, input: &[u8]) -> io::Result<usize> {
                self.written.extend_from_slice(input);
                Ok(input.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let url = parse_http_git_url("https://example.test/repository.git").unwrap();
        let mut stream = TestStream {
            response: Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec()),
            written: Vec::new(),
        };
        http_request_stream(
            &mut stream,
            &url,
            "GET",
            "/repository.git/info/refs",
            &[],
            "",
            Some(b"bmlhOnNlY3JldA=="),
        )
        .unwrap();
        assert!(
            stream
                .written
                .windows(b"Authorization: Basic bmlhOnNlY3JldA==\r\n".len())
                .any(|window| window == b"Authorization: Basic bmlhOnNlY3JldA==\r\n")
        );

        let unauthorized =
            parse_http_response(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                .unwrap_err();
        let error = explain_http_auth_error(unauthorized, &url, false);
        assert!(
            error
                .to_string()
                .contains("configure a Git credential helper")
        );
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn http_url_parser_selects_tls_and_rejects_embedded_credentials() {
        let plain = parse_http_git_url("http://example.test/repository.git").unwrap();
        assert!(!plain.secure);
        assert_eq!(plain.port, 80);
        let secure = parse_http_git_url("https://example.test/repository.git").unwrap();
        assert!(secure.secure);
        assert_eq!(secure.port, 443);
        assert_eq!(
            parse_http_git_url("https://user@example.test/repository.git")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        for invalid in [
            "http://example.test:0/repository.git",
            "http://exa mple.test/repository.git",
            "http://example.test/repo\r\nInjected: header",
            "http://[not-ipv6]/repository.git",
        ] {
            assert!(parse_http_git_url(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(
            parse_http_git_url("https://[2001:db8::1]:8443/repository.git")
                .unwrap()
                .host,
            "2001:db8::1"
        );
    }

    #[test]
    fn http_proxy_selection_obeys_protocol_and_no_proxy_rules() {
        use std::collections::HashMap;

        let secure = parse_http_git_url("https://git.example.test/repository.git").unwrap();
        let mut vars = HashMap::from([
            ("HTTPS_PROXY", "http://secure-proxy.test:9000"),
            ("HTTP_PROXY", "http://plain-proxy.test:8000"),
            ("ALL_PROXY", "http://fallback-proxy.test"),
        ]);
        let proxy =
            http_proxy_for_url_with(&secure, |name| vars.get(name).map(|v| v.to_string())).unwrap();
        assert_eq!(
            proxy,
            Some(HttpProxy {
                host: "secure-proxy.test".into(),
                port: 9000,
            })
        );

        vars.insert("NO_PROXY", ".example.test:443,localhost");
        assert_eq!(
            http_proxy_for_url_with(&secure, |name| vars.get(name).map(|v| v.to_string())).unwrap(),
            None
        );
        vars.insert("NO_PROXY", "example.test:80");
        assert!(
            http_proxy_for_url_with(&secure, |name| vars.get(name).map(|v| v.to_string()))
                .unwrap()
                .is_some()
        );

        let plain = parse_http_git_url("http://git.example.test/repository.git").unwrap();
        vars.remove("HTTP_PROXY");
        vars.remove("NO_PROXY");
        assert_eq!(
            http_proxy_for_url_with(&plain, |name| vars.get(name).map(|v| v.to_string()))
                .unwrap()
                .unwrap()
                .host,
            "fallback-proxy.test"
        );
    }

    #[test]
    fn http_proxy_receives_absolute_target_and_origin_host_header() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let url = parse_http_git_url("http://git.example.test:8081/repository.git").unwrap();
        let proxy = HttpProxy {
            host: address.ip().to_string(),
            port: address.port(),
        };
        http_request_with_proxy(
            &url,
            "GET",
            "/repository.git/info/refs?service=git-upload-pack",
            &[],
            "",
            None,
            Some(&proxy),
        )
        .unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with(
            "GET http://git.example.test:8081/repository.git/info/refs?service=git-upload-pack HTTP/1.1\r\n"
        ));
        assert!(request.contains("\r\nHost: git.example.test:8081\r\n"));
        assert!(!request.contains("Authorization:"));
    }

    #[test]
    fn https_proxy_tunnel_uses_origin_authority_without_credentials() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let url = parse_http_git_url("https://[2001:db8::1]/repository.git").unwrap();
        let stream = TcpStream::connect(address).unwrap();
        establish_proxy_tunnel(&stream, &url).unwrap();
        let request = server.join().unwrap();
        assert!(
            request
                .starts_with("CONNECT [2001:db8::1]:443 HTTP/1.1\r\nHost: [2001:db8::1]:443\r\n")
        );
        assert!(!request.contains("Authorization:"));
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn arbitrary_http_and_chunked_bodies_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let _ = parse_http_response(&bytes);
            let _ = decode_chunked_body(&bytes);
        }

        #[test]
        fn arbitrary_chunk_lengths_do_not_panic(length in any::<usize>(), bytes in prop::collection::vec(any::<u8>(), 0..256)) {
            let mut input = format!("{length:x}\r\n").into_bytes();
            input.extend(bytes);
            let _ = decode_chunked_body(&input);
        }

        #[test]
        fn fixed_and_chunked_http_round_trip(chunks in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..128), 0..16)) {
            let expected: Vec<u8> = chunks.iter().flatten().copied().collect();
            let mut fixed = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", expected.len()).into_bytes();
            fixed.extend_from_slice(&expected);
            prop_assert_eq!(parse_http_response(&fixed).unwrap(), expected.clone());
            let mut chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            for chunk in chunks {
                write!(chunked, "{:x}\r\n", chunk.len()).unwrap();
                chunked.extend_from_slice(&chunk);
                chunked.extend_from_slice(b"\r\n");
            }
            chunked.extend_from_slice(b"0\r\n\r\n");
            prop_assert_eq!(parse_http_response(&chunked).unwrap(), expected);
        }
    }
}
