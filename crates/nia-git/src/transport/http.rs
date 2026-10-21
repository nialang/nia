// SPDX-License-Identifier: GPL-3.0-or-later
//! HTTP URL and proxy policy for Git transport.

use std::io::{self, Write};
use std::process::Command;

use crate::invalid_packet;
use crate::object::encode_hex;
use crate::transport::local::{checkout_local, fs_create_dir};
use crate::{
    build_upload_pack_request, http_request_with_proxy, parse_advertised_refs,
    parse_upload_pack_response, select_advertised_revision,
};
use zeroize::Zeroizing;

const MAX_HTTP_RESPONSE: usize = 128 * 1024 * 1024;

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
    destination: &std::path::Path,
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

    let git_dir = destination.join(".git");
    let pack_dir = git_dir.join("objects/pack");
    fs_create_dir(&pack_dir)?;
    std::fs::write(git_dir.join("HEAD"), encode_hex(&commit))?;
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

pub(crate) fn parse_http_response(response: &[u8]) -> io::Result<Vec<u8>> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| invalid_packet("Git HTTP response has no header terminator"))?;
    let header = std::str::from_utf8(&response[..header_end])
        .map_err(|_| invalid_packet("Git HTTP response headers are not UTF-8"))?;
    let mut lines = header.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| invalid_packet("Git HTTP response status is malformed"))?;
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
        if let Some((name, value)) = line.split_once(':') {
            match name.to_ascii_lowercase().as_str() {
                "transfer-encoding" if value.trim().eq_ignore_ascii_case("chunked") => {
                    chunked = true
                }
                "content-length" => {
                    content_length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .map_err(|_| invalid_packet("Git HTTP content length is malformed"))?,
                    )
                }
                _ => {}
            }
        }
    }
    let body = &response[header_end + 4..];
    if chunked {
        decode_chunked_body(body)
    } else if let Some(length) = content_length {
        if body.len() < length {
            return Err(invalid_packet("Git HTTP body is truncated"));
        }
        Ok(body[..length].to_vec())
    } else {
        Ok(body.to_vec())
    }
}

fn decode_chunked_body(mut input: &[u8]) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| invalid_packet("Git HTTP chunk header is truncated"))?;
        let size = usize::from_str_radix(
            std::str::from_utf8(&input[..line_end])
                .map_err(|_| invalid_packet("Git HTTP chunk size is invalid"))?
                .split(';')
                .next()
                .unwrap_or_default(),
            16,
        )
        .map_err(|_| invalid_packet("Git HTTP chunk size is invalid"))?;
        input = &input[line_end + 2..];
        if size == 0 {
            return Ok(output);
        }
        if input.len() < size + 2 || &input[size..size + 2] != b"\r\n" {
            return Err(invalid_packet("Git HTTP chunk is truncated"));
        }
        output.extend_from_slice(&input[..size]);
        if output.len() > MAX_HTTP_RESPONSE {
            return Err(invalid_packet("Git HTTP response is too large"));
        }
        input = &input[size + 2..];
    }
}

pub(crate) fn lookup_http_authorization(url: &HttpGitUrl) -> Option<Zeroizing<Vec<u8>>> {
    run_http_credential_fill(url, Command::new("git"))
}

pub(crate) fn run_http_credential_fill(
    url: &HttpGitUrl,
    mut command: Command,
) -> Option<Zeroizing<Vec<u8>>> {
    use std::process::Stdio;

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

    let mut child = command
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let output = Zeroizing::new(output.stdout);
    let credentials = parse_git_credentials(&output)?;
    let mut plain = Zeroizing::new(Vec::with_capacity(
        credentials.username.len() + credentials.password.len() + 1,
    ));
    plain.extend_from_slice(&credentials.username);
    plain.push(b':');
    plain.extend_from_slice(&credentials.password);
    Some(base64_encode(&plain))
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
