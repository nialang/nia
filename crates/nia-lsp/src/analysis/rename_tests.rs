// SPDX-License-Identifier: GPL-3.0-or-later

use nia_test_support::{TestWorkload, acquire_test_resources, test_dir};

use super::{tests::test_session, *};

fn rename_at(path: &Path, token: &str, new_name: &str) -> Result<lsp_types::WorkspaceEdit, String> {
    let documents = Documents::new();
    let analysis = test_session().analyze(&documents, path)?;
    assert_eq!(analysis.first_error(), None);
    let file = analysis.file(path).unwrap();
    let start = file.text.find(token).unwrap();
    test_session().rename(
        &analysis,
        &documents,
        crate::rename::Rename {
            path,
            position: file.lines.range(&file.text, Span::new(start, start)).start,
            new_name,
            folders: &[],
            versioned: true,
            resource_rename: true,
        },
    )
}

#[test]
fn rename_preserves_the_native_symbol_of_an_extern_declaration() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-ffi");
    let path = root.join("root.nia");
    std::fs::write(
        &path,
        concat!(
            "extern fun native() i32;\n",
            "@[linkName(\"other_native\")] extern fun explicit() i32;\n",
            "pub extern fun exported() i32 { native() + explicit() }\n",
        ),
    )
    .unwrap();
    for (old, new, expected) in [
        (
            "native()",
            "call_native",
            vec!["@[linkName(\"native\")] ", "call_native", "call_native"],
        ),
        (
            "explicit()",
            "call_explicit",
            vec!["call_explicit", "call_explicit"],
        ),
        (
            "exported()",
            "call_exported",
            vec!["@[exportName(\"exported\")] ", "call_exported"],
        ),
    ] {
        let edit = rename_at(&path, old, new).unwrap();
        let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
            panic!("expected text edits");
        };
        let actual: Vec<_> = edits[0]
            .edits
            .iter()
            .map(|edit| match edit {
                lsp_types::OneOf::Left(edit) => edit.new_text.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn rename_follows_a_public_alias_through_an_explicit_import() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-public-alias");
    let entry = root.join("root.nia");
    std::fs::write(
        &entry,
        concat!(
            "module facade;\nmodule original;\n",
            "using facade::amount;\n",
            "fun use_it() i32 { amount() + facade::amount() + original::amount() }\n",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("facade.nia"),
        "pub using super::original::amount as amount;\n",
    )
    .unwrap();
    std::fs::write(root.join("original.nia"), "pub fun amount() i32 { 1 }\n").unwrap();
    let edit = rename_at(&entry, "amount;", "quantity").unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected text edits");
    };
    assert_eq!(
        edits
            .iter()
            .map(|edit| edit.edits.len())
            .collect::<Vec<_>>(),
        [1, 3]
    );
    std::fs::write(
        &entry,
        concat!(
            "module outer;\nmodule facade;\nmodule original;\n",
            "using outer::*;\n",
            "fun use_it() i32 { amount() + outer::amount() + original::amount() }\n",
        ),
    )
    .unwrap();
    std::fs::write(root.join("outer.nia"), "pub using super::facade::*;\n").unwrap();
    let edit = rename_at(&entry, "amount()", "quantity").unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected text edits");
    };
    assert_eq!(
        edits
            .iter()
            .map(|edit| edit.edits.len())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn rename_moves_a_module_and_its_child_directory() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-module");
    let entry = root.join("root.nia");
    std::fs::create_dir(root.join("outer")).unwrap();
    std::fs::write(
        &entry,
        concat!(
            "module outer;\n",
            "using root::outer as local;\n",
            "fun use_it() i32 { outer::inner::value() + local::inner::value() }\n",
        ),
    )
    .unwrap();
    std::fs::write(root.join("outer.nia"), "pub module inner;\n").unwrap();
    std::fs::write(root.join("outer/inner.nia"), "pub fun value() i32 { 2 }\n").unwrap();
    let edit = rename_at(&entry, "outer;", "moved").unwrap();
    let Some(lsp_types::DocumentChanges::Operations(ops)) = edit.document_changes else {
        panic!("expected a module move");
    };
    assert_eq!(ops.len(), 3);
    assert!(
        matches!(&ops[0], lsp_types::DocumentChangeOperation::Edit(edit) if edit.edits.len() == 3)
    );
    for (operation, from, to) in [
        (&ops[1], "outer.nia", "moved.nia"),
        (&ops[2], "outer", "moved"),
    ] {
        assert!(
            matches!(operation, lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Rename(rename))
            if rename.old_uri == uri(&root.join(from)).unwrap() && rename.new_uri == uri(&root.join(to)).unwrap())
        );
    }
    assert!(root.join("outer/inner.nia").is_file());
    assert!(!root.join("moved.nia").exists());
}

#[test]
fn rename_updates_fields_and_trait_methods_at_their_semantic_uses() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-members");
    let path = root.join("root.nia");
    std::fs::write(&path, concat!(
        "struct Point { width: i32 }\n",
        "struct Box { width: i32 }\n",
        "trait Read { fun read(&self) i32; }\n",
        "extend Point : Read { fun read(&self) i32 { self.width } }\n",
            "fun use_it() i32 { let width = 1; let a = Point { width }; let b = Box { width: 2 }; a.read() + b.width }\n",
    )).unwrap();
    for (token, replacement, count) in [("width:", "size", 3), ("read(", "get", 3)] {
        let edit = rename_at(&path, token, replacement).unwrap();
        let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
            panic!("expected text edits");
        };
        assert_eq!(edits[0].edits.len(), count);
    }
}

#[test]
fn rename_updates_enum_constructors_and_named_patterns() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-enum-members");
    let path = root.join("root.nia");
    std::fs::write(&path, concat!(
        "enum Event { Close, Resize { width: i32 } }\n",
        "fun create() Event { Event::Resize { width: 3 } }\n",
        "fun size(event: Event) i32 { match event { Event::Close => 0, Event::Resize { width } => width } }\n",
    )).unwrap();
    for (old, new) in [("Resize", "Scale"), ("width:", "extent")] {
        let edit = rename_at(&path, old, new).unwrap();
        let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
            panic!("expected text edits");
        };
        assert_eq!(edits[0].edits.len(), 3);
    }
}

#[test]
fn configured_generated_modules_are_indexed_and_restrict_rename() {
    use nia_build_plan::{
        ActionKey, ActionKind, ArtifactConfiguration, BuildPlan, BuildPlanDraft, LogicalPath,
        LogicalPathRoot, ModuleImport, ModuleKey, OptimizationMode, PackageKey, PlanAction,
        PlanModule, PlanPackage, PlanPackageSource, PlanStep, Runtime, StepKey, StepVisibility,
        publish_build_plan,
    };
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-generated-module");
    let path = root.join("root.nia");
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    )
    .unwrap();
    std::fs::write(
        &path,
        "using generated::amount as local;\nfun use_it() i32 { local() }\n",
    )
    .unwrap();
    let output = root.join("build/out");
    std::fs::create_dir_all(&output).unwrap();
    let generated = output.join("generated.nia");
    let generated_text = "pub fun amount() i32 { 42 }\n";
    std::fs::write(&generated, generated_text).unwrap();
    let package = PackageKey::root();
    let module = ModuleKey::new(package.clone(), "source").unwrap();
    let generated_path = LogicalPath::new(LogicalPathRoot::Build, "generated.nia").unwrap();
    let generate = ActionKey::new(package.clone(), "generate").unwrap();
    let check = ActionKey::new(package.clone(), "check").unwrap();
    let generator = StepKey::new(package.clone(), "generate").unwrap();
    let checker = StepKey::new(package.clone(), "check").unwrap();
    let target = nia_target::TargetConfig::host().unwrap();
    let plan = BuildPlan::freeze(BuildPlanDraft {
        root_package: package.clone(),
        packages: vec![PlanPackage {
            key: package.clone(),
            source: PlanPackageSource::Local(String::new()),
        }],
        host_target: target,
        artifact_target: target,
        modules: vec![PlanModule {
            key: module.clone(),
            root_source: LogicalPath::new(LogicalPathRoot::Package(package), "root.nia").unwrap(),
            optimization: OptimizationMode::O0,
            imports: vec![ModuleImport {
                name: "generated".into(),
                path: generated_path.clone(),
            }],
        }],
        artifacts: Vec::new(),
        actions: vec![
            PlanAction {
                key: generate.clone(),
                kind: ActionKind::GeneratedFile {
                    output: generated_path,
                    contents: generated_text.as_bytes().to_vec(),
                },
            },
            PlanAction {
                key: check.clone(),
                kind: ActionKind::CompilerCheck {
                    module,
                    target,
                    runtime: Runtime::Bare,
                    mode: nia_build_plan::CompilationMode::Normal,
                    configuration: ArtifactConfiguration::default(),
                },
            },
        ],
        steps: vec![
            PlanStep {
                key: generator.clone(),
                visibility: StepVisibility::Internal,
                action: generate,
                dependencies: Vec::new(),
            },
            PlanStep {
                key: checker.clone(),
                visibility: StepVisibility::Public,
                action: check,
                dependencies: vec![generator],
            },
        ],
        default_step: Some(checker),
        selected_step: None,
    })
    .unwrap();
    publish_build_plan(&output.join("build-plan.bin"), &plan).unwrap();
    let analyzed = test_session().analyze(&Documents::new(), &path).unwrap();
    assert_eq!(analyzed.first_error(), None);
    assert_eq!(
        analyzed
            .definition(&path, Position::new(1, 19))
            .unwrap()
            .uri,
        uri(&generated).unwrap()
    );
    assert!(rename_at(&path, "local;", "renamed").is_ok());
    assert!(
        rename_at(&path, "amount as", "renamed")
            .unwrap_err()
            .contains("outside the workspace")
    );
    assert_eq!(std::fs::read_to_string(&generated).unwrap(), generated_text);
}
