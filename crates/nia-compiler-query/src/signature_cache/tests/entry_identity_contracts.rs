use super::*;

fn stable_module(path: &str) -> StableModuleKey {
    StableModuleKey::from_source_identity(SourceIdentity::new(path))
}

fn source_version() -> SourceVersion {
    SourceVersion {
        id: SourceId(1),
        revision: SourceRevision(1),
    }
}

#[test]
fn type_resolution_entry_identity_rejects_each_stale_field() {
    let module = stable_module("src/main.nia");
    let other_module = stable_module("src/other.nia");
    let identity = SignatureTypeResolutionIdentity {
        key: crate::FrontendSignatureTypeResolutionCacheKey::from_parts([1, 2]),
        namespace: crate::FrontendCacheNamespace::from_parts([3, 4]),
        module: &module,
        set: SignatureItemSet::Types,
        program_sources: crate::FrontendProgramSourceFingerprint::from_parts([5, 6]),
        source_version: source_version(),
        source_len: 31,
    };
    let encoded = encode_entry(identity, b"type-resolution");

    assert_eq!(
        decode_entry(&encoded, identity),
        Some(b"type-resolution".as_slice())
    );
    for stale in [
        SignatureTypeResolutionIdentity {
            key: crate::FrontendSignatureTypeResolutionCacheKey::from_parts([7, 8]),
            ..identity
        },
        SignatureTypeResolutionIdentity {
            namespace: crate::FrontendCacheNamespace::from_parts([9, 10]),
            ..identity
        },
        SignatureTypeResolutionIdentity {
            module: &other_module,
            ..identity
        },
        SignatureTypeResolutionIdentity {
            set: SignatureItemSet::Values,
            ..identity
        },
        SignatureTypeResolutionIdentity {
            program_sources: crate::FrontendProgramSourceFingerprint::from_parts([11, 12]),
            ..identity
        },
        SignatureTypeResolutionIdentity {
            source_len: 32,
            ..identity
        },
    ] {
        assert_eq!(decode_entry(&encoded, stale), None);
    }
}

#[test]
fn type_lowering_entry_identity_rejects_each_stale_field() {
    let module = stable_module("src/main.nia");
    let other_module = stable_module("src/other.nia");
    let identity = SignatureTypeLoweringIdentity {
        key: crate::FrontendSignatureTypeLoweringCacheKey::from_parts([1, 2]),
        namespace: crate::FrontendCacheNamespace::from_parts([3, 4]),
        module: &module,
        set: SignatureItemSet::Types,
        program_sources: crate::FrontendProgramSourceFingerprint::from_parts([5, 6]),
        source_version: source_version(),
        source_len: 31,
    };
    let encoded = encode_type_lowering_entry(identity, b"type-lowering");

    assert_eq!(
        decode_type_lowering_entry(&encoded, identity),
        Some(b"type-lowering".as_slice())
    );
    for stale in [
        SignatureTypeLoweringIdentity {
            key: crate::FrontendSignatureTypeLoweringCacheKey::from_parts([7, 8]),
            ..identity
        },
        SignatureTypeLoweringIdentity {
            namespace: crate::FrontendCacheNamespace::from_parts([9, 10]),
            ..identity
        },
        SignatureTypeLoweringIdentity {
            module: &other_module,
            ..identity
        },
        SignatureTypeLoweringIdentity {
            set: SignatureItemSet::Values,
            ..identity
        },
        SignatureTypeLoweringIdentity {
            program_sources: crate::FrontendProgramSourceFingerprint::from_parts([11, 12]),
            ..identity
        },
        SignatureTypeLoweringIdentity {
            source_len: 32,
            ..identity
        },
    ] {
        assert_eq!(decode_type_lowering_entry(&encoded, stale), None);
    }
}

#[test]
fn item_signatures_entry_identity_rejects_each_stale_field() {
    let module = stable_module("src/main.nia");
    let other_module = stable_module("src/other.nia");
    let identity = SignatureItemSignaturesIdentity {
        key: crate::FrontendSignatureItemSignaturesCacheKey::from_parts([1, 2]),
        namespace: crate::FrontendCacheNamespace::from_parts([3, 4]),
        module: &module,
        set: SignatureItemSet::Types,
        program_sources: crate::FrontendProgramSourceFingerprint::from_parts([5, 6]),
        source_len: 31,
    };
    let encoded = encode_item_signatures_entry(identity, b"item-signatures");

    assert_eq!(
        decode_item_signatures_entry(&encoded, identity),
        Some(b"item-signatures".as_slice())
    );
    for stale in [
        SignatureItemSignaturesIdentity {
            key: crate::FrontendSignatureItemSignaturesCacheKey::from_parts([7, 8]),
            ..identity
        },
        SignatureItemSignaturesIdentity {
            namespace: crate::FrontendCacheNamespace::from_parts([9, 10]),
            ..identity
        },
        SignatureItemSignaturesIdentity {
            module: &other_module,
            ..identity
        },
        SignatureItemSignaturesIdentity {
            set: SignatureItemSet::Values,
            ..identity
        },
        SignatureItemSignaturesIdentity {
            program_sources: crate::FrontendProgramSourceFingerprint::from_parts([11, 12]),
            ..identity
        },
        SignatureItemSignaturesIdentity {
            source_len: 32,
            ..identity
        },
    ] {
        assert_eq!(decode_item_signatures_entry(&encoded, stale), None);
    }
}

#[test]
fn extension_diagnostics_entry_identity_rejects_each_stale_field() {
    let module = stable_module("src/main.nia");
    let other_module = stable_module("src/other.nia");
    let identity = ExtensionValidationDiagnosticsIdentity {
        key: crate::FrontendExtensionValidationDiagnosticsCacheKey::from_parts([1, 2]),
        namespace: crate::FrontendCacheNamespace::from_parts([3, 4]),
        module: &module,
        program_sources: crate::FrontendProgramSourceFingerprint::from_parts([5, 6]),
        source_len: 31,
    };
    let encoded = encode_extension_validation_diagnostics_entry(identity, b"diagnostics");

    assert_eq!(
        decode_extension_validation_diagnostics_entry(&encoded, identity),
        Some(b"diagnostics".as_slice())
    );
    for stale in [
        ExtensionValidationDiagnosticsIdentity {
            key: crate::FrontendExtensionValidationDiagnosticsCacheKey::from_parts([7, 8]),
            ..identity
        },
        ExtensionValidationDiagnosticsIdentity {
            namespace: crate::FrontendCacheNamespace::from_parts([9, 10]),
            ..identity
        },
        ExtensionValidationDiagnosticsIdentity {
            module: &other_module,
            ..identity
        },
        ExtensionValidationDiagnosticsIdentity {
            program_sources: crate::FrontendProgramSourceFingerprint::from_parts([11, 12]),
            ..identity
        },
        ExtensionValidationDiagnosticsIdentity {
            source_len: 32,
            ..identity
        },
    ] {
        assert_eq!(
            decode_extension_validation_diagnostics_entry(&encoded, stale),
            None
        );
    }
}

#[test]
fn executable_edges_entry_identity_rejects_each_stale_field() {
    let module = stable_module("src/main.nia");
    let other_module = stable_module("src/other.nia");
    let identity = ExecutableValueRefEdgesIdentity {
        key: crate::FrontendExecutableValueRefEdgesCacheKey::from_parts([1, 2]),
        namespace: crate::FrontendCacheNamespace::from_parts([3, 4]),
        module: &module,
        owner: DefId(17),
        program_sources: crate::FrontendProgramSourceFingerprint::from_parts([5, 6]),
    };
    let encoded = encode_executable_value_ref_edges_entry(identity, b"edges");

    assert_eq!(
        decode_executable_value_ref_edges_entry(&encoded, identity),
        Some(b"edges".as_slice())
    );
    for stale in [
        ExecutableValueRefEdgesIdentity {
            key: crate::FrontendExecutableValueRefEdgesCacheKey::from_parts([7, 8]),
            ..identity
        },
        ExecutableValueRefEdgesIdentity {
            namespace: crate::FrontendCacheNamespace::from_parts([9, 10]),
            ..identity
        },
        ExecutableValueRefEdgesIdentity {
            module: &other_module,
            ..identity
        },
        ExecutableValueRefEdgesIdentity {
            owner: DefId(18),
            ..identity
        },
        ExecutableValueRefEdgesIdentity {
            program_sources: crate::FrontendProgramSourceFingerprint::from_parts([11, 12]),
            ..identity
        },
    ] {
        assert_eq!(
            decode_executable_value_ref_edges_entry(&encoded, stale),
            None
        );
    }
}

#[test]
fn check_certificate_entry_identity_rejects_each_stale_field() {
    let entry = stable_module("src/main.nia");
    let other_entry = stable_module("src/other.nia");
    let source_lengths = BTreeMap::new();
    let identity = CheckCertificateIdentity {
        key: crate::FrontendCheckCertificateCacheKey::from_parts([1, 2]),
        namespace: crate::FrontendCacheNamespace::from_parts([3, 4]),
        entry: &entry,
        input: crate::FrontendCheckInputFingerprint::from_parts([5, 6]),
        scope: crate::FrontendCheckScope::Entry,
        source_lengths: &source_lengths,
    };
    let certificate = CachedCheckCertificate {
        checked_body_count: 7,
        reachable_body_count: 5,
        diagnostics: Vec::new(),
    };
    let encoded = encode_check_certificate(identity, &certificate).expect("encode certificate");

    assert_eq!(
        decode_check_certificate(&encoded, identity),
        Some(certificate)
    );
    for stale in [
        CheckCertificateIdentity {
            key: crate::FrontendCheckCertificateCacheKey::from_parts([7, 8]),
            ..identity
        },
        CheckCertificateIdentity {
            namespace: crate::FrontendCacheNamespace::from_parts([9, 10]),
            ..identity
        },
        CheckCertificateIdentity {
            entry: &other_entry,
            ..identity
        },
        CheckCertificateIdentity {
            input: crate::FrontendCheckInputFingerprint::from_parts([11, 12]),
            ..identity
        },
        CheckCertificateIdentity {
            scope: crate::FrontendCheckScope::AllModules,
            ..identity
        },
    ] {
        assert_eq!(decode_check_certificate(&encoded, stale), None);
    }
}
