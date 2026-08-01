use super::*;

#[test]
fn extension_validation_diagnostics_roundtrip_and_retire_corruption() {
    let root = temp_dir("extension_validation_diagnostics");
    let cache = PersistentSignatureCache::new(root.clone());
    let module = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let source = crate::source_content_fingerprint("extend ! { fn invalid(self) void {} }");
    let program_sources = crate::frontend_program_source_fingerprint([(&module, source, 128)]);
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeModel::Bare,
    );
    let key = crate::FrontendExtensionValidationDiagnosticsCacheKey::new(
        namespace,
        &module,
        program_sources,
    );
    let identity = ExtensionValidationDiagnosticsIdentity {
        key,
        namespace,
        module: &module,
        program_sources,
        source_len: 128,
    };
    let diagnostics = vec![Diagnostic::user_error_at(
        codes::NAME_RESOLUTION,
        nia_span::Span::new(7, 18),
        "extend target must be an extendable value type",
    )];

    cache
        .publish_extension_validation_diagnostics(identity, &diagnostics, false)
        .expect("publish validation diagnostics");
    assert_eq!(
        cache
            .load_extension_validation_diagnostics(identity)
            .expect("load validation diagnostics"),
        ExtensionValidationDiagnosticsLookup::Hit(diagnostics.clone())
    );

    let complete = Diagnostic::user_error(codes::NAME_RESOLUTION, "complete diagnostic shape")
        .primary(nia_span::Span::new(1, 2), "primary")
        .secondary(nia_span::Span::new(3, 4), "secondary")
        .note("note")
        .help("help")
        .related(nia_span::Span::new(5, 6), "related")
        .debug("owner", 7)
        .finish();
    cache
        .publish_extension_validation_diagnostics(identity, std::slice::from_ref(&complete), true)
        .expect("publish complete validation diagnostic");
    assert_eq!(
        cache
            .load_extension_validation_diagnostics(identity)
            .expect("load complete validation diagnostic"),
        ExtensionValidationDiagnosticsLookup::Hit(vec![complete])
    );

    let invalid_span = Diagnostic::user_error_at(
        codes::NAME_RESOLUTION,
        nia_span::Span::new(127, 129),
        "invalid span",
    );
    assert!(
        cache
            .publish_extension_validation_diagnostics(identity, &[invalid_span], true)
            .is_err()
    );

    let path = cache.extension_validation_diagnostics_path(key);
    let mut corrupt = fs::read(&path).expect("read validation diagnostics entry");
    corrupt[0] ^= 0xff;
    fs::write(&path, corrupt).expect("corrupt validation diagnostics entry");
    assert_eq!(
        cache
            .load_extension_validation_diagnostics(identity)
            .expect("load corrupt validation diagnostics"),
        ExtensionValidationDiagnosticsLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}
