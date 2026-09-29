// SPDX-License-Identifier: GPL-3.0-or-later
//! Cache restoration and environment identity for final linked artifacts.

use std::path::Path;

use nia_linker::{ArchiveOptions, LinkOptions};

use super::*;

impl Driver {
    /// Restores an executable artifact when its complete cache identity matches.
    pub fn restore_executable_cache(
        &self,
        reference: ExecutableCacheReference,
        output: &Path,
    ) -> ExecutableCacheRestore {
        let Some(cache) = &self.link_cache else {
            return ExecutableCacheRestore::Disabled;
        };
        // The environment is described exactly as a link prepares it.
        let Ok((options, _)) = self.prepare_link_options(LinkOptions::default(), Path::new(""))
        else {
            return ExecutableCacheRestore::Invalidated;
        };
        if !matches!(
            options.matches_result_environment(
                self.config.artifact_target,
                reference.fingerprints.components,
                self.config.toolchain.identity().fingerprint(),
            ),
            Ok(true)
        ) {
            return ExecutableCacheRestore::Invalidated;
        }
        match cache.restore(reference.fingerprints, output) {
            Ok(lookup) => match lookup {
                crate::executable_cache::LinkResultCacheLookup::Hit => ExecutableCacheRestore::Hit,
                crate::executable_cache::LinkResultCacheLookup::NotFound => {
                    ExecutableCacheRestore::NotFound
                }
                crate::executable_cache::LinkResultCacheLookup::Invalidated(_) => {
                    ExecutableCacheRestore::Invalidated
                }
                crate::executable_cache::LinkResultCacheLookup::Corrupt => {
                    ExecutableCacheRestore::Corrupt
                }
            },
            Err(_) => ExecutableCacheRestore::ReadError,
        }
    }

    /// Returns the current executable cache environment fingerprint.
    pub fn executable_cache_environment(&self) -> Option<ExecutableCacheEnvironment> {
        self.executable_cache_environment_for(&LinkOptions::default())
    }

    /// Computes an executable cache environment for explicit link options.
    pub fn executable_cache_environment_for(
        &self,
        link_options: &LinkOptions,
    ) -> Option<ExecutableCacheEnvironment> {
        self.link_cache.as_ref()?;
        let (link_options, _) = self
            .prepare_link_options(link_options.clone(), Path::new(""))
            .ok()?;
        link_options
            .result_environment_fingerprint(
                self.config.artifact_target,
                self.config.toolchain.identity().fingerprint(),
            )
            .ok()?
            .map(|fingerprint| ExecutableCacheEnvironment { fingerprint })
    }

    /// Restores a static archive when its complete cache identity matches.
    pub fn restore_static_archive_cache(
        &self,
        reference: StaticArchiveCacheReference,
        output: &Path,
    ) -> StaticArchiveCacheRestore {
        let Some(cache) = &self.archive_cache else {
            return StaticArchiveCacheRestore::Disabled;
        };
        let options = ArchiveOptions::default();
        if !matches!(
            options.matches_result_environment(
                self.config.artifact_target,
                reference.fingerprints.components,
                self.config.toolchain.identity().fingerprint(),
            ),
            Ok(true)
        ) {
            return StaticArchiveCacheRestore::Invalidated;
        }
        match cache.restore(reference.fingerprints, output) {
            Ok(lookup) => match lookup {
                crate::archive_cache::ArchiveCacheLookup::Hit => StaticArchiveCacheRestore::Hit,
                crate::archive_cache::ArchiveCacheLookup::NotFound => {
                    StaticArchiveCacheRestore::NotFound
                }
                crate::archive_cache::ArchiveCacheLookup::Invalidated(_) => {
                    StaticArchiveCacheRestore::Invalidated
                }
                crate::archive_cache::ArchiveCacheLookup::Corrupt => {
                    StaticArchiveCacheRestore::Corrupt
                }
            },
            Err(_) => StaticArchiveCacheRestore::ReadError,
        }
    }

    /// Returns the current static archive cache environment fingerprint.
    pub fn static_archive_cache_environment(&self) -> Option<StaticArchiveCacheEnvironment> {
        self.archive_cache.as_ref()?;
        ArchiveOptions::default()
            .environment_fingerprint(
                self.config.artifact_target,
                self.config.toolchain.identity().fingerprint(),
            )
            .ok()
            .map(|fingerprint| StaticArchiveCacheEnvironment { fingerprint })
    }
}
