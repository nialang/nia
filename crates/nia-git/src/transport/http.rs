// SPDX-License-Identifier: GPL-3.0-or-later
//! HTTP URL and proxy policy for Git transport.

use std::io;

use crate::invalid_packet;

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
