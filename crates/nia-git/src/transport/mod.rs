// SPDX-License-Identifier: GPL-3.0-or-later
//! Git transport implementations.

pub(crate) mod git_tcp;
pub(crate) mod http;
pub(crate) mod local;
mod protocol;
pub(crate) mod ssh;

const MAX_RESPONSE_BYTES: usize = 128 * 1024 * 1024;
