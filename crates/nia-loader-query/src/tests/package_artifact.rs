use super::*;
use nia_package_metadata::{
    DefinitionId, InterfaceRecord, InterfaceSection, PackageId, PackageManifest, SectionKind,
    encode, encode_artifact, encode_interface, interface_module_hash,
};
use std::fs;

fn temp_artifact(name: &str) -> std::path::PathBuf {
    let root =
        std::env::temp_dir().join(format!("nia-loader-artifact-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root.join("package.niapkg")
}

fn manifest() -> PackageManifest {
    PackageManifest::current(PackageId {
        namespace: "example".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    })
}

#[test]
fn optional_artifact_loads_and_preserves_relocation_independent_identity() {
    let path = temp_artifact("valid");
    fs::write(&path, encode(&manifest()).unwrap()).unwrap();
    let request = LoadRequest::new("main.nia").with_package_artifact(&path);
    let loader = LoaderDatabase::new(request);
    let selection = loader.package_artifact().unwrap().unwrap();
    match selection {
        PackageArtifactLoad::Loaded {
            artifact,
            interface,
            ..
        } => {
            assert_eq!(artifact.manifest().package, manifest().package);
            assert_eq!(interface.records().len(), 0);
        }
        PackageArtifactLoad::SourceFallback { .. } => panic!("valid artifact must load"),
    }
    let other = path.with_file_name("relocated.niapkg");
    fs::copy(&path, &other).unwrap();
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_package_artifact(&other));
    assert!(matches!(
        loader.package_artifact().unwrap(),
        Some(PackageArtifactLoad::Loaded { .. })
    ));
}

#[test]
fn optional_artifact_falls_back_for_missing_corrupt_and_incompatible_inputs() {
    let missing = temp_artifact("missing");
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_package_artifact(&missing));
    assert!(matches!(
        loader.package_artifact().unwrap(),
        Some(PackageArtifactLoad::SourceFallback {
            reason: PackageArtifactFallback::Missing,
            ..
        })
    ));

    let corrupt = temp_artifact("corrupt");
    fs::write(&corrupt, b"not-an-artifact").unwrap();
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_package_artifact(&corrupt));
    assert!(matches!(
        loader.package_artifact().unwrap(),
        Some(PackageArtifactLoad::SourceFallback {
            reason: PackageArtifactFallback::InvalidMetadata(_),
            ..
        })
    ));

    let incompatible = temp_artifact("incompatible");
    let mut value = manifest();
    value.compiler_version = "0.0.0-incompatible".into();
    fs::write(&incompatible, encode(&value).unwrap()).unwrap();
    let loader =
        LoaderDatabase::new(LoadRequest::new("main.nia").with_package_artifact(&incompatible));
    assert!(matches!(
        loader.package_artifact().unwrap(),
        Some(PackageArtifactLoad::SourceFallback {
            reason: PackageArtifactFallback::Incompatible(
                PackageArtifactMismatch::CompilerVersion { .. }
            ),
            ..
        })
    ));
}

#[test]
fn required_artifact_rejection_is_a_typed_error() {
    let path = temp_artifact("required");
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").require_package_artifact(&path));
    assert!(matches!(
        loader.package_artifact(),
        Err(PackageArtifactError::Missing { .. })
    ));
}

#[test]
fn corrupt_lazy_section_is_rejected_before_selection() {
    let path = temp_artifact("section-corrupt");
    let bytes = encode_artifact(&manifest(), &[(SectionKind::Interface, b"interface")]).unwrap();
    let artifact = nia_package_metadata::PackageArtifact::open(bytes.clone()).unwrap();
    let offset = artifact
        .section(SectionKind::Interface)
        .unwrap()
        .map(|section| bytes.len() - section.len())
        .unwrap();
    let mut corrupt = bytes;
    corrupt[offset] ^= 1;
    fs::write(&path, corrupt).unwrap();
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_package_artifact(&path));
    assert!(matches!(
        loader.package_artifact().unwrap(),
        Some(PackageArtifactLoad::SourceFallback {
            reason: PackageArtifactFallback::InvalidMetadata(
                nia_package_metadata::MetadataError::Integrity
            ),
            ..
        })
    ));
}

#[test]
fn explicit_package_identity_mismatch_is_reported() {
    let path = temp_artifact("package-mismatch");
    fs::write(&path, encode(&manifest()).unwrap()).unwrap();
    let expected = PackageId {
        namespace: "other".into(),
        name: "demo".into(),
        version: "1.0.0".into(),
    };
    let request = PackageArtifactRequest::Required(path);
    assert!(matches!(
        select_package_artifact(&request, Some(&expected), None),
        Err(PackageArtifactError::Incompatible {
            mismatch: PackageArtifactMismatch::Package { .. },
            ..
        })
    ));
}

#[test]
fn loaded_artifact_exposes_indexed_interface_without_source_access() {
    let path = temp_artifact("indexed-interface");
    let package = manifest().package;
    let interface = InterfaceSection {
        records: vec![InterfaceRecord {
            definition: DefinitionId {
                package: package.clone(),
                module: "src/lib.nia".into(),
                name: "answer".into(),
                kind: 2,
            },
            signature: b"fn() Int".to_vec(),
        }],
    };
    let interface_bytes = encode_interface(&interface).unwrap();
    let mut metadata = PackageManifest::current(package.clone());
    metadata
        .modules
        .push(nia_package_metadata::ModuleInterface {
            path: "src/lib.nia".into(),
            interface_hash: interface_module_hash(&interface, "src/lib.nia").unwrap(),
        });
    fs::write(
        &path,
        encode_artifact(&metadata, &[(SectionKind::Interface, &interface_bytes)]).unwrap(),
    )
    .unwrap();
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_package_artifact(&path));
    let Some(PackageArtifactLoad::Loaded {
        interface: indexed, ..
    }) = loader.package_artifact().unwrap()
    else {
        panic!("indexed interface must load");
    };
    let definition = &indexed.records()[0].definition;
    assert_eq!(
        indexed.definition(definition).unwrap().signature,
        b"fn() Int"
    );
    assert_eq!(indexed.module_records("src/lib.nia").count(), 1);
}
