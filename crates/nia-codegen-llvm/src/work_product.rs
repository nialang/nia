// SPDX-License-Identifier: GPL-3.0-or-later
use std::io;

use nia_backend_ir::{CodegenUnitFingerprint, CodegenUnitKey};

pub trait ObjectWorkProductCache: Send + Sync {
    fn load(
        &self,
        key: &CodegenUnitKey,
        fingerprint: CodegenUnitFingerprint,
    ) -> io::Result<Option<Vec<u8>>>;

    fn publish(
        &self,
        key: &CodegenUnitKey,
        fingerprint: CodegenUnitFingerprint,
        bytes: &[u8],
    ) -> io::Result<()>;
}
