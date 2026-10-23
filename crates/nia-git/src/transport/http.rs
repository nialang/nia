// SPDX-License-Identifier: GPL-3.0-or-later
//! Git smart HTTP(S): URLs, proxies, request framing, bounded response parsing,
//! and the `git credential` bridge for HTTPS authentication.

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
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    checkout_http_with(
        &url,
        proxy.as_ref(),
        &roots,
        &|| Command::new("git"),
        revision,
        destination_dir,
    )
}

/// The smart-HTTP checkout, with the TLS roots and the `git` command for
/// credential-helper calls supplied by the caller.
///
/// Like Git, the first request is anonymous: the credential helper is asked
/// only after the server answers `401`, so public repositories never reach
/// it. Accepted credentials are approved so the helper can store or refresh
/// them; credentials that still receive `401` are rejected so it stops
/// offering them.
fn checkout_http_with(
    url: &HttpGitUrl,
    proxy: Option<&HttpProxy>,
    roots: &RootCertStore,
    git: &dyn Fn() -> Command,
    revision: &str,
    destination_dir: &Dir,
) -> io::Result<String> {
    let send = |request: HttpRequest<'_>| {
        http_request_with_roots(url, HttpRequest { proxy, ..request }, roots.clone())
    };
    let info_path = format!("{}/info/refs?service=git-upload-pack", url.path);
    let mut credentials = None;
    let info = match send(HttpRequest::get(&info_path)) {
        Err(error) if url.secure && error.kind() == io::ErrorKind::PermissionDenied => {
            let Some(found) = run_http_credential_fill(url, git()) else {
                return Err(explain_http_auth_error(error, url, false));
            };
            let header = found.basic_authorization();
            let retried = send(HttpRequest {
                authorization: Some(header.as_slice()),
                ..HttpRequest::get(&info_path)
            });
            match retried {
                Ok(info) => {
                    credentials = Some((found, header));
                    info
                }
                Err(error) => {
                    if error.kind() == io::ErrorKind::PermissionDenied {
                        reject_http_credentials_with(url, &found, git());
                    }
                    return Err(explain_http_auth_error(error, url, true));
                }
            }
        }
        result => result.map_err(|error| explain_http_auth_error(error, url, false))?,
    };
    let refs = parse_advertised_refs(&info)?;
    let commit = select_advertised_revision(&refs, revision)?;

    let request = build_upload_pack_request(&commit)?;
    let upload_path = format!("{}/git-upload-pack", url.path);
    let authorization = credentials.as_ref().map(|(_, header)| header.as_slice());
    let response = send(HttpRequest {
        method: "POST",
        body: &request,
        content_type: "application/x-git-upload-pack-request",
        authorization,
        ..HttpRequest::get(&upload_path)
    });
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            if error.kind() == io::ErrorKind::PermissionDenied
                && let Some((found, _)) = &credentials
            {
                reject_http_credentials_with(url, found, git());
            }
            return Err(explain_http_auth_error(error, url, authorization.is_some()));
        }
    };
    if let Some((found, _)) = &credentials {
        approve_http_credentials_with(url, found, git());
    }
    let pack = parse_upload_pack_response(&response)?;

    write_pack_checkout_into(&pack, &commit, destination_dir)
}

/// Fetches one public HTTPS resource using the Git HTTP proxy and TLS policy.
/// Resource requests do not inherit Git credentials from the credential helper.
pub fn fetch_https_with_limit(source: &str, limit: usize) -> io::Result<Vec<u8>> {
    if limit == 0 || limit > MAX_RESPONSE_BYTES * 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Git HTTPS response limit is invalid",
        ));
    }
    let url = parse_http_git_url(source)?;
    if !url.secure {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "registry resources must use HTTPS",
        ));
    }
    let proxy = http_proxy_for_url(&url)?;
    http_request(
        &url,
        HttpRequest {
            proxy: proxy.as_ref(),
            response_limit: limit,
            ..HttpRequest::get(&url.path)
        },
    )
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
            response_limit: MAX_RESPONSE_BYTES,
        },
        roots,
    )
}

/// Sends one request with the bundled WebPKI roots.
pub(crate) fn http_request(url: &HttpGitUrl, request: HttpRequest<'_>) -> io::Result<Vec<u8>> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    http_request_with_roots(url, request, roots)
}

/// One Git HTTP request. `path` is the origin-form path; a plain-HTTP proxy
/// request rewrites it to absolute form before sending.
#[derive(Clone, Copy)]
pub(crate) struct HttpRequest<'a> {
    pub(crate) method: &'a str,
    pub(crate) path: &'a str,
    pub(crate) body: &'a [u8],
    pub(crate) content_type: &'a str,
    pub(crate) authorization: Option<&'a [u8]>,
    pub(crate) proxy: Option<&'a HttpProxy>,
    pub(crate) response_limit: usize,
}

impl<'a> HttpRequest<'a> {
    /// A credential-free GET with the default response limit.
    pub(crate) fn get(path: &'a str) -> Self {
        Self {
            method: "GET",
            path,
            body: &[],
            content_type: "",
            authorization: None,
            proxy: None,
            response_limit: MAX_RESPONSE_BYTES,
        }
    }
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
            HttpRequest {
                path: &request_target,
                ..request
            },
        );
    }
    http_request_stream(
        stream,
        url,
        HttpRequest {
            path: &request_target,
            ..request
        },
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

/// Writes `request` to an established stream and reads the bounded response.
/// `request.path` is sent verbatim as the request target.
pub(crate) fn http_request_stream<S: Read + Write>(
    mut stream: S,
    url: &HttpGitUrl,
    request: HttpRequest<'_>,
) -> io::Result<Vec<u8>> {
    let HttpRequest {
        method,
        path: request_target,
        body,
        content_type,
        authorization,
        response_limit,
        ..
    } = request;
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
        response_limit.saturating_add(MAX_HTTP_HEADER_BYTES),
        "Git HTTP response exceeds size limit",
    )?;
    parse_http_response_with_limit(&response, response_limit)
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

#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn parse_http_response(response: &[u8]) -> io::Result<Vec<u8>> {
    parse_http_response_with_limit(response, MAX_RESPONSE_BYTES)
}

fn parse_http_response_with_limit(response: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    if response.len() > limit.saturating_add(MAX_HTTP_HEADER_BYTES) {
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
        decode_chunked_body_with_limit(body, limit)
    } else if let Some(length) = content_length {
        if length > limit {
            return Err(invalid_packet("Git HTTP response exceeds size limit"));
        }
        if body.len() != length {
            return Err(invalid_packet(
                "Git HTTP body length does not match content length",
            ));
        }
        Ok(body[..length].to_vec())
    } else {
        if body.len() > limit {
            return Err(invalid_packet("Git HTTP response exceeds size limit"));
        }
        Ok(body.to_vec())
    }
}

#[cfg(test)]
fn decode_chunked_body(input: &[u8]) -> io::Result<Vec<u8>> {
    decode_chunked_body_with_limit(input, MAX_RESPONSE_BYTES)
}

fn decode_chunked_body_with_limit(mut input: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    if input.len() > limit.saturating_add(MAX_HTTP_HEADER_BYTES) {
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
        if size > limit.saturating_sub(output.len()) {
            return Err(invalid_packet("Git HTTP response exceeds size limit"));
        }
        output.extend_from_slice(&input[..size]);
        input = &input[chunk_end..];
    }
}

/// Tells the credential helper that `credentials` were rejected, so a stored
/// password is not offered again. Failures are ignored: the fetch has already
/// failed with an authentication error.
fn reject_http_credentials_with(url: &HttpGitUrl, credentials: &GitCredentials, command: Command) {
    report_http_credentials(url, credentials, command, "reject");
}

/// Tells the credential helper that `credentials` were accepted, so it can
/// store or refresh them. Failures are ignored: the fetch already succeeded.
fn approve_http_credentials_with(url: &HttpGitUrl, credentials: &GitCredentials, command: Command) {
    report_http_credentials(url, credentials, command, "approve");
}

/// Sends `git credential approve` or `reject` for the credentials of `url`.
fn report_http_credentials(
    url: &HttpGitUrl,
    credentials: &GitCredentials,
    command: Command,
    action: &str,
) {
    let mut input = Zeroizing::new(credential_request(url).into_bytes());
    // `credential_request` ends with the blank terminator line; replace it
    // with the username and password being reported.
    input.pop();
    input.extend_from_slice(b"username=");
    input.extend_from_slice(&credentials.username);
    input.extend_from_slice(b"\npassword=");
    input.extend_from_slice(&credentials.password);
    input.extend_from_slice(b"\n\n");
    let _ = run_git_credential(command, action, &input, CREDENTIAL_HELPER_TIMEOUT);
}

pub(crate) fn run_http_credential_fill(
    url: &HttpGitUrl,
    command: Command,
) -> Option<GitCredentials> {
    run_http_credential_fill_with_timeout(url, command, CREDENTIAL_HELPER_TIMEOUT)
}

fn run_http_credential_fill_with_timeout(
    url: &HttpGitUrl,
    command: Command,
    timeout: Duration,
) -> Option<GitCredentials> {
    let request = credential_request(url);
    let output = run_git_credential(command, "fill", request.as_bytes(), timeout)?;
    parse_git_credentials(&output)
}

/// The `git credential` description of an HTTPS repository, ending with the
/// blank line that terminates the request.
fn credential_request(url: &HttpGitUrl) -> String {
    // An IPv6 host keeps its brackets, so a port stays unambiguous.
    let mut input = format!(
        "protocol=https\nhost={}\n",
        format_authority(&url.host, url.port, 443)
    );
    if url.path != "/" {
        input.push_str("path=");
        input.push_str(url.path.trim_start_matches('/'));
        input.push('\n');
    }
    input.push('\n');
    input
}

/// Runs `git credential <action>` with a deadline and bounded output, and
/// returns its standard output when it exits successfully.
fn run_git_credential(
    mut command: Command,
    action: &str,
    input: &[u8],
    timeout: Duration,
) -> Option<Zeroizing<Vec<u8>>> {
    prepare_credential_helper_command(&mut command);
    let mut child = command
        .args(["credential", action])
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
    if stdin.write_all(input).is_err() {
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
    Some(output)
}

impl GitCredentials {
    /// The base64 `username:password` value of a Basic `Authorization` header.
    pub(crate) fn basic_authorization(&self) -> Zeroizing<Vec<u8>> {
        let mut plain = Zeroizing::new(Vec::with_capacity(
            self.username.len() + self.password.len() + 1,
        ));
        plain.extend_from_slice(&self.username);
        plain.push(b':');
        plain.extend_from_slice(&self.password);
        base64_encode(&plain)
    }
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
    fn http_response_parser_enforces_caller_body_limit_for_fixed_and_chunked_data() {
        for response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata".as_slice(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ndata\r\n0\r\n\r\n",
        ] {
            assert!(parse_http_response_with_limit(response, 3).is_err());
            assert_eq!(
                parse_http_response_with_limit(response, 4).unwrap(),
                b"data"
            );
        }
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
        assert_eq!(&*credentials.basic_authorization(), b"bmlhOnNlY3JldA==");

        assert!(parse_git_credentials(b"username=nia\n").is_none());
        assert!(parse_git_credentials(b"username=nia\npassword=one\npassword=two\n").is_none());
        assert!(parse_git_credentials(b"username=nia:other\npassword=secret\n").is_none());
    }

    #[test]
    fn git_credential_requests_name_the_repository_unambiguously() {
        let request = |source: &str| credential_request(&parse_http_git_url(source).unwrap());
        assert_eq!(
            request("https://example.test/org/repo.git"),
            "protocol=https\nhost=example.test\npath=org/repo.git\n\n"
        );
        assert_eq!(
            request("https://example.test:8443/repo.git"),
            "protocol=https\nhost=example.test:8443\npath=repo.git\n\n"
        );
        assert_eq!(
            request("https://[2001:db8::1]:8443/repo.git"),
            "protocol=https\nhost=[2001:db8::1]:8443\npath=repo.git\n\n"
        );
        assert_eq!(
            request("https://[2001:db8::1]/repo.git"),
            "protocol=https\nhost=[2001:db8::1]\npath=repo.git\n\n"
        );
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
        let git = || {
            let mut command = Command::new("git");
            command
                .args(["-c", &store_arg, "-c", "credential.useHttpPath=true"])
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", &config);
            command
        };
        let credentials = run_http_credential_fill(&url, git()).unwrap();
        assert_eq!(&*credentials.basic_authorization(), b"bmlhOnNlY3JldA==");

        // Rejecting the credentials erases only that repository's entry.
        reject_http_credentials_with(&url, &credentials, git());
        let remaining = std::fs::read_to_string(&store).unwrap();
        assert!(!remaining.contains("nia:secret"), "{remaining}");
        assert!(remaining.contains("other:ignored"), "{remaining}");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// One HTTPS smart-protocol repository served from a local Git checkout.
    /// Requests whose `Authorization` header differs from `accepted` get
    /// `401`; with `accepted` unset every request is served anonymously.
    struct HttpsGitFixture {
        root: std::path::PathBuf,
        port: u16,
        certificate: Vec<u8>,
        revision: String,
        server: Option<std::thread::JoinHandle<Vec<Option<String>>>>,
    }

    impl HttpsGitFixture {
        fn start(label: &str, accepted: Option<&'static str>, connections: usize) -> Self {
            use crate::pktline::encode_packet_line;
            use std::net::TcpListener;
            use std::process::Command;

            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "nia-git-https-{label}-{}-{nonce}",
                std::process::id()
            ));
            let repository = root.join("repository");
            std::fs::create_dir_all(&repository).unwrap();
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
                String::from_utf8(output.stdout).unwrap()
            };
            git(&["init", "-q"]);
            git(&["config", "user.email", "nia-test@example.invalid"]);
            git(&["config", "user.name", "Nia Test"]);
            std::fs::write(repository.join("lib.nia"), "pub fun main() {}\n").unwrap();
            git(&["add", "lib.nia"]);
            git(&["commit", "-qm", "fixture"]);
            let revision = git(&["rev-parse", "HEAD"]).trim().to_string();
            git(&["gc", "--prune=now", "-q"]);
            let pack_path = std::fs::read_dir(repository.join(".git/objects/pack"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "pack")
                })
                .unwrap();
            let pack = std::fs::read(pack_path).unwrap();

            let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
            let certificate = certified.cert.der().to_vec();
            let server_config = Arc::new(
                rustls::ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(
                        vec![rustls::pki_types::CertificateDer::from(certificate.clone())],
                        rustls::pki_types::PrivateKeyDer::Pkcs8(
                            rustls::pki_types::PrivatePkcs8KeyDer::from(
                                certified.signing_key.serialize_der(),
                            ),
                        ),
                    )
                    .unwrap(),
            );
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let advertised = revision.clone();
            let server = std::thread::spawn(move || {
                let mut seen = Vec::new();
                for _ in 0..connections {
                    let (stream, _) = listener.accept().unwrap();
                    let connection = rustls::ServerConnection::new(server_config.clone()).unwrap();
                    let mut stream = rustls::StreamOwned::new(connection, stream);
                    let mut request = Vec::new();
                    let mut byte = [0; 1];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        stream.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                    }
                    let headers = String::from_utf8(request).unwrap();
                    let authorization = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("Authorization: ").map(str::to_owned));
                    let body_length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("Content-Length: "))
                        .map_or(0, |length| length.trim().parse::<usize>().unwrap());
                    let mut request_body = vec![0; body_length];
                    stream.read_exact(&mut request_body).unwrap();
                    seen.push(authorization.clone());
                    let allowed =
                        accepted.is_none_or(|expected| authorization.as_deref() == Some(expected));
                    if !allowed {
                        stream
                            .write_all(
                                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .unwrap();
                    } else {
                        let mut body = Vec::new();
                        if headers.starts_with("GET ") {
                            encode_packet_line(b"# service=git-upload-pack\n", &mut body).unwrap();
                            body.extend_from_slice(b"0000");
                            let line =
                                format!("{advertised} refs/heads/main\0ofs-delta side-band-64k\n");
                            encode_packet_line(line.as_bytes(), &mut body).unwrap();
                            body.extend_from_slice(b"0000");
                        } else {
                            encode_packet_line(b"NAK\n", &mut body).unwrap();
                            for chunk in pack.chunks(60_000) {
                                let mut sideband = vec![1];
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
                    stream.conn.send_close_notify();
                    stream.flush().unwrap();
                }
                seen
            });
            Self {
                root,
                port,
                certificate,
                revision,
                server: Some(server),
            }
        }

        fn url(&self) -> HttpGitUrl {
            parse_http_git_url(&format!("https://127.0.0.1:{}/repository.git", self.port)).unwrap()
        }

        /// A `git` command whose only credential helper answers `get` with
        /// `nia`/`secret` and appends every `store` and `erase` request to a
        /// log, so tests see exactly which reports the client sent.
        fn git_with_recording_helper(
            &self,
            log: &std::path::Path,
        ) -> impl Fn() -> Command + 'static {
            let log_path = log.display().to_string().replace('\\', "/");
            let script = self.root.join("helper.sh");
            std::fs::write(
                &script,
                format!(
                    "case \"$1\" in\n  get) printf 'username=nia\\npassword=secret\\n' ;;\n  store|erase) echo \"$1\" >> '{log_path}'; cat >> '{log_path}' ;;\nesac\n"
                ),
            )
            .unwrap();
            let script = script.display().to_string().replace('\\', "/");
            let config = self.root.join("gitconfig");
            std::fs::write(&config, "").unwrap();
            move || {
                let mut command = Command::new("git");
                command
                    .args([
                        "-c",
                        "credential.helper=",
                        "-c",
                        &format!("credential.helper=!sh '{script}'"),
                        "-c",
                        "credential.useHttpPath=true",
                    ])
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", &config);
                command
            }
        }

        /// A `git` command whose only credential helper is a `store` file.
        fn git_with_store(&self, store: &std::path::Path) -> impl Fn() -> Command + 'static {
            let store_path = store.display().to_string().replace('\\', "/");
            let config = self.root.join("gitconfig");
            std::fs::write(&config, "").unwrap();
            move || {
                let mut command = Command::new("git");
                command
                    .args([
                        "-c",
                        &format!("credential.helper=store --file=\"{store_path}\""),
                        "-c",
                        "credential.useHttpPath=true",
                    ])
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", &config);
                command
            }
        }

        fn checkout(&self, git: &dyn Fn() -> Command) -> io::Result<String> {
            let mut roots = RootCertStore::empty();
            roots
                .add(rustls::pki_types::CertificateDer::from(
                    self.certificate.clone(),
                ))
                .unwrap();
            let destination = self.root.join(format!(
                "checkout-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&destination).unwrap();
            let destination_dir =
                Dir::open_ambient_dir(&destination, cap_std::ambient_authority()).unwrap();
            checkout_http_with(&self.url(), None, &roots, git, "main", &destination_dir)
        }

        fn finish(mut self) -> Vec<Option<String>> {
            let seen = self.server.take().unwrap().join().unwrap();
            std::fs::remove_dir_all(&self.root).unwrap();
            seen
        }
    }

    #[test]
    fn public_https_repositories_never_call_the_credential_helper() {
        let fixture = HttpsGitFixture::start("public", None, 2);
        let calls = std::cell::Cell::new(0);
        let git = || {
            calls.set(calls.get() + 1);
            Command::new("git")
        };
        assert_eq!(fixture.checkout(&git).unwrap(), fixture.revision);
        assert_eq!(
            calls.get(),
            0,
            "public fetch consulted the credential helper"
        );
        assert_eq!(fixture.finish(), [None, None]);
    }

    #[test]
    fn private_https_repositories_authenticate_after_401_and_approve() {
        // `nia:secret`, the value stored below.
        const ACCEPTED: &str = "Basic bmlhOnNlY3JldA==";
        let fixture = HttpsGitFixture::start("private", Some(ACCEPTED), 3);
        let log = fixture.root.join("helper.log");
        let git = fixture.git_with_recording_helper(&log);
        assert_eq!(fixture.checkout(&git).unwrap(), fixture.revision);
        let reports = std::fs::read_to_string(&log).unwrap_or_default();
        // Anonymous first, then one authenticated retry reused for the pack.
        assert_eq!(
            fixture.finish(),
            [None, Some(ACCEPTED.to_owned()), Some(ACCEPTED.to_owned())]
        );
        // Exactly one approval of the credentials that worked, and no erase.
        assert_eq!(reports.matches("store\n").count(), 1, "{reports}");
        assert!(!reports.contains("erase"), "{reports}");
        assert!(
            reports.contains("username=nia\npassword=secret"),
            "{reports}"
        );
    }

    #[test]
    fn rejected_https_credentials_are_erased_from_the_helper() {
        let fixture = HttpsGitFixture::start("rejected", Some("Basic never"), 2);
        let store = fixture.root.join("credentials");
        std::fs::write(
            &store,
            format!(
                "https://nia:stale@127.0.0.1%3a{}/repository.git\n",
                fixture.port
            ),
        )
        .unwrap();
        let git = fixture.git_with_store(&store);
        let error = fixture.checkout(&git).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            error.to_string().contains("credentials were rejected"),
            "{error}"
        );
        let stored = std::fs::read_to_string(&store).unwrap();
        assert_eq!(fixture.finish().len(), 2);
        assert!(!stored.contains("nia:stale"), "{stored}");
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
            HttpRequest {
                authorization: Some(b"bmlhOnNlY3JldA=="),
                ..HttpRequest::get("/repository.git/info/refs")
            },
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
        http_request(
            &url,
            HttpRequest {
                proxy: Some(&proxy),
                ..HttpRequest::get("/repository.git/info/refs?service=git-upload-pack")
            },
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
