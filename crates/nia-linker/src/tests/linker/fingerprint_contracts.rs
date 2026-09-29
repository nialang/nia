use super::*;

#[test]
fn link_result_fingerprint_depends_on_typed_identity_not_object_representation() {
    let linker = fingerprint_linker("representation", b"linker-v1");
    let paths = link_inputs("main.o");
    let bytes = IncrementalLinkInputs::new(vec![IncrementalLinkInput {
        key: paths.as_slice()[0].key.clone(),
        fingerprint: paths.as_slice()[0].fingerprint,
        object: b"object bytes".to_vec(),
    }])
    .expect("build incremental link inputs");
    let options = fingerprint_options(&linker);

    assert_eq!(
        options
            .result_fingerprint(
                linux(),
                &paths,
                nia_toolchain::ToolchainIdentityFingerprint::current(),
            )
            .expect("path fingerprint"),
        options
            .result_fingerprint(
                linux(),
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
            linux(),
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
        linux(),
        &inputs,
        nia_toolchain::ToolchainIdentityFingerprint::current(),
    )
    .expect("changed entry fingerprint")
    .expect("cacheable link");
    let changed_inputs = IncrementalLinkInputs::new(vec![IncrementalLinkInput {
        key: inputs.as_slice()[0].key.clone(),
        fingerprint: CodegenUnitFingerprint::from_parts([9, 10]),
        object: PathBuf::from("main.o"),
    }])
    .expect("build incremental link inputs");
    let changed_object = options
        .result_fingerprint(
            linux(),
            &changed_inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("changed object fingerprint")
        .expect("cacheable link");
    let changed_target = options
        .result_fingerprint(
            target("aarch64-unknown-linux"),
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("changed target fingerprint")
        .expect("cacheable link");
    let changed_toolchain = options
        .result_fingerprint(
            linux(),
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::from_parts([9, 11]),
        )
        .expect("changed toolchain fingerprint")
        .expect("cacheable link");
    fs::write(&linker, b"linker-v2").expect("change fingerprint linker");
    let changed_linker = options
        .result_fingerprint(
            linux(),
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
fn static_archive_fingerprints_track_identity_and_contents_but_not_physical_paths() {
    let linker = fingerprint_linker("static-archive-inputs", b"linker-v1");
    let inputs = link_inputs("main.o");
    let fingerprint = |archives| {
        fingerprint_options(&linker)
            .with_static_archives(archives)
            .result_fingerprint(
                linux(),
                &inputs,
                nia_toolchain::ToolchainIdentityFingerprint::current(),
            )
            .expect("static archive fingerprint")
            .expect("cacheable static archive link")
    };
    let baseline = fingerprint(vec![StaticArchiveLinkInput::from_bytes(
        "root",
        "support",
        "/first/libsupport.a",
        b"archive-v1",
    )]);
    let relocated = fingerprint(vec![StaticArchiveLinkInput::from_bytes(
        "root",
        "support",
        "/second/libsupport.a",
        b"archive-v1",
    )]);
    let changed_contents = fingerprint(vec![StaticArchiveLinkInput::from_bytes(
        "root",
        "support",
        "/second/libsupport.a",
        b"archive-v2",
    )]);
    let changed_identity = fingerprint(vec![StaticArchiveLinkInput::from_bytes(
        "root",
        "other",
        "/second/libsupport.a",
        b"archive-v1",
    )]);

    assert_eq!(baseline, relocated);
    assert_eq!(baseline.cache_key, changed_contents.cache_key);
    assert_ne!(
        baseline.components.inputs,
        changed_contents.components.inputs
    );
    assert_ne!(baseline.cache_key, changed_identity.cache_key);
}

#[test]
fn link_result_fingerprint_rejects_untracked_external_inputs() {
    let linker = fingerprint_linker("opaque-inputs", b"linker-v1");
    let inputs = link_inputs("main.o");

    assert_eq!(
        fingerprint_options(&linker)
            .add_library("c")
            .result_fingerprint(
                linux(),
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
                linux(),
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
            linux(),
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("sysroot cacheability"),
        None
    );
}

#[test]
fn link_result_environment_match_tracks_toolchain_target_linker_and_options() {
    let linker = fingerprint_linker("environment-match", b"linker-v1");
    let options = fingerprint_options(&linker);
    let toolchain = nia_toolchain::ToolchainIdentityFingerprint::current();
    let expected = options
        .result_environment_fingerprint(linux(), toolchain)
        .expect("environment fingerprint")
        .expect("cacheable environment");
    let components = LinkResultFingerprintComponents {
        inputs: LinkResultFingerprint::from_parts([1, 2]),
        toolchain: expected.toolchain,
        target: expected.target,
        linker: expected.linker,
        options: expected.options,
    };

    assert!(
        options
            .matches_result_environment(linux(), components, toolchain)
            .expect("matching environment")
    );
    assert!(
        !options
            .matches_result_environment(
                linux(),
                components,
                nia_toolchain::ToolchainIdentityFingerprint::from_parts([9, 10]),
            )
            .expect("changed toolchain environment")
    );
    assert!(
        !options
            .matches_result_environment(target("aarch64-unknown-linux"), components, toolchain)
            .expect("changed target environment")
    );
    assert!(
        !LinkOptions {
            entry: Some("custom_start".to_string()),
            ..options.clone()
        }
        .matches_result_environment(linux(), components, toolchain)
        .expect("changed option environment")
    );
    fs::write(&linker, b"linker-v2").expect("change environment linker");
    assert!(
        !options
            .matches_result_environment(linux(), components, toolchain)
            .expect("changed linker environment")
    );
}

#[test]
fn system_import_descriptions_are_target_identity_but_paths_are_not() {
    let linker = fingerprint_linker("system-imports", b"linker-v1");
    let inputs = link_inputs("main.obj");
    let windows = target("x86_64-pc-windows-msvc");
    let fingerprint = |path: &str, description: &[u8]| {
        LinkOptions {
            linker: ExecutableLinker::with_program(linker.to_string_lossy()),
            ..LinkOptions::default()
        }
        .with_system_imports(vec![SystemImportLibrary::new(
            "kernel32",
            path,
            description,
        )])
        .result_fingerprint(
            windows,
            &inputs,
            nia_toolchain::ToolchainIdentityFingerprint::current(),
        )
        .expect("system import fingerprint")
        .expect("cacheable link")
    };
    let baseline = fingerprint("first/kernel32.lib", b"LIBRARY kernel32.dll");
    let relocated = fingerprint("second/kernel32.lib", b"LIBRARY kernel32.dll");
    let changed = fingerprint("first/kernel32.lib", b"LIBRARY kernel32.dll\nEXPORTS\n");

    assert_eq!(baseline, relocated);
    assert_eq!(baseline.cache_key, changed.cache_key);
    assert_ne!(baseline.components.target, changed.components.target);
    assert_eq!(baseline.components.options, changed.components.options);
}
