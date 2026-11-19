// SPDX-License-Identifier: GPL-3.0-or-later

use nia_test_support::{TestWorkload, acquire_test_resources, test_dir};

use super::{tests::test_session, *};

#[cfg(windows)]
#[test]
fn rename_accepts_lowercase_drive_uris_from_editors() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-lowercase-drive");
    let entry = root.join("root.nia");
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    )
    .unwrap();
    std::fs::write(
        &entry,
        "fun amount() i32 { 1 }\nfun run() i32 { amount() }\n",
    )
    .unwrap();
    let mut editor_path = normalized(&entry);
    editor_path[..1].make_ascii_lowercase();
    let editor_uri = uri(Path::new(&editor_path)).unwrap();
    let decoded = path(&editor_uri).unwrap();
    assert_eq!(normalized(&decoded), normalized(&entry));
    assert!(rename_at(&decoded, "amount()", "quantity").is_ok());
}

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
    let cancel = Cancellation::default();
    let index = session
        .workspace_index(&entry, &documents, &[], &cancel)
        .unwrap();
    assert_eq!(
        index
            .references(&analysis, &entry, Position::new(1, 18), false, &cancel)
            .unwrap()
            .len(),
        1
    );
    assert!(
        index
            .reference_coverage_warning()
            .unwrap()
            .contains("build-plan.bin")
    );
    assert!(
        rename_at(&entry, "amount()", "quantity")
            .unwrap_err()
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
    let build_entry = index
        .entries
        .iter()
        .find(|entry| entry.request.check.entry_path.as_str() == normalized(&build))
        .expect("build candidate is indexed");
    assert!(matches!(
        build_entry.coverage,
        crate::workspace::Coverage::Complete
    ));
    assert_eq!(
        build_entry.request.context.runtime,
        nia_build_plan::Runtime::Bare
    );
    let warning = index.reference_coverage_warning().unwrap();
    assert!(warning.contains("no configured build plan"), "{warning}");
    let documents = Documents::new();
    let analysis = session.analyze(&documents, &entry).unwrap();
    assert_eq!(analysis.first_error(), None);
    assert!(
        analysis
            .diagnostics(&entry, &Document::new(1, "module build;\n".into()))
            .iter()
            .any(|diagnostic| diagnostic.message.contains("no configured build plan"))
    );
}

#[test]
fn opening_a_build_script_child_uses_the_build_dependency_scope() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-build-child-owner");
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    };
    write(
        "nia.toml",
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n\
         [library]\npath = \"root.nia\"\n\
         [build-dependencies]\nhelper = { path = \"helper\" }\n",
    );
    write("root.nia", "pub fun library() i32 { 0 }\n");
    write(
        "build.nia",
        "using std::build;\nmodule options;\n\
         pub fun build(b: &mut build::Build) build::Error!() { _ = b; _ = options::value(); !() }\n",
    );
    let child = write("options.nia", "pub fun value() i32 { 0 }\n");
    write(
        "helper/nia.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    );
    let helper = write("helper/root.nia", "pub fun amount() i32 { 42 }\n");
    let edited = "pub fun value() i32 { helper::amount() }\n";
    let documents = Documents::from([(
        child.clone(),
        OpenDocument {
            uri: uri(&child).unwrap(),
            document: Document::new(1, edited.into()),
        },
    )]);
    let analysis = test_session().analyze(&documents, &child).unwrap();
    assert_eq!(analysis.first_error(), None);
    assert_eq!(
        analysis
            .definition(&child, Position::new(0, 31))
            .unwrap()
            .uri,
        uri(&helper).unwrap()
    );
}

#[test]
fn workspace_implementation_search_follows_trait_and_member_bindings_across_packages() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-workspace-implementations");
    let write = |path: &str, text: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    };
    write("nia.toml", "[workspace]\nmembers = [\"api\", \"client\"]\n");
    write(
        "api/nia.toml",
        "[package]\nname = \"api\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    );
    let api = write(
        "api/root.nia",
        "pub trait Source { fun read(&self) i32; }\n",
    );
    write(
        "client/nia.toml",
        "[package]\nname = \"client\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n\
         [dependencies]\napi = { path = \"../api\" }\n",
    );
    let client = write("client/root.nia", "pub struct Value {}\n");
    let documents = Documents::from([(
        client.clone(),
        OpenDocument {
            uri: uri(&client).unwrap(),
            document: Document::new(
                1,
                concat!(
                    "pub struct Value {}\n",
                    "extend Value : api::Source { fun read(&self) i32 { 42 } }\n",
                    "fun consume[T](src: &T) i32 where T: api::Source { src.read() + src.read() }\n",
                ).into(),
            ),
        },
    )]);
    let mut session = test_session();
    let analysis = session.analyze(&documents, &api).unwrap();
    let cancel = Cancellation::default();
    let index = session
        .workspace_index(&api, &documents, &[root.to_path_buf()], &cancel)
        .unwrap();
    for (position, expected_column) in [(Position::new(0, 12), 15), (Position::new(0, 24), 33)] {
        let locations = index
            .implementations(&analysis, &api, position, &cancel)
            .unwrap();
        assert_eq!(locations.len(), 1, "{position:?}");
        assert_eq!(locations[0].uri, uri(&client).unwrap());
        assert_eq!(locations[0].range.start, Position::new(1, expected_column));
    }
    let method = analysis
        .prepare_call_hierarchy(&api, Position::new(0, 24), 1)
        .unwrap();
    assert_eq!(method.kind, lsp_types::SymbolKind::METHOD);
    let context: crate::workspace::CallHierarchyContext =
        serde_json::from_value(method.data.clone().unwrap()).unwrap();
    let incoming = index
        .call_relations(&method, &context, true, &cancel)
        .unwrap();
    let [(caller, spans)] = incoming.as_slice() else {
        panic!("expected one caller from the client package, got {incoming:?}");
    };
    assert_eq!(caller.uri, uri(&client).unwrap());
    assert_eq!(caller.name, "consume");
    assert_eq!(spans.len(), 2);
    assert_eq!(
        index
            .call_relations(caller, &context, false, &cancel)
            .unwrap(),
        vec![(method, spans.clone())]
    );
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
fn rename_preserves_earlier_diagnostics_and_rejects_new_ones() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-rename-diagnostics");
    let entry = root.join("root.nia");
    let source = concat!(
        "pub fun amount() i32 { 1 }\n",
        "pub fun taken() i32 { 2 }\n",
        "fun use_it() i32 { _ = amount(); missing() }\n",
    );
    std::fs::write(&entry, source).unwrap();
    let documents = Documents::new();
    let mut session = test_session();
    let analysis = session.analyze(&documents, &entry).unwrap();
    assert!(
        analysis
            .errors()
            .any(|item| item.diagnostic.summary.contains("missing"))
    );
    let folders = [root.to_path_buf()];
    let mut rename = |new_name| {
        session.rename(
            &analysis,
            &documents,
            crate::rename::Rename {
                path: &entry,
                position: Position::new(0, 9),
                new_name,
                folders: &folders,
                versioned: false,
                resource_rename: false,
            },
            &Cancellation::default(),
        )
    };
    let edit = rename("available_quantity").unwrap();
    assert_eq!(edit.changes.unwrap()[&uri(&entry).unwrap()].len(), 2);
    assert!(rename("taken").unwrap_err().contains("Rename would break"),);
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
fn file_rename_updates_the_live_module_declaration_and_imports() {
    let _resources = acquire_test_resources(TestWorkload::Compiler);
    let root = test_dir("lsp-file-rename-module");
    let entry = root.join("root.nia");
    std::fs::write(
        root.join("nia.toml"),
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n[library]\npath = \"root.nia\"\n",
    )
    .unwrap();
    let source = "module outer;\nfun use_it() i32 { outer::inner::value() + other::value() }\nmodule other;\n";
    std::fs::write(&entry, source).unwrap();
    std::fs::create_dir(root.join("outer")).unwrap();
    std::fs::write(root.join("outer.nia"), "pub module inner;\n").unwrap();
    std::fs::write(root.join("outer/inner.nia"), "pub fun value() i32 { 2 }\n").unwrap();
    std::fs::write(root.join("other.nia"), "pub fun value() i32 { 3 }\n").unwrap();
    let entry_uri = uri(&entry).unwrap();
    let documents = Documents::from([(
        entry,
        OpenDocument {
            uri: entry_uri.clone(),
            document: Document::new(7, format!("// Unsaved comment\n{source}")),
        },
    )]);
    let rename = |from: &str, to: &str| lsp_types::FileRename {
        old_uri: uri(&root.join(from)).unwrap().to_string(),
        new_uri: uri(&root.join(to)).unwrap().to_string(),
    };
    let file = rename("outer.nia", "moved.nia");
    let directory = rename("outer", "moved");
    let mut session = test_session();
    let folders = [root.to_path_buf()];
    let mut request = |moves, versioned| {
        session.will_rename_files(
            moves,
            &documents,
            &folders,
            versioned,
            &Cancellation::default(),
        )
    };
    let edit = request(vec![file.clone(), directory.clone()], true)
        .unwrap()
        .unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected text edits only");
    };
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].text_document.uri, entry_uri);
    assert_eq!(edits[0].text_document.version, Some(7));
    let lines: Vec<_> = edits[0]
        .edits
        .iter()
        .map(|edit| match edit {
            lsp_types::OneOf::Left(edit) => {
                assert_eq!(edit.new_text, "moved");
                edit.range.start.line
            }
            _ => panic!("expected a plain text edit"),
        })
        .collect();
    assert_eq!(lines, [1, 2]);
    assert!(
        request(vec![file.clone()], true)
            .unwrap_err()
            .contains("child directory")
    );
    assert!(
        request(vec![directory.clone()], true)
            .unwrap_err()
            .contains("source file")
    );
    assert!(
        request(vec![rename("outer.nia", "sub/moved.nia")], true)
            .unwrap_err()
            .contains("declaring module's directory")
    );
    assert!(
        request(vec![rename("README.md", "README.txt")], true)
            .unwrap()
            .is_none()
    );
    let edit = request(vec![file.clone(), directory.clone()], false)
        .unwrap()
        .unwrap();
    assert_eq!(edit.changes.unwrap()[&entry_uri].len(), 2);
    let batch = [
        file.clone(),
        directory.clone(),
        rename("other.nia", "renamed.nia"),
    ];
    let edit = request(batch.to_vec(), true).unwrap().unwrap();
    let Some(lsp_types::DocumentChanges::Edits(edits)) = edit.document_changes else {
        panic!("expected a single set of versioned text edits");
    };
    let [edit] = edits.as_slice() else {
        panic!("expected one edited document");
    };
    assert_eq!(edit.text_document.version, Some(7));
    assert_eq!(
        edit.edits
            .iter()
            .map(|edit| match edit {
                lsp_types::OneOf::Left(edit) => (edit.range.start.line, edit.new_text.as_str()),
                _ => panic!("expected a plain text edit"),
            })
            .collect::<Vec<_>>(),
        [(1, "moved"), (2, "moved"), (2, "renamed"), (3, "renamed")]
    );
    let shared_destination = vec![
        file.clone(),
        directory.clone(),
        rename("other.nia", "moved.nia"),
    ];
    assert!(
        request(shared_destination, false)
            .unwrap_err()
            .contains("Overlapping module moves")
    );
    let case_only = request(
        vec![rename("outer.nia", "Outer.nia"), rename("outer", "Outer")],
        false,
    )
    .unwrap()
    .unwrap();
    assert!(
        case_only.changes.unwrap()[&entry_uri]
            .iter()
            .all(|edit| edit.new_text == "Outer")
    );
    std::fs::write(root.join("moved.nia"), "").unwrap();
    assert!(
        request(vec![file, directory], true)
            .unwrap_err()
            .contains("destination already exists")
    );
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
    let _resources = acquire_test_resources(TestWorkload::Compiler);
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
    let alternate_target = nia_target::SUPPORTED_TARGETS
        .into_iter()
        .find(|candidate| {
            candidate.os() != target.os() && candidate.arch() == nia_target::Arch::X86_64
        })
        .unwrap();
    let mut actions = Vec::new();
    let mut steps = Vec::new();
    for (name, mode, action_target) in [
        ("check", CompilationMode::Normal, target),
        ("test", CompilationMode::Test, alternate_target),
    ] {
        let action = ActionKey::new(root.clone(), name).unwrap();
        actions.push(PlanAction {
            key: action.clone(),
            kind: ActionKind::CompilerCheck {
                module: module.clone(),
                target: action_target,
                runtime: Runtime::Bare,
                mode,
                configuration: ArtifactConfiguration {
                    cpu: (mode == CompilationMode::Test).then_some(nia_target::CpuLevel::X86_64V3),
                    ..ArtifactConfiguration::default()
                },
            },
        });
        steps.push(PlanStep {
            key: StepKey::new(root.clone(), name).unwrap(),
            visibility: StepVisibility::Public,
            action,
            dependencies: Vec::new(),
        });
    }
    let verify_step = StepKey::new(root.clone(), "verify").unwrap();
    let verify_action = ActionKey::new(root.clone(), "verify").unwrap();
    actions.push(PlanAction {
        key: verify_action.clone(),
        kind: ActionKind::Aggregate,
    });
    steps.push(PlanStep {
        key: verify_step.clone(),
        visibility: StepVisibility::Public,
        action: verify_action,
        dependencies: vec![StepKey::new(root.clone(), "test").unwrap()],
    });
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
        default_step: Some(StepKey::new(PackageKey::root(), "check").unwrap()),
        selected_step: Some(verify_step),
    })
    .unwrap();
    let output = directory.join("build/out");
    std::fs::create_dir_all(&output).unwrap();
    let plan_path = output.join("build-plan.bin");
    let mut draft = nia_build_plan::BuildDraft {
        plan,
        observations: Vec::new(),
    };
    let config = nia_build_plan::PlanConfiguration {
        profile: nia_target_config::BuildProfile::Release,
        ..Default::default()
    };
    publish_build_plan(&plan_path, &draft, config).unwrap();

    let mut session = test_session();
    let toolchain = Arc::clone(&session.toolchain);
    let entry = directory.join("root.nia");
    let error = session
        .projects
        .package(&entry)
        .unwrap()
        .unwrap()
        .configured_requests(&toolchain)
        .err()
        .expect("plan has no configuration evidence");
    assert!(error.contains("build plan is corrupt"), "{error}");

    let mut observations =
        nia_build::ConfigureManifest::new(nia_build::ObservationPolicy::Disallow);
    observations
        .push(nia_build::observe_file_contents(&directory, "nia.toml").unwrap())
        .unwrap();
    draft.observations = observations.encode().unwrap();
    publish_build_plan(&plan_path, &draft, config).unwrap();
    session.reload();
    let package = session.projects.package(&entry).unwrap().unwrap();
    let requests = package.configured_requests(&toolchain).unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].context.target, alternate_target,
        "the selected step's check takes precedence over the default step"
    );
    assert_eq!(
        requests[0].check.compilation_mode,
        nia_target_config::CompilationMode::Test
    );
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

    let source = concat!(
        "@[release] pub fun amount() i32 { 1 }\n",
        "fun sample() i32 { amount() }\n",
        "@[test] @[if hasFeature(\"avx2\")] fun feature() i32 { 1 }\n",
        "@[test] @[if hasFeature(\"avx2\")] fun feature_use() i32 { feature() }\n",
        "@[if not hasFeature(\"avx2\")] pub fun taken() i32 { 2 }\n",
        "@[test] fun test_use() i32 { amount() }\n",
        "fun test_statement() () { @[test] _ = amount(); }\n",
    );
    std::fs::write(&entry, source).unwrap();
    let documents = Documents::new();
    let analysis = session.analyze(&documents, &entry).unwrap();
    assert_eq!(analysis.first_error(), None);
    let lines = Lines::new(source);
    let feature = lines.position(source, source.rfind("feature()").unwrap());
    assert_eq!(
        analysis
            .definition(&entry, feature)
            .unwrap()
            .range
            .start
            .line,
        2
    );
    let position = lines.position(source, source.rfind("amount").unwrap());
    assert_eq!(
        analysis
            .definition(&entry, position)
            .unwrap()
            .range
            .start
            .line,
        0
    );
    let cancel = Cancellation::default();
    let index = session
        .workspace_index(&entry, &documents, &[], &cancel)
        .unwrap();
    assert_eq!(index.entries.len(), 2);
    assert!(index.reference_coverage_warning().is_none());
    let mut rename = |new_name| {
        session.rename(
            &analysis,
            &documents,
            crate::rename::Rename {
                path: &entry,
                position,
                new_name,
                folders: &[],
                versioned: false,
                resource_rename: false,
            },
            &cancel,
        )
    };
    let edit = rename("quantity").unwrap();
    assert_eq!(edit.changes.unwrap()[&uri(&entry).unwrap()].len(), 4);
    let error = rename("taken").unwrap_err();
    let native_context =
        format!("target {target}, CPU baseline, Bare runtime, Release profile, Normal mode");
    assert!(
        error.contains("Rename would break") && error.contains(&native_context),
        "{error}"
    );

    let edited = source.replace("{ 2 }", "{ missing() }");
    let documents = Documents::from([(
        entry.clone(),
        OpenDocument {
            uri: uri(&entry).unwrap(),
            document: Document::new(1, edited),
        },
    )]);
    let index = session
        .workspace_index(&entry, &documents, &[], &cancel)
        .unwrap();
    let warning = index.reference_coverage_warning().unwrap();
    assert!(
        warning.contains(&native_context) && warning.contains("missing"),
        "{warning}"
    );
}

#[test]
fn configured_generated_modules_are_indexed_and_restrict_rename() {
    let _resources = acquire_test_resources(TestWorkload::Build);
    let root = test_dir("lsp-generated-module");
    let path = root.join("root.nia");
    let manifest = root.join("nia.toml");
    let manifest_text = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
    std::fs::write(&manifest, manifest_text).unwrap();
    let source = "using generated::amount as local;\nfun use_it() i32 { local() }\n";
    std::fs::write(&path, source).unwrap();
    std::fs::write(
        root.join("build.nia"),
        r#"using std::build;
using std::fs;

pub fun build(b: &mut build::Build) build::Error!() {
    let generated = build::BuildPathView::init(&"generated.nia");
    let imports = [build::ModuleImport::fromBuild(&"generated", generated)];
    let source = b.addModule(
        build::ModuleOptions::init(&"source", fs::PathView::init(&"root.nia"))
            .withImports(&imports[..]),
    ).?;
    let generate = b.addGeneratedFileStep(
        &"generate",
        generated,
        &b"@[release]\npub fun amount() i32 { 42 }\n"[..],
    ).?;
    let check = b.addCheckModuleStep(&"check", source).?;
    b.dependOn(check, generate).?;
    b.setDefaultStep(check).?;
    !()
}
"#,
    )
    .unwrap();

    let mut session = test_session();
    nia_build::run_build(
        nia_build::BuildRequest::new(Arc::clone(&session.toolchain))
            .with_root(&root)
            .with_profile(nia_target_config::BuildProfile::Release),
    )
    .unwrap();
    let generated = root.join("build/out/generated.nia");
    let generated_text = "@[release]\npub fun amount() i32 { 42 }\n";
    let documents = Documents::new();
    let completions = session
        .complete_names(
            &documents,
            &path,
            Position::new(1, 22),
            &Cancellation::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].label, "local");
    assert_eq!(completions[0].detail.as_deref(), Some("fun amount() i32"));
    let analyzed = session.analyze(&documents, &path).unwrap();
    assert_eq!(
        analyzed.complete(&path, Position::new(1, 22)),
        Some(completions)
    );
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

    std::fs::write(
        &manifest,
        format!("{manifest_text}# configuration changed\n"),
    )
    .unwrap();
    session.reload();
    let analyzed = session.analyze(&documents, &path).unwrap();
    assert!(analyzed.definition(&path, Position::new(1, 19)).is_none());
    let diagnostics = analyzed.diagnostics(&path, &Document::new(1, source.into()));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("build plan is stale")
            && diagnostic.message.contains("nia.toml")
    }));
}
