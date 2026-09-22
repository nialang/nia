// SPDX-License-Identifier: GPL-3.0-or-later
//! Link and archive workflows for driver-produced object artifacts.

use std::{fs, path::PathBuf, process::Command};

use nia_linker::{ArchiveOptions, LinkOptions, LinkTarget};
use nia_toolchain::RuntimeSpec;

use super::output::{
    TempDir, archive_member_file_name, install_streamed_output, object_file_name, write_output_file,
};
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkResultReuse {
    Hit,
    Miss(LinkResultReuseMiss),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkResultReuseMiss {
    Disabled,
    Uncacheable,
    NotFound,
    Invalidated(nia_linker::LinkResultInvalidation),
    Corrupt,
    ReadError,
}

fn emit_link_result_reuse(timings: TimingMode, reuse: LinkResultReuse) {
    if !timings.enabled() {
        return;
    }
    let hit = u64::from(reuse == LinkResultReuse::Hit);
    nia_timing::emit_counter("link.result_reuse_hits", hit);
    nia_timing::emit_counter("link.result_reuse_misses", 1 - hit);
    for (name, reason) in [
        (
            "link.result_reuse_miss_disabled",
            LinkResultReuseMiss::Disabled,
        ),
        (
            "link.result_reuse_miss_uncacheable",
            LinkResultReuseMiss::Uncacheable,
        ),
        (
            "link.result_reuse_miss_not_found",
            LinkResultReuseMiss::NotFound,
        ),
        (
            "link.result_reuse_miss_corrupt",
            LinkResultReuseMiss::Corrupt,
        ),
        (
            "link.result_reuse_miss_read_error",
            LinkResultReuseMiss::ReadError,
        ),
    ] {
        nia_timing::emit_counter(name, u64::from(reuse == LinkResultReuse::Miss(reason)));
    }
    let invalidation = match reuse {
        LinkResultReuse::Miss(LinkResultReuseMiss::Invalidated(reasons)) => Some(reasons),
        _ => None,
    };
    nia_timing::emit_counter(
        "link.result_reuse_miss_invalidated",
        u64::from(invalidation.is_some()),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_inputs",
        u64::from(invalidation.is_some_and(|reasons| reasons.inputs)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_toolchain",
        u64::from(invalidation.is_some_and(|reasons| reasons.toolchain)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_target",
        u64::from(invalidation.is_some_and(|reasons| reasons.target)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_linker",
        u64::from(invalidation.is_some_and(|reasons| reasons.linker)),
    );
    nia_timing::emit_counter(
        "link.result_invalidation_options",
        u64::from(invalidation.is_some_and(|reasons| reasons.options)),
    );
}

impl Driver {
    /// Links an executable from a checked request and writes it to `output`.
    pub fn link_executable(
        &self,
        request: LinkExecutableRequest,
    ) -> DriverOutput<ExecutableArtifact> {
        self.link_executable_with_source_manifest(request)
            .map(|output| output.artifact)
    }

    /// Links an executable while retaining the exact source manifest.
    pub fn link_executable_with_source_manifest(
        &self,
        request: LinkExecutableRequest,
    ) -> DriverOutput<LinkedExecutableWithSourceManifest> {
        let mut request = request;
        request.link_options.target = LinkTarget::from_target_config(&self.config.artifact_target);
        if request.link_options.linker.flavor == nia_linker::LinkerFlavor::Lld
            && request.link_options.linker.program.is_empty()
            && let Some(lld) = self.config.toolchain.bundled_lld()
        {
            request.link_options.linker = request
                .link_options
                .linker
                .clone()
                .with_bundled_program(lld.to_string_lossy());
        }
        let timings = request.check.timings;
        let runtime =
            match RuntimeSpec::freestanding(&self.config.toolchain, &self.config.artifact_target) {
                Ok(runtime) => runtime,
                Err(error) => return DriverOutput::from_error(DriverError::Runtime(error)),
            };
        request.link_options.entry = runtime
            .source()
            .map(|runtime| runtime.entry_point().linker_symbol().to_owned());
        let preserved_symbols = request
            .link_options
            .entry
            .as_deref()
            .into_iter()
            .collect::<Vec<_>>();
        let linkage_source_identity = request.check.entry_path.identity();
        let emission_mode = match request.link_time_optimization {
            LinkTimeOptimization::Off => ObjectEmissionMode::NoLto,
            LinkTimeOptimization::Thin => ObjectEmissionMode::Lto {
                lto_mode: nia_codegen_llvm::LtoMode::Thin,
                linkage_source_identity: &linkage_source_identity,
                preserved_symbols: &preserved_symbols,
                freestanding: true,
                backend_cache_directory: self.thin_lto_backend_cache_directory.as_deref(),
            },
            LinkTimeOptimization::Full => ObjectEmissionMode::Lto {
                lto_mode: nia_codegen_llvm::LtoMode::Full,
                linkage_source_identity: &linkage_source_identity,
                preserved_symbols: &preserved_symbols,
                freestanding: true,
                backend_cache_directory: None,
            },
        };
        let emission_stage = match request.link_time_optimization {
            LinkTimeOptimization::Off => "emit_native_objects",
            LinkTimeOptimization::Thin => "emit_thin_lto_objects",
            LinkTimeOptimization::Full => "emit_full_lto_objects",
        };
        let output = nia_timing::time_stage(
            timings,
            nia_timing::TimingLevel::Summary,
            emission_stage,
            || {
                self.emit_objects_with_source_manifest(
                    EmitObjectRequest {
                        check: request.check.with_runtime(runtime),
                    },
                    emission_mode,
                )
            },
        );
        let objects = match output.result {
            Ok(objects) => objects,
            Err(error) => return DriverOutput::from_error(error),
        };
        let linked = self.link_executable_from_objects(
            &objects.artifact,
            request.output,
            request.link_options,
            timings,
        );
        match linked.result {
            Ok(artifact) => DriverOutput::success(LinkedExecutableWithSourceManifest {
                artifact,
                source_manifest: objects.source_manifest,
            }),
            Err(error) => DriverOutput::from_error(error),
        }
    }

    /// Links an executable from already emitted object inputs.
    pub fn link_executable_from_objects(
        &self,
        objects: &ObjectArtifact,
        output: PathBuf,
        mut link_options: LinkOptions,
        timings: TimingMode,
    ) -> DriverOutput<ExecutableArtifact> {
        link_options.target = LinkTarget::from_target_config(&self.config.artifact_target);
        let link_fingerprint = match link_options.result_fingerprint(
            &objects.link_inputs,
            self.config.toolchain.identity().fingerprint(),
        ) {
            Ok(fingerprint) => fingerprint,
            Err(error) => return DriverOutput::from_error(DriverError::LinkerConfig(error)),
        };
        let reuse = match (&self.link_cache, link_fingerprint) {
            (None, _) => LinkResultReuse::Miss(LinkResultReuseMiss::Disabled),
            (Some(_), None) => LinkResultReuse::Miss(LinkResultReuseMiss::Uncacheable),
            (Some(cache), Some(fingerprint)) => match cache.restore(fingerprint, &output) {
                Ok(crate::executable_cache::LinkResultCacheLookup::Hit) => LinkResultReuse::Hit,
                Ok(crate::executable_cache::LinkResultCacheLookup::NotFound) => {
                    LinkResultReuse::Miss(LinkResultReuseMiss::NotFound)
                }
                Ok(crate::executable_cache::LinkResultCacheLookup::Invalidated(reasons)) => {
                    LinkResultReuse::Miss(LinkResultReuseMiss::Invalidated(reasons))
                }
                Ok(crate::executable_cache::LinkResultCacheLookup::Corrupt) => {
                    LinkResultReuse::Miss(LinkResultReuseMiss::Corrupt)
                }
                Err(_) => LinkResultReuse::Miss(LinkResultReuseMiss::ReadError),
            },
        };
        emit_link_result_reuse(timings, reuse);
        if reuse == LinkResultReuse::Hit {
            return DriverOutput::success(ExecutableArtifact {
                path: output,
                optimization: objects.optimization,
                optimization_report: objects.optimization_report.clone(),
                diagnostics: objects.diagnostics.clone(),
                cache_reference: link_fingerprint.map(ExecutableCacheReference::from),
            });
        }
        let temp = TempDir::new("nia_emit_exe");
        let link_inputs = nia_timing::time_stage(
            timings,
            nia_timing::TimingLevel::Summary,
            "link_prepare_inputs",
            || {
                if let Err(error) = fs::create_dir_all(temp.path()) {
                    return Err(DriverError::Io {
                        path: temp.path().to_path_buf(),
                        operation: "create temporary object directory",
                        error,
                    });
                }
                let mut link_inputs = Vec::with_capacity(objects.link_inputs.len());
                for (index, input) in objects.link_inputs.as_slice().iter().enumerate() {
                    let object_path = temp
                        .path()
                        .join(object_file_name(index, &input.object.name));
                    if let Err(error) = write_output_file(&object_path, &input.object.bytes) {
                        return Err(DriverError::Io {
                            path: object_path,
                            operation: "write temporary object file",
                            error,
                        });
                    }
                    link_inputs.push(nia_codegen_llvm::IncrementalLinkInput {
                        key: input.key.clone(),
                        fingerprint: input.fingerprint,
                        object: object_path,
                    });
                }
                nia_codegen_llvm::IncrementalLinkInputs::new(link_inputs)
                    .map_err(|error| DriverError::InternalDiagnostic(Diagnostic::from(error)))
            },
        );
        let link_inputs = match link_inputs {
            Ok(inputs) => inputs,
            Err(error) => return DriverOutput::from_error(error),
        };
        if let Some(parent) = output.parent()
            && !parent.as_os_str().is_empty()
            && let Err(error) = fs::create_dir_all(parent)
        {
            return DriverOutput::from_error(DriverError::Io {
                path: parent.to_path_buf(),
                operation: "create executable output directory",
                error,
            });
        }

        let invocation = match link_options.invocation(&link_inputs, output.clone()) {
            Ok(invocation) => invocation,
            Err(error) => return DriverOutput::from_error(DriverError::LinkerConfig(error)),
        };
        let linker_status = nia_timing::time_stage(
            timings,
            nia_timing::TimingLevel::Summary,
            "link_invoke",
            || {
                Command::new(&invocation.program)
                    .args(&invocation.args)
                    .status()
            },
        );
        match linker_status {
            Ok(status) if status.success() => {
                let cache_reference = nia_timing::time_stage(
                    timings,
                    nia_timing::TimingLevel::Summary,
                    "link_publish_result",
                    || {
                        if let (Some(cache), Some(fingerprint)) =
                            (&self.link_cache, link_fingerprint)
                        {
                            let publish_error = cache.publish(fingerprint, &output).is_err();
                            if timings.enabled() {
                                nia_timing::emit_counter(
                                    "link.result_cache_publish_errors",
                                    u64::from(publish_error),
                                );
                            }
                            (!publish_error).then(|| ExecutableCacheReference::from(fingerprint))
                        } else {
                            None
                        }
                    },
                );
                DriverOutput::success(ExecutableArtifact {
                    path: output,
                    optimization: objects.optimization,
                    optimization_report: objects.optimization_report.clone(),
                    diagnostics: objects.diagnostics.clone(),
                    cache_reference,
                })
            }
            Ok(status) => DriverOutput::from_error(DriverError::LinkerStatus {
                program: invocation.program,
                status,
            }),
            Err(error) => DriverOutput::from_error(DriverError::LinkerIo {
                program: invocation.program,
                error,
            }),
        }
    }

    /// Archives already emitted objects into a static library.
    pub fn archive_static_library_from_objects(
        &self,
        objects: &ObjectArtifact,
        output: PathBuf,
        mut archive_options: ArchiveOptions,
    ) -> DriverOutput<StaticArchiveArtifact> {
        archive_options.target = LinkTarget::from_target_config(&self.config.artifact_target);
        let archive_fingerprint = match archive_options.result_fingerprint(
            &objects.link_inputs,
            self.config.toolchain.identity().fingerprint(),
        ) {
            Ok(fingerprint) => fingerprint,
            Err(error) => return DriverOutput::from_error(DriverError::ArchiveConfig(error)),
        };
        if let Some(cache) = &self.archive_cache
            && matches!(
                cache.restore(archive_fingerprint, &output),
                Ok(crate::archive_cache::ArchiveCacheLookup::Hit)
            )
        {
            return DriverOutput::success(StaticArchiveArtifact {
                path: output,
                optimization: objects.optimization,
                optimization_report: objects.optimization_report.clone(),
                diagnostics: objects.diagnostics.clone(),
                cache_reference: Some(StaticArchiveCacheReference::from(archive_fingerprint)),
            });
        }
        let temp = TempDir::new("nia_archive");
        if let Err(error) = fs::create_dir_all(temp.path()) {
            return DriverOutput::from_error(DriverError::Io {
                path: temp.path().to_path_buf(),
                operation: "create temporary archive directory",
                error,
            });
        }
        let mut inputs = Vec::with_capacity(objects.link_inputs.len());
        for (index, input) in objects.link_inputs.as_slice().iter().enumerate() {
            let object_path = temp
                .path()
                .join(archive_member_file_name(index, &input.key));
            if let Err(error) = write_output_file(&object_path, &input.object.bytes) {
                return DriverOutput::from_error(DriverError::Io {
                    path: object_path,
                    operation: "write temporary archive member",
                    error,
                });
            }
            inputs.push(object_path);
        }
        let temporary_archive = temp.path().join("output.a");
        let invocation = match archive_options.invocation(&inputs, temporary_archive.clone()) {
            Ok(invocation) => invocation,
            Err(error) => return DriverOutput::from_error(DriverError::ArchiveConfig(error)),
        };
        match Command::new(&invocation.program)
            .args(&invocation.args)
            .status()
        {
            Ok(status) if status.success() => {
                if let Err(error) = install_streamed_output(&temporary_archive, &output) {
                    return DriverOutput::from_error(DriverError::Io {
                        path: output,
                        operation: "install temporary static archive",
                        error,
                    });
                }
                let cache_reference = self.archive_cache.as_ref().and_then(|cache| {
                    cache
                        .publish(archive_fingerprint, &output)
                        .ok()
                        .map(|()| StaticArchiveCacheReference::from(archive_fingerprint))
                });
                DriverOutput::success(StaticArchiveArtifact {
                    path: output,
                    optimization: objects.optimization,
                    optimization_report: objects.optimization_report.clone(),
                    diagnostics: objects.diagnostics.clone(),
                    cache_reference,
                })
            }
            Ok(status) => DriverOutput::from_error(DriverError::ArchiveStatus {
                program: invocation.program,
                status,
            }),
            Err(error) => DriverOutput::from_error(DriverError::ArchiveIo {
                program: invocation.program,
                error,
            }),
        }
    }
}
