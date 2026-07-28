use super::*;

#[test]
fn check_certificate_roundtrips_and_retires_corruption() {
    let root = temp_dir("check_certificate");
    let cache = PersistentSignatureCache::new(root.clone());
    let entry = StableModuleKey::from_source_identity(SourceIdentity::new("src/main.nia"));
    let namespace = crate::FrontendCacheNamespace::new(
        &nia_target_config::TargetConfig::host(),
        crate::RuntimeModel::Bare,
    );
    let input = crate::FrontendCheckInputFingerprint::from_parts([11, 29]);
    let key = crate::FrontendCheckCertificateCacheKey::new(
        namespace,
        &entry,
        input,
        crate::FrontendCheckScope::Entry,
    );
    let source_lengths = BTreeMap::from([("src/main.nia".to_owned(), 32)]);
    let identity = CheckCertificateIdentity {
        key,
        namespace,
        entry: &entry,
        input,
        scope: crate::FrontendCheckScope::Entry,
        source_lengths: &source_lengths,
    };
    let certificate = CachedCheckCertificate {
        checked_body_count: 17,
        reachable_body_count: 13,
        diagnostics: vec![ProgramDiagnostic {
            path: SourcePath::new("src/main.nia"),
            diagnostic: Diagnostic::user_error_at(
                codes::TYPE_CHECK,
                nia_span::Span::new(2, 8),
                "bad type",
            ),
        }],
    };
    cache
        .publish_check_certificate(identity, certificate.clone(), false)
        .expect("publish check certificate");
    assert_eq!(
        cache
            .load_check_certificate(identity)
            .expect("load check certificate"),
        CheckCertificateLookup::Hit(certificate)
    );

    let path = cache.check_certificate_path(key);
    fs::write(&path, b"corrupt").expect("corrupt check certificate");
    assert_eq!(
        cache
            .load_check_certificate(identity)
            .expect("load corrupt check certificate"),
        CheckCertificateLookup::Corrupt
    );
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}
