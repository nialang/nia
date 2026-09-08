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
fn artifact_selection_cache_refreshes_after_file_replacement() {
    let path = temp_artifact("cache-refresh");
    fs::write(&path, encode(&manifest()).unwrap()).unwrap();
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").with_package_artifact(&path));
    let first = loader.package_artifact().unwrap().unwrap();
    assert!(matches!(first, PackageArtifactLoad::Loaded { .. }));

    let mut replacement = manifest();
    replacement.package.name = "replacement".into();
    fs::write(&path, encode(&replacement).unwrap()).unwrap();
    let second = loader.package_artifact().unwrap().unwrap();
    let PackageArtifactLoad::Loaded { interface, .. } = second else {
        panic!("replacement artifact must load");
    };
    assert_eq!(interface.manifest().package, replacement.package);
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
fn required_artifact_errors_are_not_silently_downgraded_by_compiler_facts() {
    let path = temp_artifact("required-facts");
    let loader = LoaderDatabase::new(LoadRequest::new("main.nia").require_package_artifact(&path));
    let result = nia_compiler_query::LoaderFactProvider::compiled_package_interfaces(&loader);
    assert!(matches!(
        result,
        Err(nia_query::QueryError::InvalidInput { .. })
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
                module: nia_package_metadata::ModuleId {
                    package: package.clone(),
                    path: "src/lib.nia".into(),
                },
                name: "answer".into(),
                kind: 2,
            },
            declaration: b"fn() Int".to_vec(),
            type_roots: Vec::new(),
        }],
    };
    let interface_bytes = encode_interface(&interface).unwrap();
    let target = nia_target_config::TargetConfig::host();
    let native = nia_package_metadata::NativeSection {
        target: nia_package_metadata::NativeTarget {
            arch: target.arch,
            vendor: target.vendor,
            os: target.os,
            env: target.env,
            abi: target.abi,
            endian: target.endian,
            pointer_width: target.pointer_width,
        },
        profile: 0,
        optimization: 0,
        objects: vec![nia_package_metadata::NativeObject {
            key: "unit-0".into(),
            fingerprint: [0, 0],
            bytes: vec![1, 2, 3],
        }],
    };
    let native_bytes = nia_package_metadata::encode_native(&native).unwrap();
    let mut metadata = PackageManifest::current(package.clone());
    metadata
        .modules
        .push(nia_package_metadata::ModuleInterface {
            path: "src/lib.nia".into(),
            interface_hash: interface_module_hash(&interface, "src/lib.nia").unwrap(),
        });
    fs::write(
        &path,
        encode_artifact(
            &metadata,
            &[
                (SectionKind::Interface, &interface_bytes),
                (SectionKind::Native, &native_bytes),
            ],
        )
        .unwrap(),
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
        indexed.definition(definition).unwrap().declaration,
        b"fn() Int"
    );
    assert_eq!(indexed.module_records("src/lib.nia").count(), 1);
    let modules =
        nia_compiler_query::LoaderFactProvider::compiled_package_module_identities(&loader)
            .unwrap();
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].package, package);
    assert_eq!(modules[0].path, "src/lib.nia");

    let compiler =
        nia_compiler_query::CompilerDatabase::new(nia_compiler_query::CompileRequest::new(loader));
    let installed = compiler
        .install_compiled_package_module_interfaces()
        .unwrap();
    assert_eq!(installed, modules);
    let installed_again = compiler
        .install_compiled_package_module_interfaces()
        .unwrap();
    assert_eq!(installed_again, modules);
    let fact = compiler
        .compiled_package_module_interface(modules[0].clone())
        .unwrap();
    assert_eq!(fact.identity(), &modules[0]);
    assert_eq!(fact.records().len(), 1);
    let native_packages = compiler.install_compiled_package_native().unwrap();
    assert_eq!(native_packages, vec![package.clone()]);
    assert_eq!(
        compiler
            .compiled_package_native(package)
            .unwrap()
            .section()
            .objects
            .len(),
        1
    );
}

#[test]
fn compiler_reads_loader_selected_interface_without_dependency_source() {
    let path = temp_artifact("compiler-interface");
    let package = manifest().package;
    let interface = InterfaceSection {
        records: vec![InterfaceRecord {
            definition: DefinitionId {
                module: nia_package_metadata::ModuleId {
                    package: package.clone(),
                    path: "src/lib.nia".into(),
                },
                name: "answer".into(),
                kind: 2,
            },
            declaration: b"fn() Int".to_vec(),
            type_roots: Vec::new(),
        }],
    };
    let interface_bytes = encode_interface(&interface).unwrap();
    let mut metadata = PackageManifest::current(package);
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
    let compiler =
        nia_compiler_query::CompilerDatabase::new(nia_compiler_query::CompileRequest::new(loader));
    let interfaces = compiler.compiled_package_interfaces().unwrap();
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces[0].records()[0].definition.name, "answer");
    let index = compiler.compiled_package_interface_index().unwrap();
    assert_eq!(
        index
            .package(&interfaces[0].manifest().package)
            .unwrap()
            .records()
            .len(),
        1
    );
    assert!(
        index
            .definition(&interfaces[0].records()[0].definition)
            .is_some()
    );
}
