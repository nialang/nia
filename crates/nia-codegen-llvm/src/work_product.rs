// SPDX-License-Identifier: GPL-3.0-or-later
use std::io;

use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitKey};

#[derive(Debug, PartialEq, Eq)]
pub enum ObjectWorkProductLookup {
    Hit(Vec<u8>),
    NotFound,
    Corrupt,
}

pub trait ObjectWorkProductCache: Send + Sync {
    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprint: CodegenUnitFingerprint,
    ) -> io::Result<ObjectWorkProductLookup>;

    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprint: CodegenUnitFingerprint,
        bytes: &[u8],
    ) -> io::Result<()>;
}
