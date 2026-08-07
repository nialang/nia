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
