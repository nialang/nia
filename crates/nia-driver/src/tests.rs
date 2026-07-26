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
fn extension_validation_diagnostics_persist_across_driver_sessions() {
    let _permit = nia_test_support::compiler_permit();
    let root = common::temp_dir("extension_validation_diagnostics_persist");
    let main = root.join("main.nia");
    let cache = root.join("cache");
    common::write(
        &main,
        "extend ! { fn invalid(self) void {} } pub fn main() i32 { 0 }",
    );
    let request = || crate::CheckRequest::new(main.to_string_lossy().into_owned());
    let compile = |verify_frontend_cache| {
        let driver = crate::Driver::with_config(crate::DriverConfig {
            artifact_cache_dir: Some(cache.clone()),
            verify_frontend_cache,
            ..crate::DriverConfig::default()
        });
        common::checked_program_from_output(driver.check_entry(request()))
    };

    let cold = compile(false);
    let warm = compile(false);
    let verified = compile(true);
    assert_eq!(cold.diagnostics, warm.diagnostics);
    assert_eq!(cold.diagnostics, verified.diagnostics);
    assert!(cold.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .diagnostic
            .summary
            .contains("extend target must be an extendable value type")
    }));
    assert!(
        std::fs::read_dir(
            cache
                .join("artifacts")
                .join("frontend")
                .join("v3")
                .join("extension-validation-diagnostics")
        )
        .expect("read extension validation diagnostics cache")
        .next()
        .is_some()
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
        optimization: crate::OptimizationPolicy::default(),
        optimization_report: crate::BackendOptimizationReport::default(),
        diagnostics: Vec::new(),
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
