use super::*;

#[test]
fn default_static_gnu_invocation_keeps_freestanding_shape() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(invocation.program, "ld");
    assert_eq!(
        invocation.args,
        vec!["-e", "_start", "main.o", "-static", "-o", "main"]
    );
}

#[test]
fn i686_linux_gnu_invocation_selects_elf_i386_emulation() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        target: target("i686", "linux", "gnu"),
        ..LinkOptions::default()
    };
    let invocation = options
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
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
    ]);
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    };

    let invocation = options
        .invocation(&inputs, PathBuf::from("main"))
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
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(
        invocation.args,
        vec![
            "-e",
            "_start",
            "main.o",
            "lib/first.a",
            "vendor/second.a",
            "-static",
            "-o",
            "main"
        ]
    );
}

#[test]
fn dynamic_gnu_invocation_accepts_structured_options() {
    let options = LinkOptions {
        linker: ExecutableLinker::with_program("ld"),
        ..LinkOptions::default()
    }
    .with_dynamic_mode()
    .with_dynamic_linker(DynamicLinker::Path("/loader".to_string()))
    .add_library_path("/lib")
    .add_rpath("$ORIGIN")
    .add_library("native_api")
    .with_raw_args(vec!["-z".to_string(), "now".to_string()]);
    let invocation = options
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    assert_eq!(
        invocation.args,
        vec![
            "-e",
            "_start",
            "main.o",
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
            "main"
        ]
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
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
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
    .with_dynamic_mode()
    .add_static_library("compiler_runtime")
    .add_dynamic_library("LLVM")
    .add_dynamic_library(":libgcc_s.so.1")
    .add_dynamic_library("c");
    let invocation = options
        .invocation(&link_inputs("main.o"), PathBuf::from("main"))
        .expect("link invocation");
    let dynamic_linker =
        standard_dynamic_linker().unwrap_or_else(|| "/lib64/ld-linux-x86-64.so.2".to_string());
    assert_eq!(
        invocation.args,
        vec![
            "-e",
            "_start",
            "main.o",
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
            "main"
        ]
    );
}
