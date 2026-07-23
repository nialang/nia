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
