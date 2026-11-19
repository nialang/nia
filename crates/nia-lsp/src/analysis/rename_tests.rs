// SPDX-License-Identifier: GPL-3.0-or-later

use nia_test_support::{TestWorkload, acquire_test_resources, test_dir};

use super::{tests::test_session, *};

#[test]
fn unreadable_plan_keeps_manifest_navigation_available() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-unreadable-build-plan");
    let entry = root.join("root.nia");
    let source = "fun amount() i32 { 1 }\nfun run() i32 { amount() }\n";
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    )
    .unwrap();
    std::fs::write(&entry, source).unwrap();
    let output = root.join("build/out");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("build-plan.bin"), b"invalid plan").unwrap();
    let mut session = test_session();
    let documents = Documents::new();
    let analysis = session.analyze(&documents, &entry).unwrap();
    assert_eq!(analysis.first_error(), None);
    assert_eq!(
        analysis
            .definition(&entry, Position::new(1, 18))
            .unwrap()
            .range
            .start,
        Position::new(0, 4)
    );
    let diagnostics = analysis.diagnostics(&entry, &Document::new(1, source.into()));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::WARNING));
    assert!(diagnostics[0].message.contains("build-plan.bin"));
    assert!(
        session
            .workspace_index(&entry, &documents, &[], &Cancellation::default())
            .err()
            .expect("incomplete build context")
            .contains("build-plan.bin")
    );
}

#[test]
fn build_named_library_module_keeps_its_owner_in_navigation_and_rename() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-build-module-owner");
    let entry = root.join("root.nia");
    let build = root.join("build.nia");
    let commands = root.join("build/commands.nia");
    std::fs::create_dir(root.join("build")).unwrap();
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    )
    .unwrap();
    std::fs::write(&entry, "module build;\nfun run() i32 { build::amount() }\n").unwrap();
    std::fs::write(
        &build,
        "module commands;\npub fun amount() i32 { commands::value() }\n",
    )
    .unwrap();
    std::fs::write(&commands, "pub fun value() i32 { 1 }\n").unwrap();
    let documents = Documents::from([(
        build.clone(),
        OpenDocument {
            uri: uri(&build).unwrap(),
            document: Document::new(
                2,
                "module commands;\npub fun amount() i32 { commands::value() + 1 }\n".into(),
            ),
        },
    )]);
    let mut session = test_session();
    let analysis = session.analyze(&documents, &build).unwrap();
    assert_eq!(analysis.first_error(), None);
    assert_eq!(
        analysis
            .definition(&build, Position::new(1, 34))
            .unwrap()
            .uri,
        uri(&commands).unwrap()
    );
    let edit = session
        .rename(
            &analysis,
            &documents,
            crate::rename::Rename {
                path: &build,
                position: Position::new(1, 34),
                new_name: "quantity",
                folders: &[],
                versioned: true,
                resource_rename: false,
            },
            &Cancellation::default(),
        )
        .unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected text edits");
    };
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].text_document.version, Some(2));
    assert_eq!(edits[1].text_document.uri, uri(&commands).unwrap());
}

#[test]
fn build_hook_still_contributes_its_independent_context() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-build-hook-owner");
    let entry = root.join("root.nia");
    let build = root.join("build.nia");
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    )
    .unwrap();
    std::fs::write(&entry, "module build;\n").unwrap();
    std::fs::write(
        &build,
        "using std::build;\npub fun build(b: &mut build::Build) build::Error!() { _ = b; !() }\n",
    )
    .unwrap();
    let mut session = test_session();
    let index = session
        .workspace_index(&entry, &Documents::new(), &[], &Cancellation::default())
        .unwrap();
    assert!(index.entries.iter().any(|entry| {
        entry.request.check.entry_path.as_str() == normalized(&build) && entry.error.is_none()
    }));
}

#[test]
fn rename_updates_associated_types_in_implementations_projections_and_bounds() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-associated-types");
    let path = root.join("root.nia");
    std::fs::write(&path, concat!(
        "trait Source { type Item; fun get(&self) [Self as Source]::Item; }\n",
        "struct Value {}\n",
        "extend Value : Source { type Item = i32; fun get(&self) i32 { 1 } }\n",
        "fun read[T](source: &T) [T as Source]::Item where T: Source[Item = i32] { source.get() }\n",
    )).unwrap();
    let edit = rename_at(&path, "Item;", "Output").unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected text edits");
    };
    assert_eq!(edits[0].edits.len(), 5);
}

#[test]
fn rename_updates_trait_constants_in_implementations_and_projections() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-associated-constants");
    let path = root.join("root.nia");
    std::fs::write(
        &path,
        concat!(
            "trait Lanes { const COUNT: usize; }\n",
            "struct Value {}\n",
            "extend Value : Lanes { const COUNT: usize = 1; }\n",
            "fun count[T]() usize where T: Lanes { [T as Lanes]::COUNT }\n",
            "fun count_value() usize { Value::COUNT }\n",
        ),
    )
    .unwrap();
    let edit = rename_at(&path, "COUNT:", "SIZE").unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected text edits");
    };
    assert_eq!(edits[0].edits.len(), 4);
}

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
        &Cancellation::default(),
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
fn rename_follows_a_public_module_alias_through_chained_imports() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-public-module-alias");
    let path = root.join("root.nia");
    std::fs::write(&path, concat!(
        "module origin;\nmodule aliases;\nmodule forward;\n",
        "using forward::visible;\n",
        "fun value() i32 { visible::amount() + aliases::visible::amount() + origin::amount() }\n",
    )).unwrap();
    std::fs::write(root.join("origin.nia"), "pub fun amount() i32 { 1 }\n").unwrap();
    std::fs::write(
        root.join("aliases.nia"),
        "pub using super::origin as visible;\n",
    )
    .unwrap();
    std::fs::write(
        root.join("forward.nia"),
        "pub using super::aliases::visible;\n",
    )
    .unwrap();
    let edit = rename_at(&path, "visible;", "renamed").unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected text edits");
    };
    assert_eq!(
        edits
            .iter()
            .map(|edit| edit.edits.len())
            .collect::<Vec<_>>(),
        [1, 1, 3]
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
fn configured_imports_respect_the_scopes_of_local_plan_packages() {
    use nia_build_plan::{
        ActionKey, ActionKind, ArtifactConfiguration, BuildPlan, BuildPlanDraft, CompilationMode,
        LogicalPath, LogicalPathRoot, ModuleImport, ModuleKey, OptimizationMode, PackageKey,
        PlanAction, PlanModule, PlanPackage, PlanPackageSource, PlanStep, Runtime, StepKey,
        StepVisibility, publish_build_plan,
    };
    let directory = test_dir("lsp-configured-dependency-scopes");
    std::fs::write(
        directory.join("nia.toml"),
        "[package]\nname = \"app\"\nversion = \"1.0.0\"\n\
         [build-dependencies]\nhelper = { path = \"helper\" }\n\
         [dev-dependencies]\nfixture = { path = \"fixture\" }\n",
    )
    .unwrap();
    let root = PackageKey::root();
    let module = ModuleKey::new(root.clone(), "app").unwrap();
    let mut packages = vec![PlanPackage {
        key: root.clone(),
        source: PlanPackageSource::Local(String::new()),
    }];
    let mut imports = Vec::new();
    for name in ["helper", "fixture", "independent"] {
        let path = directory.join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(
            path.join("nia.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n[library]\npath = \"root.nia\"\n"),
        )
        .unwrap();
        let key = PackageKey::new(name).unwrap();
        packages.push(PlanPackage {
            key: key.clone(),
            source: PlanPackageSource::Local(name.into()),
        });
        imports.push(ModuleImport {
            name: format!("{name}Import"),
            path: LogicalPath::new(LogicalPathRoot::Package(key), "root.nia").unwrap(),
        });
    }
    let target = test_session().toolchain.host_target().to_owned();
    let mut actions = Vec::new();
    let mut steps = Vec::new();
    for (name, mode) in [
        ("check", CompilationMode::Normal),
        ("test", CompilationMode::Test),
    ] {
        let action = ActionKey::new(root.clone(), name).unwrap();
        actions.push(PlanAction {
            key: action.clone(),
            kind: ActionKind::CompilerCheck {
                module: module.clone(),
                target,
                runtime: Runtime::Bare,
                mode,
                configuration: ArtifactConfiguration::default(),
            },
        });
        steps.push(PlanStep {
            key: StepKey::new(root.clone(), name).unwrap(),
            visibility: StepVisibility::Public,
            action,
            dependencies: Vec::new(),
        });
    }
    let plan = BuildPlan::freeze(BuildPlanDraft {
        root_package: root.clone(),
        packages,
        host_target: target,
        artifact_target: target,
        modules: vec![PlanModule {
            key: module,
            root_source: LogicalPath::new(LogicalPathRoot::Package(root), "root.nia").unwrap(),
            optimization: OptimizationMode::O0,
            imports,
        }],
        artifacts: Vec::new(),
        actions,
        steps,
        default_step: None,
        selected_step: None,
    })
    .unwrap();
    let output = directory.join("build/out");
    std::fs::create_dir_all(&output).unwrap();
    publish_build_plan(&output.join("build-plan.bin"), &plan).unwrap();

    let mut session = test_session();
    let toolchain = Arc::clone(&session.toolchain);
    let package = session
        .projects
        .package(&directory.join("root.nia"))
        .unwrap()
        .unwrap();
    let requests = package.configured_requests(&toolchain).unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests {
        let request = request.check;
        let modules = request.module_map.for_source(&request.entry_path);
        assert!(modules.get("independentImport").is_some());
        assert!(modules.get("helperImport").is_none());
        assert_eq!(
            modules.get("fixtureImport").is_some(),
            request.compilation_mode == nia_target_config::CompilationMode::Test
        );
    }
    assert!(!directory.join("nia.lock").exists());
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
