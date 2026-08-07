// SPDX-License-Identifier: GPL-3.0-or-later
mod associated_types;
mod builtin_traits;
mod cases;
mod common;
mod const_eval;
mod const_generics;
mod imports;
mod soundness;
mod supertraits;
mod trait_impls;
mod trait_objects;
mod type_system;
mod using;

#[test]
fn driver_invocation_target_overrides_toolchain_default() {
    let toolchain = common::test_toolchain_layout();
    let mut invocation_target = toolchain.artifact_target().clone();
    invocation_target.os = "driver-invocation-target".to_string();
    let driver = crate::Driver::with_config(
        crate::DriverConfig::new(toolchain).with_artifact_target(invocation_target.clone()),
    );
    driver.set_source(
        "main.nia",
        r#"
@[if os == "driver-invocation-target"]
fn selected() i32 { 1 }

fn main() i32 { selected() }
"#,
    );

    let output = driver.check_entry(crate::CheckRequest::new("main.nia"));

    assert_eq!(driver.config().artifact_target, invocation_target);
    output
        .result
        .expect("invocation target must drive loader target pruning");
}

#[test]
fn driver_exposes_loader_owned_recursive_source_manifest() {
    let root = common::temp_dir("recursive-source-manifest");
    let source_dir = root.join("src");
    std::fs::create_dir_all(&source_dir).expect("create manifest source directory");
    common::write(&source_dir.join("main.nia"), "module child;");
    common::write(&source_dir.join("child.nia"), "fn value() i32 { 1 }");
    let entry = crate::SourcePath::with_identity(
        source_dir.join("main.nia").to_string_lossy(),
        "build-package:root:/src/main.nia",
    );
    let driver = common::test_driver();

    let manifest = driver
        .source_input_manifest(&crate::CheckRequest::from_source_path(entry))
        .result
        .expect("Driver source manifest");

    assert!(manifest.fingerprint().is_some());
    assert_eq!(
        manifest
            .sources()
            .iter()
            .map(|source| source.path.identity())
            .map(|identity| identity.normalized_path().to_string())
            .collect::<Vec<_>>(),
        [
            "build-package:root:/src/child.nia".to_string(),
            "build-package:root:/src/main.nia".to_string(),
        ]
    );
}

#[test]
fn writing_native_object_preserves_incremental_link_identity() {
    use nia_backend_ir::{
        CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey, IncrementalLinkInput,
        IncrementalLinkInputs,
    };
    use nia_codegen_llvm::NativeObject;

    let root = common::temp_dir("writing_native_object_preserves_incremental_link_identity");
    let path = root.join("builtins.o");
    let key = CodegenUnitKey::CompilerBuiltins;
    let fingerprint = CodegenUnitFingerprint::from_parts([7, 8]);
    let artifact = crate::ObjectArtifact {
        link_inputs: IncrementalLinkInputs::new(vec![IncrementalLinkInput {
            key: key.clone(),
            fingerprint,
            object: NativeObject {
                unit: CodegenUnitId::CompilerBuiltins,
                name: "nia.compiler_builtins".to_string(),
                bytes: b"native-object".to_vec(),
            },
        }]),
        optimization: crate::OptimizationPolicy::default(),
        optimization_report: crate::BackendOptimizationReport::default(),
        diagnostics: Vec::new(),
    };

    let written = crate::Driver::new(common::test_toolchain_layout())
        .write_native_objects_from_artifact(&artifact, crate::ObjectOutput::Single(path.clone()))
        .result
        .expect("write native object");

    assert_eq!(
        std::fs::read(&path).expect("read native object"),
        b"native-object"
    );
    assert_eq!(written.link_inputs.as_slice().len(), 1);
    let input = &written.link_inputs.as_slice()[0];
    assert_eq!(input.key, key);
    assert_eq!(input.fingerprint, fingerprint);
    assert_eq!(input.object, path);
}

#[cfg(unix)]
fn static_archive_test_objects(first_fingerprint: [u64; 2]) -> crate::ObjectArtifact {
    use nia_backend_ir::{
        CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey, IncrementalLinkInput,
        IncrementalLinkInputs,
    };
    use nia_codegen_llvm::NativeObject;
    use nia_ids::ModuleIdAllocator;
    use nia_source::SourceIdentity;

    let mut module_ids = ModuleIdAllocator::new();
    let mut input = |fingerprint, source: &str, name: &str, bytes: &[u8]| IncrementalLinkInput {
        key: CodegenUnitKey::SourceModule {
            source_identity: SourceIdentity::new(source),
            ordinal: 0,
        },
        fingerprint: CodegenUnitFingerprint::from_parts(fingerprint),
        object: NativeObject {
            unit: CodegenUnitId::SourceModule {
                module_id: module_ids.allocate(),
                ordinal: 0,
            },
            name: name.to_string(),
            bytes: bytes.to_vec(),
        },
    };
    crate::ObjectArtifact {
        link_inputs: IncrementalLinkInputs::new(vec![
            input(
                first_fingerprint,
                "first.nia",
                "backend_first_name",
                b"first-object",
            ),
            input(
                [3, 4],
                "second.nia",
                "backend_second_name",
                b"second-object",
            ),
        ]),
        optimization: crate::OptimizationPolicy::default(),
        optimization_report: crate::BackendOptimizationReport::default(),
        diagnostics: Vec::new(),
    }
}

#[cfg(unix)]
fn write_static_archive_test_tool(path: &std::path::Path, source: impl AsRef<[u8]>) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, source).expect("write mock archive tool");
    let mut permissions = std::fs::metadata(path)
        .expect("mock archive tool metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("make mock archive tool executable");
}

#[test]
#[cfg(unix)]
fn static_archive_materializes_canonical_members_through_typed_archive_tool() {
    use nia_linker::{ArchiveOptions, ArchiveTool};

    let root = common::temp_dir("static_archive_materializes_objects_through_typed_archive_tool");
    let tool = root.join("archive.sh");
    let log = root.join("members.log");
    write_static_archive_test_tool(
        &tool,
        format!(
            "#!/bin/sh\ntest \"$1\" = rcsD || exit 9\ntest ! -e \"$2\" || exit 10\ntest \"$(cat \"$3\")\" = first-object || exit 11\ntest \"$(cat \"$4\")\" = second-object || exit 12\nprintf '%s\\n%s\\n' \"$3\" \"$4\" > '{}'\nprintf static-archive > \"$2\"\n",
            log.display()
        ),
    );
    let objects = static_archive_test_objects([1, 2]);
    let output = root.join("nested/libsample.a");

    let archived = crate::Driver::new(common::test_toolchain_layout())
        .archive_static_library_from_objects(
            &objects,
            output.clone(),
            ArchiveOptions::default().with_tool(ArchiveTool::with_program(tool.to_string_lossy())),
        )
        .result
        .expect("create static archive");

    assert_eq!(archived.path, output);
    assert_eq!(archived.cache_reference, None);
    assert_eq!(std::fs::read(&archived.path).unwrap(), b"static-archive");
    let members = std::fs::read_to_string(log).unwrap();
    let names = members
        .lines()
        .map(|path| {
            std::path::Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["0000_first.o", "0001_second.o"]);
}

#[test]
#[cfg(unix)]
fn static_archive_failure_preserves_existing_output() {
    use nia_linker::{ArchiveOptions, ArchiveTool};

    let root = common::temp_dir("static_archive_failure_preserves_existing_output");
    let tool = root.join("archive.sh");
    write_static_archive_test_tool(&tool, "#!/bin/sh\nexit 23\n");
    let objects = static_archive_test_objects([1, 2]);
    let output = root.join("libsample.a");
    std::fs::write(&output, b"existing-archive").expect("seed existing archive");

    let error = crate::Driver::new(common::test_toolchain_layout())
        .archive_static_library_from_objects(
            &objects,
            output.clone(),
            ArchiveOptions::default().with_tool(ArchiveTool::with_program(tool.to_string_lossy())),
        )
        .result
        .unwrap_err();

    assert!(matches!(
        error,
        crate::DriverError::ArchiveStatus { status, .. } if status.code() == Some(23)
    ));
    assert_eq!(std::fs::read(output).unwrap(), b"existing-archive");
}

#[test]
#[cfg(unix)]
fn static_archive_requires_successful_tool_to_produce_output() {
    use nia_linker::{ArchiveOptions, ArchiveTool};

    let root = common::temp_dir("static_archive_requires_successful_tool_to_produce_output");
    let tool = root.join("archive.sh");
    write_static_archive_test_tool(&tool, "#!/bin/sh\nexit 0\n");
    let objects = static_archive_test_objects([1, 2]);
    let output = root.join("libsample.a");
    std::fs::write(&output, b"existing-archive").expect("seed existing archive");

    let error = crate::Driver::new(common::test_toolchain_layout())
        .archive_static_library_from_objects(
            &objects,
            output.clone(),
            ArchiveOptions::default().with_tool(ArchiveTool::with_program(tool.to_string_lossy())),
        )
        .result
        .unwrap_err();

    assert!(matches!(
        error,
        crate::DriverError::Io {
            operation: "read temporary static archive",
            ..
        }
    ));
    assert_eq!(std::fs::read(output).unwrap(), b"existing-archive");
}

#[test]
#[cfg(unix)]
fn static_archive_cache_skips_tool_until_typed_input_changes() {
    use nia_linker::{ArchiveOptions, ArchiveTool};

    let root = common::temp_dir("static_archive_cache_skips_tool_until_typed_input_changes");
    let tool = root.join("archive.sh");
    let invocation_log = root.join("archive-invocations");
    write_static_archive_test_tool(
        &tool,
        format!(
            "#!/bin/sh\nprintf x >> '{}'\nprintf cached-archive > \"$2\"\n",
            invocation_log.display()
        ),
    );
    let driver = crate::Driver::with_config(crate::DriverConfig {
        artifact_cache_dir: Some(root.join("cache")),
        ..crate::DriverConfig::new(common::test_toolchain_layout())
    });
    let options =
        ArchiveOptions::default().with_tool(ArchiveTool::with_program(tool.to_string_lossy()));
    let first_objects = static_archive_test_objects([1, 2]);

    let first = driver
        .archive_static_library_from_objects(&first_objects, root.join("first.a"), options.clone())
        .result
        .expect("first archive");
    let reference = first.cache_reference.expect("archive cache reference");
    let encoded = reference.encode();
    assert_eq!(
        crate::StaticArchiveCacheReference::decode(&encoded),
        Some(reference)
    );
    for end in 0..encoded.len() {
        assert!(crate::StaticArchiveCacheReference::decode(&encoded[..end]).is_none());
    }
    let mut trailing = encoded.to_vec();
    trailing.push(0);
    assert!(crate::StaticArchiveCacheReference::decode(&trailing).is_none());

    let second = driver
        .archive_static_library_from_objects(&first_objects, root.join("second.a"), options.clone())
        .result
        .expect("cached archive");
    assert_eq!(second.cache_reference, Some(reference));
    assert_eq!(std::fs::read(second.path).unwrap(), b"cached-archive");
    assert_eq!(std::fs::read_to_string(&invocation_log).unwrap(), "x");

    let changed_objects = static_archive_test_objects([9, 10]);
    driver
        .archive_static_library_from_objects(&changed_objects, root.join("changed.a"), options)
        .result
        .expect("changed archive");
    assert_eq!(std::fs::read_to_string(invocation_log).unwrap(), "xx");
}

#[test]
#[cfg(unix)]
fn link_result_cache_skips_linker_until_typed_input_changes() {
    use std::os::unix::fs::PermissionsExt;

    use nia_backend_ir::{
        CodegenUnitFingerprint, CodegenUnitId, CodegenUnitKey, IncrementalLinkInput,
        IncrementalLinkInputs,
    };
    use nia_codegen_llvm::NativeObject;
    use nia_linker::{ExecutableLinker, LinkOptions};

    let root = common::temp_dir("link_result_cache_skips_linker_until_typed_input_changes");
    let linker = root.join("linker.sh");
    let invocation_log = root.join("linker-invocations");
    std::fs::write(
        &linker,
        format!(
            "#!/bin/sh\nprintf x >> '{}'\nfor output in \"$@\"; do :; done\nprintf linked-executable > \"$output\"\nchmod +x \"$output\"\n",
            invocation_log.display()
        ),
    )
    .expect("write mock linker");
    let mut permissions = std::fs::metadata(&linker)
        .expect("mock linker metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&linker, permissions).expect("make mock linker executable");
    let driver = crate::Driver::with_config(crate::DriverConfig {
        artifact_cache_dir: Some(root.join("cache")),
        ..crate::DriverConfig::new(common::test_toolchain_layout())
    });
    let options = LinkOptions {
        linker: ExecutableLinker::with_program(linker.to_string_lossy()),
        ..LinkOptions::default()
    };
    let object = |fingerprint| crate::ObjectArtifact {
        link_inputs: IncrementalLinkInputs::new(vec![IncrementalLinkInput {
            key: CodegenUnitKey::CompilerBuiltins,
            fingerprint,
            object: NativeObject {
                unit: CodegenUnitId::CompilerBuiltins,
                name: "nia.compiler_builtins".to_string(),
                bytes: b"object".to_vec(),
            },
        }]),
        optimization: crate::OptimizationPolicy::default(),
        optimization_report: crate::BackendOptimizationReport::default(),
        diagnostics: Vec::new(),
    };
    let first_objects = object(CodegenUnitFingerprint::from_parts([1, 2]));
    let first_output = root.join("first");
    let second_output = root.join("second");

    let first = driver
        .link_executable_from_objects(
            &first_objects,
            first_output.clone(),
            options.clone(),
            crate::TimingMode::Off,
        )
        .result
        .expect("first link");
    let reference = first
        .cache_reference
        .expect("published link cache reference");
    let encoded = reference.encode();
    assert_eq!(
        crate::ExecutableCacheReference::decode(&encoded),
        Some(reference)
    );
    for end in 0..encoded.len() {
        assert!(crate::ExecutableCacheReference::decode(&encoded[..end]).is_none());
    }
    let mut trailing = encoded.to_vec();
    trailing.push(0);
    assert!(crate::ExecutableCacheReference::decode(&trailing).is_none());

    let second = driver
        .link_executable_from_objects(
            &first_objects,
            second_output.clone(),
            options.clone(),
            crate::TimingMode::Off,
        )
        .result
        .expect("cached link");
    assert_eq!(second.cache_reference, Some(reference));

    assert_eq!(
        std::fs::read_to_string(&invocation_log).expect("read linker invocations"),
        "x"
    );
    assert_eq!(
        std::fs::read(&second_output).expect("read restored executable"),
        b"linked-executable"
    );

    let changed_objects = object(CodegenUnitFingerprint::from_parts([3, 4]));
    driver
        .link_executable_from_objects(
            &changed_objects,
            root.join("changed"),
            options,
            crate::TimingMode::Off,
        )
        .result
        .expect("changed link");
    assert_eq!(
        std::fs::read_to_string(invocation_log).expect("read changed invocations"),
        "xx"
    );
}
