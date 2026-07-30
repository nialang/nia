use super::*;

#[test]
fn link_result_fingerprint_depends_on_typed_identity_not_object_representation() {
    let linker = fingerprint_linker("representation", b"linker-v1");
    let paths = link_inputs("main.o");
    let bytes = IncrementalLinkInputs::new(vec![IncrementalLinkInput {
        key: paths.as_slice()[0].key.clone(),
        fingerprint: paths.as_slice()[0].fingerprint,
        object: b"object bytes".to_vec(),
    }]);
    let options = fingerprint_options(&linker);

    assert_eq!(
        options
            .result_fingerprint(
                &paths,
                nia_toolchain::ToolchainIdentityFingerprint::current(),
            )
            .expect("path fingerprint"),
        options
            .result_fingerprint(
                &bytes,
                nia_toolchain::ToolchainIdentityFingerprint::current(),
            )
            .expect("bytes fingerprint")
    );
}

#[test]
fn link_result_fingerprint_tracks_inputs_options_and_linker_binary() {
    let linker = fingerprint_linker("tracked-inputs", b"linker-v1");
    let inputs = link_inputs("main.o");
    let options = fingerprint_options(&linker);
    let baseline = options
        .result_fingerprint(
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("baseline fingerprint")
        .expect("cacheable link");
    let changed_entry = LinkOptions {
        entry: Some("custom_start".to_string()),
        ..options.clone()
    }
    .result_fingerprint(
        &inputs,
        nia_toolchain::ToolchainIdentityFingerprint::current(),
    )
    .expect("changed entry fingerprint")
    .expect("cacheable link");
    let changed_inputs = IncrementalLinkInputs::new(vec![IncrementalLinkInput {
        key: inputs.as_slice()[0].key.clone(),
        fingerprint: CodegenUnitFingerprint::from_parts([9, 10]),
        object: PathBuf::from("main.o"),
    }]);
    let changed_object = options
        .result_fingerprint(
            &changed_inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("changed object fingerprint")
        .expect("cacheable link");
    let changed_target = LinkOptions {
        target: LinkTarget {
            arch: "aarch64".to_string(),
            ..options.target.clone()
        },
        ..options.clone()
    }
    .result_fingerprint(
        &inputs,
        nia_toolchain::ToolchainIdentityFingerprint::current(),
    )
    .expect("changed target fingerprint")
    .expect("cacheable link");
    let changed_toolchain = options
        .result_fingerprint(
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::from_parts([9, 11]),
        )
        .expect("changed toolchain fingerprint")
        .expect("cacheable link");
    fs::write(&linker, b"linker-v2").expect("change fingerprint linker");
    let changed_linker = options
        .result_fingerprint(
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("changed linker fingerprint")
        .expect("cacheable link");

    assert_eq!(baseline.cache_key, changed_entry.cache_key);
    assert_eq!(baseline.cache_key, changed_object.cache_key);
    assert_eq!(baseline.cache_key, changed_target.cache_key);
    assert_eq!(baseline.cache_key, changed_toolchain.cache_key);
    assert_eq!(baseline.cache_key, changed_linker.cache_key);
    assert_eq!(
        LinkResultInvalidation::between(baseline.components, changed_entry.components),
        LinkResultInvalidation {
            inputs: false,
            toolchain: false,
            target: false,
            linker: false,
            options: true,
        }
    );
    assert_eq!(
        LinkResultInvalidation::between(baseline.components, changed_object.components),
        LinkResultInvalidation {
            inputs: true,
            toolchain: false,
            target: false,
            linker: false,
            options: false,
        }
    );
    assert_eq!(
        LinkResultInvalidation::between(baseline.components, changed_target.components),
        LinkResultInvalidation {
            inputs: false,
            toolchain: false,
            target: true,
            linker: false,
            options: false,
        }
    );
    assert_eq!(
        LinkResultInvalidation::between(baseline.components, changed_toolchain.components),
        LinkResultInvalidation {
            inputs: false,
            toolchain: true,
            target: false,
            linker: false,
            options: false,
        }
    );
    assert_eq!(
        LinkResultInvalidation::between(baseline.components, changed_linker.components),
        LinkResultInvalidation {
            inputs: false,
            toolchain: false,
            target: false,
            linker: true,
            options: false,
        }
    );
}

#[test]
fn link_result_fingerprint_rejects_untracked_external_inputs() {
    let linker = fingerprint_linker("opaque-inputs", b"linker-v1");
    let inputs = link_inputs("main.o");

    assert_eq!(
        fingerprint_options(&linker)
            .add_library("c")
            .result_fingerprint(
                &inputs,
                nia_toolchain::ToolchainIdentityFingerprint::current(),
            )
            .expect("library cacheability"),
        None
    );
    assert_eq!(
        fingerprint_options(&linker)
            .with_raw_args(vec!["script.ld".to_string()])
            .result_fingerprint(
                &inputs,
                nia_toolchain::ToolchainIdentityFingerprint::current(),
            )
            .expect("raw input cacheability"),
        None
    );
    assert_eq!(
        LinkOptions {
            sysroot: Some("/sdk".to_string()),
            ..fingerprint_options(&linker)
        }
        .result_fingerprint(
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("sysroot cacheability"),
        None
    );
}
