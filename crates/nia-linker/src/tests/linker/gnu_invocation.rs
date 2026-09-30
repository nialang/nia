use super::*;

fn args(base: &[&str]) -> Vec<String> {
    base.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn default_gnu_invocation_is_a_static_pie() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(invocation.program, "ld");
    assert_eq!(
        invocation.args,
        args(&[
            "-e",
            "_start",
            "main.o",
            "-static",
            "-pie",
            "--no-dynamic-linker",
            "-z",
            "text",
            "-o",
            "main",
        ])
    );
}

#[test]
fn static_form_links_at_a_fixed_address() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    }
    .with_form(ExecutableForm::Static);
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(
        invocation.args,
        args(&[
            "-e", "_start", "main.o", "-static", "-z", "text", "-o", "main"
        ])
    );
}

#[test]
fn lld_static_pie_packs_relative_relocations() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program_and_flavor("ld.lld", LinkerFlavor::Lld),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert!(
        invocation
            .args
            .windows(2)
            .any(|pair| pair == ["-z", "pack-relative-relocs"]),
        "{:?}",
        invocation.args
    );
}

#[test]
fn i686_linux_gnu_invocation_selects_elf_i386_emulation() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(
            target("x86-unknown-linux"),
            &link_inputs("main.o"),
            PathBuf::from("main"),
        )
        .expect("link invocation");
    assert!(
        invocation
            .args
            .windows(2)
            .any(|args| args == ["-m", "elf_i386"])
    );
}

#[test]
fn invocation_preserves_typed_link_input_order() {
    let inputs = IncrementalLinkInputs::new(vec![
        IncrementalLinkInput {
            key: CodegenUnitKey::SourceModule {
                source_identity: SourceIdentity::new("main.nia"),
                ordinal: 0,
            },
            fingerprint: CodegenUnitFingerprint::from_parts([3, 4]),
            object: PathBuf::from("main.o"),
        },
        IncrementalLinkInput {
            key: CodegenUnitKey::CompilerBuiltins,
            fingerprint: CodegenUnitFingerprint::from_parts([5, 6]),
            object: PathBuf::from("builtins.o"),
        },
    ])
    .expect("build incremental link inputs");
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    };

    let invocation = options
        .invocation(linux(), &inputs, PathBuf::from("main"))
        .expect("link invocation");
    let main_index = invocation
        .args
        .iter()
        .position(|arg| arg == "main.o")
        .expect("source object argument");
    let builtins_index = invocation
        .args
        .iter()
        .position(|arg| arg == "builtins.o")
        .expect("compiler builtins object argument");

    assert!(main_index < builtins_index);
}

#[test]
fn invocation_passes_exact_static_archive_paths_in_declaration_order() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    }
    .with_static_archives(vec![
        StaticArchiveLinkInput::from_bytes("root", "first", "lib/first.a", b"first"),
        StaticArchiveLinkInput::from_bytes("root", "second", "vendor/second.a", b"second"),
    ]);

    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(
        invocation.args,
        args(&[
            "-e",
            "_start",
            "main.o",
            "lib/first.a",
            "vendor/second.a",
            "-static",
            "-pie",
            "--no-dynamic-linker",
            "-z",
            "text",
            "-o",
            "main",
        ])
    );
}

#[test]
fn dynamic_gnu_invocation_accepts_structured_options() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    }
    .with_form(ExecutableForm::Dynamic(Interpreter::Path(
        "/loader".to_string(),
    )))
    .add_library_path("/lib")
    .add_rpath("$ORIGIN")
    .add_library("native_api")
    .with_raw_args(vec!["-z".to_string(), "now".to_string()]);
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(
        invocation.args,
        args(&[
            "-e",
            "_start",
            "main.o",
            "-pie",
            "-z",
            "text",
            "-L",
            "/lib",
            "-rpath",
            "$ORIGIN",
            "-l",
            "native_api",
            "--dynamic-linker",
            "/loader",
            "-z",
            "now",
            "-o",
            "main",
        ])
    );
}

#[test]
fn static_gnu_invocation_selects_static_libraries_before_library_search() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    }
    .add_library_path("/lib")
    .add_library("native_api");
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    let static_index = invocation
        .args
        .iter()
        .position(|arg| arg == "-static")
        .expect("-static argument");
    let library_index = invocation
        .args
        .iter()
        .position(|arg| arg == "-l")
        .expect("-l argument");
    assert!(
        static_index < library_index,
        "static mode must be selected before library lookup: {:?}",
        invocation.args
    );
}

#[test]
fn dynamic_gnu_invocation_can_mix_static_and_dynamic_libraries() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    }
    .with_form(ExecutableForm::Dynamic(Interpreter::Standard))
    .add_static_library("compiler_runtime")
    .add_dynamic_library("LLVM")
    .add_dynamic_library(":libgcc_s.so.1")
    .add_dynamic_library("c");
    let invocation = options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    // A Linux host reports its own loader; any other host uses the standard one.
    let dynamic_linker = dynamic_linker_for_target(linux())
        .expect("resolve dynamic linker")
        .expect("Linux targets have a dynamic linker");
    assert_eq!(
        invocation.args,
        args(&[
            "-e",
            "_start",
            "main.o",
            "-pie",
            "-z",
            "text",
            "-Bstatic",
            "-l",
            "compiler_runtime",
            "-Bdynamic",
            "-l",
            "LLVM",
            "-l",
            ":libgcc_s.so.1",
            "-l",
            "c",
            "--dynamic-linker",
            dynamic_linker.as_str(),
            "-o",
            "main",
        ])
    );
}

#[test]
fn dynamic_only_options_require_a_dynamic_executable() {
    for options in [
        LinkOptions::default().add_rpath("$ORIGIN"),
        LinkOptions::default().add_dynamic_library("c"),
        LinkOptions::default()
            .with_form(ExecutableForm::Static)
            .add_rpath("$ORIGIN"),
    ] {
        let options = LinkOptions {
            linker: ExecutableLinker::with_program("ld"),
            ..options
        };
        assert!(matches!(
            options.invocation(linux(), &link_inputs("main.o"), PathBuf::from("main")),
            Err(LinkerConfigError::RequiresDynamicExecutable { .. })
        ));
    }
    // A static library is valid in every form.
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    }
    .add_static_library("support");
    options
        .invocation(linux(), &link_inputs("main.o"), PathBuf::from("main"))
        .expect("static library in a static-pie");
}

#[test]
fn only_elf_targets_select_an_executable_form() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("lld-link"),
        ..LinkOptions::default()
    };
    let windows = target("x86_64-pc-windows-msvc");
    options
        .invocation(windows, &link_inputs("main.obj"), PathBuf::from("main.exe"))
        .expect("default Windows form");
    let error = options
        .clone()
        .with_form(ExecutableForm::StaticPie)
        .invocation(windows, &link_inputs("main.obj"), PathBuf::from("main.exe"))
        .expect_err("Windows has one executable form");
    assert_eq!(
        error.to_string(),
        "target `x86_64-pc-windows-msvc` has one executable form; `static-pie` cannot be selected"
    );
}
