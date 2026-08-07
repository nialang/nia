use super::*;

#[test]
fn archive_invocation_is_deterministic_and_preserves_member_order() {
    let tool = fingerprint_linker("archive-invocation", b"archive-tool");
    let options =
        ArchiveOptions::default().with_tool(ArchiveTool::with_program(tool.to_string_lossy()));

    let invocation = options
        .invocation(
            &[PathBuf::from("second.o"), PathBuf::from("first.o")],
            PathBuf::from("libsample.a"),
        )
        .expect("archive invocation");

    assert_eq!(invocation.program, tool.to_string_lossy().into_owned());
    assert_eq!(
        invocation.args,
        [
            "rcsD".to_string(),
            "libsample.a".to_string(),
            "second.o".to_string(),
            "first.o".to_string(),
        ]
    );
}

#[test]
fn archive_environment_fingerprint_tracks_tool_target_and_toolchain() {
    let tool = fingerprint_linker("archive-fingerprint", b"archive-tool-v1");
    let options =
        ArchiveOptions::default().with_tool(ArchiveTool::with_program(tool.to_string_lossy()));
    let toolchain = nia_toolchain::ToolchainIdentityFingerprint::from_parts([1, 2]);
    let baseline = options
        .environment_fingerprint(toolchain)
        .expect("baseline archive fingerprint");

    let target_changed = options
        .clone()
        .with_target(LinkTarget {
            arch: "aarch64".to_string(),
            ..options.target.clone()
        })
        .environment_fingerprint(toolchain)
        .expect("changed target fingerprint");
    let toolchain_changed = options
        .environment_fingerprint(nia_toolchain::ToolchainIdentityFingerprint::from_parts([
            3, 4,
        ]))
        .expect("changed toolchain fingerprint");
    fs::write(&tool, b"archive-tool-v2").expect("change archive tool");
    make_executable(&tool);
    let tool_changed = options
        .environment_fingerprint(toolchain)
        .expect("changed tool fingerprint");

    assert_ne!(baseline.target, target_changed.target);
    assert_eq!(baseline.tool, target_changed.tool);
    assert_ne!(baseline.toolchain, toolchain_changed.toolchain);
    assert_ne!(baseline.tool, tool_changed.tool);
    assert_eq!(baseline.options, tool_changed.options);
}

#[test]
fn archive_result_fingerprint_tracks_typed_inputs_and_environment() {
    let tool = fingerprint_linker("archive-result-fingerprint", b"archive-tool-v1");
    let inputs = link_inputs("first.o");
    let options =
        ArchiveOptions::default().with_tool(ArchiveTool::with_program(tool.to_string_lossy()));
    let toolchain = nia_toolchain::ToolchainIdentityFingerprint::from_parts([1, 2]);
    let baseline = options
        .result_fingerprint(&inputs, toolchain)
        .expect("baseline archive result fingerprint");
    let changed_inputs = IncrementalLinkInputs::new(vec![IncrementalLinkInput {
        key: inputs.as_slice()[0].key.clone(),
        fingerprint: CodegenUnitFingerprint::from_parts([9, 10]),
        object: PathBuf::from("unrelated-representation.o"),
    }]);
    let input_changed = options
        .result_fingerprint(&changed_inputs, toolchain)
        .expect("changed archive input fingerprint");
    let target_changed = options
        .clone()
        .with_target(LinkTarget {
            arch: "aarch64".to_string(),
            ..options.target.clone()
        })
        .result_fingerprint(&inputs, toolchain)
        .expect("changed archive target fingerprint");
    fs::write(&tool, b"archive-tool-v2").expect("change archive tool");
    make_executable(&tool);
    let tool_changed = options
        .result_fingerprint(&inputs, toolchain)
        .expect("changed archive tool fingerprint");

    assert_eq!(baseline.cache_key, input_changed.cache_key);
    assert_eq!(baseline.cache_key, target_changed.cache_key);
    assert_eq!(baseline.cache_key, tool_changed.cache_key);
    assert_eq!(
        ArchiveInvalidation::between(baseline.components, input_changed.components),
        ArchiveInvalidation {
            inputs: true,
            toolchain: false,
            target: false,
            tool: false,
            options: false,
        }
    );
    assert_eq!(
        ArchiveInvalidation::between(baseline.components, target_changed.components),
        ArchiveInvalidation {
            inputs: false,
            toolchain: false,
            target: true,
            tool: false,
            options: false,
        }
    );
    assert_eq!(
        ArchiveInvalidation::between(baseline.components, tool_changed.components),
        ArchiveInvalidation {
            inputs: false,
            toolchain: false,
            target: false,
            tool: true,
            options: false,
        }
    );
    assert!(
        options
            .matches_result_environment(tool_changed.components, toolchain)
            .expect("matching archive result environment")
    );
    assert!(
        !options
            .matches_result_environment(
                target_changed.components,
                nia_toolchain::ToolchainIdentityFingerprint::from_parts([3, 4]),
            )
            .expect("mismatched archive result environment")
    );
}

#[test]
fn explicit_missing_archive_tool_is_rejected() {
    let missing = env::temp_dir().join(format!("nia-missing-archive-tool-{}", std::process::id()));
    let error = ArchiveOptions::default()
        .with_tool(ArchiveTool::with_program(missing.to_string_lossy()))
        .invocation(&[], PathBuf::from("empty.a"))
        .unwrap_err();

    assert!(matches!(
        error,
        LinkerConfigError::ArchiveToolNotFound { program }
            if program == missing.to_string_lossy().into_owned()
    ));
}
