// SPDX-License-Identifier: GPL-3.0-or-later
mod associated_types;
mod builtin_traits;
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
    };

    let written = crate::Driver::new()
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
        ..crate::DriverConfig::default()
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
    };
    let first_objects = object(CodegenUnitFingerprint::from_parts([1, 2]));
    let first_output = root.join("first");
    let second_output = root.join("second");

    driver
        .link_executable_from_objects(
            &first_objects,
            first_output.clone(),
            options.clone(),
            crate::TimingMode::Off,
        )
        .result
        .expect("first link");
    driver
        .link_executable_from_objects(
            &first_objects,
            second_output.clone(),
            options.clone(),
            crate::TimingMode::Off,
        )
        .result
        .expect("cached link");

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
