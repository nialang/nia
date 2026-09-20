// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrontendCachePublication {
    Immediate,
    Deferred { replaced: bool },
}

#[derive(Default)]
pub(super) struct PendingFrontendCachePublications {
    program_sources: Option<FrontendProgramSources>,
    type_resolutions: HashMap<
        (StableModuleKey, nia_item_tree::SignatureItemSet),
        PendingFrontendCacheProduct<TypeResolution>,
    >,
    type_lowerings: HashMap<
        (StableModuleKey, nia_item_tree::SignatureItemSet),
        PendingFrontendCacheProduct<TypeLowering>,
    >,
    item_signatures: HashMap<
        (StableModuleKey, nia_item_tree::SignatureItemSet),
        PendingFrontendCacheProduct<ItemSignatures>,
    >,
}

struct PendingFrontendCacheProduct<T> {
    program_sources: crate::FrontendProgramSourceFingerprint,
    value: Arc<T>,
}

impl CompilerContext {
    pub(super) fn observe_frontend_program_sources(&self, sources: &FrontendProgramSources) {
        let mut publications = self.frontend_cache_publications.lock();
        let Some(publications) = publications.as_mut() else {
            return;
        };
        if publications
            .program_sources
            .as_ref()
            .is_none_or(|observed| observed.fingerprint != sources.fingerprint)
        {
            publications.program_sources = Some(sources.clone());
        }
    }

    pub(super) fn begin_frontend_cache_publications(&self) -> QueryResult<()> {
        let mut publications = self.frontend_cache_publications.lock();
        if publications.is_some() {
            return Err(QueryError::internal(
                "frontend cache publication session is already active",
            ));
        }
        *publications = Some(PendingFrontendCachePublications::default());
        Ok(())
    }

    pub(super) fn finish_frontend_cache_publications(&self) -> PendingFrontendCachePublications {
        self.frontend_cache_publications
            .lock()
            .take()
            .unwrap_or_default()
    }

    pub(super) fn defer_signature_type_resolution(
        &self,
        module: StableModuleKey,
        set: nia_item_tree::SignatureItemSet,
        program_sources: crate::FrontendProgramSourceFingerprint,
        value: Arc<TypeResolution>,
    ) -> FrontendCachePublication {
        let mut publications = self.frontend_cache_publications.lock();
        let Some(publications) = publications.as_mut() else {
            return FrontendCachePublication::Immediate;
        };
        let replaced = publications
            .type_resolutions
            .insert(
                (module, set),
                PendingFrontendCacheProduct {
                    program_sources,
                    value,
                },
            )
            .is_some();
        FrontendCachePublication::Deferred { replaced }
    }

    pub(super) fn deferred_signature_type_resolution(
        &self,
        module: &StableModuleKey,
        set: nia_item_tree::SignatureItemSet,
        program_sources: crate::FrontendProgramSourceFingerprint,
    ) -> Option<Arc<TypeResolution>> {
        self.frontend_cache_publications
            .lock()
            .as_ref()?
            .type_resolutions
            .get(&(module.clone(), set))
            .filter(|product| product.program_sources == program_sources)
            .map(|product| Arc::clone(&product.value))
    }

    pub(super) fn defer_signature_type_lowering(
        &self,
        module: StableModuleKey,
        set: nia_item_tree::SignatureItemSet,
        program_sources: crate::FrontendProgramSourceFingerprint,
        value: Arc<TypeLowering>,
    ) -> FrontendCachePublication {
        let mut publications = self.frontend_cache_publications.lock();
        let Some(publications) = publications.as_mut() else {
            return FrontendCachePublication::Immediate;
        };
        let replaced = publications
            .type_lowerings
            .insert(
                (module, set),
                PendingFrontendCacheProduct {
                    program_sources,
                    value,
                },
            )
            .is_some();
        FrontendCachePublication::Deferred { replaced }
    }

    pub(super) fn deferred_signature_type_lowering(
        &self,
        module: &StableModuleKey,
        set: nia_item_tree::SignatureItemSet,
        program_sources: crate::FrontendProgramSourceFingerprint,
    ) -> Option<Arc<TypeLowering>> {
        self.frontend_cache_publications
            .lock()
            .as_ref()?
            .type_lowerings
            .get(&(module.clone(), set))
            .filter(|product| product.program_sources == program_sources)
            .map(|product| Arc::clone(&product.value))
    }

    pub(super) fn defer_signature_item_signatures(
        &self,
        module: StableModuleKey,
        set: nia_item_tree::SignatureItemSet,
        program_sources: crate::FrontendProgramSourceFingerprint,
        value: Arc<ItemSignatures>,
    ) -> FrontendCachePublication {
        let mut publications = self.frontend_cache_publications.lock();
        let Some(publications) = publications.as_mut() else {
            return FrontendCachePublication::Immediate;
        };
        let replaced = publications
            .item_signatures
            .insert(
                (module, set),
                PendingFrontendCacheProduct {
                    program_sources,
                    value,
                },
            )
            .is_some();
        FrontendCachePublication::Deferred { replaced }
    }

    pub(super) fn deferred_signature_item_signatures(
        &self,
        module: &StableModuleKey,
        set: nia_item_tree::SignatureItemSet,
        program_sources: crate::FrontendProgramSourceFingerprint,
    ) -> Option<Arc<ItemSignatures>> {
        self.frontend_cache_publications
            .lock()
            .as_ref()?
            .item_signatures
            .get(&(module.clone(), set))
            .filter(|product| product.program_sources == program_sources)
            .map(|product| Arc::clone(&product.value))
    }
}

impl CompilerDatabase {
    pub(super) fn flush_frontend_cache_publications(
        &self,
        publications: PendingFrontendCachePublications,
    ) {
        let Some(cache) = self.db.context().signature_cache.as_ref() else {
            return;
        };
        if publications.type_resolutions.is_empty()
            && publications.type_lowerings.is_empty()
            && publications.item_signatures.is_empty()
        {
            return;
        }
        let Some(program_sources) = publications.program_sources.as_ref() else {
            nia_timing::emit_counter("frontend.signature_reuse_flush_errors", 1);
            return;
        };
        let namespace = self.db.context().frontend_cache_namespace();
        let symbols = self.db.context().symbols();
        for ((module, set), resolution) in publications.type_resolutions {
            if resolution.program_sources != program_sources.fingerprint {
                nia_timing::emit_counter(
                    "frontend.signature_type_resolution_reuse_publish_stale_skipped",
                    1,
                );
                continue;
            }
            let Some(source) = final_program_source(program_sources, &module) else {
                nia_timing::emit_counter("frontend.signature_reuse_flush_errors", 1);
                continue;
            };
            let key = crate::FrontendSignatureTypeResolutionCacheKey::new(
                namespace,
                &module,
                set,
                program_sources.fingerprint,
            );
            nia_timing::emit_counter(
                "frontend.signature_type_resolution_reuse_publish_attempts",
                1,
            );
            let publication = cache.publish_type_resolution(
                crate::signature_cache::SignatureTypeResolutionIdentity {
                    key,
                    namespace,
                    module: &module,
                    set,
                    program_sources: program_sources.fingerprint,
                    source_version: source.version,
                    source_len: source.len,
                },
                &resolution.value,
                &program_sources.path_by_module,
                &symbols,
                false,
            );
            emit_frontend_cache_publication_result(
                publication,
                "frontend.signature_type_resolution_reuse_publish_successes",
                "frontend.signature_type_resolution_reuse_publish_errors",
            );
        }
        for ((module, set), lowering) in publications.type_lowerings {
            if lowering.program_sources != program_sources.fingerprint {
                nia_timing::emit_counter(
                    "frontend.signature_type_lowering_reuse_publish_stale_skipped",
                    1,
                );
                continue;
            }
            let Some(source) = final_program_source(program_sources, &module) else {
                nia_timing::emit_counter("frontend.signature_reuse_flush_errors", 1);
                continue;
            };
            let key = crate::FrontendSignatureTypeLoweringCacheKey::new(
                namespace,
                &module,
                set,
                program_sources.fingerprint,
            );
            nia_timing::emit_counter("frontend.signature_type_lowering_reuse_publish_attempts", 1);
            let publication = cache.publish_type_lowering(
                crate::signature_cache::SignatureTypeLoweringIdentity {
                    key,
                    namespace,
                    module: &module,
                    set,
                    program_sources: program_sources.fingerprint,
                    source_version: source.version,
                    source_len: source.len,
                },
                &lowering.value,
                &program_sources.path_by_module,
                &symbols,
                self.db.context().type_store(),
                false,
            );
            emit_frontend_cache_publication_result(
                publication,
                "frontend.signature_type_lowering_reuse_publish_successes",
                "frontend.signature_type_lowering_reuse_publish_errors",
            );
        }
        for ((module, set), signatures) in publications.item_signatures {
            if signatures.program_sources != program_sources.fingerprint {
                nia_timing::emit_counter(
                    "frontend.signature_item_signatures_reuse_publish_stale_skipped",
                    1,
                );
                continue;
            }
            let Some(source) = final_program_source(program_sources, &module) else {
                nia_timing::emit_counter("frontend.signature_reuse_flush_errors", 1);
                continue;
            };
            let key = crate::FrontendSignatureItemSignaturesCacheKey::new(
                namespace,
                &module,
                set,
                program_sources.fingerprint,
            );
            nia_timing::emit_counter(
                "frontend.signature_item_signatures_reuse_publish_attempts",
                1,
            );
            let publication = cache.publish_item_signatures(
                crate::signature_cache::SignatureItemSignaturesIdentity {
                    key,
                    namespace,
                    module: &module,
                    set,
                    program_sources: program_sources.fingerprint,
                    source_len: source.len,
                },
                &signatures.value,
                &program_sources.path_by_module,
                &symbols,
                self.db.context().type_store(),
                false,
            );
            emit_frontend_cache_publication_result(
                publication,
                "frontend.signature_item_signatures_reuse_publish_successes",
                "frontend.signature_item_signatures_reuse_publish_errors",
            );
        }
    }
}

fn final_program_source<'a>(
    program_sources: &'a FrontendProgramSources,
    module: &StableModuleKey,
) -> Option<&'a FrontendProgramSource> {
    let module_id = program_sources
        .module_by_path
        .get(module.source_identity().normalized_path())?;
    program_sources.by_module.get(module_id)
}

pub(super) fn emit_frontend_cache_publication_deferral(
    publication: FrontendCachePublication,
    deferred_counter: &'static str,
    replaced_counter: &'static str,
) -> bool {
    let FrontendCachePublication::Deferred { replaced } = publication else {
        return false;
    };
    nia_timing::emit_counter(deferred_counter, 1);
    if replaced {
        nia_timing::emit_counter(replaced_counter, 1);
    }
    true
}

pub(super) fn emit_frontend_cache_publication_result(
    publication: std::io::Result<()>,
    success_counter: &'static str,
    error_counter: &'static str,
) {
    nia_timing::emit_counter(
        if publication.is_ok() {
            success_counter
        } else {
            error_counter
        },
        1,
    );
}
