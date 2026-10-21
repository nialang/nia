// SPDX-License-Identifier: GPL-3.0-or-later
//! Git transport implementations.

pub(crate) mod git_tcp;
pub(crate) mod http;
pub(crate) mod local;
mod protocol;
pub(crate) mod ssh;

#[cfg(feature = "fuzzing")]
pub(super) fn fuzz_http_response(input: &[u8]) {
    let _ = http::parse_http_response(input);
}

const MAX_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
